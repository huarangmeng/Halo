use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use halo_discovery::{
    Capabilities, DeviceType, DiscoveryHandle, DiscoveryManager, DiscoverySession, LocalPresence,
    PresenceId, ProtocolRange, ProviderId, ProviderKind, ProviderState,
    ble::{decode_presence, encode_presence},
    providers::{MdnsProvider, PresenceV4Provider, PresenceV6Provider},
};
use thiserror::Error;
use tokio::runtime::{Builder, Runtime};

const BLE_OBSERVATION_TTL: Duration = Duration::from_secs(15);
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
    pub candidate_count: u32,
    pub quarantined: bool,
}

/// Current health of one independently running discovery provider.
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

impl From<DiscoveryDeviceType> for DeviceType {
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

impl From<DeviceType> for DiscoveryDeviceType {
    fn from(value: DeviceType) -> Self {
        match value {
            DeviceType::Unknown => Self::Unknown,
            DeviceType::Android => Self::Android,
            DeviceType::Ios => Self::Ios,
            DeviceType::Macos => Self::Macos,
            DeviceType::Windows => Self::Windows,
            DeviceType::Linux => Self::Linux,
        }
    }
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
    #[error("discovery session does not exist")]
    SessionNotFound,
    #[error("discovery core rejected the operation: {message}")]
    Core { message: String },
    #[error("discovery session registry is unavailable")]
    InternalState,
}

struct SessionRuntime {
    runtime: Runtime,
    local: LocalPresence,
    handle: DiscoveryHandle,
    session: Option<DiscoverySession>,
    sequence: u64,
}

/// Starts the one Rust-owned discovery session used by the Flutter application.
///
/// `enable_lan` is false only in deterministic unit tests. Product clients use
/// true after the platform has granted local-network access.
pub fn discovery_start(
    quic_port: u16,
    enable_lan: bool,
    device_type: DiscoveryDeviceType,
) -> Result<DiscoveryBootstrap, HaloApiError> {
    let protocol = ProtocolRange::new(1, 1).map_err(core_error)?;
    let local = LocalPresence::new(
        PresenceId::random(),
        protocol,
        Capabilities::default().with_device_type(device_type.into()),
        quic_port,
    )
    .map_err(core_error)?;
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("halo-discovery")
        .build()
        .map_err(|error| HaloApiError::Core {
            message: error.to_string(),
        })?;

    let mut manager = DiscoveryManager::new(local.clone());
    if enable_lan {
        manager = manager
            .with_provider(MdnsProvider::default())
            .with_provider(PresenceV4Provider::default())
            .with_provider(PresenceV6Provider::default());
    }
    let session = runtime.block_on(manager.start()).map_err(core_error)?;
    let handle = session.handle();
    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    if session_id == 0 {
        return Err(HaloApiError::InternalState);
    }
    let presence_id = local.presence_id.to_string();
    let ble_presence = encode_presence(&local, 1).to_vec();
    let session_runtime = SessionRuntime {
        runtime,
        local,
        handle,
        session: Some(session),
        sequence: 1,
    };
    sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?
        .insert(session_id, session_runtime);

    Ok(DiscoveryBootstrap {
        session_id,
        presence_id,
        device_type,
        ble_presence,
    })
}

/// Returns a newly sequenced descriptor for a native BLE driver to expose.
pub fn discovery_refresh_ble_presence(session_id: u64) -> Result<Vec<u8>, HaloApiError> {
    with_session_mut(session_id, |session| {
        session.sequence = session.sequence.saturating_add(1);
        Ok(encode_presence(&session.local, session.sequence).to_vec())
    })
}

/// Validates a raw platform BLE value in Rust and submits it to the shared manager.
pub fn discovery_submit_ble(
    session_id: u64,
    platform: String,
    descriptor: Vec<u8>,
) -> Result<Vec<DiscoveryPeer>, HaloApiError> {
    let provider = ble_provider_id(&platform)?;
    with_session_mut(session_id, |session| {
        let observation =
            decode_presence(&descriptor, provider, BLE_OBSERVATION_TTL).map_err(|error| {
                HaloApiError::Core {
                    message: error.to_string(),
                }
            })?;
        session
            .runtime
            .block_on(session.handle.submit_observation(observation))
            .map_err(core_error)?;
        snapshot(session)
    })
}

/// Normalizes raw native provider health into the Rust discovery event model.
pub fn discovery_report_ble_state(
    session_id: u64,
    platform: String,
    state: PlatformProviderState,
    detail: Option<String>,
) -> Result<(), HaloApiError> {
    let provider = ble_provider_id(&platform)?;
    let normalized = normalize_provider_state(state, detail);
    with_session_mut(session_id, |session| {
        session
            .runtime
            .block_on(session.handle.report_provider_state(provider, normalized))
            .map_err(core_error)
    })
}

pub fn discovery_snapshot(session_id: u64) -> Result<Vec<DiscoveryPeer>, HaloApiError> {
    with_session_mut(session_id, snapshot)
}

/// Returns a stable, sorted health snapshot for all providers seen by Rust.
pub fn discovery_provider_statuses(
    session_id: u64,
) -> Result<Vec<DiscoveryProviderStatus>, HaloApiError> {
    with_session_mut(session_id, |session| {
        let mut statuses = session
            .runtime
            .block_on(session.handle.provider_states())
            .map_err(core_error)?
            .into_iter()
            .map(|(provider, state)| provider_status(provider, state))
            .collect::<Vec<_>>();
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(statuses)
    })
}

pub fn discovery_stop(session_id: u64) -> Result<(), HaloApiError> {
    let mut session = sessions()
        .lock()
        .map_err(|_| HaloApiError::InternalState)?
        .remove(&session_id)
        .ok_or(HaloApiError::SessionNotFound)?;
    if let Some(discovery) = session.session.take() {
        session
            .runtime
            .block_on(discovery.shutdown())
            .map_err(core_error)?;
    }
    Ok(())
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

fn snapshot(session: &mut SessionRuntime) -> Result<Vec<DiscoveryPeer>, HaloApiError> {
    let peers = session
        .runtime
        .block_on(session.handle.snapshot())
        .map_err(core_error)?;
    Ok(peers
        .into_iter()
        .map(|peer| DiscoveryPeer {
            presence_id: peer.presence_id.to_string(),
            device_type: peer.capabilities.device_type().into(),
            compatible: peer.compatible,
            capabilities: peer.capabilities.bits(),
            sources: peer
                .sources
                .into_iter()
                .map(|source| source.to_string())
                .collect(),
            best_endpoint: peer
                .best_endpoint
                .map(|endpoint| endpoint.address().to_string()),
            candidate_count: u32::try_from(peer.candidates.len()).unwrap_or(u32::MAX),
            quarantined: peer.quarantined,
        })
        .collect())
}

fn provider_status(provider: ProviderId, state: ProviderState) -> DiscoveryProviderStatus {
    let (state_name, detail) = match state {
        ProviderState::Starting => ("starting", None),
        ProviderState::Ready => ("ready", None),
        ProviderState::Degraded(detail) => ("degraded", Some(detail)),
        ProviderState::PermissionRequired(detail) => ("permission_required", Some(detail)),
        ProviderState::PermissionDenied(detail) => ("permission_denied", Some(detail)),
        ProviderState::HardwareOff => ("hardware_off", None),
        ProviderState::Unsupported => ("unsupported", None),
        ProviderState::TemporarilyUnavailable(detail) => ("temporarily_unavailable", Some(detail)),
        ProviderState::Failed {
            recoverable,
            reason,
        } => (
            if recoverable {
                "failed_recoverable"
            } else {
                "failed"
            },
            Some(reason),
        ),
        ProviderState::Stopped => ("stopped", None),
        _ => ("unknown", None),
    };
    DiscoveryProviderStatus {
        name: provider.name().to_owned(),
        kind: provider_kind_name(provider.kind()).to_owned(),
        state: state_name.to_owned(),
        detail,
    }
}

fn provider_kind_name(kind: &ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Ble => "ble",
        ProviderKind::Mdns => "mdns",
        ProviderKind::PresenceV4 => "presence_v4",
        ProviderKind::PresenceV6 => "presence_v6",
        ProviderKind::Direct => "direct",
        ProviderKind::WifiAware => "wifi_aware",
        ProviderKind::WifiDirect => "wifi_direct",
        ProviderKind::Custom => "custom",
        _ => "unknown",
    }
}

fn ble_provider_id(platform: &str) -> Result<ProviderId, HaloApiError> {
    let name = match platform {
        "android" => "ble-android",
        "ios" => "ble-ios",
        "macos" => "ble-macos",
        "windows" => "ble-windows",
        _ => {
            return Err(HaloApiError::InvalidArgument {
                message: "unknown platform BLE provider".to_owned(),
            });
        }
    };
    ProviderId::new(ProviderKind::Ble, name).map_err(core_error)
}

fn normalize_provider_state(state: PlatformProviderState, detail: Option<String>) -> ProviderState {
    let bounded_detail = detail
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect::<String>();
    match state {
        PlatformProviderState::Starting => ProviderState::Starting,
        PlatformProviderState::Ready => ProviderState::Ready,
        PlatformProviderState::Degraded => ProviderState::Degraded(bounded_detail),
        PlatformProviderState::PermissionRequired => {
            ProviderState::PermissionRequired(bounded_detail)
        }
        PlatformProviderState::PermissionDenied => ProviderState::PermissionDenied(bounded_detail),
        PlatformProviderState::HardwareOff => ProviderState::HardwareOff,
        PlatformProviderState::Unsupported => ProviderState::Unsupported,
        PlatformProviderState::TemporarilyUnavailable => {
            ProviderState::TemporarilyUnavailable(bounded_detail)
        }
        PlatformProviderState::Stopped => ProviderState::Stopped,
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
    fn ble_observation_crosses_ffi_into_rust_manager() {
        let bootstrap = discovery_start(44_330, false, DiscoveryDeviceType::Macos)
            .unwrap_or_else(|error| panic!("start failed: {error}"));
        let remote = LocalPresence::new(
            PresenceId::from_bytes([0x42; 16]),
            ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("range: {error}")),
            Capabilities::from_bits(7).with_device_type(DeviceType::Android),
            4433,
        )
        .unwrap_or_else(|error| panic!("remote: {error}"));

        let peers = discovery_submit_ble(
            bootstrap.session_id,
            "android".to_owned(),
            encode_presence(&remote, 9).to_vec(),
        )
        .unwrap_or_else(|error| panic!("submit failed: {error}"));

        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].presence_id, remote.presence_id.to_string());
        assert_eq!(peers[0].device_type, DiscoveryDeviceType::Android);
        assert_eq!(bootstrap.device_type, DiscoveryDeviceType::Macos);
        assert!(!bootstrap.presence_id.is_empty());
        assert_eq!(peers[0].sources, vec!["ble-android"]);
        assert!(peers[0].best_endpoint.is_none());
        discovery_stop(bootstrap.session_id).unwrap_or_else(|error| panic!("stop failed: {error}"));
    }

    #[test]
    fn malformed_native_bytes_are_rejected_by_rust() {
        let bootstrap = discovery_start(44_331, false, DiscoveryDeviceType::Android)
            .unwrap_or_else(|error| panic!("start failed: {error}"));
        let error = discovery_submit_ble(bootstrap.session_id, "macos".to_owned(), vec![0; 58])
            .expect_err("invalid descriptor must fail");
        assert!(matches!(error, HaloApiError::Core { .. }));
        discovery_stop(bootstrap.session_id).unwrap_or_else(|error| panic!("stop failed: {error}"));
    }

    #[test]
    fn provider_health_snapshot_includes_native_ble_and_is_sorted() {
        let bootstrap = discovery_start(44_332, false, DiscoveryDeviceType::Ios)
            .unwrap_or_else(|error| panic!("start failed: {error}"));
        discovery_report_ble_state(
            bootstrap.session_id,
            "ios".to_owned(),
            PlatformProviderState::PermissionDenied,
            Some("denied by user".to_owned()),
        )
        .unwrap_or_else(|error| panic!("state failed: {error}"));

        let statuses = discovery_provider_statuses(bootstrap.session_id)
            .unwrap_or_else(|error| panic!("statuses failed: {error}"));
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].name, "ble-ios");
        assert_eq!(statuses[0].kind, "ble");
        assert_eq!(statuses[0].state, "permission_denied");
        assert_eq!(statuses[0].detail.as_deref(), Some("denied by user"));
        discovery_stop(bootstrap.session_id).unwrap_or_else(|error| panic!("stop failed: {error}"));
    }
}
