//! Rust-owned handoff for UDP sockets prepared by native platform adapters.

use std::{
    net::UdpSocket,
    sync::{Mutex, OnceLock},
};

#[cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    allow(
        dead_code,
        reason = "variants are produced only by native platform adapters"
    )
)]
pub(crate) enum RegisteredLanEndpoint {
    SharedUnmetered(UdpSocket),
    UserApprovedHotspot(UdpSocket),
    Disabled,
}

pub(crate) enum RegisteredDiscoveryEndpoint {
    Bound(UdpSocket),
    Disabled,
}

#[derive(Default)]
struct LanEndpointRegistry {
    endpoint: Option<RegisteredLanEndpoint>,
    discovery: Option<RegisteredDiscoveryEndpoint>,
}

impl LanEndpointRegistry {
    #[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", test))]
    fn replace(&mut self, endpoint: RegisteredLanEndpoint, discovery: RegisteredDiscoveryEndpoint) {
        self.endpoint = Some(endpoint);
        self.discovery = Some(discovery);
    }

    fn take(&mut self) -> Option<RegisteredLanEndpoint> {
        self.endpoint.take()
    }

    fn take_discovery(&mut self) -> Option<RegisteredDiscoveryEndpoint> {
        self.discovery.take()
    }
}

static LAN_ENDPOINT: OnceLock<Mutex<LanEndpointRegistry>> = OnceLock::new();

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos"))]
pub(crate) fn register_bound_lan_sockets(
    socket: UdpSocket,
    discovery: UdpSocket,
) -> Result<(), ()> {
    replace(
        RegisteredLanEndpoint::SharedUnmetered(socket),
        RegisteredDiscoveryEndpoint::Bound(discovery),
    )
}

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos"))]
pub(crate) fn register_user_approved_hotspot_sockets(
    socket: UdpSocket,
    discovery: UdpSocket,
) -> Result<(), ()> {
    replace(
        RegisteredLanEndpoint::UserApprovedHotspot(socket),
        RegisteredDiscoveryEndpoint::Bound(discovery),
    )
}

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos"))]
pub(crate) fn disable_lan_endpoint() -> Result<(), ()> {
    replace(
        RegisteredLanEndpoint::Disabled,
        RegisteredDiscoveryEndpoint::Disabled,
    )
}

pub(crate) fn take_lan_endpoint() -> Result<Option<RegisteredLanEndpoint>, ()> {
    let mut registry = registry().lock().map_err(|_| ())?;
    Ok(registry.take())
}

pub(crate) fn take_discovery_endpoint() -> Result<Option<RegisteredDiscoveryEndpoint>, ()> {
    let mut registry = registry().lock().map_err(|_| ())?;
    Ok(registry.take_discovery())
}

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos"))]
fn replace(
    endpoint: RegisteredLanEndpoint,
    discovery: RegisteredDiscoveryEndpoint,
) -> Result<(), ()> {
    registry()
        .lock()
        .map_err(|_| ())?
        .replace(endpoint, discovery);
    Ok(())
}

fn registry() -> &'static Mutex<LanEndpointRegistry> {
    LAN_ENDPOINT.get_or_init(|| Mutex::new(LanEndpointRegistry::default()))
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use super::*;

    #[test]
    fn replacement_drops_stale_endpoint_and_take_is_single_use() {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .unwrap_or_else(|error| panic!("bind: {error}"));
        let discovery = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .unwrap_or_else(|error| panic!("discovery bind: {error}"));
        let mut registry = LanEndpointRegistry::default();
        registry.replace(
            RegisteredLanEndpoint::SharedUnmetered(socket),
            RegisteredDiscoveryEndpoint::Bound(discovery),
        );
        registry.replace(
            RegisteredLanEndpoint::Disabled,
            RegisteredDiscoveryEndpoint::Disabled,
        );

        assert!(matches!(
            registry.take(),
            Some(RegisteredLanEndpoint::Disabled)
        ));
        assert!(matches!(
            registry.take_discovery(),
            Some(RegisteredDiscoveryEndpoint::Disabled)
        ));
        assert!(registry.take().is_none());
        assert!(registry.take_discovery().is_none());
    }
}
