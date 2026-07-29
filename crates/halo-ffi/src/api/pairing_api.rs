use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use halo_core::{PairingConfig, PairingError, PairingService};
use tokio::runtime::Runtime;

use super::HaloApiError;

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
    pub peer_presence_id: Option<String>,
    pub peer_fingerprint: Option<String>,
    pub short_code: Option<String>,
    pub already_trusted: bool,
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
            detail: event.detail,
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
        PairingError::InternalState => HaloApiError::InternalState,
        error => HaloApiError::Core {
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
        pairing_stop(session.session_id).unwrap_or_else(|error| panic!("stop: {error}"));
    }
}
