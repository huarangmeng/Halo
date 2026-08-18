use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use halo_crypto::{DeviceIdentity, FileTrustStore, PairingCode, PeerId, SecretIdentityBlob};
use halo_discovery::is_local_network_ip;
use halo_protocol::{Capabilities, ProtocolRange};
use halo_transport::{
    ConnectErrorKind, ConnectionCandidate, ConnectionRacer, ControlIo, DataChannelCost,
    DataChannelPathClass, EstablishedPathProperties, LocalNetworkScope, PairingFlowError,
    PairingPrompt, PairingUserInteraction, PlatformControlDriver, PlatformControlError,
    PlatformControlIo, QuicConnection, QuicEndpoint, pair_as_initiator, pair_as_responder,
    platform_control_channel,
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::transfer::{
    ReceiveTransferDecision, TransferCoordinator, TransferFileSource, TransferPolicy,
};
use crate::{TransferEvent, TransferServiceError};

const EVENT_LIMIT: usize = 128;
const MAX_CANDIDATES: usize = 8;
const MAX_PAIRING_CONNECTIONS: usize = 4;
const MAX_PLATFORM_CHANNELS: usize = 8;
const MAX_AUTHENTICATED_SESSIONS: usize = 8;
const MAX_RECENT_PEER_ATTEMPTS: usize = 128;
const MIN_CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const MIN_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_RETRY_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PairingPolicy {
    pub connect_attempt_timeout: Duration,
    pub confirmation_timeout: Duration,
    pub retry_cooldown: Duration,
}

impl Default for PairingPolicy {
    fn default() -> Self {
        Self {
            connect_attempt_timeout: Duration::from_secs(8),
            confirmation_timeout: Duration::from_secs(60),
            retry_cooldown: Duration::from_secs(2),
        }
    }
}

impl PairingPolicy {
    pub fn validate(self) -> Result<Self, PairingError> {
        if self.connect_attempt_timeout < MIN_CONNECT_ATTEMPT_TIMEOUT
            || self.connect_attempt_timeout > MAX_CONNECT_ATTEMPT_TIMEOUT
            || self.confirmation_timeout < MIN_CONFIRMATION_TIMEOUT
            || self.confirmation_timeout > MAX_CONFIRMATION_TIMEOUT
            || self.retry_cooldown.is_zero()
            || self.retry_cooldown > MAX_RETRY_COOLDOWN
        {
            return Err(PairingError::InvalidPairingPolicy);
        }
        Ok(self)
    }
}

/// Configuration for one foreground pairing service.
#[derive(Debug)]
pub struct PairingConfig {
    identity_blob: Option<Vec<u8>>,
    trust_store_directory: PathBuf,
    transfer_state_directory: PathBuf,
    bind_address: SocketAddr,
    bound_socket: Option<(UdpSocket, EstablishedPathProperties)>,
    pairing_policy: PairingPolicy,
    transfer_policy: TransferPolicy,
}

impl PairingConfig {
    #[must_use]
    pub fn new(trust_store_directory: impl Into<PathBuf>) -> Self {
        let trust_store_directory = trust_store_directory.into();
        let transfer_state_directory = trust_store_directory.join("transfer-resume");
        Self {
            identity_blob: None,
            trust_store_directory,
            transfer_state_directory,
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            bound_socket: None,
            pairing_policy: PairingPolicy::default(),
            transfer_policy: TransferPolicy::default(),
        }
    }

    /// Supplies bytes loaded from the platform protected-blob adapter.
    #[must_use]
    pub fn with_identity_blob(mut self, identity_blob: Vec<u8>) -> Self {
        self.identity_blob = Some(identity_blob);
        self
    }

    #[must_use]
    pub fn with_transfer_state_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.transfer_state_directory = directory.into();
        self
    }

    /// Primarily useful for deterministic tests and embedded applications.
    #[must_use]
    pub fn with_bind_address(mut self, bind_address: SocketAddr) -> Self {
        self.bind_address = bind_address;
        self.bound_socket = None;
        self
    }

    /// Uses a UDP socket that a platform adapter already bound to an approved
    /// local interface or OS network. The socket becomes owned by Quinn.
    #[must_use]
    pub fn with_bound_socket(
        mut self,
        socket: UdpSocket,
        path_properties: EstablishedPathProperties,
    ) -> Self {
        self.bound_socket = Some((socket, path_properties));
        self
    }

    /// Uses a UDP socket that a platform adapter has pinned to a local,
    /// unmetered OS network. This is the preferred entry point for platform
    /// adapters because callers cannot accidentally attest broader path
    /// properties than the adapter contract permits.
    #[must_use]
    pub fn with_bound_local_unmetered_socket(self, socket: UdpSocket) -> Self {
        self.with_bound_socket(
            socket,
            EstablishedPathProperties {
                path_class: DataChannelPathClass::LocalNetwork,
                cost: DataChannelCost::Unmetered,
                local_network_scope: Some(LocalNetworkScope::Shared),
                interface_bound: true,
            },
        )
    }

    /// Uses a UDP socket pinned to a foreground, user-approved local-only
    /// hotspot. The platform may report this Wi-Fi path as metered or unknown,
    /// but it must never supply a cellular, Internet, or VPN route here.
    #[must_use]
    pub fn with_bound_user_approved_hotspot_socket(self, socket: UdpSocket) -> Self {
        self.with_bound_socket(
            socket,
            EstablishedPathProperties {
                path_class: DataChannelPathClass::LocalNetwork,
                cost: DataChannelCost::Metered,
                local_network_scope: Some(LocalNetworkScope::UserApprovedHotspot),
                interface_bound: true,
            },
        )
    }

    #[must_use]
    pub fn with_transfer_policy(mut self, transfer_policy: TransferPolicy) -> Self {
        self.transfer_policy = transfer_policy;
        self
    }

    #[must_use]
    pub fn with_pairing_policy(mut self, pairing_policy: PairingPolicy) -> Self {
        self.pairing_policy = pairing_policy;
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

pub struct PlatformTlsIdentity {
    pub certificate_der: Vec<u8>,
    pub private_key_x963: Vec<u8>,
}

pub fn create_platform_tls_identity() -> Result<PlatformTlsIdentity, PairingError> {
    let identity = halo_transport::generate_native_tls_identity()?;
    Ok(PlatformTlsIdentity {
        certificate_der: identity.certificate_der,
        private_key_x963: identity.private_key_x963,
    })
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
    Disconnected,
    Revoked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformPairingRole {
    Initiator,
    Responder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformPairingChannelState {
    Pending,
    Authenticated,
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
    /// Present only when Rust retained the authenticated LAN QUIC connection.
    /// Native Apple sessions remain owned by their platform adapter.
    pub authenticated_session_id: Option<u64>,
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
        let PairingConfig {
            identity_blob,
            trust_store_directory,
            transfer_state_directory,
            bind_address,
            bound_socket,
            pairing_policy,
            transfer_policy,
        } = config;
        let pairing_policy = pairing_policy.validate()?;
        tokio::fs::create_dir_all(&transfer_state_directory)
            .await
            .map_err(|_| PairingError::TransferState)?;
        let transfer_state_metadata = tokio::fs::symlink_metadata(&transfer_state_directory)
            .await
            .map_err(|_| PairingError::TransferState)?;
        if !transfer_state_metadata.file_type().is_dir()
            || transfer_state_metadata.file_type().is_symlink()
        {
            return Err(PairingError::TransferState);
        }
        let trust_store = Arc::new(FileTrustStore::new(trust_store_directory)?);
        let (identity, new_identity_blob) = match identity_blob {
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
        let endpoint = Arc::new(match bound_socket {
            Some((socket, properties)) => {
                let eligible = matches!(
                    (
                        properties.path_class,
                        properties.local_network_scope,
                        properties.cost,
                    ),
                    (
                        DataChannelPathClass::LocalNetwork,
                        Some(LocalNetworkScope::Shared),
                        DataChannelCost::Unmetered,
                    ) | (
                        DataChannelPathClass::LocalNetwork,
                        Some(LocalNetworkScope::UserApprovedHotspot),
                        _,
                    ) | (
                        DataChannelPathClass::PeerToPeer,
                        None,
                        DataChannelCost::Unmetered
                    )
                );
                if !properties.interface_bound || !eligible {
                    return Err(PairingError::IneligiblePath);
                }
                QuicEndpoint::server_with_socket(socket)?
            }
            None => QuicEndpoint::server(bind_address)?,
        });
        let listen_port = endpoint.local_addr()?.port();
        let service = Self {
            endpoint,
            identity: Arc::new(identity),
            trust_store,
            shared: Arc::new(
                PairingShared::new(transfer_policy, pairing_policy, transfer_state_directory)
                    .map_err(|_| PairingError::InvalidTransferPolicy)?,
            ),
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
            .filter(|address| is_local_network_ip(address.ip()))
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
        let peer_attempt = match self.shared.reserve_peer_attempt(reference.as_deref()) {
            Ok(attempt) => attempt,
            Err(error @ PairingError::RateLimited) => {
                self.shared
                    .emit(failed_event(reference.clone(), "retry_rate_limited"))?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
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
            _peer_attempt: peer_attempt,
            _permit: permit,
        }));
        Ok(())
    }

    /// Attaches an already-established native QUIC control stream. The caller
    /// supplies the TLS exporter from that exact connection and relays only
    /// complete bounded Halo frames through the returned opaque channel ID.
    pub fn attach_platform_channel(
        &self,
        peer_reference: Option<String>,
        role: PlatformPairingRole,
        channel_binding: [u8; 32],
    ) -> Result<u64, PairingError> {
        if peer_reference
            .as_ref()
            .is_some_and(|reference| reference.is_empty() || reference.len() > 64)
        {
            return Err(PairingError::InvalidPeerReference);
        }
        let permit = Arc::clone(&self.shared.connection_slots)
            .try_acquire_owned()
            .map_err(|_| PairingError::Busy)?;
        let peer_attempt = match self.shared.reserve_peer_attempt(peer_reference.as_deref()) {
            Ok(attempt) => attempt,
            Err(error @ PairingError::RateLimited) => {
                self.shared
                    .emit(failed_event(peer_reference.clone(), "retry_rate_limited"))?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let (driver, io) =
            platform_control_channel(halo_crypto::TlsChannelBinding::new(channel_binding));
        let channel_id = self.shared.insert_platform_channel(driver)?;
        if let Err(error) = self.shared.emit(new_event(
            PairingEventKind::Connecting,
            peer_reference.clone(),
            None,
            None,
        )) {
            let _ = self.shared.remove_platform_channel(channel_id);
            return Err(error);
        }
        tokio::spawn(run_platform_pairing(PlatformPairingTask {
            channel_id,
            io,
            role,
            identity: Arc::clone(&self.identity),
            trust_store: Arc::clone(&self.trust_store),
            shared: Arc::clone(&self.shared),
            cancellation: self.cancellation.child_token(),
            reference: peer_reference,
            _peer_attempt: peer_attempt,
            _permit: permit,
        }));
        Ok(channel_id)
    }

    pub fn submit_platform_frame(
        &self,
        channel_id: u64,
        frame: Vec<u8>,
    ) -> Result<(), PairingError> {
        self.shared
            .with_platform_channel(channel_id, |driver| driver.try_submit_frame(frame))?
            .map_err(platform_control_error)
    }

    pub fn drain_platform_frames(
        &self,
        channel_id: u64,
        maximum_frames: usize,
    ) -> Result<Vec<Vec<u8>>, PairingError> {
        self.shared
            .with_platform_channel(channel_id, |driver| driver.drain_outbound(maximum_frames))?
            .map_err(platform_control_error)
    }

    pub fn close_platform_channel(&self, channel_id: u64) -> Result<(), PairingError> {
        self.shared.remove_platform_channel(channel_id)
    }

    pub fn platform_channel_state(
        &self,
        channel_id: u64,
    ) -> Result<PlatformPairingChannelState, PairingError> {
        self.shared.platform_channel_state(channel_id)
    }

    pub fn authenticated_sessions(&self) -> Result<Vec<AuthenticatedSessionInfo>, PairingError> {
        self.shared.authenticated_sessions()
    }

    pub async fn remembered_endpoint_addresses(&self) -> Result<Vec<IpAddr>, PairingError> {
        self.trust_store
            .remembered_endpoints(8)
            .await
            .map(|endpoints| {
                endpoints
                    .into_iter()
                    .map(|endpoint| endpoint.address)
                    .collect()
            })
            .map_err(Into::into)
    }

    pub async fn remembered_peers(&self) -> Result<Vec<RememberedPeerInfo>, PairingError> {
        let peers = self.trust_store.trusted_peers(128).await?;
        let sessions = self
            .shared
            .authenticated_sessions
            .lock()
            .map_err(|_| PairingError::InternalState)?;
        Ok(peers
            .into_iter()
            .map(|peer| {
                let peer_id = peer.peer_id();
                RememberedPeerInfo {
                    handle: peer_handle(peer_id),
                    fingerprint: peer_fingerprint(peer_id),
                    active_session_id: sessions.iter().find_map(|(session_id, session)| {
                        (session.peer_id == peer_id).then_some(*session_id)
                    }),
                }
            })
            .collect())
    }

    pub async fn revoke_authenticated_peer(&self, session_id: u64) -> Result<(), PairingError> {
        let peer_id = {
            let sessions = self
                .shared
                .authenticated_sessions
                .lock()
                .map_err(|_| PairingError::InternalState)?;
            let session = sessions
                .get(&session_id)
                .ok_or(PairingError::AuthenticatedSessionNotFound)?;
            session.peer_id
        };
        self.revoke_peer_id(peer_id).await
    }

    pub async fn revoke_remembered_peer(&self, handle: &str) -> Result<(), PairingError> {
        let peer_id = parse_peer_handle(handle).ok_or(PairingError::InvalidPeerHandle)?;
        self.revoke_peer_id(peer_id).await
    }

    async fn revoke_peer_id(&self, peer_id: PeerId) -> Result<(), PairingError> {
        let active = {
            let sessions = self
                .shared
                .authenticated_sessions
                .lock()
                .map_err(|_| PairingError::InternalState)?;
            sessions
                .iter()
                .filter(|(_, session)| session.peer_id == peer_id)
                .map(|(session_id, session)| {
                    (
                        *session_id,
                        session.connection.clone(),
                        session.peer_reference.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        self.trust_store.revoke_peer(peer_id).await?;
        for (session_id, connection, _) in &active {
            self.shared.remove_authenticated_session(*session_id, false);
            connection.close();
        }
        let mut event = new_event(
            PairingEventKind::Revoked,
            active
                .first()
                .and_then(|(_, _, reference)| reference.clone()),
            Some(peer_fingerprint(peer_id)),
            None,
        );
        event.authenticated_session_id = active.first().map(|(session_id, _, _)| *session_id);
        event.detail = Some("trust_revoked".to_owned());
        self.shared.emit(event)
    }

    /// Starts one transfer on a retained authenticated LAN QUIC session.
    ///
    /// A single file is represented as a one-entry resumable manifest.
    pub async fn send_file(
        &self,
        authenticated_session_id: u64,
        source_path: PathBuf,
        advertised_name: Option<String>,
    ) -> Result<String, TransferServiceError> {
        self.shared
            .transfers
            .send_file(authenticated_session_id, source_path, advertised_name)
            .await
    }

    pub async fn send_files(
        &self,
        authenticated_session_id: u64,
        sources: Vec<TransferFileSource>,
    ) -> Result<String, TransferServiceError> {
        self.shared
            .transfers
            .send_files(
                authenticated_session_id,
                sources
                    .into_iter()
                    .map(|source| {
                        halo_transfer::BatchSource::new(source.source_path, source.advertised_name)
                    })
                    .collect(),
            )
            .await
    }

    pub fn transfer_events_after(
        &self,
        event_id: u64,
    ) -> Result<Vec<TransferEvent>, TransferServiceError> {
        self.shared.transfers.events_after(event_id)
    }

    pub fn respond_to_transfer(
        &self,
        request_id: u64,
        accepted: bool,
        staging_directory: Option<PathBuf>,
        destination_directory: Option<PathBuf>,
    ) -> Result<(), TransferServiceError> {
        self.respond_to_transfer_with_space(
            request_id,
            accepted,
            staging_directory,
            destination_directory,
            None,
        )
    }

    pub fn respond_to_transfer_with_space(
        &self,
        request_id: u64,
        accepted: bool,
        staging_directory: Option<PathBuf>,
        destination_directory: Option<PathBuf>,
        available_bytes: Option<u64>,
    ) -> Result<(), TransferServiceError> {
        self.shared.transfers.respond(
            request_id,
            ReceiveTransferDecision {
                accepted,
                staging_directory,
                destination_directory,
                available_bytes,
            },
        )
    }

    pub fn pause_transfer(&self, transfer_id: &str) -> Result<(), TransferServiceError> {
        self.shared.transfers.pause(transfer_id)
    }

    pub async fn retry_transfer(
        &self,
        authenticated_session_id: u64,
        transfer_id: &str,
    ) -> Result<String, TransferServiceError> {
        self.shared
            .transfers
            .retry(authenticated_session_id, transfer_id)
            .await
    }

    pub async fn cancel_transfer(&self, transfer_id: &str) -> Result<(), TransferServiceError> {
        self.shared.transfers.cancel(transfer_id).await
    }

    pub async fn take_finished_transfer_sources(
        &self,
        transfer_id: &str,
    ) -> Result<Vec<PathBuf>, TransferServiceError> {
        self.shared
            .transfers
            .take_finished_sources(transfer_id)
            .await
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
        self.shared.transfers.shutdown();
        self.shared.cancel_platform_channels();
        self.shared.close_authenticated_sessions();
        self.endpoint.close();
        self.endpoint.wait_idle().await;
    }
}

impl Drop for PairingService {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.shared.transfers.shutdown();
        self.shared.cancel_platform_channels();
        self.shared.close_authenticated_sessions();
        self.endpoint.close();
    }
}

struct PairingShared {
    next_event_id: AtomicU64,
    next_request_id: AtomicU64,
    events: Mutex<VecDeque<PairingEvent>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    next_platform_channel_id: AtomicU64,
    platform_channels: Mutex<HashMap<u64, PlatformChannelEntry>>,
    next_authenticated_session_id: AtomicU64,
    authenticated_sessions: Mutex<HashMap<u64, AuthenticatedQuicSession>>,
    transfers: Arc<TransferCoordinator>,
    connection_slots: Arc<Semaphore>,
    active_peer_attempts: Mutex<HashSet<String>>,
    recent_peer_attempts: Mutex<HashMap<String, Instant>>,
    pairing_policy: PairingPolicy,
}

struct PeerAttemptGuard {
    shared: std::sync::Weak<PairingShared>,
    key: String,
}

impl Drop for PeerAttemptGuard {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade()
            && let Ok(mut active) = shared.active_peer_attempts.lock()
        {
            active.remove(&self.key);
            drop(active);
            if let Ok(mut recent) = shared.recent_peer_attempts.lock() {
                if recent.len() >= MAX_RECENT_PEER_ATTEMPTS && !recent.contains_key(&self.key) {
                    let oldest = recent
                        .iter()
                        .min_by_key(|(_, completed_at)| **completed_at)
                        .map(|(key, _)| key.clone());
                    if let Some(oldest) = oldest {
                        recent.remove(&oldest);
                    }
                }
                recent.insert(self.key.clone(), Instant::now());
            }
        }
    }
}

struct PlatformChannelEntry {
    driver: PlatformControlDriver,
    state: PlatformPairingChannelState,
}

struct AuthenticatedQuicSession {
    connection: QuicConnection,
    peer_id: PeerId,
    peer_reference: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedSessionInfo {
    pub session_id: u64,
    pub peer_fingerprint: String,
    pub peer_reference: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RememberedPeerInfo {
    /// Opaque command handle. Applications must not display or log it.
    pub handle: String,
    pub fingerprint: String,
    pub active_session_id: Option<u64>,
}

impl PairingShared {
    fn new(
        transfer_policy: TransferPolicy,
        pairing_policy: PairingPolicy,
        transfer_state_directory: PathBuf,
    ) -> Result<Self, TransferServiceError> {
        Ok(Self {
            next_event_id: AtomicU64::new(0),
            next_request_id: AtomicU64::new(0),
            events: Mutex::new(VecDeque::new()),
            pending: Mutex::new(HashMap::new()),
            next_platform_channel_id: AtomicU64::new(0),
            platform_channels: Mutex::new(HashMap::new()),
            next_authenticated_session_id: AtomicU64::new(0),
            authenticated_sessions: Mutex::new(HashMap::new()),
            transfers: Arc::new(TransferCoordinator::new(
                transfer_policy,
                transfer_state_directory,
            )?),
            connection_slots: Arc::new(Semaphore::new(MAX_PAIRING_CONNECTIONS)),
            active_peer_attempts: Mutex::new(HashSet::new()),
            recent_peer_attempts: Mutex::new(HashMap::new()),
            pairing_policy,
        })
    }

    fn reserve_peer_attempt(
        self: &Arc<Self>,
        peer_reference: Option<&str>,
    ) -> Result<Option<PeerAttemptGuard>, PairingError> {
        let Some(peer_reference) = peer_reference else {
            return Ok(None);
        };
        let key = peer_reference.to_ascii_lowercase();
        let now = Instant::now();
        let mut active = self
            .active_peer_attempts
            .lock()
            .map_err(|_| PairingError::InternalState)?;
        if !active.insert(key.clone()) {
            return Err(PairingError::Busy);
        }
        let mut recent = match self.recent_peer_attempts.lock() {
            Ok(recent) => recent,
            Err(_) => {
                active.remove(&key);
                return Err(PairingError::InternalState);
            }
        };
        recent.retain(|_, completed_at| {
            now.saturating_duration_since(*completed_at) < self.pairing_policy.retry_cooldown
        });
        if recent.contains_key(&key) {
            active.remove(&key);
            return Err(PairingError::RateLimited);
        }
        Ok(Some(PeerAttemptGuard {
            shared: Arc::downgrade(self),
            key,
        }))
    }
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

    fn insert_platform_channel(&self, driver: PlatformControlDriver) -> Result<u64, PairingError> {
        let mut channels = self
            .platform_channels
            .lock()
            .map_err(|_| PairingError::InternalState)?;
        if channels.len() >= MAX_PLATFORM_CHANNELS {
            return Err(PairingError::Busy);
        }
        let channel_id = self
            .next_platform_channel_id
            .fetch_add(1, Ordering::Relaxed)
            .checked_add(1)
            .ok_or(PairingError::InternalState)?;
        if channel_id == 0
            || channels
                .insert(
                    channel_id,
                    PlatformChannelEntry {
                        driver,
                        state: PlatformPairingChannelState::Pending,
                    },
                )
                .is_some()
        {
            return Err(PairingError::InternalState);
        }
        Ok(channel_id)
    }

    fn with_platform_channel<T>(
        &self,
        channel_id: u64,
        operation: impl FnOnce(&mut PlatformControlDriver) -> T,
    ) -> Result<T, PairingError> {
        let mut channels = self
            .platform_channels
            .lock()
            .map_err(|_| PairingError::InternalState)?;
        let entry = channels
            .get_mut(&channel_id)
            .ok_or(PairingError::PlatformChannelNotFound)?;
        Ok(operation(&mut entry.driver))
    }

    fn set_platform_channel_state(&self, channel_id: u64, state: PlatformPairingChannelState) {
        if let Ok(mut channels) = self.platform_channels.lock()
            && let Some(entry) = channels.get_mut(&channel_id)
        {
            entry.state = state;
        }
    }

    fn platform_channel_state(
        &self,
        channel_id: u64,
    ) -> Result<PlatformPairingChannelState, PairingError> {
        self.platform_channels
            .lock()
            .map_err(|_| PairingError::InternalState)?
            .get(&channel_id)
            .map(|entry| entry.state)
            .ok_or(PairingError::PlatformChannelNotFound)
    }

    fn remove_platform_channel(&self, channel_id: u64) -> Result<(), PairingError> {
        let driver = self
            .platform_channels
            .lock()
            .map_err(|_| PairingError::InternalState)?
            .remove(&channel_id)
            .ok_or(PairingError::PlatformChannelNotFound)?;
        driver.driver.cancel();
        Ok(())
    }

    fn cancel_platform_channels(&self) {
        if let Ok(mut channels) = self.platform_channels.lock() {
            for entry in channels.values() {
                entry.driver.cancel();
            }
            channels.clear();
        }
    }

    fn insert_authenticated_session(
        self: &Arc<Self>,
        connection: QuicConnection,
        peer_id: PeerId,
        peer_binding: [u8; 32],
        peer_reference: Option<String>,
    ) -> Result<u64, PairingError> {
        let mut sessions = self
            .authenticated_sessions
            .lock()
            .map_err(|_| PairingError::InternalState)?;
        if let Some(existing_id) = sessions
            .iter()
            .find_map(|(session_id, session)| (session.peer_id == peer_id).then_some(*session_id))
        {
            connection.close();
            return Ok(existing_id);
        }
        if sessions.len() >= MAX_AUTHENTICATED_SESSIONS {
            return Err(PairingError::Busy);
        }
        let session_id = self
            .next_authenticated_session_id
            .fetch_add(1, Ordering::Relaxed)
            .checked_add(1)
            .ok_or(PairingError::InternalState)?;
        if session_id == 0 {
            return Err(PairingError::InternalState);
        }
        if sessions
            .insert(
                session_id,
                AuthenticatedQuicSession {
                    connection: connection.clone(),
                    peer_id,
                    peer_reference,
                },
            )
            .is_some()
        {
            return Err(PairingError::InternalState);
        }
        drop(sessions);
        if self
            .transfers
            .attach_session(session_id, connection.clone(), peer_binding)
            .is_err()
        {
            if let Ok(mut sessions) = self.authenticated_sessions.lock() {
                sessions.remove(&session_id);
            }
            return Err(PairingError::InternalState);
        }
        let monitor_connection = connection;
        let shared = Arc::downgrade(self);
        tokio::spawn(async move {
            monitor_connection.closed().await;
            if let Some(shared) = shared.upgrade() {
                shared.remove_authenticated_session(session_id, true);
            }
        });
        Ok(session_id)
    }

    fn remove_authenticated_session(&self, session_id: u64, emit_event: bool) {
        let session = self
            .authenticated_sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(&session_id));
        let Some(session) = session else {
            return;
        };
        self.transfers.detach_session(session_id);
        if emit_event {
            let mut event = new_event(
                PairingEventKind::Disconnected,
                session.peer_reference,
                Some(peer_fingerprint(session.peer_id)),
                None,
            );
            event.authenticated_session_id = Some(session_id);
            event.detail = Some("transport_closed".to_owned());
            emit_ignoring_closed(self, event);
        }
    }

    fn authenticated_sessions(&self) -> Result<Vec<AuthenticatedSessionInfo>, PairingError> {
        let sessions = self
            .authenticated_sessions
            .lock()
            .map_err(|_| PairingError::InternalState)?;
        let mut infos = sessions
            .iter()
            .map(|(session_id, session)| AuthenticatedSessionInfo {
                session_id: *session_id,
                peer_fingerprint: peer_fingerprint(session.peer_id),
                peer_reference: session.peer_reference.clone(),
            })
            .collect::<Vec<_>>();
        infos.sort_by_key(|info| info.session_id);
        Ok(infos)
    }

    fn close_authenticated_sessions(&self) {
        self.transfers.shutdown();
        if let Ok(mut sessions) = self.authenticated_sessions.lock() {
            for session in sessions.values() {
                session.connection.close();
            }
            sessions.clear();
        }
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
            result = timeout(self.shared.pairing_policy.confirmation_timeout, receiver) => match result {
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
    _peer_attempt: Option<PeerAttemptGuard>,
    _permit: OwnedSemaphorePermit,
}

struct PlatformPairingTask {
    channel_id: u64,
    io: PlatformControlIo,
    role: PlatformPairingRole,
    identity: Arc<DeviceIdentity>,
    trust_store: Arc<FileTrustStore>,
    shared: Arc<PairingShared>,
    cancellation: CancellationToken,
    reference: Option<String>,
    _peer_attempt: Option<PeerAttemptGuard>,
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
        if !is_local_network_ip(connection.remote_address().ip()) {
            connection.close();
            continue;
        }
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
                let pairing_result = pair_as_responder(
                    &mut io,
                    &identity,
                    protocol_versions()?,
                    Capabilities::default(),
                    expected.as_ref(),
                    trust_store.as_ref(),
                    &interaction,
                )
                .await;
                io.close().await;
                let outcome = pairing_result?;
                trust_store.bind_ip(remote_ip, &outcome.peer).await?;
                Ok(outcome)
            }
            .await;
            retain_quic_session_or_emit_failure(&shared, None, connection, result);
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
        _peer_attempt,
        _permit,
    } = task;
    let racer = match ConnectionRacer::new(
        shared.pairing_policy.connect_attempt_timeout,
        MAX_CANDIDATES,
    ) {
        Ok(racer) => racer,
        Err(_) => {
            emit_ignoring_closed(&shared, failed_event(reference, "configuration"));
            return;
        }
    };
    let mut remaining = addresses;
    while !remaining.is_empty() {
        let candidates = remaining
            .iter()
            .copied()
            .enumerate()
            .map(|(index, address)| ConnectionCandidate {
                endpoint: address,
                start_delay: Duration::from_millis((index as u64).saturating_mul(150)),
            })
            .collect();
        let (selected, connection) = match racer
            .connect(
                Arc::clone(&endpoint),
                candidates,
                cancellation.child_token(),
            )
            .await
        {
            Ok(connected) => connected,
            Err(error) => {
                emit_connect_failure(&shared, reference, error.kind);
                return;
            }
        };
        remaining.retain(|address| *address != selected);
        let remote_ip = connection.remote_address().ip();
        let result = async {
            let expected = trust_store.load_expected_for_ip(remote_ip).await?;
            let mut io = connection.open_control().await?;
            let interaction = EventInteraction {
                shared: Arc::clone(&shared),
                peer_reference: reference.clone(),
                cancellation: cancellation.child_token(),
            };
            let pairing_result = pair_as_initiator(
                &mut io,
                &identity,
                protocol_versions()?,
                Capabilities::default(),
                expected.as_ref(),
                trust_store.as_ref(),
                &interaction,
            )
            .await;
            io.close().await;
            let outcome = pairing_result?;
            trust_store.bind_ip(remote_ip, &outcome.peer).await?;
            Ok(outcome)
        }
        .await;
        match result {
            Ok(outcome) => {
                retain_quic_session_or_emit_failure(&shared, reference, connection, Ok(outcome));
                return;
            }
            Err(error)
                if is_recoverable_candidate_failure(&error)
                    && !remaining.is_empty()
                    && !cancellation.is_cancelled() =>
            {
                connection.close();
            }
            Err(error) => {
                connection.close();
                emit_outcome(&shared, reference, Err(error), None);
                return;
            }
        }
    }
}

fn emit_connect_failure(shared: &PairingShared, reference: Option<String>, kind: ConnectErrorKind) {
    let event_kind = match kind {
        ConnectErrorKind::Cancelled => PairingEventKind::Cancelled,
        ConnectErrorKind::Timeout => PairingEventKind::TimedOut,
        ConnectErrorKind::IdentityChanged => PairingEventKind::IdentityChanged,
        _ => PairingEventKind::Failed,
    };
    let mut event = new_event(event_kind, reference, None, None);
    event.detail = Some(connect_error_category(kind).to_owned());
    emit_ignoring_closed(shared, event);
}

fn is_recoverable_candidate_failure(error: &PairingFlowError) -> bool {
    matches!(
        error,
        PairingFlowError::Io(_) | PairingFlowError::Crypto(_) | PairingFlowError::UnexpectedMessage
    )
}

async fn run_platform_pairing(task: PlatformPairingTask) {
    let PlatformPairingTask {
        channel_id,
        mut io,
        role,
        identity,
        trust_store,
        shared,
        cancellation,
        reference,
        _peer_attempt,
        _permit,
    } = task;
    let interaction = EventInteraction {
        shared: Arc::clone(&shared),
        peer_reference: reference.clone(),
        cancellation,
    };
    let result = async {
        let versions = protocol_versions()?;
        match role {
            PlatformPairingRole::Initiator => {
                pair_as_initiator(
                    &mut io,
                    &identity,
                    versions,
                    Capabilities::default(),
                    None,
                    trust_store.as_ref(),
                    &interaction,
                )
                .await
            }
            PlatformPairingRole::Responder => {
                pair_as_responder(
                    &mut io,
                    &identity,
                    versions,
                    Capabilities::default(),
                    None,
                    trust_store.as_ref(),
                    &interaction,
                )
                .await
            }
        }
    }
    .await;
    shared.set_platform_channel_state(
        channel_id,
        if result.is_ok() {
            PlatformPairingChannelState::Authenticated
        } else {
            PlatformPairingChannelState::Failed
        },
    );
    io.close().await;
    emit_outcome(&shared, reference, result, None);
}

fn retain_quic_session_or_emit_failure(
    shared: &Arc<PairingShared>,
    reference: Option<String>,
    connection: QuicConnection,
    result: Result<halo_transport::PairingOutcome, PairingFlowError>,
) {
    match result {
        Ok(outcome) => {
            let peer_id = outcome.peer.peer_id();
            let peer_binding = Sha256::digest(outcome.peer.identity_key.as_bytes()).into();
            match shared.insert_authenticated_session(
                connection.clone(),
                peer_id,
                peer_binding,
                reference.clone(),
            ) {
                Ok(session_id) => {
                    emit_outcome(shared, reference, Ok(outcome), Some(session_id));
                }
                Err(_) => {
                    connection.close();
                    emit_ignoring_closed(
                        shared,
                        failed_event(reference, "authenticated_session_capacity"),
                    );
                }
            }
        }
        Err(error) => {
            connection.close();
            emit_outcome(shared, reference, Err(error), None);
        }
    }
}

fn protocol_versions() -> Result<ProtocolRange, PairingFlowError> {
    ProtocolRange::new(1, 1).map_err(|_| PairingFlowError::UnexpectedMessage)
}

fn emit_outcome(
    shared: &PairingShared,
    reference: Option<String>,
    result: Result<halo_transport::PairingOutcome, PairingFlowError>,
    authenticated_session_id: Option<u64>,
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
            event.authenticated_session_id = authenticated_session_id;
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
        authenticated_session_id: None,
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

fn peer_handle(peer_id: PeerId) -> String {
    peer_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_peer_handle(value: &str) -> Option<PeerId> {
    if value.len() != 32 || !value.is_ascii() {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16).ok()?;
    }
    Some(PeerId::from_bytes(bytes))
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
    #[error("pairing retry is temporarily rate limited")]
    RateLimited,
    #[error("pairing request is no longer pending")]
    RequestNotPending,
    #[error("authenticated session does not exist")]
    AuthenticatedSessionNotFound,
    #[error("remembered peer handle is invalid")]
    InvalidPeerHandle,
    #[error("platform pairing channel does not exist")]
    PlatformChannelNotFound,
    #[error("platform pairing channel is backpressured")]
    PlatformChannelBackpressure,
    #[error("platform pairing frame or drain limit is invalid")]
    InvalidPlatformFrame,
    #[error("pairing internal state is unavailable")]
    InternalState,
    #[error("protected identity is invalid")]
    Identity,
    #[error("trust persistence failed")]
    Store,
    #[error("QUIC endpoint setup failed")]
    Endpoint,
    #[error("transfer policy configuration is invalid")]
    InvalidTransferPolicy,
    #[error("transfer resume storage is unavailable")]
    TransferState,
    #[error("pairing policy configuration is invalid")]
    InvalidPairingPolicy,
    #[error("pre-bound QUIC socket does not satisfy the nearby path policy")]
    IneligiblePath,
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

fn platform_control_error(error: PlatformControlError) -> PairingError {
    match error {
        PlatformControlError::QueueFull => PairingError::PlatformChannelBackpressure,
        PlatformControlError::Closed => PairingError::PlatformChannelNotFound,
        PlatformControlError::InvalidFrameLength(_) | PlatformControlError::InvalidDrainLimit => {
            PairingError::InvalidPlatformFrame
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halo-core-pairing-{name}-{}", std::process::id()))
    }

    #[test]
    fn pairing_policy_rejects_unbounded_or_zero_timing() {
        for policy in [
            PairingPolicy {
                connect_attempt_timeout: Duration::ZERO,
                ..PairingPolicy::default()
            },
            PairingPolicy {
                connect_attempt_timeout: MAX_CONNECT_ATTEMPT_TIMEOUT + Duration::from_secs(1),
                ..PairingPolicy::default()
            },
            PairingPolicy {
                confirmation_timeout: Duration::from_secs(1),
                ..PairingPolicy::default()
            },
            PairingPolicy {
                confirmation_timeout: MAX_CONFIRMATION_TIMEOUT + Duration::from_secs(1),
                ..PairingPolicy::default()
            },
            PairingPolicy {
                retry_cooldown: Duration::ZERO,
                ..PairingPolicy::default()
            },
        ] {
            assert!(matches!(
                policy.validate(),
                Err(PairingError::InvalidPairingPolicy)
            ));
        }
        assert_eq!(
            PairingPolicy::default()
                .validate()
                .unwrap_or_else(|error| panic!("default policy: {error}")),
            PairingPolicy::default()
        );
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

    async fn wait_for_after(
        service: &PairingService,
        event_id: u64,
        predicate: impl Fn(&PairingEvent) -> bool,
    ) -> PairingEvent {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            let events = service
                .events_after(event_id)
                .unwrap_or_else(|error| panic!("pairing events: {error}"));
            if let Some(event) = events.into_iter().find(&predicate) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for later pairing event")
    }

    async fn wait_for_transfer(
        service: &PairingService,
        predicate: impl Fn(&TransferEvent) -> bool,
    ) -> TransferEvent {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            let events = service
                .transfer_events_after(0)
                .unwrap_or_else(|error| panic!("transfer events: {error}"));
            if let Some(event) = events.into_iter().find(&predicate) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for transfer event")
    }

    fn relay_platform_frames(
        source: &PairingService,
        source_channel: u64,
        destination: &PairingService,
        destination_channel: u64,
    ) {
        let frames = match source.drain_platform_frames(source_channel, 4) {
            Ok(frames) => frames,
            Err(PairingError::PlatformChannelNotFound) => return,
            Err(error) => panic!("drain platform frames: {error}"),
        };
        for frame in frames {
            match destination.submit_platform_frame(destination_channel, frame) {
                Ok(()) | Err(PairingError::PlatformChannelNotFound) => {}
                Err(error) => panic!("submit platform frame: {error}"),
            }
        }
    }

    #[tokio::test]
    async fn platform_control_bridge_completes_exporter_bound_pairing() {
        let initiator = PairingService::start(
            PairingConfig::new(test_directory("platform-initiator"))
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("initiator start: {error}"));
        let responder = PairingService::start(
            PairingConfig::new(test_directory("platform-responder"))
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("responder start: {error}"));
        let binding = [0x7a; 32];
        let initiator_channel = initiator
            .service
            .attach_platform_channel(
                Some("apple-p2p-peer".to_owned()),
                PlatformPairingRole::Initiator,
                binding,
            )
            .unwrap_or_else(|error| panic!("attach initiator: {error}"));
        let responder_channel = responder
            .service
            .attach_platform_channel(None, PlatformPairingRole::Responder, binding)
            .unwrap_or_else(|error| panic!("attach responder: {error}"));

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut responded = false;
        loop {
            relay_platform_frames(
                &initiator.service,
                initiator_channel,
                &responder.service,
                responder_channel,
            );
            relay_platform_frames(
                &responder.service,
                responder_channel,
                &initiator.service,
                initiator_channel,
            );

            let responder_events = responder
                .service
                .events_after(0)
                .unwrap_or_else(|error| panic!("responder events: {error}"));
            if !responded
                && let Some(request) = responder_events
                    .iter()
                    .find(|event| event.kind == PairingEventKind::ConfirmationRequired)
            {
                responder
                    .service
                    .respond(
                        request.request_id.unwrap_or_else(|| panic!("request id")),
                        true,
                    )
                    .unwrap_or_else(|error| panic!("respond: {error}"));
                responded = true;
            }
            let initiator_events = initiator
                .service
                .events_after(0)
                .unwrap_or_else(|error| panic!("initiator events: {error}"));
            let initiator_trusted = initiator_events
                .iter()
                .any(|event| event.kind == PairingEventKind::Trusted);
            let responder_trusted = responder_events
                .iter()
                .any(|event| event.kind == PairingEventKind::Trusted);
            if initiator_trusted && responder_trusted {
                let initiator_code = initiator_events
                    .iter()
                    .find_map(|event| event.short_code.as_ref());
                let responder_code = responder_events
                    .iter()
                    .find_map(|event| event.short_code.as_ref());
                assert_eq!(initiator_code, responder_code);
                assert_eq!(
                    initiator
                        .service
                        .platform_channel_state(initiator_channel)
                        .unwrap_or_else(|error| panic!("initiator channel state: {error}")),
                    PlatformPairingChannelState::Authenticated
                );
                assert_eq!(
                    responder
                        .service
                        .platform_channel_state(responder_channel)
                        .unwrap_or_else(|error| panic!("responder channel state: {error}")),
                    PlatformPairingChannelState::Authenticated
                );
                break;
            }
            assert!(Instant::now() < deadline, "platform pairing timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        initiator
            .service
            .close_platform_channel(initiator_channel)
            .unwrap_or_else(|error| panic!("close initiator: {error}"));
        responder
            .service
            .close_platform_channel(responder_channel)
            .unwrap_or_else(|error| panic!("close responder: {error}"));
        initiator.service.shutdown().await;
        responder.service.shutdown().await;
    }

    #[tokio::test]
    async fn pairing_service_accepts_a_platform_bound_udp_socket() {
        let directory = test_directory("platform-bound-socket");
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .unwrap_or_else(|error| panic!("bound socket: {error}"));
        let expected_port = socket
            .local_addr()
            .unwrap_or_else(|error| panic!("bound address: {error}"))
            .port();
        let startup = PairingService::start(
            PairingConfig::new(&directory).with_bound_local_unmetered_socket(socket),
        )
        .await
        .unwrap_or_else(|error| panic!("service start: {error}"));
        assert_eq!(startup.listen_port, expected_port);
        startup.service.shutdown().await;

        let hotspot_socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .unwrap_or_else(|error| panic!("hotspot socket: {error}"));
        let hotspot_port = hotspot_socket
            .local_addr()
            .unwrap_or_else(|error| panic!("hotspot address: {error}"))
            .port();
        let hotspot = PairingService::start(
            PairingConfig::new(directory.join("hotspot"))
                .with_bound_user_approved_hotspot_socket(hotspot_socket),
        )
        .await
        .unwrap_or_else(|error| panic!("hotspot service start: {error}"));
        assert_eq!(hotspot.listen_port, hotspot_port);
        hotspot.service.shutdown().await;

        let unbound_socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .unwrap_or_else(|error| panic!("unbound policy socket: {error}"));
        let rejected = PairingService::start(
            PairingConfig::new(directory.join("rejected")).with_bound_socket(
                unbound_socket,
                EstablishedPathProperties {
                    path_class: DataChannelPathClass::LocalNetwork,
                    cost: DataChannelCost::Unmetered,
                    local_network_scope: Some(LocalNetworkScope::Shared),
                    interface_bound: false,
                },
            ),
        )
        .await;
        assert!(matches!(rejected, Err(PairingError::IneligiblePath)));
        let _ = tokio::fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn duplicate_active_attempt_for_the_same_peer_is_rejected() {
        let directory = test_directory("duplicate-peer-attempt");
        let startup = PairingService::start(
            PairingConfig::new(&directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .with_pairing_policy(PairingPolicy {
                    connect_attempt_timeout: Duration::from_secs(1),
                    ..PairingPolicy::default()
                }),
        )
        .await
        .unwrap_or_else(|error| panic!("service start: {error}"));
        let unreachable = SocketAddr::from((Ipv4Addr::LOCALHOST, 9));
        startup
            .service
            .connect("Peer-A".to_owned(), vec![unreachable])
            .await
            .unwrap_or_else(|error| panic!("first attempt: {error}"));
        assert!(matches!(
            startup
                .service
                .connect("peer-a".to_owned(), vec![unreachable])
                .await,
            Err(PairingError::Busy)
        ));
        let completed = wait_for(&startup.service, |event| {
            event.kind == PairingEventKind::Failed || event.kind == PairingEventKind::TimedOut
        })
        .await;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match startup
                .service
                .connect("peer-a".to_owned(), vec![unreachable])
                .await
            {
                Err(PairingError::Busy) if Instant::now() < deadline => {
                    tokio::task::yield_now().await;
                }
                result => {
                    assert!(matches!(result, Err(PairingError::RateLimited)));
                    break;
                }
            }
        }
        let limited = wait_for_after(&startup.service, completed.event_id, |event| {
            event.detail.as_deref() == Some("retry_rate_limited")
        })
        .await;
        assert_eq!(limited.kind, PairingEventKind::Failed);
        startup.service.shutdown().await;
        let _ = tokio::fs::remove_dir_all(directory).await;
    }

    #[tokio::test]
    async fn closed_transport_removes_authenticated_session_and_emits_event() {
        let first_directory = test_directory("disconnect-first");
        let second_directory = test_directory("disconnect-second");
        let first = PairingService::start(
            PairingConfig::new(&first_directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .with_pairing_policy(PairingPolicy {
                    retry_cooldown: Duration::from_millis(10),
                    ..PairingPolicy::default()
                }),
        )
        .await
        .unwrap_or_else(|error| panic!("first start: {error}"));
        let second = PairingService::start(
            PairingConfig::new(&second_directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("second start: {error}"));
        first
            .service
            .connect(
                "second".to_owned(),
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
        let trusted = wait_for(&first.service, |event| {
            event.kind == PairingEventKind::Trusted
        })
        .await;
        let session_id = trusted
            .authenticated_session_id
            .unwrap_or_else(|| panic!("authenticated session"));
        tokio::time::sleep(Duration::from_millis(15)).await;
        first
            .service
            .connect(
                "second".to_owned(),
                vec![SocketAddr::from((Ipv4Addr::LOCALHOST, second.listen_port))],
            )
            .await
            .unwrap_or_else(|error| panic!("repeat connect: {error}"));
        let repeated = wait_for_after(&first.service, trusted.event_id, |event| {
            event.kind == PairingEventKind::Trusted
        })
        .await;
        assert_eq!(repeated.authenticated_session_id, Some(session_id));
        assert_eq!(
            first
                .service
                .authenticated_sessions()
                .unwrap_or_else(|error| panic!("deduplicated sessions: {error}"))
                .len(),
            1
        );
        second.service.shutdown().await;

        let disconnected = wait_for(&first.service, |event| {
            event.kind == PairingEventKind::Disconnected
        })
        .await;
        assert_eq!(disconnected.authenticated_session_id, Some(session_id));
        assert_eq!(disconnected.detail.as_deref(), Some("transport_closed"));
        assert!(
            first
                .service
                .authenticated_sessions()
                .unwrap_or_else(|error| panic!("authenticated sessions: {error}"))
                .is_empty()
        );

        first.service.shutdown().await;
        let _ = tokio::fs::remove_dir_all(first_directory).await;
        let _ = tokio::fs::remove_dir_all(second_directory).await;
    }

    #[tokio::test]
    async fn authenticated_lan_session_transfers_verified_file_after_receiver_consent() {
        let sender_directory = test_directory("transfer-sender");
        let receiver_directory = test_directory("transfer-receiver");
        let staging_directory = receiver_directory.join("staging");
        let destination_directory = receiver_directory.join("received");
        tokio::fs::create_dir_all(&sender_directory)
            .await
            .unwrap_or_else(|error| panic!("sender directory: {error}"));
        tokio::fs::create_dir_all(&staging_directory)
            .await
            .unwrap_or_else(|error| panic!("staging directory: {error}"));
        tokio::fs::create_dir_all(&destination_directory)
            .await
            .unwrap_or_else(|error| panic!("destination directory: {error}"));
        let source_path = sender_directory.join("hello.txt");
        let expected = b"authenticated QUIC transfer";
        tokio::fs::write(&source_path, expected)
            .await
            .unwrap_or_else(|error| panic!("source file: {error}"));

        let sender = PairingService::start(
            PairingConfig::new(sender_directory.join("trust"))
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("sender start: {error}"));
        let receiver = PairingService::start(
            PairingConfig::new(receiver_directory.join("trust"))
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("receiver start: {error}"));
        sender
            .service
            .connect(
                "receiver".to_owned(),
                vec![SocketAddr::from((
                    Ipv4Addr::LOCALHOST,
                    receiver.listen_port,
                ))],
            )
            .await
            .unwrap_or_else(|error| panic!("connect: {error}"));
        let pairing_request = wait_for(&receiver.service, |event| {
            event.kind == PairingEventKind::ConfirmationRequired
        })
        .await;
        receiver
            .service
            .respond(
                pairing_request
                    .request_id
                    .unwrap_or_else(|| panic!("pairing request id")),
                true,
            )
            .unwrap_or_else(|error| panic!("pairing response: {error}"));
        let trusted = wait_for(&sender.service, |event| {
            event.kind == PairingEventKind::Trusted
        })
        .await;
        let receiver_trusted = wait_for(&receiver.service, |event| {
            event.kind == PairingEventKind::Trusted
        })
        .await;
        let session_id = trusted
            .authenticated_session_id
            .unwrap_or_else(|| panic!("authenticated session"));

        let transfer_id = sender
            .service
            .send_file(session_id, source_path, None)
            .await
            .unwrap_or_else(|error| panic!("start transfer: {error}"));
        let offer = wait_for_transfer(&receiver.service, |event| {
            event.kind == crate::TransferEventKind::OfferReceived
        })
        .await;
        assert_eq!(offer.transfer_id, transfer_id);
        receiver
            .service
            .respond_to_transfer_with_space(
                offer
                    .request_id
                    .unwrap_or_else(|| panic!("transfer request id")),
                true,
                Some(staging_directory.clone()),
                Some(destination_directory.clone()),
                Some(u64::MAX),
            )
            .unwrap_or_else(|error| panic!("accept transfer: {error}"));
        let received = wait_for_transfer(&receiver.service, |event| {
            event.kind == crate::TransferEventKind::Completed
        })
        .await;
        let sent = wait_for_transfer(&sender.service, |event| {
            event.kind == crate::TransferEventKind::Completed
        })
        .await;
        assert_eq!(received.transfer_id, transfer_id);
        assert_eq!(sent.transfer_id, transfer_id);
        assert!(
            receiver
                .service
                .transfer_events_after(0)
                .unwrap_or_else(|error| panic!("receiver progress events: {error}"))
                .iter()
                .any(|event| {
                    event.kind == crate::TransferEventKind::Transferring
                        && event.authenticated_session_id
                            == receiver_trusted
                                .authenticated_session_id
                                .unwrap_or_else(|| panic!("receiver session"))
                        && event.transferred_bytes == expected.len() as u64
                })
        );
        assert_eq!(
            received.final_path,
            Some(destination_directory.join("hello.txt"))
        );
        assert_eq!(
            tokio::fs::read(destination_directory.join("hello.txt"))
                .await
                .unwrap_or_else(|error| panic!("received file: {error}")),
            expected
        );

        sender.service.shutdown().await;
        receiver.service.shutdown().await;
        let _ = tokio::fs::remove_dir_all(sender_directory).await;
        let _ = tokio::fs::remove_dir_all(receiver_directory).await;
    }

    #[tokio::test]
    async fn malformed_fast_lan_candidate_falls_through_to_authenticated_peer() {
        let sender_directory = test_directory("fallback-sender");
        let receiver_directory = test_directory("fallback-receiver");
        let malformed = Arc::new(
            QuicEndpoint::server(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
                .unwrap_or_else(|error| panic!("malformed endpoint: {error}")),
        );
        let malformed_address = malformed
            .local_addr()
            .unwrap_or_else(|error| panic!("malformed address: {error}"));
        let malformed_task = {
            let malformed = Arc::clone(&malformed);
            tokio::spawn(async move {
                let connection = malformed
                    .accept(CancellationToken::new())
                    .await
                    .unwrap_or_else(|error| panic!("malformed accept: {error}"));
                let mut control = connection
                    .accept_control()
                    .await
                    .unwrap_or_else(|error| panic!("malformed control: {error}"));
                let _ = control.receive_frame(4096).await;
                control
                    .send_frame(&[0_u8; 12])
                    .await
                    .unwrap_or_else(|error| panic!("malformed response: {error}"));
                control.close().await;
            })
        };
        let sender = PairingService::start(
            PairingConfig::new(&sender_directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("sender start: {error}"));
        let receiver = PairingService::start(
            PairingConfig::new(&receiver_directory)
                .with_bind_address(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        )
        .await
        .unwrap_or_else(|error| panic!("receiver start: {error}"));
        sender
            .service
            .connect(
                "fallback-peer".to_owned(),
                vec![
                    malformed_address,
                    SocketAddr::from((Ipv4Addr::LOCALHOST, receiver.listen_port)),
                ],
            )
            .await
            .unwrap_or_else(|error| panic!("connect: {error}"));
        let request = wait_for(&receiver.service, |event| {
            event.kind == PairingEventKind::ConfirmationRequired
        })
        .await;
        receiver
            .service
            .respond(
                request.request_id.unwrap_or_else(|| panic!("request id")),
                true,
            )
            .unwrap_or_else(|error| panic!("respond: {error}"));
        let trusted = wait_for(&sender.service, |event| {
            event.kind == PairingEventKind::Trusted
        })
        .await;
        assert!(trusted.authenticated_session_id.is_some());
        malformed_task
            .await
            .unwrap_or_else(|error| panic!("malformed task: {error}"));
        sender.service.shutdown().await;
        receiver.service.shutdown().await;
        malformed.close();
        let _ = tokio::fs::remove_dir_all(sender_directory).await;
        let _ = tokio::fs::remove_dir_all(receiver_directory).await;
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
        let first_trusted = wait_for(&first.service, |event| {
            event.kind == PairingEventKind::Trusted
        })
        .await;
        let second_trusted = wait_for(&second.service, |event| {
            event.kind == PairingEventKind::Trusted
        })
        .await;
        assert!(!first_trusted.already_trusted);
        assert!(!second_trusted.already_trusted);
        assert!(first_trusted.authenticated_session_id.is_some());
        assert!(second_trusted.authenticated_session_id.is_some());
        assert_eq!(
            first
                .service
                .authenticated_sessions()
                .unwrap_or_else(|error| panic!("first sessions: {error}"))
                .len(),
            1
        );
        assert_eq!(
            second
                .service
                .authenticated_sessions()
                .unwrap_or_else(|error| panic!("second sessions: {error}"))
                .len(),
            1
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
