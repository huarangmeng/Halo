//! Connection racing, cancellation, and bounded pairing control I/O.

#![forbid(unsafe_code)]

mod frame_io;
mod pairing_flow;
mod quic;
mod race;

pub use frame_io::{ControlIo, FrameIoError, receive_message, send_message};
pub use pairing_flow::{
    PairingFlowError, PairingOutcome, PairingPrompt, PairingUserInteraction, pair_as_initiator,
    pair_as_responder,
};
pub use quic::{QuicConnection, QuicEndpoint, QuicEndpointError};
pub use race::{
    ConnectAttemptError, ConnectErrorKind, ConnectionCandidate, ConnectionRacer, SecureConnector,
};
