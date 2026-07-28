//! Multi-provider nearby discovery and endpoint selection for Halo.
//!
//! Discovery observations are untrusted rendezvous hints. Nothing in this
//! crate authenticates a device; consumers must complete Halo's secure
//! transport handshake before granting trust or disclosing file metadata.

#![forbid(unsafe_code)]

pub mod ble;
mod error;
mod manager;
mod model;
mod provider;
mod ranking;
pub mod wire;

pub mod providers;

pub use error::{DiscoveryError, ProviderError};
pub use manager::{DiscoveryHandle, DiscoveryManager, DiscoverySession};
pub use model::{
    Capabilities, ConnectionFailure, ConnectionOutcome, DeviceType, DiscoveryConfig,
    DiscoveryEvent, Endpoint, EndpointCandidate, LocalPresence, Observation, PeerSnapshot,
    PresenceId, ProtocolRange, ProviderId, ProviderKind, ProviderState,
};
pub use provider::{DiscoveryProvider, ProviderContext};
