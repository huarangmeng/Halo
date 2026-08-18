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
    BATCH_MANIFEST_DIGEST_LEN, BatchCancel, BatchCancelReason, BatchChunk, BatchChunkRef,
    BatchComplete, BatchDecision, BatchPause, BatchPauseReason, CONTENT_DIGEST_LEN,
    DATA_RECORD_HEADER_LEN, DEFAULT_CHUNK_SIZE, MAX_BATCH_FILES, MAX_CHUNK_SIZE,
    MAX_DATA_RECORD_LEN, MAX_FILE_NAME_LEN, MAX_FILE_SIZE, ManifestFile, ResumePosition,
    TRANSFER_ID_LEN, TRANSFER_WIRE_VERSION, TransferManifest, TransferMessage,
    TransferProtocolError,
};
