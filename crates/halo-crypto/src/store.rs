use std::{
    net::IpAddr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};

use crate::{IdentityPublicKey, SecretIdentityBlob};

const TRUST_MAGIC: &[u8; 4] = b"HTR1";
const ENDPOINT_MAGIC: &[u8; 4] = b"HEP1";
const TRUST_RECORD_LEN: usize = 4 + 65 + 2;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PeerId([u8; 16]);

impl PeerId {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

#[must_use]
pub fn derive_peer_id(key: &IdentityPublicKey) -> PeerId {
    let digest = Sha256::digest(key.as_bytes());
    let mut id = [0_u8; 16];
    id.copy_from_slice(&digest[..16]);
    PeerId(id)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedPeer {
    pub identity_key: IdentityPublicKey,
    pub protocol_version: u16,
}

impl TrustedPeer {
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        derive_peer_id(&self.identity_key)
    }
}

/// Platform implementations protect opaque bytes and never interpret them.
#[async_trait]
pub trait IdentityBlobStore: Send + Sync {
    async fn load(&self) -> Result<Option<SecretIdentityBlob>, StoreError>;
    async fn save(&self, blob: &SecretIdentityBlob) -> Result<(), StoreError>;
    async fn delete(&self) -> Result<(), StoreError>;
}

/// Trust records contain public data but still require atomic, durable writes.
#[async_trait]
pub trait TrustStore: Send + Sync {
    async fn load(&self, peer_id: PeerId) -> Result<Option<TrustedPeer>, StoreError>;
    async fn save(&self, peer: &TrustedPeer) -> Result<(), StoreError>;
    async fn delete(&self, peer_id: PeerId) -> Result<(), StoreError>;
}

/// Rust-owned, deterministic trust-record persistence. The containing
/// directory must be app-private; records contain public keys, not identity
/// secrets, but writes are still atomic and malformed records fail closed.
#[derive(Clone, Debug)]
pub struct FileTrustStore {
    root: PathBuf,
}

impl FileTrustStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(StoreError::Persistence);
        }
        Ok(Self { root })
    }

    fn path_for(&self, peer_id: PeerId) -> PathBuf {
        self.root.join(format!("{}.peer", hex(peer_id.as_bytes())))
    }

    fn endpoint_path(&self, address: IpAddr) -> PathBuf {
        let mut digest = Sha256::new();
        digest.update(b"Halo endpoint binding v1");
        match address {
            IpAddr::V4(address) => {
                digest.update([4]);
                digest.update(address.octets());
            }
            IpAddr::V6(address) => {
                digest.update([6]);
                digest.update(address.octets());
            }
        }
        self.root
            .join(format!("{}.endpoint", hex(digest.finalize().as_slice())))
    }

    /// Returns the identity previously authenticated at a local-network IP.
    /// The port is deliberately excluded because listeners choose a new port
    /// after restart. A changed key at the same address fails closed.
    pub async fn load_expected_for_ip(
        &self,
        address: IpAddr,
    ) -> Result<Option<TrustedPeer>, StoreError> {
        load_record(&self.endpoint_path(address), ENDPOINT_MAGIC, None).await
    }

    /// Atomically updates the address-to-identity binding only after a full
    /// pairing flow has committed trust.
    pub async fn bind_ip(&self, address: IpAddr, peer: &TrustedPeer) -> Result<(), StoreError> {
        self.save_record(self.endpoint_path(address), ENDPOINT_MAGIC, peer)
            .await
    }

    async fn save_record(
        &self,
        final_path: PathBuf,
        magic: &[u8; 4],
        peer: &TrustedPeer,
    ) -> Result<(), StoreError> {
        fs::create_dir_all(&self.root)
            .await
            .map_err(|_| StoreError::Persistence)?;
        reject_non_file_target(&final_path).await?;
        let suffix = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let temp_path = self.root.join(format!(".trust-{suffix}.tmp"));
        let mut bytes = Vec::with_capacity(TRUST_RECORD_LEN);
        bytes.extend_from_slice(magic);
        bytes.extend_from_slice(peer.identity_key.as_bytes());
        bytes.extend_from_slice(&peer.protocol_version.to_be_bytes());
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .await
            .map_err(|_| StoreError::Persistence)?;
        if file.write_all(&bytes).await.is_err() || file.sync_all().await.is_err() {
            let _ = fs::remove_file(&temp_path).await;
            return Err(StoreError::Persistence);
        }
        drop(file);
        if fs::rename(&temp_path, &final_path).await.is_err() {
            let _ = fs::remove_file(&temp_path).await;
            return Err(StoreError::Persistence);
        }
        Ok(())
    }
}

#[async_trait]
impl TrustStore for FileTrustStore {
    async fn load(&self, peer_id: PeerId) -> Result<Option<TrustedPeer>, StoreError> {
        load_record(&self.path_for(peer_id), TRUST_MAGIC, Some(peer_id)).await
    }

    async fn save(&self, peer: &TrustedPeer) -> Result<(), StoreError> {
        self.save_record(self.path_for(peer.peer_id()), TRUST_MAGIC, peer)
            .await
    }

    async fn delete(&self, peer_id: PeerId) -> Result<(), StoreError> {
        let path = self.path_for(peer_id);
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StoreError::Persistence),
        }
    }
}

async fn load_record(
    path: &Path,
    magic: &[u8; 4],
    expected_peer_id: Option<PeerId>,
) -> Result<Option<TrustedPeer>, StoreError> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(StoreError::Persistence),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(StoreError::Corrupt);
    }
    let bytes = fs::read(path).await.map_err(|_| StoreError::Persistence)?;
    if bytes.len() != TRUST_RECORD_LEN || &bytes[..4] != magic {
        return Err(StoreError::Corrupt);
    }
    let mut key = [0_u8; 65];
    key.copy_from_slice(&bytes[4..69]);
    let identity_key = IdentityPublicKey::from_bytes(key).map_err(|_| StoreError::Corrupt)?;
    let peer = TrustedPeer {
        identity_key,
        protocol_version: u16::from_be_bytes([bytes[69], bytes[70]]),
    };
    if expected_peer_id.is_some_and(|expected| peer.peer_id() != expected) {
        return Err(StoreError::Corrupt);
    }
    Ok(Some(peer))
}

async fn reject_non_file_target(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(StoreError::Corrupt),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StoreError::Persistence),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StoreError {
    #[error("protected identity storage is unavailable")]
    Unavailable,
    #[error("protected identity storage is locked")]
    Locked,
    #[error("stored identity or trust data is corrupt")]
    Corrupt,
    #[error("identity or trust persistence failed")]
    Persistence,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceIdentity;

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "halo-trust-{name}-{}",
            NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn file_store_round_trips_and_rejects_tampering() {
        let root = test_directory("round-trip");
        let store = FileTrustStore::new(&root).unwrap_or_else(|error| panic!("store: {error}"));
        let identity =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("identity: {error}"));
        let peer = TrustedPeer {
            identity_key: identity.public_key(),
            protocol_version: 1,
        };
        store
            .save(&peer)
            .await
            .unwrap_or_else(|error| panic!("save: {error}"));
        assert_eq!(store.load(peer.peer_id()).await, Ok(Some(peer.clone())));

        fs::write(store.path_for(peer.peer_id()), b"tampered")
            .await
            .unwrap_or_else(|error| panic!("tamper: {error}"));
        assert_eq!(store.load(peer.peer_id()).await, Err(StoreError::Corrupt));
        let _ = fs::remove_dir_all(root).await;
    }

    #[test]
    fn relative_trust_directory_is_rejected() {
        assert_eq!(
            FileTrustStore::new("relative").unwrap_err(),
            StoreError::Persistence
        );
    }
}
