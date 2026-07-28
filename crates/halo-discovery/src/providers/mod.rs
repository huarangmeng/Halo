//! Built-in network discovery providers.

mod direct;
mod mdns;
mod presence;

pub use direct::{DirectProbeConfig, DirectProbeProvider, KnownEndpoint};
pub use mdns::{MdnsConfig, MdnsProvider};
pub use presence::{
    PRESENCE_IPV4_GROUP, PRESENCE_IPV6_GROUP, PRESENCE_PORT, PresenceV4Config, PresenceV4Provider,
    PresenceV6Config, PresenceV6Provider,
};
