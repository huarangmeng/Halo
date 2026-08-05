//! Deterministic, bounded Halo control-plane messages.

#![forbid(unsafe_code)]

mod pairing;
mod transfer;

pub use pairing::{
    Capabilities, ClientHello, HEADER_LEN, IDENTITY_KEY_LEN, MAX_FRAME_LEN, NONCE_LEN,
    PairingCommit, PairingDecision, PairingMessage, ProtocolError, ProtocolRange, SIGNATURE_LEN,
    ServerHello, TRANSCRIPT_HASH_LEN, WIRE_VERSION,
};
pub use transfer::{
    CONTENT_DIGEST_LEN, DATA_RECORD_HEADER_LEN, DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE,
    MAX_DATA_RECORD_LEN, MAX_FILE_NAME_LEN, MAX_FILE_SIZE, TRANSFER_ID_LEN, TransferCancel,
    TransferCancelReason, TransferChunk, TransferComplete, TransferDecision, TransferMessage,
    TransferOffer, TransferProtocolError,
};
