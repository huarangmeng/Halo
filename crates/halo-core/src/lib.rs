//! Stable, product-level Rust SDK facade for Halo.
//!
//! Applications depend on this crate. Protocol, cryptography, and transport
//! crates are implementation details and their types are not exposed here.

#![forbid(unsafe_code)]

mod discovery;
mod pairing;

pub use discovery::{
    DeviceType, DiscoveryConfig, DiscoveryError, DiscoveryPeer, DiscoveryProviderStatus,
    DiscoveryService, DiscoveryStartup, PlatformProviderState,
};
pub use pairing::{
    PairingConfig, PairingError, PairingEvent, PairingEventKind, PairingService, PairingStartup,
};
