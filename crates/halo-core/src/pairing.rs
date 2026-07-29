use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use halo_crypto::{DeviceIdentity, FileTrustStore, PairingCode, PeerId, SecretIdentityBlob};
use halo_protocol::{Capabilities, ProtocolRange};
use halo_transport::{
    ConnectErrorKind, ConnectionCandidate, ConnectionRacer, PairingFlowError, PairingPrompt,
    PairingUserInteraction, QuicEndpoint, pair_as_initiator, pair_as_responder,
};
use thiserror::Error;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

const EVENT_LIMIT: usize = 128;
const MAX_CANDIDATES: usize = 8;
const MAX_PAIRING_CONNECTIONS: usize = 4;
const USER_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(60);

/// Configuration for one foreground pairing service.
#[derive(Debug)]
pub struct PairingConfig {
    identity_blob: Option<Vec<u8>>,
    trust_store_directory: PathBuf,
    bind_address: SocketAddr,
}

impl PairingConfig {
    #[must_use]
    pub fn new(trust_store_directory: impl Into<PathBuf>) -> Self {
        Self {
            identity_blob: None,
            trust_store_directory: trust_store_directory.into(),
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        }
    }

    /// Supplies bytes loaded from the platform protected-blob adapter.
    #[must_use]
    pub fn with_identity_blob(mut self, identity_blob: Vec<u8>) -> Self {
        self.identity_blob = Some(identity_blob);
        self
    }

    /// Primarily useful for deterministic tests and embedded applications.
    #[must_use]
    pub fn with_bind_address(mut self, bind_address: SocketAddr) -> Self {
        self.bind_address = bind_address;
        self
    }
}

/// Result of starting the SDK service. Persist `new_identity_blob` through an
/// OS-backed adapter before advertising `listen_port`.
pub struct PairingStartup {
    pub service: PairingService,
    pub listen_port: u16,
    new_identity_blob: Option<SecretIdentityBlob>,
}

impl PairingStartup {
    #[must_use]
    pub fn new_identity_blob(&self) -> Option<&[u8]> {
        self.new_identity_blob
            .as_ref()
            .map(SecretIdentityBlob::as_bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingEventKind {
    Connecting,
    CodeAvailable,
    ConfirmationRequired,
    Trusted,
    Rejected,
    IdentityChanged,
    TimedOut,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingEvent {
    pub event_id: u64,
    pub request_id: Option<u64>,
    pub kind: PairingEventKind,
    /// Caller-provided correlation value, normally a discovery Presence ID.
    pub peer_reference: Option<String>,
    pub peer_fingerprint: Option<String>,
    pub short_code: Option<String>,
    pub already_trusted: bool,
    /// Stable category only; never contains addresses, keys, or filesystem paths.
    pub detail: Option<String>,
}

/// One Rust-owned listener/client and pairing workflow.
pub struct PairingService {
    endpoint: Arc<QuicEndpoint>,
    identity: Arc<DeviceIdentity>,
    trust_store: Arc<FileTrustStore>,
    shared: Arc<PairingShared>,
    cancellation: CancellationToken,
}

impl PairingService {
    pub async fn start(config: PairingConfig) -> Result<PairingStartup, PairingError> {
        let trust_store = Arc::new(FileTrustStore::new(config.trust_store_directory)?);
        let (identity, new_identity_blob) = match config.identity_blob {
            Some(bytes) => {
                let blob = SecretIdentityBlob::new(bytes)?;
                (DeviceIdentity::from_blob(&blob)?, None)
            }
            None => {
                let identity = DeviceIdentity::generate()?;
                let blob = identity.to_blob();
                (identity, Some(blob))
            }
        };
        let endpoint = Arc::new(QuicEndpoint::server(config.bind_address)?);
        let listen_port = endpoint.local_addr()?.port();
        let service = Self {
            endpoint,
            identity: Arc::new(identity),
            trust_store,
            shared: Arc::new(PairingShared::default()),
            cancellation: CancellationToken::new(),
        };
        tokio::spawn(accept_loop(
            Arc::clone(&service.endpoint),
            Arc::clone(&service.identity),
            Arc::clone(&service.trust_store),
            Arc::clone(&service.shared),
            service.cancellation.clone(),
        ));
        Ok(PairingStartup {
            service,
            listen_port,
            new_identity_blob,
        })
    }

    /// Starts a bounded connection race and returns after the task is accepted.
    pub async fn connect(
        &self,
        peer_reference: String,
        endpoints: Vec<SocketAddr>,
    ) -> Result<(), PairingError> {
        if peer_reference.is_empty() || peer_reference.len() > 64 {
            return Err(PairingError::InvalidPeerReference);
        }
        let mut unique = HashSet::new();
        let addresses = endpoints
            .into_iter()
            .filter(|address| unique.insert(*address))
            .take(MAX_CANDIDATES)
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(PairingError::NoEndpoints);
        }
        let permit = Arc::clone(&self.shared.connection_slots)
            .try_acquire_owned()
            .map_err(|_| PairingError::Busy)?;
        let reference = Some(peer_reference);
        self.shared.emit(new_event(
            PairingEventKind::Connecting,
            reference.clone(),
            None,
            None,
        ))?;
        tokio::spawn(run_initiator(InitiatorTask {
            endpoint: Arc::clone(&self.endpoint),
            identity: Arc::clone(&self.identity),
            trust_store: Arc::clone(&self.trust_store),
            shared: Arc::clone(&self.shared),
            cancellation: self.cancellation.child_token(),
            reference,
            addresses,
            _permit: permit,
        }));
        Ok(())
    }

    pub fn events_after(&self, event_id: u64) -> Result<Vec<PairingEvent>, PairingError> {
        self.shared.events_after(event_id)
    }

    pub fn respond(&self, request_id: u64, accepted: bool) -> Result<(), PairingError> {
        let sender = self
            .shared
            .pending
            .lock()
            .map_err(|_| PairingError::InternalState)?
            .remove(&request_id)
            .ok_or(PairingError::RequestNotPending)?;
        sender
            .send(accepted)
            .map_err(|_| PairingError::RequestNotPending)
    }

    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        self.endpoint.close();
        self.endpoint.wait_idle().await;
    }
}

impl Drop for PairingService {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.endpoint.close();
    }
}

struct PairingShared {
    next_event_id: AtomicU64,
    next_request_id: AtomicU64,
    events: Mutex<VecDeque<PairingEvent>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    connection_slots: Arc<Semaphore>,
}

impl Default for PairingShared {
    fn default() -> Self {
        Self {
            next_event_id: AtomicU64::new(0),
            next_request_id: AtomicU64::new(0),
            events: Mutex::new(VecDeque::new()),
            pending: Mutex::new(HashMap::new()),
            connection_slots: Arc::new(Semaphore::new(MAX_PAIRING_CONNECTIONS)),
        }
    }
}

impl PairingShared {
    fn emit(&self, mut event: PairingEvent) -> Result<(), PairingError> {
        event.event_id = self.next_event_id.fetch_add(1, Ordering::Relaxed) + 1;
        let mut events = self
            .events
            .lock()
            .map_err(|_| PairingError::InternalState)?;
        if events.len() == EVENT_LIMIT {
            events.pop_front();
        }
        events.push_back(event);
        Ok(())
    }

    fn events_after(&self, event_id: u64) -> Result<Vec<PairingEvent>, PairingError> {
        self.events
            .lock()
            .map_err(|_| PairingError::InternalState)
            .map(|events| {
                events
                    .iter()
                    .filter(|event| event.event_id > event_id)
                    .cloned()
                    .collect()
            })
    }
}

struct EventInteraction {
    shared: Arc<PairingShared>,
    peer_reference: Option<String>,
    cancellation: CancellationToken,
}

#[async_trait]
impl PairingUserInteraction for EventInteraction {
    async fn present(&self, prompt: PairingPrompt) -> Result<bool, PairingFlowError> {
        let fingerprint = peer_fingerprint(prompt.peer_id);
        let code = pairing_code_text(prompt.code);
        if !prompt.confirmation_required {
            self.shared
                .emit(new_event(
                    PairingEventKind::CodeAvailable,
                    self.peer_reference.clone(),
                    Some(fingerprint),
                    Some(code),
                ))
                .map_err(|_| PairingFlowError::UserInterface)?;
            return Ok(true);
        }

        let request_id = self.shared.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .map_err(|_| PairingFlowError::UserInterface)?
            .insert(request_id, sender);
        let mut event = new_event(
            PairingEventKind::ConfirmationRequired,
            self.peer_reference.clone(),
            Some(fingerprint),
            Some(code),
        );
        event.request_id = Some(request_id);
        self.shared
            .emit(event)
            .map_err(|_| PairingFlowError::UserInterface)?;

        let response = tokio::select! {
            () = self.cancellation.cancelled() => Err(PairingFlowError::UserCancelled),
            result = timeout(USER_CONFIRMATION_TIMEOUT, receiver) => match result {
                Ok(Ok(accepted)) => Ok(accepted),
                Ok(Err(_)) | Err(_) => Err(PairingFlowError::UserCancelled),
            },
        };
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.remove(&request_id);
        }
        response
    }
}

struct InitiatorTask {
    endpoint: Arc<QuicEndpoint>,
    identity: Arc<DeviceIdentity>,
    trust_store: Arc<FileTrustStore>,
    shared: Arc<PairingShared>,
    cancellation: CancellationToken,
    reference: Option<String>,
    addresses: Vec<SocketAddr>,
    _permit: OwnedSemaphorePermit,
}

async fn accept_loop(
    endpoint: Arc<QuicEndpoint>,
    identity: Arc<DeviceIdentity>,
    trust_store: Arc<FileTrustStore>,
    shared: Arc<PairingShared>,
    cancellation: CancellationToken,
) {
    loop {
        let connection = match endpoint.accept(cancellation.child_token()).await {
            Ok(connection) => connection,
            Err(_) if cancellation.is_cancelled() => break,
            Err(_) => continue,
        };
        let permit = match Arc::clone(&shared.connection_slots).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                connection.close();
                continue;
            }
        };
        let identity = Arc::clone(&identity);
        let trust_store = Arc::clone(&trust_store);
        let shared = Arc::clone(&shared);
        let child_cancellation = cancellation.child_token();
        tokio::spawn(async move {
            let _permit = permit;
            let result = async {
                let remote_ip = connection.remote_address().ip();
                let expected = trust_store.load_expected_for_ip(remote_ip).await?;
                let mut io = connection.accept_control().await?;
                let interaction = EventInteraction {
                    shared: Arc::clone(&shared),
                    peer_reference: None,
                    cancellation: child_cancellation,
                };
                let outcome = pair_as_responder(
                    &mut io,
                    &identity,
                    protocol_versions()?,
                    Capabilities::default(),
                    expected.as_ref(),
                    trust_store.as_ref(),
                    &interaction,
                )
                .await?;
                trust_store.bind_ip(remote_ip, &outcome.peer).await?;
                Ok(outcome)
            }
            .await;
            emit_outcome(&shared, None, result);
            connection.close();
        });
    }
}

async fn run_initiator(task: InitiatorTask) {
    let InitiatorTask {
        endpoint,
        identity,
        trust_store,
        shared,
        cancellation,
        reference,
        addresses,
        _permit,
    } = task;
    let candidates = addresses
        .into_iter()
        .enumerate()
        .map(|(index, address)| ConnectionCandidate {
            endpoint: address,
            start_delay: Duration::from_millis((index as u64).saturating_mul(150)),
        })
        .collect();
    let racer = match ConnectionRacer::new(Duration::from_secs(8), MAX_CANDIDATES) {
        Ok(racer) => racer,
        Err(_) => {
            emit_ignoring_closed(&shared, failed_event(reference, "configuration"));
            return;
        }
    };
    let (_, connection) = match racer
        .connect(
            Arc::clone(&endpoint),
            candidates,
            cancellation.child_token(),
        )
        .await
    {
        Ok(connected) => connected,
        Err(error) => {
            let kind = match error.kind {
                ConnectErrorKind::Cancelled => PairingEventKind::Cancelled,
                ConnectErrorKind::Timeout => PairingEventKind::TimedOut,
                ConnectErrorKind::IdentityChanged => PairingEventKind::IdentityChanged,
                _ => PairingEventKind::Failed,
            };
            let mut event = new_event(kind, reference, None, None);
            event.detail = Some(connect_error_category(error.kind).to_owned());
            emit_ignoring_closed(&shared, event);
            return;
        }
    };
    let remote_ip = connection.remote_address().ip();
    let result = async {
        let expected = trust_store.load_expected_for_ip(remote_ip).await?;
        let mut io = connection.open_control().await?;
        let interaction = EventInteraction {
            shared: Arc::clone(&shared),
            peer_reference: reference.clone(),
            cancellation,
        };
        let outcome = pair_as_initiator(
            &mut io,
            &identity,
            protocol_versions()?,
            Capabilities::default(),
            expected.as_ref(),
            trust_store.as_ref(),
            &interaction,
        )
        .await?;
        trust_store.bind_ip(remote_ip, &outcome.peer).await?;
        Ok(outcome)
    }
    .await;
    connection.close();
    emit_outcome(&shared, reference, result);
}

fn protocol_versions() -> Result<ProtocolRange, PairingFlowError> {
    ProtocolRange::new(1, 1).map_err(|_| PairingFlowError::UnexpectedMessage)
}

fn emit_outcome(
    shared: &PairingShared,
    reference: Option<String>,
    result: Result<halo_transport::PairingOutcome, PairingFlowError>,
) {
    let event = match result {
        Ok(outcome) => {
            let mut event = new_event(
                PairingEventKind::Trusted,
                reference,
                Some(peer_fingerprint(outcome.peer.peer_id())),
                None,
            );
            event.already_trusted = outcome.already_trusted;
            event
        }
        Err(PairingFlowError::IdentityChanged) => {
            new_event(PairingEventKind::IdentityChanged, reference, None, None)
        }
        Err(PairingFlowError::Rejected) => {
            new_event(PairingEventKind::Rejected, reference, None, None)
        }
        Err(PairingFlowError::UserCancelled) => {
            new_event(PairingEventKind::TimedOut, reference, None, None)
        }
        Err(error) => {
            let mut event = new_event(PairingEventKind::Failed, reference, None, None);
            event.detail = Some(pairing_error_category(&error).to_owned());
            event
        }
    };
    emit_ignoring_closed(shared, event);
}

fn emit_ignoring_closed(shared: &PairingShared, event: PairingEvent) {
    let _ = shared.emit(event);
}

fn new_event(
    kind: PairingEventKind,
    reference: Option<String>,
    fingerprint: Option<String>,
    code: Option<String>,
) -> PairingEvent {
    PairingEvent {
        event_id: 0,
        request_id: None,
        kind,
        peer_reference: reference,
        peer_fingerprint: fingerprint,
        short_code: code,
        already_trusted: false,
        detail: None,
    }
}

fn failed_event(reference: Option<String>, detail: &str) -> PairingEvent {
    let mut event = new_event(PairingEventKind::Failed, reference, None, None);
    event.detail = Some(detail.to_owned());
    event
}

fn pairing_code_text(code: PairingCode) -> String {
    format!("{:06}", code.value())
}

fn peer_fingerprint(peer_id: PeerId) -> String {
    peer_id.as_bytes()[..6]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn connect_error_category(kind: ConnectErrorKind) -> &'static str {
    match kind {
        ConnectErrorKind::InvalidConfiguration => "connect_configuration",
        ConnectErrorKind::Cancelled => "connect_cancelled",
        ConnectErrorKind::Timeout => "connect_timeout",
        ConnectErrorKind::Unreachable => "connect_unreachable",
        ConnectErrorKind::Tls => "connect_tls",
        ConnectErrorKind::Authentication => "connect_authentication",
        ConnectErrorKind::Protocol => "connect_protocol",
        ConnectErrorKind::IdentityChanged => "connect_identity_changed",
        ConnectErrorKind::NetworkChanged => "connect_network_changed",
        ConnectErrorKind::InternalTask => "connect_internal",
    }
}

fn pairing_error_category(error: &PairingFlowError) -> &'static str {
    match error {
        PairingFlowError::Io(_) => "control_io",
        PairingFlowError::Crypto(_) => "authentication",
        PairingFlowError::Store(_) => "persistence",
        PairingFlowError::UnexpectedMessage => "protocol",
        PairingFlowError::IdentityChanged => "identity_changed",
        PairingFlowError::Rejected => "rejected",
        PairingFlowError::UserCancelled => "cancelled",
        PairingFlowError::UserInterface => "user_interface",
    }
}

#[derive(Debug, Error)]
pub enum PairingError {
    #[error("invalid peer reference")]
    InvalidPeerReference,
    #[error("no usable connection endpoint")]
    NoEndpoints,
    #[error("pairing service is busy")]
    Busy,
    #[error("pairing request is no longer pending")]
    RequestNotPending,
    #[error("pairing internal state is unavailable")]
    InternalState,
    #[error("protected identity is invalid")]
    Identity,
    #[error("trust persistence failed")]
    Store,
    #[error("QUIC endpoint setup failed")]
    Endpoint,
}

impl From<halo_crypto::IdentityError> for PairingError {
    fn from(_: halo_crypto::IdentityError) -> Self {
        Self::Identity
    }
}

impl From<halo_crypto::StoreError> for PairingError {
    fn from(_: halo_crypto::StoreError) -> Self {
        Self::Store
    }
}

impl From<halo_transport::QuicEndpointError> for PairingError {
    fn from(_: halo_transport::QuicEndpointError) -> Self {
        Self::Endpoint
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halo-core-pairing-{name}-{}", std::process::id()))
    }

    async fn wait_for(
        service: &PairingService,
        predicate: impl Fn(&PairingEvent) -> bool,
    ) -> PairingEvent {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            let events = service
                .events_after(0)
                .unwrap_or_else(|error| panic!("pairing events: {error}"));
            if let Some(event) = events.into_iter().find(&predicate) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for pairing event")
    }

    #[tokio::test]
    async fn public_facade_pairs_and_recognizes_after_restart() {
        let first_directory = test_directory("first");
        let second_directory = test_directory("second");
        let first = PairingService::start(
            PairingConfig::new(&first_directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("first start: {error}"));
        let second = PairingService::start(
            PairingConfig::new(&second_directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("second start: {error}"));
        let first_blob = first
            .new_identity_blob()
            .unwrap_or_else(|| panic!("first identity"))
            .to_vec();
        let second_blob = second
            .new_identity_blob()
            .unwrap_or_else(|| panic!("second identity"))
            .to_vec();
        first
            .service
            .connect(
                "first-contact".to_owned(),
                vec![SocketAddr::from((Ipv4Addr::LOCALHOST, second.listen_port))],
            )
            .await
            .unwrap_or_else(|error| panic!("connect: {error}"));
        let request = wait_for(&second.service, |event| {
            event.kind == PairingEventKind::ConfirmationRequired
        })
        .await;
        second
            .service
            .respond(
                request.request_id.unwrap_or_else(|| panic!("request id")),
                true,
            )
            .unwrap_or_else(|error| panic!("respond: {error}"));
        assert!(
            !wait_for(&first.service, |event| event.kind
                == PairingEventKind::Trusted)
            .await
            .already_trusted
        );
        assert!(
            !wait_for(&second.service, |event| event.kind
                == PairingEventKind::Trusted)
            .await
            .already_trusted
        );
        first.service.shutdown().await;
        second.service.shutdown().await;
        drop(first);
        drop(second);

        let first = PairingService::start(
            PairingConfig::new(&first_directory)
                .with_identity_blob(first_blob.clone())
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("first restart: {error}"));
        let second = PairingService::start(
            PairingConfig::new(&second_directory)
                .with_identity_blob(second_blob)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("second restart: {error}"));
        first
            .service
            .connect(
                "restart".to_owned(),
                vec![SocketAddr::from((Ipv4Addr::LOCALHOST, second.listen_port))],
            )
            .await
            .unwrap_or_else(|error| panic!("reconnect: {error}"));
        assert!(
            wait_for(&first.service, |event| event.kind
                == PairingEventKind::Trusted)
            .await
            .already_trusted
        );
        assert!(
            wait_for(&second.service, |event| event.kind
                == PairingEventKind::Trusted)
            .await
            .already_trusted
        );
        first.service.shutdown().await;
        second.service.shutdown().await;
        drop(first);
        drop(second);

        let imposter_directory = test_directory("imposter");
        let first = PairingService::start(
            PairingConfig::new(&first_directory)
                .with_identity_blob(first_blob)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("identity-change client: {error}"));
        let imposter = PairingService::start(
            PairingConfig::new(&imposter_directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("imposter: {error}"));
        first
            .service
            .connect(
                "imposter".to_owned(),
                vec![SocketAddr::from((
                    Ipv4Addr::LOCALHOST,
                    imposter.listen_port,
                ))],
            )
            .await
            .unwrap_or_else(|error| panic!("imposter connect: {error}"));
        assert_eq!(
            wait_for(&first.service, |event| event.kind
                == PairingEventKind::IdentityChanged)
            .await
            .kind,
            PairingEventKind::IdentityChanged
        );
        first.service.shutdown().await;
        imposter.service.shutdown().await;
        let _ = tokio::fs::remove_dir_all(first_directory).await;
        let _ = tokio::fs::remove_dir_all(second_directory).await;
        let _ = tokio::fs::remove_dir_all(imposter_directory).await;
    }
}
