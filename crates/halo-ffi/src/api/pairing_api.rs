use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use halo_core::{PairingConfig, PairingError, PairingService, TransferServiceError};
use tokio::runtime::Runtime;

use super::HaloApiError;
use crate::platform_socket::{RegisteredLanEndpoint, take_lan_endpoint};

const MAX_CANDIDATES: usize = 8;
static NEXT_PAIRING_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static PAIRING_SESSIONS: OnceLock<Mutex<HashMap<u64, PairingSession>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct PairingBootstrap {
    pub session_id: u64,
    pub listen_port: u16,
    /// Present only for a newly generated identity. The caller must persist it
    /// using the platform protected-blob adapter before advertising the port.
    pub identity_blob_to_persist: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct PlatformTlsIdentity {
    pub certificate_der: Vec<u8>,
    pub private_key_x963: Vec<u8>,
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
    pub peer_presence_id: Option<String>,
    pub peer_fingerprint: Option<String>,
    pub short_code: Option<String>,
    pub already_trusted: bool,
    pub authenticated_session_id: Option<u64>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedSessionInfo {
    pub session_id: u64,
    pub peer_fingerprint: String,
    pub peer_presence_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferDirection {
    Sending,
    Receiving,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferEventKind {
    OfferReceived,
    AwaitingDecision,
    Transferring,
    Completed,
    Rejected,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferEvent {
    pub event_id: u64,
    pub request_id: Option<u64>,
    pub authenticated_session_id: u64,
    pub transfer_id: String,
    pub direction: TransferDirection,
    pub kind: TransferEventKind,
    pub file_name: String,
    pub file_size: u64,
    pub transferred_bytes: u64,
    pub final_path: Option<String>,
    pub detail: Option<String>,
}

struct PairingSession {
    runtime: Runtime,
    service: PairingService,
}

pub fn pairing_start(
    identity_blob: Option<Vec<u8>>,
    trust_store_directory: String,
) -> Result<PairingBootstrap, HaloApiError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("halo-pairing")
        .build()
        .map_err(core_error)?;
    let mut config = PairingConfig::new(PathBuf::from(trust_store_directory));
    match take_lan_endpoint().map_err(|()| HaloApiError::InternalState)? {
        Some(RegisteredLanEndpoint::Bound(socket)) => {
            config = config.with_bound_local_unmetered_socket(socket);
        }
        Some(RegisteredLanEndpoint::Disabled) => {
            config = config.with_bind_address("127.0.0.1:0".parse().map_err(core_error)?);
        }
        None if cfg!(target_os = "android") => {
            // Fail closed if native preparation did not run or its JNI handoff
            // failed. Android must never fall back to a wildcard listener.
            config = config.with_bind_address("127.0.0.1:0".parse().map_err(core_error)?);
        }
        None => {}
    }
    if let Some(identity_blob) = identity_blob {
        config = config.with_identity_blob(identity_blob);
    }
    let startup = runtime
        .block_on(PairingService::start(config))
        .map_err(pairing_error)?;
    let listen_port = startup.listen_port;
    let identity_blob_to_persist = startup.new_identity_blob().map(<[u8]>::to_vec);
    let session_id = NEXT_PAIRING_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    if session_id == 0 {
        return Err(HaloApiError::InternalState);
    }
    pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?
        .insert(
            session_id,
            PairingSession {
                runtime,
                service: startup.service,
            },
        );
    Ok(PairingBootstrap {
        session_id,
        listen_port,
        identity_blob_to_persist,
    })
}

pub fn pairing_create_platform_tls_identity() -> Result<PlatformTlsIdentity, HaloApiError> {
    let identity = halo_core::create_platform_tls_identity().map_err(pairing_error)?;
    Ok(PlatformTlsIdentity {
        certificate_der: identity.certificate_der,
        private_key_x963: identity.private_key_x963,
    })
}

pub fn pairing_connect(
    session_id: u64,
    peer_presence_id: String,
    endpoints: Vec<String>,
) -> Result<(), HaloApiError> {
    let mut unique = HashSet::new();
    let addresses = endpoints
        .into_iter()
        .filter_map(|endpoint| endpoint.parse::<SocketAddr>().ok())
        .filter(|address| unique.insert(*address))
        .take(MAX_CANDIDATES)
        .collect::<Vec<_>>();
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    let session = sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?;
    session
        .runtime
        .block_on(session.service.connect(peer_presence_id, addresses))
        .map_err(pairing_error)
}

pub fn pairing_attach_platform_channel(
    session_id: u64,
    peer_presence_id: Option<String>,
    role: PlatformPairingRole,
    channel_binding: Vec<u8>,
) -> Result<u64, HaloApiError> {
    let binding: [u8; 32] =
        channel_binding
            .try_into()
            .map_err(|_| HaloApiError::InvalidArgument {
                message: "platform TLS exporter must contain exactly 32 bytes".to_owned(),
            })?;
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    let session = sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?;
    session
        .service
        .attach_platform_channel(peer_presence_id, role.into(), binding)
        .map_err(pairing_error)
}

pub fn pairing_submit_platform_frame(
    session_id: u64,
    channel_id: u64,
    frame: Vec<u8>,
) -> Result<(), HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .submit_platform_frame(channel_id, frame)
        .map_err(pairing_error)
}

pub fn pairing_drain_platform_frames(
    session_id: u64,
    channel_id: u64,
    maximum_frames: u32,
) -> Result<Vec<Vec<u8>>, HaloApiError> {
    let maximum_frames =
        usize::try_from(maximum_frames).map_err(|_| HaloApiError::InvalidArgument {
            message: "platform frame drain limit is invalid".to_owned(),
        })?;
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .drain_platform_frames(channel_id, maximum_frames)
        .map_err(pairing_error)
}

pub fn pairing_close_platform_channel(
    session_id: u64,
    channel_id: u64,
) -> Result<(), HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .close_platform_channel(channel_id)
        .map_err(pairing_error)
}

pub fn pairing_platform_channel_state(
    session_id: u64,
    channel_id: u64,
) -> Result<PlatformPairingChannelState, HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .platform_channel_state(channel_id)
        .map(Into::into)
        .map_err(pairing_error)
}

pub fn pairing_events(
    session_id: u64,
    after_event_id: u64,
) -> Result<Vec<PairingEvent>, HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .events_after(after_event_id)
        .map_err(pairing_error)
        .map(|events| events.into_iter().map(PairingEvent::from).collect())
}

pub fn pairing_authenticated_sessions(
    session_id: u64,
) -> Result<Vec<AuthenticatedSessionInfo>, HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .authenticated_sessions()
        .map_err(pairing_error)
        .map(|sessions| {
            sessions
                .into_iter()
                .map(AuthenticatedSessionInfo::from)
                .collect()
        })
}

pub fn pairing_respond(
    session_id: u64,
    request_id: u64,
    accepted: bool,
) -> Result<(), HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .respond(request_id, accepted)
        .map_err(pairing_error)
}

pub fn pairing_transfer_send_file(
    session_id: u64,
    authenticated_session_id: u64,
    source_path: String,
    advertised_name: Option<String>,
) -> Result<String, HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    let session = sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?;
    session
        .runtime
        .block_on(session.service.send_file(
            authenticated_session_id,
            PathBuf::from(source_path),
            advertised_name,
        ))
        .map_err(transfer_error)
}

pub fn pairing_transfer_events(
    session_id: u64,
    after_event_id: u64,
) -> Result<Vec<TransferEvent>, HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .transfer_events_after(after_event_id)
        .map_err(transfer_error)
        .map(|events| events.into_iter().map(TransferEvent::from).collect())
}

pub fn pairing_transfer_respond(
    session_id: u64,
    request_id: u64,
    accepted: bool,
    staging_directory: Option<String>,
    destination_directory: Option<String>,
) -> Result<(), HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .respond_to_transfer(
            request_id,
            accepted,
            staging_directory.map(PathBuf::from),
            destination_directory.map(PathBuf::from),
        )
        .map_err(transfer_error)
}

pub fn pairing_transfer_cancel(session_id: u64, transfer_id: String) -> Result<(), HaloApiError> {
    let sessions = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?;
    sessions
        .get(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?
        .service
        .cancel_transfer(&transfer_id)
        .map_err(transfer_error)
}

pub fn pairing_stop(session_id: u64) -> Result<(), HaloApiError> {
    let session = pairing_sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?
        .remove(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?;
    session.runtime.block_on(session.service.shutdown());
    Ok(())
}

impl From<halo_core::PairingEvent> for PairingEvent {
    fn from(event: halo_core::PairingEvent) -> Self {
        Self {
            event_id: event.event_id,
            request_id: event.request_id,
            kind: event.kind.into(),
            peer_presence_id: event.peer_reference,
            peer_fingerprint: event.peer_fingerprint,
            short_code: event.short_code,
            already_trusted: event.already_trusted,
            authenticated_session_id: event.authenticated_session_id,
            detail: event.detail,
        }
    }
}

impl From<halo_core::AuthenticatedSessionInfo> for AuthenticatedSessionInfo {
    fn from(session: halo_core::AuthenticatedSessionInfo) -> Self {
        Self {
            session_id: session.session_id,
            peer_fingerprint: session.peer_fingerprint,
            peer_presence_id: session.peer_reference,
        }
    }
}

impl From<halo_core::TransferEvent> for TransferEvent {
    fn from(event: halo_core::TransferEvent) -> Self {
        Self {
            event_id: event.event_id,
            request_id: event.request_id,
            authenticated_session_id: event.authenticated_session_id,
            transfer_id: event.transfer_id,
            direction: event.direction.into(),
            kind: event.kind.into(),
            file_name: event.file_name,
            file_size: event.file_size,
            transferred_bytes: event.transferred_bytes,
            final_path: event
                .final_path
                .map(|path| path.to_string_lossy().into_owned()),
            detail: event.detail,
        }
    }
}

impl From<halo_core::TransferDirection> for TransferDirection {
    fn from(direction: halo_core::TransferDirection) -> Self {
        match direction {
            halo_core::TransferDirection::Sending => Self::Sending,
            halo_core::TransferDirection::Receiving => Self::Receiving,
        }
    }
}

impl From<halo_core::TransferEventKind> for TransferEventKind {
    fn from(kind: halo_core::TransferEventKind) -> Self {
        match kind {
            halo_core::TransferEventKind::OfferReceived => Self::OfferReceived,
            halo_core::TransferEventKind::AwaitingDecision => Self::AwaitingDecision,
            halo_core::TransferEventKind::Transferring => Self::Transferring,
            halo_core::TransferEventKind::Completed => Self::Completed,
            halo_core::TransferEventKind::Rejected => Self::Rejected,
            halo_core::TransferEventKind::Cancelled => Self::Cancelled,
            halo_core::TransferEventKind::Failed => Self::Failed,
        }
    }
}

impl From<halo_core::PairingEventKind> for PairingEventKind {
    fn from(kind: halo_core::PairingEventKind) -> Self {
        match kind {
            halo_core::PairingEventKind::Connecting => Self::Connecting,
            halo_core::PairingEventKind::CodeAvailable => Self::CodeAvailable,
            halo_core::PairingEventKind::ConfirmationRequired => Self::ConfirmationRequired,
            halo_core::PairingEventKind::Trusted => Self::Trusted,
            halo_core::PairingEventKind::Rejected => Self::Rejected,
            halo_core::PairingEventKind::IdentityChanged => Self::IdentityChanged,
            halo_core::PairingEventKind::TimedOut => Self::TimedOut,
            halo_core::PairingEventKind::Cancelled => Self::Cancelled,
            halo_core::PairingEventKind::Failed => Self::Failed,
            halo_core::PairingEventKind::Disconnected => Self::Disconnected,
        }
    }
}

impl From<PlatformPairingRole> for halo_core::PlatformPairingRole {
    fn from(role: PlatformPairingRole) -> Self {
        match role {
            PlatformPairingRole::Initiator => Self::Initiator,
            PlatformPairingRole::Responder => Self::Responder,
        }
    }
}

impl From<halo_core::PlatformPairingChannelState> for PlatformPairingChannelState {
    fn from(state: halo_core::PlatformPairingChannelState) -> Self {
        match state {
            halo_core::PlatformPairingChannelState::Pending => Self::Pending,
            halo_core::PlatformPairingChannelState::Authenticated => Self::Authenticated,
            halo_core::PlatformPairingChannelState::Failed => Self::Failed,
        }
    }
}

fn pairing_sessions() -> &'static Mutex<HashMap<u64, PairingSession>> {
    PAIRING_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pairing_error(error: PairingError) -> HaloApiError {
    match error {
        PairingError::InvalidPeerReference => HaloApiError::InvalidArgument {
            message: "invalid peer presence identifier".to_owned(),
        },
        PairingError::NoEndpoints => HaloApiError::InvalidArgument {
            message: "no valid socket endpoint".to_owned(),
        },
        PairingError::RequestNotPending => HaloApiError::InvalidArgument {
            message: "pairing request is no longer pending".to_owned(),
        },
        PairingError::PlatformChannelNotFound => HaloApiError::InvalidArgument {
            message: "platform pairing channel does not exist".to_owned(),
        },
        PairingError::PlatformChannelBackpressure => HaloApiError::Core {
            message: "platform pairing channel is backpressured".to_owned(),
        },
        PairingError::InvalidPlatformFrame => HaloApiError::InvalidArgument {
            message: "platform pairing frame or drain limit is invalid".to_owned(),
        },
        PairingError::InternalState => HaloApiError::InternalState,
        error => HaloApiError::Core {
            message: error.to_string(),
        },
    }
}

fn transfer_error(error: TransferServiceError) -> HaloApiError {
    match error {
        TransferServiceError::InvalidPolicy => HaloApiError::InternalState,
        TransferServiceError::FileRejectedByPolicy => HaloApiError::Core {
            message: "file exceeds the configured transfer size limit".to_owned(),
        },
        TransferServiceError::SessionNotFound => HaloApiError::InvalidArgument {
            message: "authenticated LAN session does not exist".to_owned(),
        },
        TransferServiceError::Busy => HaloApiError::Core {
            message: "authenticated LAN session already has an active transfer".to_owned(),
        },
        TransferServiceError::RequestNotPending => HaloApiError::InvalidArgument {
            message: "transfer request is no longer pending".to_owned(),
        },
        TransferServiceError::MissingDestination => HaloApiError::InvalidArgument {
            message: "accepted transfer requires private staging and destination directories"
                .to_owned(),
        },
        TransferServiceError::TransferNotFound => HaloApiError::InvalidArgument {
            message: "transfer does not exist".to_owned(),
        },
        TransferServiceError::InternalState => HaloApiError::InternalState,
        TransferServiceError::Prepare(error) => HaloApiError::Core {
            message: error.to_string(),
        },
    }
}

fn core_error(error: impl std::fmt::Display) -> HaloApiError {
    HaloApiError::Core {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("halo-ffi-adapter-{name}-{}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn adapter_rejects_malformed_identity_and_endpoint() {
        assert!(pairing_start(Some(vec![0; 38]), test_directory("invalid")).is_err());
        let session = pairing_start(None, test_directory("endpoint"))
            .unwrap_or_else(|error| panic!("start: {error}"));
        assert!(
            pairing_connect(
                session.session_id,
                "peer".to_owned(),
                vec!["not-an-endpoint".to_owned()],
            )
            .is_err()
        );
        assert!(matches!(
            pairing_attach_platform_channel(
                session.session_id,
                Some("peer".to_owned()),
                PlatformPairingRole::Initiator,
                vec![0; 31],
            ),
            Err(HaloApiError::InvalidArgument { .. })
        ));
        let tls_identity = pairing_create_platform_tls_identity()
            .unwrap_or_else(|error| panic!("TLS identity: {error}"));
        assert!(!tls_identity.certificate_der.is_empty());
        assert_eq!(tls_identity.private_key_x963.len(), 97);
        pairing_stop(session.session_id).unwrap_or_else(|error| panic!("stop: {error}"));
    }
}
