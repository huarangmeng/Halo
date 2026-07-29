use p256::{
    SecretKey,
    ecdsa::{Signature, SigningKey, VerifyingKey, signature::Signer, signature::Verifier},
    elliptic_curve::Generate,
};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

use halo_protocol::{IDENTITY_KEY_LEN, SIGNATURE_LEN};

const BLOB_MAGIC: &[u8; 4] = b"HKEY";
const BLOB_VERSION: u16 = 1;
const SECRET_KEY_LEN: usize = 32;
const BLOB_LEN: usize = BLOB_MAGIC.len() + 2 + SECRET_KEY_LEN;

pub struct DeviceIdentity {
    signing_key: SigningKey,
}

impl DeviceIdentity {
    pub fn generate() -> Result<Self, IdentityError> {
        Ok(Self {
            signing_key: SigningKey::generate(),
        })
    }

    pub fn from_blob(blob: &SecretIdentityBlob) -> Result<Self, IdentityError> {
        let bytes = blob.as_bytes();
        if bytes.len() != BLOB_LEN {
            return Err(IdentityError::InvalidBlobLength(bytes.len()));
        }
        if &bytes[..4] != BLOB_MAGIC {
            return Err(IdentityError::InvalidBlobMagic);
        }
        let version = u16::from_be_bytes([bytes[4], bytes[5]]);
        if version != BLOB_VERSION {
            return Err(IdentityError::UnsupportedBlobVersion(version));
        }
        let secret =
            SecretKey::from_slice(&bytes[6..]).map_err(|_| IdentityError::InvalidSecret)?;
        Ok(Self {
            signing_key: SigningKey::from(secret),
        })
    }

    #[must_use]
    pub fn to_blob(&self) -> SecretIdentityBlob {
        let mut bytes = Vec::with_capacity(BLOB_LEN);
        bytes.extend_from_slice(BLOB_MAGIC);
        bytes.extend_from_slice(&BLOB_VERSION.to_be_bytes());
        bytes.extend_from_slice(&self.signing_key.to_bytes());
        SecretIdentityBlob(bytes)
    }

    #[must_use]
    pub fn public_key(&self) -> IdentityPublicKey {
        let point = self.signing_key.verifying_key().to_sec1_point(false);
        let mut bytes = [0_u8; IDENTITY_KEY_LEN];
        bytes.copy_from_slice(point.as_bytes());
        IdentityPublicKey(bytes)
    }

    #[must_use]
    pub fn sign(&self, message: &[u8]) -> IdentitySignature {
        let signature: Signature = self.signing_key.sign(message);
        let normalized = signature.normalize_s();
        IdentitySignature(normalized.to_bytes().into())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct IdentityPublicKey([u8; IDENTITY_KEY_LEN]);

impl IdentityPublicKey {
    pub fn from_bytes(bytes: [u8; IDENTITY_KEY_LEN]) -> Result<Self, IdentityError> {
        let key =
            VerifyingKey::from_sec1_bytes(&bytes).map_err(|_| IdentityError::InvalidPublicKey)?;
        if key.to_sec1_point(false).as_bytes() != bytes {
            return Err(IdentityError::NonCanonicalPublicKey);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; IDENTITY_KEY_LEN] {
        &self.0
    }

    pub fn verify(
        &self,
        message: &[u8],
        signature: &IdentitySignature,
    ) -> Result<(), IdentityError> {
        let key =
            VerifyingKey::from_sec1_bytes(&self.0).map_err(|_| IdentityError::InvalidPublicKey)?;
        let parsed = Signature::from_slice(&signature.0)
            .map_err(|_| IdentityError::InvalidSignatureEncoding)?;
        if parsed.normalize_s() != parsed {
            return Err(IdentityError::NonCanonicalSignature);
        }
        key.verify(message, &parsed)
            .map_err(|_| IdentityError::SignatureVerification)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentitySignature([u8; SIGNATURE_LEN]);

impl IdentitySignature {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; SIGNATURE_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; SIGNATURE_LEN] {
        &self.0
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretIdentityBlob(Vec<u8>);

impl SecretIdentityBlob {
    pub fn new(bytes: Vec<u8>) -> Result<Self, IdentityError> {
        if bytes.len() > 256 {
            return Err(IdentityError::InvalidBlobLength(bytes.len()));
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum IdentityError {
    #[error("identity blob has invalid length {0}")]
    InvalidBlobLength(usize),
    #[error("identity blob magic does not match")]
    InvalidBlobMagic,
    #[error("unsupported identity blob version {0}")]
    UnsupportedBlobVersion(u16),
    #[error("identity blob contains an invalid P-256 secret")]
    InvalidSecret,
    #[error("identity public key is invalid")]
    InvalidPublicKey,
    #[error("identity public key is not canonical")]
    NonCanonicalPublicKey,
    #[error("identity signature encoding is invalid")]
    InvalidSignatureEncoding,
    #[error("identity signature is not low-S canonical form")]
    NonCanonicalSignature,
    #[error("identity signature verification failed")]
    SignatureVerification,
    #[error("operating-system random generator failed")]
    Random,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_blob_round_trips_and_signatures_verify() {
        let first =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("generate identity: {error}"));
        let blob = first.to_blob();
        let restored = DeviceIdentity::from_blob(&blob)
            .unwrap_or_else(|error| panic!("restore identity: {error}"));
        assert_eq!(first.public_key(), restored.public_key());

        let signature = restored.sign(b"bound handshake");
        restored
            .public_key()
            .verify(b"bound handshake", &signature)
            .unwrap_or_else(|error| panic!("verify signature: {error}"));
        assert_eq!(
            restored.public_key().verify(b"tampered", &signature),
            Err(IdentityError::SignatureVerification)
        );
    }

    #[test]
    fn rejects_malformed_identity_material() {
        let blob = SecretIdentityBlob::new(vec![0; BLOB_LEN])
            .unwrap_or_else(|error| panic!("test blob: {error}"));
        assert!(DeviceIdentity::from_blob(&blob).is_err());
        assert_eq!(
            IdentityPublicKey::from_bytes([0; IDENTITY_KEY_LEN]),
            Err(IdentityError::InvalidPublicKey)
        );
    }
}
