use std::time::Duration;

use halo_discovery::{
    Capabilities, DiscoveryHandle, DiscoveryManager, DiscoverySession, LocalPresence, PresenceId,
    ProtocolRange, ProviderId, ProviderKind, ProviderState,
    ble::{decode_presence, encode_presence},
    providers::{MdnsProvider, PresenceV4Provider, PresenceV6Provider},
};
use thiserror::Error;

const BLE_OBSERVATION_TTL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceType {
    Unknown,
    Android,
    Ios,
    Macos,
    Windows,
    Linux,
}

impl From<DeviceType> for halo_discovery::DeviceType {
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

impl From<halo_discovery::DeviceType> for DeviceType {
    fn from(value: halo_discovery::DeviceType) -> Self {
        match value {
            halo_discovery::DeviceType::Unknown => Self::Unknown,
            halo_discovery::DeviceType::Android => Self::Android,
            halo_discovery::DeviceType::Ios => Self::Ios,
            halo_discovery::DeviceType::Macos => Self::Macos,
            halo_discovery::DeviceType::Windows => Self::Windows,
            halo_discovery::DeviceType::Linux => Self::Linux,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    quic_port: u16,
    enable_lan: bool,
    device_type: DeviceType,
}

impl DiscoveryConfig {
    #[must_use]
    pub const fn new(quic_port: u16, device_type: DeviceType) -> Self {
        Self {
            quic_port,
            enable_lan: true,
            device_type,
        }
    }

    #[must_use]
    pub const fn with_lan(mut self, enable_lan: bool) -> Self {
        self.enable_lan = enable_lan;
        self
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveryStartup {
    pub presence_id: String,
    pub device_type: DeviceType,
    pub ble_presence: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryPeer {
    pub presence_id: String,
    pub device_type: DeviceType,
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

pub struct DiscoveryService {
    local: LocalPresence,
    handle: DiscoveryHandle,
    session: Option<DiscoverySession>,
    sequence: u64,
}

impl DiscoveryService {
    pub async fn start(
        config: DiscoveryConfig,
    ) -> Result<(Self, DiscoveryStartup), DiscoveryError> {
        let protocol = ProtocolRange::new(1, 1).map_err(core_error)?;
        let local = LocalPresence::new(
            PresenceId::random(),
            protocol,
            Capabilities::default().with_device_type(config.device_type.into()),
            config.quic_port,
        )
        .map_err(core_error)?;
        let mut manager = DiscoveryManager::new(local.clone());
        if config.enable_lan {
            manager = manager
                .with_provider(MdnsProvider::default())
                .with_provider(PresenceV4Provider::default())
                .with_provider(PresenceV6Provider::default());
        }
        let session = manager.start().await.map_err(core_error)?;
        let handle = session.handle();
        let startup = DiscoveryStartup {
            presence_id: local.presence_id.to_string(),
            device_type: config.device_type,
            ble_presence: encode_presence(&local, 1).to_vec(),
        };
        Ok((
            Self {
                local,
                handle,
                session: Some(session),
                sequence: 1,
            },
            startup,
        ))
    }

    #[must_use]
    pub fn refresh_ble_presence(&mut self) -> Vec<u8> {
        self.sequence = self.sequence.saturating_add(1);
        encode_presence(&self.local, self.sequence).to_vec()
    }

    pub async fn submit_ble(
        &self,
        platform: &str,
        descriptor: &[u8],
    ) -> Result<Vec<DiscoveryPeer>, DiscoveryError> {
        let provider = ble_provider_id(platform)?;
        let observation =
            decode_presence(descriptor, provider, BLE_OBSERVATION_TTL).map_err(core_error)?;
        self.handle
            .submit_observation(observation)
            .await
            .map_err(core_error)?;
        self.snapshot().await
    }

    pub async fn report_ble_state(
        &self,
        platform: &str,
        state: PlatformProviderState,
        detail: Option<String>,
    ) -> Result<(), DiscoveryError> {
        let provider = ble_provider_id(platform)?;
        self.handle
            .report_provider_state(provider, normalize_provider_state(state, detail))
            .await
            .map_err(core_error)
    }

    pub async fn snapshot(&self) -> Result<Vec<DiscoveryPeer>, DiscoveryError> {
        let peers = self.handle.snapshot().await.map_err(core_error)?;
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
                candidate_endpoints: peer
                    .candidates
                    .iter()
                    .map(|candidate| candidate.endpoint.address().to_string())
                    .collect(),
                candidate_count: u32::try_from(peer.candidates.len()).unwrap_or(u32::MAX),
                quarantined: peer.quarantined,
            })
            .collect())
    }

    pub async fn provider_statuses(&self) -> Result<Vec<DiscoveryProviderStatus>, DiscoveryError> {
        let mut statuses = self
            .handle
            .provider_states()
            .await
            .map_err(core_error)?
            .into_iter()
            .map(|(provider, state)| provider_status(provider, state))
            .collect::<Vec<_>>();
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(statuses)
    }

    pub async fn shutdown(&mut self) -> Result<(), DiscoveryError> {
        if let Some(session) = self.session.take() {
            session.shutdown().await.map_err(core_error)?;
        }
        Ok(())
    }
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

fn ble_provider_id(platform: &str) -> Result<ProviderId, DiscoveryError> {
    let name = match platform {
        "android" => "ble-android",
        "ios" => "ble-ios",
        "macos" => "ble-macos",
        "windows" => "ble-windows",
        _ => return Err(DiscoveryError::UnknownPlatform),
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

fn core_error(error: impl std::fmt::Display) -> DiscoveryError {
    DiscoveryError::Core(error.to_string())
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DiscoveryError {
    #[error("unknown platform BLE provider")]
    UnknownPlatform,
    #[error("discovery operation failed: {0}")]
    Core(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn public_facade_accepts_platform_ble_observation() {
        let (mut service, startup) = DiscoveryService::start(
            DiscoveryConfig::new(44_330, DeviceType::Macos).with_lan(false),
        )
        .await
        .unwrap_or_else(|error| panic!("start: {error}"));
        let remote = LocalPresence::new(
            PresenceId::from_bytes([0x42; 16]),
            ProtocolRange::new(1, 1).unwrap_or_else(|error| panic!("range: {error}")),
            Capabilities::from_bits(7).with_device_type(halo_discovery::DeviceType::Android),
            4433,
        )
        .unwrap_or_else(|error| panic!("remote: {error}"));
        let peers = service
            .submit_ble("android", &encode_presence(&remote, 9))
            .await
            .unwrap_or_else(|error| panic!("submit: {error}"));
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_type, DeviceType::Android);
        assert_eq!(startup.device_type, DeviceType::Macos);
        assert!(!startup.presence_id.is_empty());
        service
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown: {error}"));
    }
}
