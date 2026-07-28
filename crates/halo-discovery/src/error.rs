use thiserror::Error;

/// Errors returned by the discovery manager.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// Configuration was rejected before tasks were started.
    #[error("invalid discovery configuration: {0}")]
    InvalidConfig(String),

    /// The discovery session has already stopped.
    #[error("discovery session is closed")]
    SessionClosed,

    /// A manager or provider task could not be joined.
    #[error("discovery task failed: {0}")]
    TaskFailed(String),
}

/// A provider-local failure. Other providers continue running after this error.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProviderError {
    /// Provider configuration is invalid for every runtime environment.
    #[error("invalid provider configuration: {0}")]
    InvalidConfig(String),

    /// An operating-system networking operation failed.
    #[error("network operation failed: {0}")]
    Network(String),

    /// A platform capability or permission is unavailable.
    #[error("provider unavailable: {0}")]
    Unavailable(String),

    /// The provider's internal event channel closed unexpectedly.
    #[error("provider event stream closed")]
    EventStreamClosed,
}

impl From<std::io::Error> for ProviderError {
    fn from(value: std::io::Error) -> Self {
        Self::Network(value.to_string())
    }
}
