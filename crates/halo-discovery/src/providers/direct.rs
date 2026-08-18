use std::{
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    time::Duration,
};

use async_trait::async_trait;
use tokio::{net::UdpSocket as TokioUdpSocket, task::JoinSet};

use crate::{
    DiscoveryProvider, Endpoint, Observation, PresenceId, ProviderContext, ProviderError,
    ProviderId, ProviderKind, ProviderState,
    wire::{MessageKind, PresenceMessage},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KnownEndpoint {
    /// Optional presence expected during the same app lifetime. Remembered
    /// addresses normally leave this unset because Presence IDs rotate.
    pub expected_presence: Option<PresenceId>,
    pub discovery_address: SocketAddr,
}

#[derive(Clone, Debug)]
pub struct DirectProbeConfig {
    pub endpoints: Vec<KnownEndpoint>,
    pub probe_interval: Duration,
    pub response_timeout: Duration,
    pub observation_ttl: Duration,
    pub max_endpoints: usize,
}

impl Default for DirectProbeConfig {
    fn default() -> Self {
        Self {
            endpoints: Vec::new(),
            probe_interval: Duration::from_secs(8),
            response_timeout: Duration::from_secs(2),
            observation_ttl: Duration::from_secs(20),
            max_endpoints: 8,
        }
    }
}

pub struct DirectProbeProvider {
    id: ProviderId,
    config: DirectProbeConfig,
    bound_socket: Option<StdUdpSocket>,
}

impl DirectProbeProvider {
    pub fn new(config: DirectProbeConfig) -> Result<Self, ProviderError> {
        validate_config(&config)?;
        Ok(Self {
            id: ProviderId::builtin(ProviderKind::Direct, "direct"),
            config,
            bound_socket: None,
        })
    }

    pub fn with_bound_socket(
        config: DirectProbeConfig,
        socket: StdUdpSocket,
    ) -> Result<Self, ProviderError> {
        validate_config(&config)?;
        Ok(Self {
            id: ProviderId::builtin(ProviderKind::Direct, "direct"),
            config,
            bound_socket: Some(socket),
        })
    }
}

#[async_trait]
impl DiscoveryProvider for DirectProbeProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn run(&self, context: ProviderContext) -> Result<(), ProviderError> {
        if self.config.endpoints.is_empty() {
            context
                .set_state(
                    self.id(),
                    ProviderState::TemporarilyUnavailable(
                        "no remembered discovery endpoints".to_owned(),
                    ),
                )
                .await?;
            context.cancellation_token().cancelled().await;
            return Ok(());
        }

        context.set_state(self.id(), ProviderState::Ready).await?;
        if let Some(socket) = &self.bound_socket {
            let socket = socket
                .try_clone()
                .map_err(|error| ProviderError::Network(error.to_string()))?;
            socket
                .set_nonblocking(true)
                .map_err(|error| ProviderError::Network(error.to_string()))?;
            let socket = TokioUdpSocket::from_std(socket)
                .map_err(|error| ProviderError::Network(error.to_string()))?;
            return probe_bound_endpoints(context, self.id(), self.config.clone(), socket).await;
        }
        let mut tasks = JoinSet::new();
        for (index, endpoint) in self.config.endpoints.iter().copied().enumerate() {
            let context = context.clone();
            let provider = self.id();
            let config = self.config.clone();
            tasks.spawn(async move {
                probe_endpoint(context, provider, config, endpoint, index as u64).await
            });
        }

        let mut failures = 0_usize;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) if context.cancellation_token().is_cancelled() => {}
                Ok(Ok(())) => failures += 1,
                Ok(Err(error)) => {
                    failures += 1;
                    context
                        .set_state(
                            self.id(),
                            ProviderState::Degraded(format!(
                                "{failures} direct probe target(s) failed: {error}"
                            )),
                        )
                        .await?;
                }
                Err(error) => {
                    failures += 1;
                    context
                        .set_state(
                            self.id(),
                            ProviderState::Degraded(format!(
                                "{failures} direct probe task(s) failed: {error}"
                            )),
                        )
                        .await?;
                }
            }
        }

        if context.cancellation_token().is_cancelled() {
            Ok(())
        } else {
            Err(ProviderError::Unavailable(
                "all direct probe targets stopped".to_owned(),
            ))
        }
    }
}

async fn probe_endpoint(
    context: ProviderContext,
    provider: ProviderId,
    config: DirectProbeConfig,
    known: KnownEndpoint,
    salt: u64,
) -> Result<(), ProviderError> {
    let bind_address = match known.discovery_address {
        SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::V6(_) => SocketAddr::from(([0_u16; 8], 0)),
    };
    let socket = TokioUdpSocket::bind(bind_address).await?;
    let cancel = context.cancellation_token();
    let mut sequence = salt;
    let mut buffer = [0_u8; 512];

    loop {
        sequence = sequence.wrapping_add(1);
        let nonce = random_nonce();
        let reply_port = socket.local_addr()?.port();
        let query =
            PresenceMessage::from_local(context.local(), MessageKind::Query, sequence, nonce)
                .with_reply_port(reply_port)
                .encode();
        let sent_at = tokio::time::Instant::now();
        socket.send_to(&query, known.discovery_address).await?;

        let response = tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            response = tokio::time::timeout(config.response_timeout, async {
                loop {
                    let (length, source) = socket.recv_from(&mut buffer).await?;
                    let Ok(message) = PresenceMessage::decode(&buffer[..length]) else {
                        continue;
                    };
                    if message.kind == MessageKind::Response
                        && message.nonce == nonce
                        && known
                            .expected_presence
                            .is_none_or(|expected| message.presence_id == expected)
                        && source.ip() == known.discovery_address.ip()
                    {
                        return Ok::<_, std::io::Error>((message, source));
                    }
                }
            }) => response,
        };

        if let Ok(Ok((message, mut source))) = response {
            source.set_port(message.quic_port);
            if let Ok(endpoint) = Endpoint::quic(source) {
                context
                    .observe(Observation {
                        provider: provider.clone(),
                        presence_id: message.presence_id,
                        protocol: message.protocol,
                        capabilities: message.capabilities,
                        sequence: message.sequence,
                        endpoints: vec![endpoint],
                        ttl: config.observation_ttl,
                        round_trip_time: Some(sent_at.elapsed()),
                    })
                    .await?;
            }
        }

        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            () = tokio::time::sleep(config.probe_interval) => {}
        }
    }
}

async fn probe_bound_endpoints(
    context: ProviderContext,
    provider: ProviderId,
    config: DirectProbeConfig,
    socket: TokioUdpSocket,
) -> Result<(), ProviderError> {
    let cancel = context.cancellation_token();
    let mut sequence = 0_u64;
    let mut buffer = [0_u8; 512];
    loop {
        for known in &config.endpoints {
            if cancel.is_cancelled() {
                return Ok(());
            }
            sequence = sequence.wrapping_add(1);
            let nonce = random_nonce();
            let reply_port = socket.local_addr()?.port();
            let query =
                PresenceMessage::from_local(context.local(), MessageKind::Query, sequence, nonce)
                    .with_reply_port(reply_port)
                    .encode();
            let sent_at = tokio::time::Instant::now();
            if socket
                .send_to(&query, known.discovery_address)
                .await
                .is_err()
            {
                continue;
            }
            let response = tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                response = tokio::time::timeout(config.response_timeout, async {
                    loop {
                        let (length, source) = socket.recv_from(&mut buffer).await?;
                        let Ok(message) = PresenceMessage::decode(&buffer[..length]) else {
                            continue;
                        };
                        if message.kind == MessageKind::Response
                            && message.nonce == nonce
                            && known.expected_presence.is_none_or(
                                |expected| message.presence_id == expected
                            )
                            && source.ip() == known.discovery_address.ip()
                        {
                            return Ok::<_, std::io::Error>((message, source));
                        }
                    }
                }) => response,
            };
            if let Ok(Ok((message, mut source))) = response {
                source.set_port(message.quic_port);
                if let Ok(endpoint) = Endpoint::quic(source) {
                    context
                        .observe(Observation {
                            provider: provider.clone(),
                            presence_id: message.presence_id,
                            protocol: message.protocol,
                            capabilities: message.capabilities,
                            sequence: message.sequence,
                            endpoints: vec![endpoint],
                            ttl: config.observation_ttl,
                            round_trip_time: Some(sent_at.elapsed()),
                        })
                        .await?;
                }
            }
        }
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            () = tokio::time::sleep(config.probe_interval) => {}
        }
    }
}

fn validate_config(config: &DirectProbeConfig) -> Result<(), ProviderError> {
    if config.probe_interval.is_zero()
        || config.response_timeout.is_zero()
        || config.observation_ttl.is_zero()
        || config.max_endpoints == 0
    {
        return Err(ProviderError::InvalidConfig(
            "direct probe timing and limits must be non-zero".to_owned(),
        ));
    }
    if config.endpoints.len() > config.max_endpoints {
        return Err(ProviderError::InvalidConfig(format!(
            "direct probe has {} endpoints, exceeding limit {}",
            config.endpoints.len(),
            config.max_endpoints
        )));
    }
    if config
        .endpoints
        .iter()
        .any(|endpoint| endpoint.discovery_address.port() == 0)
    {
        return Err(ProviderError::InvalidConfig(
            "remembered discovery endpoints must use a non-zero port".to_owned(),
        ));
    }
    Ok(())
}

fn random_nonce() -> u64 {
    let id = PresenceId::random();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&id.as_bytes()[8..]);
    u64::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capabilities, DiscoveryManager, LocalPresence, ProtocolRange};

    #[test]
    fn rejects_unbounded_target_list() {
        let endpoint = KnownEndpoint {
            expected_presence: Some(PresenceId::from_bytes([1; 16])),
            discovery_address: SocketAddr::from(([192, 0, 2, 1], 44721)),
        };
        let config = DirectProbeConfig {
            endpoints: vec![endpoint, endpoint],
            max_endpoints: 1,
            ..DirectProbeConfig::default()
        };
        assert!(DirectProbeProvider::new(config).is_err());
    }

    #[test]
    fn nonce_is_randomized() {
        assert_ne!(random_nonce(), random_nonce());
    }

    #[tokio::test]
    async fn discovers_a_known_endpoint_through_real_udp() {
        let responder = TokioUdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap_or_else(|error| panic!("bind responder: {error}"));
        let discovery_address = responder
            .local_addr()
            .unwrap_or_else(|error| panic!("responder address: {error}"));
        let remote_id = PresenceId::from_bytes([7; 16]);
        let remote = LocalPresence::new(
            remote_id,
            ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("range: {error}")),
            Capabilities::from_bits(9),
            49_999,
        )
        .unwrap_or_else(|error| panic!("remote: {error}"));
        let responder_task = tokio::spawn(async move {
            let mut bytes = [0_u8; 512];
            let (length, source) = responder
                .recv_from(&mut bytes)
                .await
                .unwrap_or_else(|error| panic!("receive query: {error}"));
            let query = PresenceMessage::decode(&bytes[..length])
                .unwrap_or_else(|error| panic!("decode query: {error}"));
            let response =
                PresenceMessage::from_local(&remote, MessageKind::Response, 2, query.nonce)
                    .encode();
            responder
                .send_to(&response, source)
                .await
                .unwrap_or_else(|error| panic!("send response: {error}"));
        });

        let local = LocalPresence::new(
            PresenceId::from_bytes([8; 16]),
            ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("range: {error}")),
            Capabilities::default(),
            48_888,
        )
        .unwrap_or_else(|error| panic!("local: {error}"));
        let provider = DirectProbeProvider::new(DirectProbeConfig {
            endpoints: vec![KnownEndpoint {
                expected_presence: Some(remote_id),
                discovery_address,
            }],
            probe_interval: Duration::from_secs(30),
            response_timeout: Duration::from_secs(2),
            observation_ttl: Duration::from_secs(10),
            max_endpoints: 1,
        })
        .unwrap_or_else(|error| panic!("provider: {error}"));
        let session = DiscoveryManager::new(local)
            .with_provider(provider)
            .start()
            .await
            .unwrap_or_else(|error| panic!("session: {error}"));
        let handle = session.handle();

        let peers = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let peers = handle
                    .snapshot()
                    .await
                    .unwrap_or_else(|error| panic!("snapshot: {error}"));
                if !peers.is_empty() {
                    break peers;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("direct discovery timed out"));

        assert_eq!(peers[0].presence_id, remote_id);
        assert_eq!(
            peers[0].best_endpoint.map(Endpoint::address),
            Some(SocketAddr::from(([127, 0, 0, 1], 49_999)))
        );
        responder_task
            .await
            .unwrap_or_else(|error| panic!("responder task: {error}"));
        session
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
    }

    #[tokio::test]
    async fn bound_socket_discovers_a_known_endpoint_without_wildcard_rebind() {
        let responder = TokioUdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap_or_else(|error| panic!("bind responder: {error}"));
        let discovery_address = responder
            .local_addr()
            .unwrap_or_else(|error| panic!("responder address: {error}"));
        let remote_id = PresenceId::from_bytes([9; 16]);
        let remote = LocalPresence::new(
            remote_id,
            ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("range: {error}")),
            Capabilities::from_bits(11),
            49_997,
        )
        .unwrap_or_else(|error| panic!("remote: {error}"));
        let responder_task = tokio::spawn(async move {
            let mut bytes = [0_u8; 512];
            let (length, source) = responder
                .recv_from(&mut bytes)
                .await
                .unwrap_or_else(|error| panic!("receive query: {error}"));
            let query = PresenceMessage::decode(&bytes[..length])
                .unwrap_or_else(|error| panic!("decode query: {error}"));
            let response =
                PresenceMessage::from_local(&remote, MessageKind::Response, 2, query.nonce)
                    .encode();
            responder
                .send_to(&response, source)
                .await
                .unwrap_or_else(|error| panic!("send response: {error}"));
        });
        let bound = StdUdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap_or_else(|error| panic!("bound socket: {error}"));
        let local = LocalPresence::new(
            PresenceId::from_bytes([10; 16]),
            ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("range: {error}")),
            Capabilities::default(),
            48_887,
        )
        .unwrap_or_else(|error| panic!("local: {error}"));
        let provider = DirectProbeProvider::with_bound_socket(
            DirectProbeConfig {
                endpoints: vec![KnownEndpoint {
                    expected_presence: Some(remote_id),
                    discovery_address,
                }],
                probe_interval: Duration::from_secs(30),
                response_timeout: Duration::from_secs(2),
                observation_ttl: Duration::from_secs(10),
                max_endpoints: 1,
            },
            bound,
        )
        .unwrap_or_else(|error| panic!("provider: {error}"));
        let session = DiscoveryManager::new(local)
            .with_provider(provider)
            .start()
            .await
            .unwrap_or_else(|error| panic!("session: {error}"));
        let peers = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let peers = session
                    .handle()
                    .snapshot()
                    .await
                    .unwrap_or_else(|error| panic!("snapshot: {error}"));
                if !peers.is_empty() {
                    break peers;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("bound direct discovery timed out"));
        assert_eq!(peers[0].presence_id, remote_id);
        responder_task
            .await
            .unwrap_or_else(|error| panic!("responder task: {error}"));
        session
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
    }
}
