use std::fmt;

use getrandom::fill;
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use thiserror::Error;

use halo_protocol::{
    Capabilities, ClientHello, NONCE_LEN, PairingCommit, PairingDecision, ProtocolRange,
    ServerHello, TRANSCRIPT_HASH_LEN,
};

use crate::{DeviceIdentity, IdentityError, IdentityPublicKey, IdentitySignature};

const CLIENT_DOMAIN: &[u8] = b"Halo ClientHello v1";
const SERVER_DOMAIN: &[u8] = b"Halo ServerHello v1";
const DECISION_DOMAIN: &[u8] = b"Halo PairingDecision v1";
const COMMIT_DOMAIN: &[u8] = b"Halo PairingCommit v1";
const TRANSCRIPT_DOMAIN: &[u8] = b"Halo Pairing Transcript v1";
const CODE_SALT: &[u8] = b"Halo pairing short code v1";
const CODE_INFO: &[u8] = b"decimal";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlsChannelBinding([u8; 32]);

impl TlsChannelBinding {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PairingCode(u32);

impl PairingCode {
    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:06}", self.0)
    }
}

pub fn create_client_hello(
    identity: &DeviceIdentity,
    versions: ProtocolRange,
    capabilities: Capabilities,
    binding: TlsChannelBinding,
) -> Result<ClientHello, PairingCryptoError> {
    let mut message = ClientHello {
        versions,
        capabilities,
        nonce: random_nonce()?,
        identity_key: *identity.public_key().as_bytes(),
        signature: [0; 64],
    };
    let input = signature_input(CLIENT_DOMAIN, binding, &message.unsigned_payload());
    message.signature = *identity.sign(&input).as_bytes();
    Ok(message)
}

pub fn verify_client_hello(
    message: &ClientHello,
    binding: TlsChannelBinding,
) -> Result<IdentityPublicKey, PairingCryptoError> {
    let key = IdentityPublicKey::from_bytes(message.identity_key)?;
    let input = signature_input(CLIENT_DOMAIN, binding, &message.unsigned_payload());
    key.verify(&input, &IdentitySignature::from_bytes(message.signature))?;
    Ok(key)
}

pub fn create_server_hello(
    identity: &DeviceIdentity,
    versions: ProtocolRange,
    capabilities: Capabilities,
    client_frame: &[u8],
    client: &ClientHello,
    binding: TlsChannelBinding,
) -> Result<ServerHello, PairingCryptoError> {
    verify_client_hello(client, binding)?;
    let selected_version = versions
        .negotiate(client.versions)
        .ok_or(PairingCryptoError::IncompatibleVersion)?;
    let mut message = ServerHello {
        selected_version,
        versions,
        capabilities,
        nonce: random_nonce()?,
        identity_key: *identity.public_key().as_bytes(),
        client_hello_hash: client_hello_hash(client_frame),
        signature: [0; 64],
    };
    let input = signature_input(SERVER_DOMAIN, binding, &message.unsigned_payload());
    message.signature = *identity.sign(&input).as_bytes();
    Ok(message)
}

pub fn verify_server_hello(
    client_frame: &[u8],
    client: &ClientHello,
    server: &ServerHello,
    binding: TlsChannelBinding,
) -> Result<IdentityPublicKey, PairingCryptoError> {
    if server.client_hello_hash != client_hello_hash(client_frame) {
        return Err(PairingCryptoError::TranscriptMismatch);
    }
    if client.versions.negotiate(server.versions) != Some(server.selected_version) {
        return Err(PairingCryptoError::VersionDowngrade);
    }
    let key = IdentityPublicKey::from_bytes(server.identity_key)?;
    let input = signature_input(SERVER_DOMAIN, binding, &server.unsigned_payload());
    key.verify(&input, &IdentitySignature::from_bytes(server.signature))?;
    Ok(key)
}

#[must_use]
pub fn client_hello_hash(frame: &[u8]) -> [u8; TRANSCRIPT_HASH_LEN] {
    Sha256::digest(frame).into()
}

#[must_use]
pub fn transcript_hash(
    binding: TlsChannelBinding,
    client_frame: &[u8],
    server_frame: &[u8],
) -> [u8; TRANSCRIPT_HASH_LEN] {
    digest_components(&[
        TRANSCRIPT_DOMAIN,
        binding.as_bytes(),
        client_frame,
        server_frame,
    ])
}

pub fn pairing_code(
    transcript: &[u8; TRANSCRIPT_HASH_LEN],
) -> Result<PairingCode, PairingCryptoError> {
    let hkdf = Hkdf::<Sha256>::new(Some(CODE_SALT), transcript);
    let mut output = [0_u8; 64];
    hkdf.expand(CODE_INFO, &mut output)
        .map_err(|_| PairingCryptoError::CodeDerivation)?;
    const RANGE: u64 = 1_000_000;
    const LIMIT: u64 = ((u32::MAX as u64 + 1) / RANGE) * RANGE;
    for bytes in output.chunks_exact(4) {
        let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64;
        if value < LIMIT {
            return Ok(PairingCode((value % RANGE) as u32));
        }
    }
    Err(PairingCryptoError::CodeDerivation)
}

#[must_use]
pub fn create_decision(
    identity: &DeviceIdentity,
    transcript_hash: [u8; TRANSCRIPT_HASH_LEN],
    accepted: bool,
    binding: TlsChannelBinding,
) -> PairingDecision {
    let mut message = PairingDecision {
        transcript_hash,
        accepted,
        signature: [0; 64],
    };
    let input = signature_input(DECISION_DOMAIN, binding, &message.unsigned_payload());
    message.signature = *identity.sign(&input).as_bytes();
    message
}

pub fn verify_decision(
    key: &IdentityPublicKey,
    decision: &PairingDecision,
    expected_transcript: &[u8; TRANSCRIPT_HASH_LEN],
    binding: TlsChannelBinding,
) -> Result<(), PairingCryptoError> {
    if &decision.transcript_hash != expected_transcript {
        return Err(PairingCryptoError::TranscriptMismatch);
    }
    let input = signature_input(DECISION_DOMAIN, binding, &decision.unsigned_payload());
    key.verify(&input, &IdentitySignature::from_bytes(decision.signature))?;
    if !decision.accepted {
        return Err(PairingCryptoError::Rejected);
    }
    Ok(())
}

#[must_use]
pub fn create_commit(
    identity: &DeviceIdentity,
    transcript_hash: [u8; TRANSCRIPT_HASH_LEN],
    binding: TlsChannelBinding,
) -> PairingCommit {
    let mut message = PairingCommit {
        transcript_hash,
        signature: [0; 64],
    };
    let input = signature_input(COMMIT_DOMAIN, binding, &message.unsigned_payload());
    message.signature = *identity.sign(&input).as_bytes();
    message
}

pub fn verify_commit(
    key: &IdentityPublicKey,
    commit: &PairingCommit,
    expected_transcript: &[u8; TRANSCRIPT_HASH_LEN],
    binding: TlsChannelBinding,
) -> Result<(), PairingCryptoError> {
    if &commit.transcript_hash != expected_transcript {
        return Err(PairingCryptoError::TranscriptMismatch);
    }
    let input = signature_input(COMMIT_DOMAIN, binding, &commit.unsigned_payload());
    key.verify(&input, &IdentitySignature::from_bytes(commit.signature))?;
    Ok(())
}

fn random_nonce() -> Result<[u8; NONCE_LEN], PairingCryptoError> {
    let mut nonce = [0_u8; NONCE_LEN];
    fill(&mut nonce).map_err(|_| PairingCryptoError::Random)?;
    Ok(nonce)
}

fn signature_input(domain: &[u8], binding: TlsChannelBinding, payload: &[u8]) -> Vec<u8> {
    encode_components(&[domain, binding.as_bytes(), payload])
}

fn digest_components(components: &[&[u8]]) -> [u8; 32] {
    Sha256::digest(encode_components(components)).into()
}

fn encode_components(components: &[&[u8]]) -> Vec<u8> {
    let capacity = components.iter().map(|component| component.len() + 4).sum();
    let mut encoded = Vec::with_capacity(capacity);
    for component in components {
        let length = u32::try_from(component.len()).unwrap_or(u32::MAX);
        encoded.extend_from_slice(&length.to_be_bytes());
        encoded.extend_from_slice(component);
    }
    encoded
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PairingCryptoError {
    #[error("device identity operation failed: {0}")]
    Identity(#[from] IdentityError),
    #[error("operating-system random generator failed")]
    Random,
    #[error("protocol versions are incompatible")]
    IncompatibleVersion,
    #[error("server selected a downgraded or invalid version")]
    VersionDowngrade,
    #[error("pairing transcript does not match")]
    TranscriptMismatch,
    #[error("peer rejected pairing")]
    Rejected,
    #[error("short authentication code derivation failed")]
    CodeDerivation,
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo_protocol::PairingMessage;

    fn range(min: u16, max: u16) -> ProtocolRange {
        ProtocolRange::new(min, max).unwrap_or_else(|error| panic!("test version range: {error}"))
    }

    #[test]
    fn complete_pairing_authenticates_and_derives_the_same_code() {
        let client_identity =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("generate client: {error}"));
        let server_identity =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("generate server: {error}"));
        let binding = TlsChannelBinding::new([0x42; 32]);
        let client = create_client_hello(
            &client_identity,
            range(1, 3),
            Capabilities::from_bits(0b111),
            binding,
        )
        .unwrap_or_else(|error| panic!("client hello: {error}"));
        let client_frame = PairingMessage::ClientHello(client.clone()).encode();
        let server = create_server_hello(
            &server_identity,
            range(2, 4),
            Capabilities::from_bits(0b110),
            &client_frame,
            &client,
            binding,
        )
        .unwrap_or_else(|error| panic!("server hello: {error}"));
        let server_frame = PairingMessage::ServerHello(server.clone()).encode();
        let server_key = verify_server_hello(&client_frame, &client, &server, binding)
            .unwrap_or_else(|error| panic!("verify server: {error}"));
        assert_eq!(server.selected_version, 3);
        assert_eq!(
            client.capabilities.intersection(server.capabilities).bits(),
            0b110
        );

        let transcript = transcript_hash(binding, &client_frame, &server_frame);
        let code = pairing_code(&transcript).unwrap_or_else(|error| panic!("derive code: {error}"));
        assert_eq!(code.to_string().len(), 6);
        let decision = create_decision(&server_identity, transcript, true, binding);
        verify_decision(&server_key, &decision, &transcript, binding)
            .unwrap_or_else(|error| panic!("verify decision: {error}"));
        let client_key = verify_client_hello(&client, binding)
            .unwrap_or_else(|error| panic!("verify client: {error}"));
        let commit = create_commit(&client_identity, transcript, binding);
        verify_commit(&client_key, &commit, &transcript, binding)
            .unwrap_or_else(|error| panic!("verify commit: {error}"));
    }

    #[test]
    fn rejects_tampering_replay_downgrade_and_code_mismatch_inputs() {
        let client_identity =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("generate client: {error}"));
        let server_identity =
            DeviceIdentity::generate().unwrap_or_else(|error| panic!("generate server: {error}"));
        let binding = TlsChannelBinding::new([1; 32]);
        let other_binding = TlsChannelBinding::new([2; 32]);
        let client = create_client_hello(
            &client_identity,
            range(1, 2),
            Capabilities::default(),
            binding,
        )
        .unwrap_or_else(|error| panic!("client hello: {error}"));
        assert!(verify_client_hello(&client, other_binding).is_err());
        let mut tampered = client.clone();
        tampered.capabilities = Capabilities::from_bits(1);
        assert!(verify_client_hello(&tampered, binding).is_err());
        let mut forged_identity = client.clone();
        forged_identity.identity_key = *server_identity.public_key().as_bytes();
        assert!(verify_client_hello(&forged_identity, binding).is_err());

        let client_frame = PairingMessage::ClientHello(client.clone()).encode();
        assert_eq!(
            create_server_hello(
                &server_identity,
                range(3, 4),
                Capabilities::default(),
                &client_frame,
                &client,
                binding,
            ),
            Err(PairingCryptoError::IncompatibleVersion)
        );
        let mut server = create_server_hello(
            &server_identity,
            range(1, 2),
            Capabilities::default(),
            &client_frame,
            &client,
            binding,
        )
        .unwrap_or_else(|error| panic!("server hello: {error}"));
        server.selected_version = 1;
        assert_eq!(
            verify_server_hello(&client_frame, &client, &server, binding),
            Err(PairingCryptoError::VersionDowngrade)
        );

        let transcript = [7; 32];
        let decision = create_decision(&server_identity, transcript, true, binding);
        assert_eq!(
            verify_decision(&server_identity.public_key(), &decision, &[8; 32], binding),
            Err(PairingCryptoError::TranscriptMismatch)
        );
    }

    #[test]
    fn tls_binding_changes_the_authenticated_transcript() {
        let first = transcript_hash(TlsChannelBinding::new([1; 32]), b"a", b"b");
        let second = transcript_hash(TlsChannelBinding::new([2; 32]), b"a", b"b");
        assert_ne!(first, second);
        assert_eq!(pairing_code(&first), pairing_code(&first));
    }

    #[test]
    fn short_code_golden_vector_is_stable() {
        let code = pairing_code(&[0x5a; TRANSCRIPT_HASH_LEN])
            .unwrap_or_else(|error| panic!("derive golden code: {error}"));
        assert_eq!(code.value(), 198_987);
    }
}
