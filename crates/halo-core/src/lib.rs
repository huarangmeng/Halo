//! Stable, product-level Rust SDK facade for Halo.
//!
//! Applications depend on this crate. Protocol, cryptography, and transport
//! crates are implementation details and their types are not exposed here.

#![forbid(unsafe_code)]

mod discovery;
mod pairing;
mod transfer;

pub use discovery::{
    DeviceType, DiscoveryConfig, DiscoveryError, DiscoveryPeer, DiscoveryProviderStatus,
    DiscoveryService, DiscoveryStartup, PlatformProviderState,
};
pub use pairing::{
    AuthenticatedSessionInfo, PairingConfig, PairingError, PairingEvent, PairingEventKind,
    PairingPolicy, PairingService, PairingStartup, PlatformPairingChannelState,
    PlatformPairingRole, PlatformTlsIdentity, create_platform_tls_identity,
};
pub use transfer::{
    TransferDirection, TransferEvent, TransferEventKind, TransferPolicy, TransferServiceError,
};
