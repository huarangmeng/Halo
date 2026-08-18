//! Rust-owned device identity, transcript authentication, and trust models.

#![forbid(unsafe_code)]

mod identity;
mod pairing;
mod store;

pub use identity::{
    DeviceIdentity, IdentityError, IdentityPublicKey, IdentitySignature, SecretIdentityBlob,
};
pub use pairing::{
    PairingCode, PairingCryptoError, TlsChannelBinding, client_hello_hash, create_client_hello,
    create_commit, create_decision, create_server_hello, pairing_code, transcript_hash,
    verify_client_hello, verify_commit, verify_decision, verify_server_hello,
};
pub use store::{
    FileTrustStore, IdentityBlobStore, PeerId, RememberedEndpoint, StoreError, TrustStore,
    TrustedPeer, derive_peer_id,
};
