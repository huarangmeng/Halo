//! Rust-owned handoff for UDP sockets prepared by native platform adapters.

use std::{
    net::UdpSocket,
    sync::{Mutex, OnceLock},
};

#[cfg_attr(
    not(target_os = "android"),
    allow(
        dead_code,
        reason = "variants are produced only by the Android JNI adapter"
    )
)]
pub(crate) enum RegisteredLanEndpoint {
    Bound(UdpSocket),
    Disabled,
}

#[derive(Default)]
struct LanEndpointRegistry {
    endpoint: Option<RegisteredLanEndpoint>,
}

impl LanEndpointRegistry {
    #[cfg(any(target_os = "android", test))]
    fn replace(&mut self, endpoint: RegisteredLanEndpoint) {
        self.endpoint = Some(endpoint);
    }

    fn take(&mut self) -> Option<RegisteredLanEndpoint> {
        self.endpoint.take()
    }
}

static LAN_ENDPOINT: OnceLock<Mutex<LanEndpointRegistry>> = OnceLock::new();

#[cfg(target_os = "android")]
pub(crate) fn register_bound_lan_socket(socket: UdpSocket) -> Result<(), ()> {
    replace(RegisteredLanEndpoint::Bound(socket))
}

#[cfg(target_os = "android")]
pub(crate) fn disable_lan_endpoint() -> Result<(), ()> {
    replace(RegisteredLanEndpoint::Disabled)
}

pub(crate) fn take_lan_endpoint() -> Result<Option<RegisteredLanEndpoint>, ()> {
    let mut registry = registry().lock().map_err(|_| ())?;
    Ok(registry.take())
}

#[cfg(target_os = "android")]
fn replace(endpoint: RegisteredLanEndpoint) -> Result<(), ()> {
    registry().lock().map_err(|_| ())?.replace(endpoint);
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
        let mut registry = LanEndpointRegistry::default();
        registry.replace(RegisteredLanEndpoint::Bound(socket));
        registry.replace(RegisteredLanEndpoint::Disabled);

        assert!(matches!(
            registry.take(),
            Some(RegisteredLanEndpoint::Disabled)
        ));
        assert!(registry.take().is_none());
    }
}
