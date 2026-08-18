use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use halo_core::{DiscoveryConfig, DiscoveryError, DiscoveryService};
use thiserror::Error;
use tokio::runtime::{Builder, Runtime};

use crate::platform_socket::{RegisteredDiscoveryEndpoint, take_discovery_endpoint};

pub mod pairing_api;
pub use pairing_api::*;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static SESSIONS: OnceLock<Mutex<HashMap<u64, SessionRuntime>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct DiscoveryBootstrap {
    pub session_id: u64,
    pub presence_id: String,
    pub device_type: DiscoveryDeviceType,
    pub ble_presence: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryPeer {
    pub presence_id: String,
    pub device_type: DiscoveryDeviceType,
    pub compatible: bool,
    pub capabilities: u64,
    pub sources: Vec<String>,
    pub best_endpoint: Option<String>,
    pub candidate_endpoints: Vec<String>,
    pub candidate_count: u32,
    pub quarantined: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryProviderStatus {
    pub name: String,
    pub kind: String,
    pub state: String,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryDeviceType {
    Unknown,
    Android,
    Ios,
    Macos,
    Windows,
    Linux,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformProviderState {
    Starting,
    Ready,
    Degraded,
    PermissionRequired,
    PermissionDenied,
    HardwareOff,
    Unsupported,
    TemporarilyUnavailable,
    Stopped,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum HaloApiError {
    #[error("invalid argument: {message}")]
    InvalidArgument { message: String },
    #[error("session does not exist")]
    SessionNotFound,
    #[error("Halo core rejected the operation: {message}")]
    Core { message: String },
    #[error("session registry is unavailable")]
    InternalState,
}

struct SessionRuntime {
    runtime: Runtime,
    service: DiscoveryService,
}

/// Starts the Rust SDK discovery service used by the Flutter application.
pub fn discovery_start(
    quic_port: u16,
    enable_lan: bool,
    device_type: DiscoveryDeviceType,
    remembered_endpoint_addresses: Vec<String>,
) -> Result<DiscoveryBootstrap, HaloApiError> {
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("halo-discovery")
        .build()
        .map_err(core_error)?;
    let remembered_endpoints = remembered_endpoint_addresses
        .into_iter()
        .filter_map(|address| address.parse::<IpAddr>().ok())
        .map(|address| (address, 1).into())
        .collect::<Vec<_>>();
    let mut config = DiscoveryConfig::new(quic_port, device_type.into()).with_lan(enable_lan);
    match take_discovery_endpoint().map_err(|()| HaloApiError::InternalState)? {
        Some(RegisteredDiscoveryEndpoint::Bound(socket)) => {
            config = config.with_direct_probe(socket, remembered_endpoints);
        }
        Some(RegisteredDiscoveryEndpoint::Disabled) | None => {}
    }
    let (service, startup) = runtime
        .block_on(DiscoveryService::start(config))
        .map_err(discovery_error)?;
    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    if session_id == 0 {
        return Err(HaloApiError::InternalState);
    }
    sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?
        .insert(session_id, SessionRuntime { runtime, service });
    Ok(DiscoveryBootstrap {
        session_id,
        presence_id: startup.presence_id,
        device_type: startup.device_type.into(),
        ble_presence: startup.ble_presence,
    })
}

pub fn discovery_refresh_ble_presence(session_id: u64) -> Result<Vec<u8>, HaloApiError> {
    with_session_mut(session_id, |session| {
        Ok(session.service.refresh_ble_presence())
    })
}

pub fn discovery_submit_ble(
    session_id: u64,
    platform: String,
    descriptor: Vec<u8>,
) -> Result<Vec<DiscoveryPeer>, HaloApiError> {
    with_session_mut(session_id, |session| {
        session
            .runtime
            .block_on(session.service.submit_ble(&platform, &descriptor))
            .map_err(discovery_error)
            .map(map_peers)
    })
}

pub fn discovery_report_ble_state(
    session_id: u64,
    platform: String,
    state: PlatformProviderState,
    detail: Option<String>,
) -> Result<(), HaloApiError> {
    with_session_mut(session_id, |session| {
        session
            .runtime
            .block_on(
                session
                    .service
                    .report_ble_state(&platform, state.into(), detail),
            )
            .map_err(discovery_error)
    })
}

pub fn discovery_snapshot(session_id: u64) -> Result<Vec<DiscoveryPeer>, HaloApiError> {
    with_session_mut(session_id, |session| {
        session
            .runtime
            .block_on(session.service.snapshot())
            .map_err(discovery_error)
            .map(map_peers)
    })
}

pub fn discovery_provider_statuses(
    session_id: u64,
) -> Result<Vec<DiscoveryProviderStatus>, HaloApiError> {
    with_session_mut(session_id, |session| {
        session
            .runtime
            .block_on(session.service.provider_statuses())
            .map_err(discovery_error)
            .map(|statuses| {
                statuses
                    .into_iter()
                    .map(|status| DiscoveryProviderStatus {
                        name: status.name,
                        kind: status.kind,
                        state: status.state,
                        detail: status.detail,
                    })
                    .collect()
            })
    })
}

pub fn discovery_stop(session_id: u64) -> Result<(), HaloApiError> {
    let mut session = sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?
        .remove(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?;
    session
        .runtime
        .block_on(session.service.shutdown())
        .map_err(discovery_error)
}

fn sessions() -> &'static Mutex<HashMap<u64, SessionRuntime>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_session_mut<T>(
    session_id: u64,
    operation: impl FnOnce(&mut SessionRuntime) -> Result<T, HaloApiError>,
) -> Result<T, HaloApiError> {
    let mut sessions = sessions().lock().map_err(|_| HaloApiError::InternalState)?;
    let session = sessions
        .get_mut(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?;
    operation(session)
}

fn map_peers(peers: Vec<halo_core::DiscoveryPeer>) -> Vec<DiscoveryPeer> {
    peers
        .into_iter()
        .map(|peer| DiscoveryPeer {
            presence_id: peer.presence_id,
            device_type: peer.device_type.into(),
            compatible: peer.compatible,
            capabilities: peer.capabilities,
            sources: peer.sources,
            best_endpoint: peer.best_endpoint,
            candidate_endpoints: peer.candidate_endpoints,
            candidate_count: peer.candidate_count,
            quarantined: peer.quarantined,
        })
        .collect()
}

impl From<DiscoveryDeviceType> for halo_core::DeviceType {
    fn from(value: DiscoveryDeviceType) -> Self {
        match value {
            DiscoveryDeviceType::Unknown => Self::Unknown,
            DiscoveryDeviceType::Android => Self::Android,
            DiscoveryDeviceType::Ios => Self::Ios,
            DiscoveryDeviceType::Macos => Self::Macos,
            DiscoveryDeviceType::Windows => Self::Windows,
            DiscoveryDeviceType::Linux => Self::Linux,
        }
    }
}

impl From<halo_core::DeviceType> for DiscoveryDeviceType {
    fn from(value: halo_core::DeviceType) -> Self {
        match value {
            halo_core::DeviceType::Unknown => Self::Unknown,
            halo_core::DeviceType::Android => Self::Android,
            halo_core::DeviceType::Ios => Self::Ios,
            halo_core::DeviceType::Macos => Self::Macos,
            halo_core::DeviceType::Windows => Self::Windows,
            halo_core::DeviceType::Linux => Self::Linux,
        }
    }
}

impl From<PlatformProviderState> for halo_core::PlatformProviderState {
    fn from(value: PlatformProviderState) -> Self {
        match value {
            PlatformProviderState::Starting => Self::Starting,
            PlatformProviderState::Ready => Self::Ready,
            PlatformProviderState::Degraded => Self::Degraded,
            PlatformProviderState::PermissionRequired => Self::PermissionRequired,
            PlatformProviderState::PermissionDenied => Self::PermissionDenied,
            PlatformProviderState::HardwareOff => Self::HardwareOff,
            PlatformProviderState::Unsupported => Self::Unsupported,
            PlatformProviderState::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            PlatformProviderState::Stopped => Self::Stopped,
        }
    }
}

fn discovery_error(error: DiscoveryError) -> HaloApiError {
    match error {
        DiscoveryError::UnknownPlatform => HaloApiError::InvalidArgument {
            message: "unknown platform BLE provider".to_owned(),
        },
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

    #[test]
    fn adapter_starts_and_reads_empty_snapshot() {
        let bootstrap = discovery_start(44_330, false, DiscoveryDeviceType::Macos, Vec::new())
            .unwrap_or_else(|error| panic!("start: {error}"));
        assert!(!bootstrap.presence_id.is_empty());
        assert_eq!(bootstrap.device_type, DiscoveryDeviceType::Macos);
        assert!(
            discovery_snapshot(bootstrap.session_id)
                .unwrap_or_else(|error| panic!("snapshot: {error}"))
                .is_empty()
        );
        discovery_stop(bootstrap.session_id).unwrap_or_else(|error| panic!("stop: {error}"));
    }

    #[test]
    fn malformed_native_bytes_are_rejected_by_core() {
        let bootstrap = discovery_start(44_331, false, DiscoveryDeviceType::Android, Vec::new())
            .unwrap_or_else(|error| panic!("start: {error}"));
        assert!(
            discovery_submit_ble(bootstrap.session_id, "macos".to_owned(), vec![0; 58],).is_err()
        );
        discovery_stop(bootstrap.session_id).unwrap_or_else(|error| panic!("stop: {error}"));
    }

    #[test]
    fn provider_health_crosses_thin_adapter() {
        let bootstrap = discovery_start(44_332, false, DiscoveryDeviceType::Ios, Vec::new())
            .unwrap_or_else(|error| panic!("start: {error}"));
        discovery_report_ble_state(
            bootstrap.session_id,
            "ios".to_owned(),
            PlatformProviderState::PermissionDenied,
            Some("denied by user".to_owned()),
        )
        .unwrap_or_else(|error| panic!("state: {error}"));
        let statuses = discovery_provider_statuses(bootstrap.session_id)
            .unwrap_or_else(|error| panic!("statuses: {error}"));
        assert_eq!(statuses[0].name, "ble-ios");
        assert_eq!(statuses[0].state, "permission_denied");
        discovery_stop(bootstrap.session_id).unwrap_or_else(|error| panic!("stop: {error}"));
    }
}
