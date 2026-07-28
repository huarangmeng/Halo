use std::{
    collections::BTreeSet,
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    time::Duration,
};

use uuid::Uuid;

use crate::DiscoveryError;

/// A random identifier shared by all providers for one discoverable presence.
/// It is deliberately not a stable or authenticated device identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PresenceId([u8; 16]);

impl PresenceId {
    /// Generates a new cryptographically random UUID-backed presence ID.
    #[must_use]
    pub fn random() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }

    /// Creates an ID from its wire representation.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the fixed-width wire representation.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Returns a short value suitable for ephemeral mDNS instance names.
    #[must_use]
    pub fn short_hex(&self) -> String {
        self.0[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl fmt::Display for PresenceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Uuid::from_bytes(self.0).fmt(formatter)
    }
}

impl FromStr for PresenceId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(|uuid| Self(*uuid.as_bytes()))
    }
}

/// Inclusive range of Halo application protocol versions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolRange {
    min: u16,
    max: u16,
}

impl ProtocolRange {
    /// Creates a non-empty inclusive version range.
    pub fn new(min: u16, max: u16) -> Result<Self, DiscoveryError> {
        if min == 0 || max == 0 || min > max {
            return Err(DiscoveryError::InvalidConfig(format!(
                "protocol range must satisfy 0 < min <= max, got {min}..={max}"
            )));
        }
        Ok(Self { min, max })
    }

    /// Lowest supported version.
    #[must_use]
    pub const fn min(self) -> u16 {
        self.min
    }

    /// Highest supported version.
    #[must_use]
    pub const fn max(self) -> u16 {
        self.max
    }

    /// Whether two ranges share at least one version.
    #[must_use]
    pub const fn overlaps(self, other: Self) -> bool {
        self.min <= other.max && other.min <= self.max
    }

    /// Returns the shared range, if any.
    #[must_use]
    pub fn intersection(self, other: Self) -> Option<Self> {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max);
        (min <= max).then_some(Self { min, max })
    }
}

/// Coarse platform type advertised as an untrusted discovery hint.
///
/// This is presentation metadata, not an authenticated identity claim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum DeviceType {
    #[default]
    Unknown = 0,
    Android = 1,
    Ios = 2,
    Macos = 3,
    Windows = 4,
    Linux = 5,
}

impl DeviceType {
    const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Android,
            2 => Self::Ios,
            3 => Self::Macos,
            4 => Self::Windows,
            5 => Self::Linux,
            _ => Self::Unknown,
        }
    }
}

/// Capability bits advertised as untrusted discovery hints.
///
/// The upper nibble is reserved for [`DeviceType`]. Existing peers that do not
/// set it remain compatible and are reported as [`DeviceType::Unknown`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Capabilities(u64);

impl Capabilities {
    const DEVICE_TYPE_SHIFT: u32 = 60;
    const DEVICE_TYPE_MASK: u64 = 0xf_u64 << Self::DEVICE_TYPE_SHIFT;

    /// Creates a bit set from its wire representation.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the wire representation.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns a copy carrying one coarse device type in the reserved nibble.
    #[must_use]
    pub const fn with_device_type(self, device_type: DeviceType) -> Self {
        Self((self.0 & !Self::DEVICE_TYPE_MASK) | ((device_type as u64) << Self::DEVICE_TYPE_SHIFT))
    }

    /// Decodes the untrusted coarse device type metadata.
    #[must_use]
    pub const fn device_type(self) -> DeviceType {
        DeviceType::from_code(((self.0 & Self::DEVICE_TYPE_MASK) >> Self::DEVICE_TYPE_SHIFT) as u8)
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn device_type_round_trips_without_changing_feature_bits() {
        let features = Capabilities::from_bits(0x0123_4567_89ab_cdef);
        let android = features.with_device_type(DeviceType::Android);
        assert_eq!(android.device_type(), DeviceType::Android);
        assert_eq!(
            android.bits() & 0x0fff_ffff_ffff_ffff,
            0x0123_4567_89ab_cdef
        );
        assert_eq!(
            android.with_device_type(DeviceType::Macos).device_type(),
            DeviceType::Macos
        );
    }

    #[test]
    fn unset_or_unknown_device_type_is_unknown() {
        assert_eq!(Capabilities::default().device_type(), DeviceType::Unknown);
        assert_eq!(
            Capabilities::from_bits(0xf000_0000_0000_0000).device_type(),
            DeviceType::Unknown
        );
    }
}

/// Broad provider class used by the endpoint ranker.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum ProviderKind {
    Ble,
    Mdns,
    PresenceV4,
    PresenceV6,
    Direct,
    WifiAware,
    WifiDirect,
    Custom,
}

/// Stable identifier for one provider instance in a discovery session.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId {
    kind: ProviderKind,
    name: String,
}

impl ProviderId {
    /// Creates an identifier with a short diagnostic-only name.
    pub fn new(kind: ProviderKind, name: impl Into<String>) -> Result<Self, DiscoveryError> {
        let name = name.into();
        let valid = !name.is_empty()
            && name.len() <= 48
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        if !valid {
            return Err(DiscoveryError::InvalidConfig(
                "provider name must be 1-48 ASCII letters, digits, '-' or '_'".to_owned(),
            ));
        }
        Ok(Self { kind, name })
    }

    #[must_use]
    pub fn kind(&self) -> &ProviderKind {
        &self.kind
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn builtin(kind: ProviderKind, name: &str) -> Self {
        Self {
            kind,
            name: name.to_owned(),
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)
    }
}

/// A candidate transport endpoint. It remains untrusted until secure handshake.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Endpoint {
    address: SocketAddr,
}

impl Endpoint {
    /// Validates and creates a QUIC endpoint.
    pub fn quic(address: SocketAddr) -> Result<Self, DiscoveryError> {
        if address.port() == 0 || address.ip().is_unspecified() || address.ip().is_multicast() {
            return Err(DiscoveryError::InvalidConfig(format!(
                "invalid QUIC endpoint {address}"
            )));
        }
        if let SocketAddr::V6(address) = address
            && address.ip().is_unicast_link_local()
            && address.scope_id() == 0
        {
            return Err(DiscoveryError::InvalidConfig(format!(
                "IPv6 link-local endpoint requires an interface scope: {address}"
            )));
        }
        Ok(Self { address })
    }

    #[must_use]
    pub const fn address(self) -> SocketAddr {
        self.address
    }

    #[must_use]
    pub const fn ip(self) -> IpAddr {
        self.address.ip()
    }
}

/// Local, ephemeral data advertised by providers.
#[derive(Clone, Debug)]
pub struct LocalPresence {
    pub presence_id: PresenceId,
    pub protocol: ProtocolRange,
    pub capabilities: Capabilities,
    pub quic_port: u16,
}

impl LocalPresence {
    pub fn new(
        presence_id: PresenceId,
        protocol: ProtocolRange,
        capabilities: Capabilities,
        quic_port: u16,
    ) -> Result<Self, DiscoveryError> {
        if quic_port == 0 {
            return Err(DiscoveryError::InvalidConfig(
                "QUIC listener port must be non-zero".to_owned(),
            ));
        }
        Ok(Self {
            presence_id,
            protocol,
            capabilities,
            quic_port,
        })
    }
}

/// One untrusted observation submitted by a discovery provider.
#[derive(Clone, Debug)]
pub struct Observation {
    pub provider: ProviderId,
    pub presence_id: PresenceId,
    pub protocol: ProtocolRange,
    pub capabilities: Capabilities,
    pub sequence: u64,
    pub endpoints: Vec<Endpoint>,
    pub ttl: Duration,
    pub round_trip_time: Option<Duration>,
}

/// Runtime health of an individual provider.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProviderState {
    Starting,
    Ready,
    Degraded(String),
    PermissionRequired(String),
    PermissionDenied(String),
    HardwareOff,
    Unsupported,
    TemporarilyUnavailable(String),
    Failed { recoverable: bool, reason: String },
    Stopped,
}

/// One ranked endpoint candidate retained for fallback or connection racing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointCandidate {
    pub endpoint: Endpoint,
    pub score: i32,
    pub sources: BTreeSet<ProviderId>,
    pub round_trip_time: Option<Duration>,
    pub successful_connections: u32,
    pub consecutive_failures: u32,
}

/// Coherent peer view emitted after provider observations are merged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerSnapshot {
    pub presence_id: PresenceId,
    pub protocol: ProtocolRange,
    pub compatible: bool,
    pub capabilities: Capabilities,
    pub sources: BTreeSet<ProviderId>,
    pub candidates: Vec<EndpointCandidate>,
    pub best_endpoint: Option<Endpoint>,
    pub quarantined: bool,
}

/// Events are bounded. A lagging receiver must fetch a fresh snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DiscoveryEvent {
    PeerAppeared(PeerSnapshot),
    PeerChanged(PeerSnapshot),
    PeerExpired(PresenceId),
    PeerQuarantined(PresenceId),
    ProviderChanged {
        provider: ProviderId,
        state: ProviderState,
    },
}

/// Structured reason for a failed active connection attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConnectionFailure {
    NoRoute,
    Timeout,
    Refused,
    ProtocolMismatch,
    AuthenticationFailed,
    Cancelled,
    Other,
}

/// Result supplied by the secure connection layer to improve endpoint ranking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionOutcome {
    Success { handshake_time: Duration },
    Failure(ConnectionFailure),
}

/// Resource, timing, and queue limits for one discovery session.
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    pub input_capacity: usize,
    pub event_capacity: usize,
    pub max_peers: usize,
    pub max_endpoints_per_peer: usize,
    pub min_observation_ttl: Duration,
    pub max_observation_ttl: Duration,
    pub expiry_interval: Duration,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            input_capacity: 256,
            event_capacity: 256,
            max_peers: 256,
            max_endpoints_per_peer: 24,
            min_observation_ttl: Duration::from_secs(2),
            max_observation_ttl: Duration::from_secs(120),
            expiry_interval: Duration::from_millis(500),
        }
    }
}

impl DiscoveryConfig {
    pub(crate) fn validate(&self) -> Result<(), DiscoveryError> {
        if self.input_capacity == 0
            || self.event_capacity == 0
            || self.max_peers == 0
            || self.max_endpoints_per_peer == 0
        {
            return Err(DiscoveryError::InvalidConfig(
                "queue and collection limits must be non-zero".to_owned(),
            ));
        }
        if self.min_observation_ttl.is_zero()
            || self.min_observation_ttl > self.max_observation_ttl
            || self.expiry_interval.is_zero()
        {
            return Err(DiscoveryError::InvalidConfig(
                "invalid discovery timing limits".to_owned(),
            ));
        }
        Ok(())
    }
}
