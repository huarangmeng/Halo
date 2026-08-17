//! Connection racing, cancellation, and bounded pairing control I/O.

#![forbid(unsafe_code)]

mod data_channel;
mod data_io;
mod frame_io;
mod pairing_flow;
mod platform_control;
mod quic;
mod race;

pub use data_channel::{
    AuthenticatedDataChannelResult, DataChannelAuthenticator, DataChannelBroker,
    DataChannelCandidate, DataChannelCandidateId, DataChannelCandidateProperties,
    DataChannelCapability, DataChannelCapabilityState, DataChannelCost, DataChannelError,
    DataChannelKind, DataChannelPathClass, DataChannelPeer, DataChannelPolicy, DataChannelProvider,
    EstablishedDataChannel, EstablishedDataChannelResult, EstablishedPathProperties,
    LocalNetworkScope,
};
pub use data_io::{DataIo, DataIoError};
pub use frame_io::{ControlIo, FrameIoError, receive_message, send_message};
pub use pairing_flow::{
    PairingFlowError, PairingOutcome, PairingPrompt, PairingUserInteraction, pair_as_initiator,
    pair_as_responder,
};
pub use platform_control::{
    PlatformControlDriver, PlatformControlError, PlatformControlIo, platform_control_channel,
};
pub use quic::{
    NativeTlsIdentity, QuicConnection, QuicDataIo, QuicEndpoint, QuicEndpointError,
    generate_native_tls_identity,
};
pub use race::{
    ConnectAttemptError, ConnectErrorKind, ConnectionCandidate, ConnectionRacer, SecureConnector,
};
