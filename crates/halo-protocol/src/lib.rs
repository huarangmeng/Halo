//! Deterministic, bounded Halo control-plane messages.

#![forbid(unsafe_code)]

mod pairing;

pub use pairing::{
    Capabilities, ClientHello, HEADER_LEN, IDENTITY_KEY_LEN, MAX_FRAME_LEN, NONCE_LEN,
    PairingCommit, PairingDecision, PairingMessage, ProtocolError, ProtocolRange, SIGNATURE_LEN,
    ServerHello, TRANSCRIPT_HASH_LEN, WIRE_VERSION,
};
