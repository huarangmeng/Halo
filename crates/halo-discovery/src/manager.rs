use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use tokio::{
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::{
    ConnectionFailure, ConnectionOutcome, DiscoveryConfig, DiscoveryError, DiscoveryEvent,
    DiscoveryProvider, Endpoint, LocalPresence, Observation, PeerSnapshot, PresenceId,
    ProtocolRange, ProviderContext, ProviderId, ProviderState,
    provider::ManagerInput,
    ranking::{EndpointEvidence, EndpointRecord, protocol_compatible},
};

/// Configures one discovery session.
pub struct DiscoveryManager {
    local: LocalPresence,
    config: DiscoveryConfig,
    providers: Vec<Arc<dyn DiscoveryProvider>>,
}

impl DiscoveryManager {
    #[must_use]
    pub fn new(local: LocalPresence) -> Self {
        Self {
            local,
            config: DiscoveryConfig::default(),
            providers: Vec::new(),
        }
    }

    #[must_use]
    pub fn config(mut self, config: DiscoveryConfig) -> Self {
        self.config = config;
        self
    }

    #[must_use]
    pub fn with_provider<P>(mut self, provider: P) -> Self
    where
        P: DiscoveryProvider,
    {
        self.providers.push(Arc::new(provider));
        self
    }

    #[must_use]
    pub fn with_shared_provider(mut self, provider: Arc<dyn DiscoveryProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    pub async fn start(self) -> Result<DiscoverySession, DiscoveryError> {
        self.config.validate()?;
        validate_provider_ids(&self.providers)?;

        let (input_tx, input_rx) = mpsc::channel(self.config.input_capacity);
        let (event_tx, _) = broadcast::channel(self.config.event_capacity);
        let cancel = CancellationToken::new();
        let manager_cancel = cancel.clone();
        let local = self.local.clone();
        let config = self.config.clone();
        let manager_events = event_tx.clone();
        let manager_task = tokio::spawn(async move {
            run_manager(local, config, input_rx, manager_events, manager_cancel).await;
        });

        let mut provider_tasks = Vec::with_capacity(self.providers.len());
        for provider in self.providers {
            let provider_id = provider.id();
            let context =
                ProviderContext::new(self.local.clone(), input_tx.clone(), cancel.child_token());
            provider_tasks.push(tokio::spawn(async move {
                let starting = context
                    .set_state(provider_id.clone(), ProviderState::Starting)
                    .await;
                if starting.is_err() {
                    return;
                }

                let result = provider.run(context.clone()).await;
                let state = match result {
                    Ok(()) if context.cancellation_token().is_cancelled() => ProviderState::Stopped,
                    Ok(()) => ProviderState::Failed {
                        recoverable: true,
                        reason: "provider stopped unexpectedly".to_owned(),
                    },
                    Err(error) => ProviderState::Failed {
                        recoverable: true,
                        reason: error.to_string(),
                    },
                };
                let _send_result = context.set_state(provider_id, state).await;
            }));
        }

        let handle = DiscoveryHandle {
            input: input_tx,
            events: event_tx,
        };
        Ok(DiscoverySession {
            handle,
            cancel,
            provider_tasks,
            manager_task,
        })
    }
}

fn validate_provider_ids(providers: &[Arc<dyn DiscoveryProvider>]) -> Result<(), DiscoveryError> {
    let mut ids = HashSet::with_capacity(providers.len());
    for provider in providers {
        let id = provider.id();
        if !ids.insert(id.clone()) {
            return Err(DiscoveryError::InvalidConfig(format!(
                "duplicate provider id '{id}'"
            )));
        }
    }
    Ok(())
}

/// Cloneable command and event handle used by core and native provider adapters.
#[derive(Clone)]
pub struct DiscoveryHandle {
    input: mpsc::Sender<ManagerInput>,
    events: broadcast::Sender<DiscoveryEvent>,
}

impl DiscoveryHandle {
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DiscoveryEvent> {
        self.events.subscribe()
    }

    /// Submits an observation from a platform-native provider such as BLE.
    pub async fn submit_observation(&self, observation: Observation) -> Result<(), DiscoveryError> {
        self.input
            .send(ManagerInput::Observation(observation))
            .await
            .map_err(|_| DiscoveryError::SessionClosed)
    }

    /// Withdraws one provider's evidence without deleting other sources.
    pub async fn withdraw(
        &self,
        provider: ProviderId,
        presence_id: PresenceId,
    ) -> Result<(), DiscoveryError> {
        self.input
            .send(ManagerInput::Withdraw {
                provider,
                presence_id,
            })
            .await
            .map_err(|_| DiscoveryError::SessionClosed)
    }

    /// Reports native provider health or permission state.
    pub async fn report_provider_state(
        &self,
        provider: ProviderId,
        state: ProviderState,
    ) -> Result<(), DiscoveryError> {
        self.input
            .send(ManagerInput::ProviderState { provider, state })
            .await
            .map_err(|_| DiscoveryError::SessionClosed)
    }

    /// Feeds a real secure connection result back into endpoint ranking.
    pub async fn report_connection_result(
        &self,
        presence_id: PresenceId,
        endpoint: Endpoint,
        outcome: ConnectionOutcome,
    ) -> Result<(), DiscoveryError> {
        self.input
            .send(ManagerInput::ConnectionResult {
                presence_id,
                endpoint,
                outcome,
            })
            .await
            .map_err(|_| DiscoveryError::SessionClosed)
    }

    /// Returns a coherent snapshot after all previously submitted commands.
    pub async fn snapshot(&self) -> Result<Vec<PeerSnapshot>, DiscoveryError> {
        let (sender, receiver) = oneshot::channel();
        self.input
            .send(ManagerInput::Snapshot(sender))
            .await
            .map_err(|_| DiscoveryError::SessionClosed)?;
        receiver.await.map_err(|_| DiscoveryError::SessionClosed)
    }
}

/// Owns provider and aggregation tasks for one discovery lifetime.
pub struct DiscoverySession {
    handle: DiscoveryHandle,
    cancel: CancellationToken,
    provider_tasks: Vec<JoinHandle<()>>,
    manager_task: JoinHandle<()>,
}

impl DiscoverySession {
    #[must_use]
    pub fn handle(&self) -> DiscoveryHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) -> Result<(), DiscoveryError> {
        self.cancel.cancel();

        for mut task in self.provider_tasks {
            if tokio::time::timeout(Duration::from_secs(3), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }

        self.manager_task
            .await
            .map_err(|error| DiscoveryError::TaskFailed(error.to_string()))
    }
}

struct PeerRecord {
    protocol: ProtocolRange,
    capabilities: crate::Capabilities,
    last_sequence: u64,
    sources: HashMap<ProviderId, Instant>,
    endpoints: HashMap<Endpoint, EndpointRecord>,
    preferred_endpoint: Option<Endpoint>,
    quarantined: bool,
}

async fn run_manager(
    local: LocalPresence,
    config: DiscoveryConfig,
    mut input: mpsc::Receiver<ManagerInput>,
    events: broadcast::Sender<DiscoveryEvent>,
    cancel: CancellationToken,
) {
    let mut peers: HashMap<PresenceId, PeerRecord> = HashMap::new();
    let mut expiry = tokio::time::interval(config.expiry_interval);
    expiry.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            maybe_input = input.recv() => {
                let Some(input) = maybe_input else { break };
                process_input(input, &local, &config, &mut peers, &events);
            }
            _ = expiry.tick() => expire_stale(&local, &mut peers, &events),
        }
    }
}

fn process_input(
    input: ManagerInput,
    local: &LocalPresence,
    config: &DiscoveryConfig,
    peers: &mut HashMap<PresenceId, PeerRecord>,
    events: &broadcast::Sender<DiscoveryEvent>,
) {
    match input {
        ManagerInput::Observation(observation) => {
            apply_observation(observation, local, config, peers, events);
        }
        ManagerInput::Withdraw {
            provider,
            presence_id,
        } => withdraw_provider(&provider, presence_id, local, peers, events),
        ManagerInput::ProviderState { provider, state } => {
            let _receiver_count = events.send(DiscoveryEvent::ProviderChanged { provider, state });
        }
        ManagerInput::ConnectionResult {
            presence_id,
            endpoint,
            outcome,
        } => apply_connection_result(presence_id, endpoint, outcome, local, peers, events),
        ManagerInput::Snapshot(sender) => {
            let mut snapshot = peers
                .iter()
                .map(|(&id, peer)| snapshot_peer(id, peer, local.protocol))
                .collect::<Vec<_>>();
            snapshot.sort_by_key(|peer| peer.presence_id);
            let _send_result = sender.send(snapshot);
        }
    }
}

fn apply_observation(
    mut observation: Observation,
    local: &LocalPresence,
    config: &DiscoveryConfig,
    peers: &mut HashMap<PresenceId, PeerRecord>,
    events: &broadcast::Sender<DiscoveryEvent>,
) {
    if observation.presence_id == local.presence_id || observation.ttl.is_zero() {
        return;
    }

    let is_new = !peers.contains_key(&observation.presence_id);
    if is_new && peers.len() >= config.max_peers {
        return;
    }

    observation.ttl = observation
        .ttl
        .clamp(config.min_observation_ttl, config.max_observation_ttl);
    observation
        .endpoints
        .sort_unstable_by_key(|endpoint| endpoint.address());
    observation.endpoints.dedup();
    observation
        .endpoints
        .truncate(config.max_endpoints_per_peer);

    let now = Instant::now();
    let expires_at = now + observation.ttl;
    let peer = peers
        .entry(observation.presence_id)
        .or_insert_with(|| PeerRecord {
            protocol: observation.protocol,
            capabilities: observation.capabilities,
            last_sequence: observation.sequence,
            sources: HashMap::new(),
            endpoints: HashMap::new(),
            preferred_endpoint: None,
            quarantined: false,
        });

    if observation.sequence >= peer.last_sequence {
        peer.protocol = observation.protocol;
        peer.capabilities = observation.capabilities;
        peer.last_sequence = observation.sequence;
    }
    peer.sources
        .insert(observation.provider.clone(), expires_at);

    for endpoint in observation.endpoints {
        if !peer.endpoints.contains_key(&endpoint)
            && peer.endpoints.len() >= config.max_endpoints_per_peer
        {
            continue;
        }
        let endpoint_record = peer
            .endpoints
            .entry(endpoint)
            .or_insert_with(EndpointRecord::new);
        endpoint_record
            .evidence
            .entry(observation.provider.clone())
            .and_modify(|evidence| {
                evidence.expires_at = expires_at;
                evidence.round_trip_time = observation.round_trip_time.or(evidence.round_trip_time);
                evidence.observations = evidence.observations.saturating_add(1);
            })
            .or_insert(EndpointEvidence {
                expires_at,
                round_trip_time: observation.round_trip_time,
                observations: 1,
            });
    }

    let snapshot = snapshot_peer(observation.presence_id, peer, local.protocol);
    let event = if is_new {
        DiscoveryEvent::PeerAppeared(snapshot)
    } else {
        DiscoveryEvent::PeerChanged(snapshot)
    };
    let _receiver_count = events.send(event);
}

fn withdraw_provider(
    provider: &ProviderId,
    presence_id: PresenceId,
    local: &LocalPresence,
    peers: &mut HashMap<PresenceId, PeerRecord>,
    events: &broadcast::Sender<DiscoveryEvent>,
) {
    let Some(peer) = peers.get_mut(&presence_id) else {
        return;
    };
    peer.sources.remove(provider);
    for endpoint in peer.endpoints.values_mut() {
        endpoint.evidence.remove(provider);
    }
    peer.endpoints
        .retain(|_, endpoint| !endpoint.evidence.is_empty());

    if peer.sources.is_empty() {
        peers.remove(&presence_id);
        let _receiver_count = events.send(DiscoveryEvent::PeerExpired(presence_id));
    } else {
        let snapshot = snapshot_peer(presence_id, peer, local.protocol);
        let _receiver_count = events.send(DiscoveryEvent::PeerChanged(snapshot));
    }
}

fn apply_connection_result(
    presence_id: PresenceId,
    endpoint: Endpoint,
    outcome: ConnectionOutcome,
    local: &LocalPresence,
    peers: &mut HashMap<PresenceId, PeerRecord>,
    events: &broadcast::Sender<DiscoveryEvent>,
) {
    let Some(peer) = peers.get_mut(&presence_id) else {
        return;
    };
    let Some(candidate) = peer.endpoints.get_mut(&endpoint) else {
        return;
    };

    match outcome {
        ConnectionOutcome::Success { handshake_time } => {
            candidate.successful_connections = candidate.successful_connections.saturating_add(1);
            candidate.consecutive_failures = 0;
            candidate.connection_rtt = Some(match candidate.connection_rtt {
                Some(previous) => average_duration(previous, handshake_time),
                None => handshake_time,
            });
            peer.preferred_endpoint = Some(endpoint);
        }
        ConnectionOutcome::Failure(failure) => {
            candidate.total_failures = candidate.total_failures.saturating_add(1);
            candidate.consecutive_failures = candidate.consecutive_failures.saturating_add(1);
            if peer.preferred_endpoint == Some(endpoint) && candidate.consecutive_failures >= 2 {
                peer.preferred_endpoint = None;
            }
            if failure == ConnectionFailure::AuthenticationFailed {
                peer.quarantined = true;
                let _receiver_count = events.send(DiscoveryEvent::PeerQuarantined(presence_id));
            }
        }
    }

    let snapshot = snapshot_peer(presence_id, peer, local.protocol);
    let _receiver_count = events.send(DiscoveryEvent::PeerChanged(snapshot));
}

fn average_duration(left: Duration, right: Duration) -> Duration {
    left.checked_add(right)
        .and_then(|sum| sum.checked_div(2))
        .unwrap_or(right)
}

fn expire_stale(
    local: &LocalPresence,
    peers: &mut HashMap<PresenceId, PeerRecord>,
    events: &broadcast::Sender<DiscoveryEvent>,
) {
    let now = Instant::now();
    let mut changed = Vec::new();
    let mut expired = Vec::new();

    for (&presence_id, peer) in peers.iter_mut() {
        let sources_before = peer.sources.len();
        let endpoints_before = peer.endpoints.len();
        peer.sources.retain(|_, expires_at| *expires_at > now);
        for endpoint in peer.endpoints.values_mut() {
            endpoint
                .evidence
                .retain(|_, evidence| evidence.expires_at > now);
        }
        peer.endpoints
            .retain(|_, endpoint| !endpoint.evidence.is_empty());

        if peer.sources.is_empty() {
            expired.push(presence_id);
        } else if peer.sources.len() != sources_before || peer.endpoints.len() != endpoints_before {
            changed.push(presence_id);
        }
    }

    for presence_id in expired {
        peers.remove(&presence_id);
        let _receiver_count = events.send(DiscoveryEvent::PeerExpired(presence_id));
    }
    for presence_id in changed {
        if let Some(peer) = peers.get(&presence_id) {
            let snapshot = snapshot_peer(presence_id, peer, local.protocol);
            let _receiver_count = events.send(DiscoveryEvent::PeerChanged(snapshot));
        }
    }
}

fn snapshot_peer(
    presence_id: PresenceId,
    peer: &PeerRecord,
    local_protocol: ProtocolRange,
) -> PeerSnapshot {
    let compatible = protocol_compatible(local_protocol, peer.protocol);
    let mut candidates = peer
        .endpoints
        .iter()
        .map(|(&endpoint, record)| record.candidate(endpoint, compatible))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.endpoint.cmp(&right.endpoint))
    });
    let preferred = peer.preferred_endpoint.and_then(|preferred| {
        candidates
            .iter()
            .find(|candidate| candidate.endpoint == preferred && candidate.consecutive_failures < 2)
            .map(|candidate| candidate.endpoint)
    });
    let best_endpoint = if peer.quarantined || !compatible {
        None
    } else {
        preferred.or_else(|| candidates.first().map(|candidate| candidate.endpoint))
    };

    PeerSnapshot {
        presence_id,
        protocol: peer.protocol,
        compatible,
        capabilities: peer.capabilities,
        sources: peer.sources.keys().cloned().collect::<BTreeSet<_>>(),
        candidates,
        best_endpoint,
        quarantined: peer.quarantined,
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use super::*;
    use crate::{Capabilities, ProviderKind};

    fn protocol() -> ProtocolRange {
        ProtocolRange::new(1, 2).unwrap_or_else(|error| panic!("test range: {error}"))
    }

    fn local() -> LocalPresence {
        LocalPresence::new(
            PresenceId::from_bytes([1; 16]),
            protocol(),
            Capabilities::default(),
            4433,
        )
        .unwrap_or_else(|error| panic!("test local: {error}"))
    }

    fn provider(kind: ProviderKind, name: &str) -> ProviderId {
        ProviderId::new(kind, name).unwrap_or_else(|error| panic!("test provider: {error}"))
    }

    fn endpoint(last: u8) -> Endpoint {
        Endpoint::quic(SocketAddr::from(([192, 0, 2, last], 4433)))
            .unwrap_or_else(|error| panic!("test endpoint: {error}"))
    }

    fn observation(
        presence_id: PresenceId,
        provider: ProviderId,
        endpoint: Option<Endpoint>,
        ttl: Duration,
    ) -> Observation {
        Observation {
            provider,
            presence_id,
            protocol: protocol(),
            capabilities: Capabilities::default(),
            sequence: 1,
            endpoints: endpoint.into_iter().collect(),
            ttl,
            round_trip_time: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn merges_sources_and_expires_them_independently() {
        let config = DiscoveryConfig {
            min_observation_ttl: Duration::from_secs(1),
            max_observation_ttl: Duration::from_secs(30),
            expiry_interval: Duration::from_millis(100),
            ..DiscoveryConfig::default()
        };
        let session = DiscoveryManager::new(local())
            .config(config)
            .start()
            .await
            .unwrap_or_else(|error| panic!("start: {error}"));
        let handle = session.handle();
        let remote = PresenceId::from_bytes([2; 16]);
        let address = endpoint(10);

        handle
            .submit_observation(observation(
                remote,
                provider(ProviderKind::Mdns, "mdns"),
                Some(address),
                Duration::from_secs(2),
            ))
            .await
            .unwrap_or_else(|error| panic!("observe mDNS: {error}"));
        handle
            .submit_observation(observation(
                remote,
                provider(ProviderKind::PresenceV4, "lan-v4"),
                Some(address),
                Duration::from_secs(10),
            ))
            .await
            .unwrap_or_else(|error| panic!("observe v4: {error}"));

        let peers = handle
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].sources.len(), 2);
        assert_eq!(peers[0].candidates[0].sources.len(), 2);

        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
        let peers = handle
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].sources.len(), 1);

        session
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
    }

    #[tokio::test]
    async fn actual_connection_results_change_best_endpoint() {
        let session = DiscoveryManager::new(local())
            .start()
            .await
            .unwrap_or_else(|error| panic!("start: {error}"));
        let handle = session.handle();
        let remote = PresenceId::from_bytes([3; 16]);
        let first = endpoint(1);
        let second = endpoint(2);
        let source = provider(ProviderKind::Mdns, "mdns");
        let mut seen = observation(remote, source, Some(first), Duration::from_secs(10));
        seen.endpoints.push(second);
        handle
            .submit_observation(seen)
            .await
            .unwrap_or_else(|error| panic!("observe: {error}"));

        handle
            .report_connection_result(
                remote,
                second,
                ConnectionOutcome::Success {
                    handshake_time: Duration::from_millis(15),
                },
            )
            .await
            .unwrap_or_else(|error| panic!("report: {error}"));
        let peers = handle
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert_eq!(peers[0].best_endpoint, Some(second));

        session
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
    }

    #[tokio::test]
    async fn successful_endpoint_is_sticky_until_repeated_failure() {
        let session = DiscoveryManager::new(local())
            .start()
            .await
            .unwrap_or_else(|error| panic!("start: {error}"));
        let handle = session.handle();
        let remote = PresenceId::from_bytes([9; 16]);
        let first = endpoint(1);
        let second = endpoint(2);
        let mut seen = observation(
            remote,
            provider(ProviderKind::Mdns, "mdns"),
            Some(first),
            Duration::from_secs(10),
        );
        seen.endpoints.push(second);
        handle
            .submit_observation(seen)
            .await
            .unwrap_or_else(|error| panic!("observe: {error}"));
        handle
            .report_connection_result(
                remote,
                second,
                ConnectionOutcome::Success {
                    handshake_time: Duration::from_millis(20),
                },
            )
            .await
            .unwrap_or_else(|error| panic!("success: {error}"));

        handle
            .submit_observation(observation(
                remote,
                provider(ProviderKind::Direct, "direct"),
                Some(first),
                Duration::from_secs(10),
            ))
            .await
            .unwrap_or_else(|error| panic!("corroborate: {error}"));
        let peers = handle
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert_eq!(peers[0].best_endpoint, Some(second));

        for _ in 0..2 {
            handle
                .report_connection_result(
                    remote,
                    second,
                    ConnectionOutcome::Failure(ConnectionFailure::Timeout),
                )
                .await
                .unwrap_or_else(|error| panic!("failure: {error}"));
        }
        let peers = handle
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert_eq!(peers[0].best_endpoint, Some(first));

        session
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
    }

    #[tokio::test]
    async fn authentication_failure_quarantines_whole_presence() {
        let session = DiscoveryManager::new(local())
            .start()
            .await
            .unwrap_or_else(|error| panic!("start: {error}"));
        let handle = session.handle();
        let remote = PresenceId::from_bytes([4; 16]);
        let address = endpoint(4);
        handle
            .submit_observation(observation(
                remote,
                provider(ProviderKind::Direct, "direct"),
                Some(address),
                Duration::from_secs(10),
            ))
            .await
            .unwrap_or_else(|error| panic!("observe: {error}"));
        handle
            .report_connection_result(
                remote,
                address,
                ConnectionOutcome::Failure(ConnectionFailure::AuthenticationFailed),
            )
            .await
            .unwrap_or_else(|error| panic!("report: {error}"));
        let peers = handle
            .snapshot()
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert!(peers[0].quarantined);
        assert_eq!(peers[0].best_endpoint, None);

        session
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
    }
}
