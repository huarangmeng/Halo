use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
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
const REMEMBERED_ENDPOINT_MAGIC: &[u8; 4] = b"HRI1";
const TRUST_RECORD_LEN: usize = 4 + 65 + 2;
const REMEMBERED_ENDPOINT_RECORD_LEN: usize = 4 + 1 + 16 + 65 + 2;
const MAX_TRUST_DIRECTORY_ENTRIES: usize = 1024;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PeerId([u8; 16]);

impl PeerId {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RememberedEndpoint {
    pub address: IpAddr,
    pub peer: TrustedPeer,
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

    fn remembered_endpoint_path(&self, address: IpAddr) -> PathBuf {
        self.endpoint_path(address).with_extension("remembered")
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
        self.save_remembered_endpoint(address, peer).await?;
        if let Err(error) = self
            .save_record(self.endpoint_path(address), ENDPOINT_MAGIC, peer)
            .await
        {
            let _ = fs::remove_file(self.remembered_endpoint_path(address)).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn remembered_endpoints(
        &self,
        maximum: usize,
    ) -> Result<Vec<RememberedEndpoint>, StoreError> {
        if maximum == 0 || maximum > MAX_TRUST_DIRECTORY_ENTRIES {
            return Err(StoreError::Persistence);
        }
        let mut entries = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(StoreError::Persistence),
        };
        let mut remembered = Vec::new();
        let mut visited = 0_usize;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| StoreError::Persistence)?
        {
            visited = visited.checked_add(1).ok_or(StoreError::Persistence)?;
            if visited > MAX_TRUST_DIRECTORY_ENTRIES {
                return Err(StoreError::Persistence);
            }
            if entry.path().extension().and_then(|value| value.to_str()) != Some("remembered") {
                continue;
            }
            remembered.push(load_remembered_endpoint(&entry.path()).await?);
        }
        remembered.sort_by_key(|endpoint| endpoint.address);
        remembered.truncate(maximum);
        Ok(remembered)
    }

    pub async fn trusted_peers(&self, maximum: usize) -> Result<Vec<TrustedPeer>, StoreError> {
        if maximum == 0 || maximum > MAX_TRUST_DIRECTORY_ENTRIES {
            return Err(StoreError::Persistence);
        }
        let mut entries = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(StoreError::Persistence),
        };
        let mut peers = Vec::new();
        let mut visited = 0_usize;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| StoreError::Persistence)?
        {
            visited = visited.checked_add(1).ok_or(StoreError::Persistence)?;
            if visited > MAX_TRUST_DIRECTORY_ENTRIES {
                return Err(StoreError::Persistence);
            }
            if entry.path().extension().and_then(|value| value.to_str()) != Some("peer") {
                continue;
            }
            let peer = load_record(&entry.path(), TRUST_MAGIC, None)
                .await?
                .ok_or(StoreError::Corrupt)?;
            if entry.path().file_stem().and_then(|value| value.to_str())
                != Some(hex(peer.peer_id().as_bytes()).as_str())
            {
                return Err(StoreError::Corrupt);
            }
            peers.push(peer);
        }
        peers.sort_by_key(TrustedPeer::peer_id);
        peers.truncate(maximum);
        Ok(peers)
    }

    async fn save_remembered_endpoint(
        &self,
        address: IpAddr,
        peer: &TrustedPeer,
    ) -> Result<(), StoreError> {
        fs::create_dir_all(&self.root)
            .await
            .map_err(|_| StoreError::Persistence)?;
        let final_path = self.remembered_endpoint_path(address);
        reject_non_file_target(&final_path).await?;
        let suffix = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let temp_path = self.root.join(format!(".remembered-{suffix}.tmp"));
        let mut bytes = Vec::with_capacity(REMEMBERED_ENDPOINT_RECORD_LEN);
        bytes.extend_from_slice(REMEMBERED_ENDPOINT_MAGIC);
        match address {
            IpAddr::V4(address) => {
                bytes.push(4);
                bytes.extend_from_slice(&[0; 12]);
                bytes.extend_from_slice(&address.octets());
            }
            IpAddr::V6(address) => {
                bytes.push(6);
                bytes.extend_from_slice(&address.octets());
            }
        }
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

    pub async fn revoke_peer(&self, peer_id: PeerId) -> Result<(), StoreError> {
        let mut endpoint_paths = Vec::new();
        let mut entries = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.delete(peer_id).await;
            }
            Err(_) => return Err(StoreError::Persistence),
        };
        let mut visited = 0_usize;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| StoreError::Persistence)?
        {
            visited = visited.checked_add(1).ok_or(StoreError::Persistence)?;
            if visited > MAX_TRUST_DIRECTORY_ENTRIES {
                return Err(StoreError::Persistence);
            }
            let path = entry.path();
            let peer = match path.extension().and_then(|extension| extension.to_str()) {
                Some("endpoint") => load_record(&path, ENDPOINT_MAGIC, None)
                    .await?
                    .ok_or(StoreError::Corrupt)?,
                Some("remembered") => load_remembered_endpoint(&path).await?.peer,
                _ => continue,
            };
            if peer.peer_id() == peer_id {
                endpoint_paths.push(path);
            }
        }
        for path in endpoint_paths {
            fs::remove_file(path)
                .await
                .map_err(|_| StoreError::Persistence)?;
        }
        self.delete(peer_id).await
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

async fn load_remembered_endpoint(path: &Path) -> Result<RememberedEndpoint, StoreError> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|_| StoreError::Persistence)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(StoreError::Corrupt);
    }
    let bytes = fs::read(path).await.map_err(|_| StoreError::Persistence)?;
    if bytes.len() != REMEMBERED_ENDPOINT_RECORD_LEN || &bytes[..4] != REMEMBERED_ENDPOINT_MAGIC {
        return Err(StoreError::Corrupt);
    }
    let address = match bytes[4] {
        4 if bytes[5..17] == [0; 12] => {
            IpAddr::V4(Ipv4Addr::new(bytes[17], bytes[18], bytes[19], bytes[20]))
        }
        6 => {
            let mut octets = [0_u8; 16];
            octets.copy_from_slice(&bytes[5..21]);
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        _ => return Err(StoreError::Corrupt),
    };
    let mut key = [0_u8; 65];
    key.copy_from_slice(&bytes[21..86]);
    let identity_key = IdentityPublicKey::from_bytes(key).map_err(|_| StoreError::Corrupt)?;
    Ok(RememberedEndpoint {
        address,
        peer: TrustedPeer {
            identity_key,
            protocol_version: u16::from_be_bytes([bytes[86], bytes[87]]),
        },
    })
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

    #[tokio::test]
    async fn revocation_removes_peer_and_endpoint_bindings_only() {
        let root = test_directory("revoke");
        let store = FileTrustStore::new(&root).unwrap_or_else(|error| panic!("store: {error}"));
        let first =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("first identity: {error}"));
        let second =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("second identity: {error}"));
        let first_peer = TrustedPeer {
            identity_key: first.public_key(),
            protocol_version: 1,
        };
        let second_peer = TrustedPeer {
            identity_key: second.public_key(),
            protocol_version: 1,
        };
        let first_address = "192.168.1.2"
            .parse()
            .unwrap_or_else(|error| panic!("first address: {error}"));
        let second_address = "192.168.1.3"
            .parse()
            .unwrap_or_else(|error| panic!("second address: {error}"));
        for (peer, address) in [(&first_peer, first_address), (&second_peer, second_address)] {
            store
                .save(peer)
                .await
                .unwrap_or_else(|error| panic!("save peer: {error}"));
            store
                .bind_ip(address, peer)
                .await
                .unwrap_or_else(|error| panic!("bind endpoint: {error}"));
        }
        assert_eq!(
            store
                .remembered_endpoints(8)
                .await
                .unwrap_or_else(|error| panic!("remembered endpoints: {error}"))
                .iter()
                .map(|endpoint| endpoint.address)
                .collect::<Vec<_>>(),
            vec![first_address, second_address]
        );
        assert_eq!(
            store
                .trusted_peers(8)
                .await
                .unwrap_or_else(|error| panic!("trusted peers: {error}"))
                .len(),
            2
        );

        store
            .revoke_peer(first_peer.peer_id())
            .await
            .unwrap_or_else(|error| panic!("revoke peer: {error}"));

        assert_eq!(store.load(first_peer.peer_id()).await, Ok(None));
        assert_eq!(store.load_expected_for_ip(first_address).await, Ok(None));
        assert_eq!(
            store.load(second_peer.peer_id()).await,
            Ok(Some(second_peer.clone()))
        );
        assert_eq!(
            store.load_expected_for_ip(second_address).await,
            Ok(Some(second_peer.clone()))
        );
        assert_eq!(
            store.remembered_endpoints(8).await,
            Ok(vec![RememberedEndpoint {
                address: second_address,
                peer: second_peer.clone(),
            }])
        );
        assert_eq!(store.trusted_peers(8).await, Ok(vec![second_peer]));
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
