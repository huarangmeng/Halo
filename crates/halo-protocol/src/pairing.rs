use thiserror::Error;

pub const WIRE_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 12;
pub const MAX_FRAME_LEN: usize = 4096;
pub const NONCE_LEN: usize = 32;
pub const IDENTITY_KEY_LEN: usize = 65;
pub const SIGNATURE_LEN: usize = 64;
pub const TRANSCRIPT_HASH_LEN: usize = 32;

const MAGIC: &[u8; 4] = b"HALO";
const CLIENT_HELLO_KIND: u8 = 1;
const SERVER_HELLO_KIND: u8 = 2;
const PAIRING_DECISION_KIND: u8 = 3;
const PAIRING_COMMIT_KIND: u8 = 4;
const CLIENT_HELLO_LEN: usize = 2 + 2 + 8 + NONCE_LEN + IDENTITY_KEY_LEN + SIGNATURE_LEN;
const SERVER_HELLO_LEN: usize =
    2 + 2 + 2 + 8 + NONCE_LEN + IDENTITY_KEY_LEN + TRANSCRIPT_HASH_LEN + SIGNATURE_LEN;
const PAIRING_DECISION_LEN: usize = TRANSCRIPT_HASH_LEN + 1 + SIGNATURE_LEN;
const PAIRING_COMMIT_LEN: usize = TRANSCRIPT_HASH_LEN + SIGNATURE_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolRange {
    min: u16,
    max: u16,
}

impl ProtocolRange {
    pub fn new(min: u16, max: u16) -> Result<Self, ProtocolError> {
        if min == 0 || max == 0 || min > max {
            return Err(ProtocolError::InvalidVersionRange { min, max });
        }
        Ok(Self { min, max })
    }

    #[must_use]
    pub const fn min(self) -> u16 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> u16 {
        self.max
    }

    #[must_use]
    pub fn negotiate(self, remote: Self) -> Option<u16> {
        let min = self.min.max(remote.min);
        let max = self.max.min(remote.max);
        (min <= max).then_some(max)
    }

    #[must_use]
    pub const fn contains(self, version: u16) -> bool {
        version >= self.min && version <= self.max
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Capabilities(u64);

impl Capabilities {
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn intersection(self, remote: Self) -> Self {
        Self(self.0 & remote.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientHello {
    pub versions: ProtocolRange,
    pub capabilities: Capabilities,
    pub nonce: [u8; NONCE_LEN],
    pub identity_key: [u8; IDENTITY_KEY_LEN],
    pub signature: [u8; SIGNATURE_LEN],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerHello {
    pub selected_version: u16,
    pub versions: ProtocolRange,
    pub capabilities: Capabilities,
    pub nonce: [u8; NONCE_LEN],
    pub identity_key: [u8; IDENTITY_KEY_LEN],
    pub client_hello_hash: [u8; TRANSCRIPT_HASH_LEN],
    pub signature: [u8; SIGNATURE_LEN],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingDecision {
    pub transcript_hash: [u8; TRANSCRIPT_HASH_LEN],
    pub accepted: bool,
    pub signature: [u8; SIGNATURE_LEN],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingCommit {
    pub transcript_hash: [u8; TRANSCRIPT_HASH_LEN],
    pub signature: [u8; SIGNATURE_LEN],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingMessage {
    ClientHello(ClientHello),
    ServerHello(ServerHello),
    Decision(PairingDecision),
    Commit(PairingCommit),
}

impl PairingMessage {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Self::ClientHello(message) => (CLIENT_HELLO_KIND, message.payload()),
            Self::ServerHello(message) => (SERVER_HELLO_KIND, message.payload()),
            Self::Decision(message) => (PAIRING_DECISION_KIND, message.payload()),
            Self::Commit(message) => (PAIRING_COMMIT_KIND, message.payload()),
        };
        let payload_len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
        frame.extend_from_slice(MAGIC);
        frame.extend_from_slice(&WIRE_VERSION.to_be_bytes());
        frame.push(kind);
        frame.push(0);
        frame.extend_from_slice(&payload_len.to_be_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    pub fn decode(frame: &[u8]) -> Result<Self, ProtocolError> {
        if frame.len() < HEADER_LEN {
            return Err(ProtocolError::TruncatedHeader(frame.len()));
        }
        if frame.len() > MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(frame.len()));
        }
        if &frame[..4] != MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }
        let version = u16::from_be_bytes([frame[4], frame[5]]);
        if version != WIRE_VERSION {
            return Err(ProtocolError::UnsupportedWireVersion(version));
        }
        let kind = frame[6];
        if frame[7] != 0 {
            return Err(ProtocolError::ReservedFlags(frame[7]));
        }
        let declared = u32::from_be_bytes([frame[8], frame[9], frame[10], frame[11]]) as usize;
        if declared > MAX_FRAME_LEN - HEADER_LEN {
            return Err(ProtocolError::FrameTooLarge(HEADER_LEN + declared));
        }
        let actual = frame.len() - HEADER_LEN;
        if declared != actual {
            return Err(ProtocolError::PayloadLength { declared, actual });
        }
        let payload = &frame[HEADER_LEN..];
        match kind {
            CLIENT_HELLO_KIND => ClientHello::decode(payload).map(Self::ClientHello),
            SERVER_HELLO_KIND => ServerHello::decode(payload).map(Self::ServerHello),
            PAIRING_DECISION_KIND => PairingDecision::decode(payload).map(Self::Decision),
            PAIRING_COMMIT_KIND => PairingCommit::decode(payload).map(Self::Commit),
            _ => Err(ProtocolError::UnknownMessageKind(kind)),
        }
    }
}

impl ClientHello {
    #[must_use]
    pub fn unsigned_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CLIENT_HELLO_LEN - SIGNATURE_LEN);
        bytes.extend_from_slice(&self.versions.min.to_be_bytes());
        bytes.extend_from_slice(&self.versions.max.to_be_bytes());
        bytes.extend_from_slice(&self.capabilities.0.to_be_bytes());
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.identity_key);
        bytes
    }

    fn payload(&self) -> Vec<u8> {
        let mut bytes = self.unsigned_payload();
        bytes.extend_from_slice(&self.signature);
        bytes
    }

    fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        exact_len(payload, CLIENT_HELLO_LEN)?;
        let versions = ProtocolRange::new(read_u16(payload, 0), read_u16(payload, 2))?;
        Ok(Self {
            versions,
            capabilities: Capabilities::from_bits(read_u64(payload, 4)),
            nonce: read_array(payload, 12),
            identity_key: read_array(payload, 44),
            signature: read_array(payload, 109),
        })
    }
}

impl ServerHello {
    #[must_use]
    pub fn unsigned_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(SERVER_HELLO_LEN - SIGNATURE_LEN);
        bytes.extend_from_slice(&self.selected_version.to_be_bytes());
        bytes.extend_from_slice(&self.versions.min.to_be_bytes());
        bytes.extend_from_slice(&self.versions.max.to_be_bytes());
        bytes.extend_from_slice(&self.capabilities.0.to_be_bytes());
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.identity_key);
        bytes.extend_from_slice(&self.client_hello_hash);
        bytes
    }

    fn payload(&self) -> Vec<u8> {
        let mut bytes = self.unsigned_payload();
        bytes.extend_from_slice(&self.signature);
        bytes
    }

    fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        exact_len(payload, SERVER_HELLO_LEN)?;
        let selected_version = read_u16(payload, 0);
        if selected_version == 0 {
            return Err(ProtocolError::InvalidSelectedVersion);
        }
        let versions = ProtocolRange::new(read_u16(payload, 2), read_u16(payload, 4))?;
        Ok(Self {
            selected_version,
            versions,
            capabilities: Capabilities::from_bits(read_u64(payload, 6)),
            nonce: read_array(payload, 14),
            identity_key: read_array(payload, 46),
            client_hello_hash: read_array(payload, 111),
            signature: read_array(payload, 143),
        })
    }
}

impl PairingDecision {
    #[must_use]
    pub fn unsigned_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PAIRING_DECISION_LEN - SIGNATURE_LEN);
        bytes.extend_from_slice(&self.transcript_hash);
        bytes.push(u8::from(self.accepted));
        bytes
    }

    fn payload(&self) -> Vec<u8> {
        let mut bytes = self.unsigned_payload();
        bytes.extend_from_slice(&self.signature);
        bytes
    }

    fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        exact_len(payload, PAIRING_DECISION_LEN)?;
        let accepted = match payload[TRANSCRIPT_HASH_LEN] {
            0 => false,
            1 => true,
            value => return Err(ProtocolError::InvalidBoolean(value)),
        };
        Ok(Self {
            transcript_hash: read_array(payload, 0),
            accepted,
            signature: read_array(payload, TRANSCRIPT_HASH_LEN + 1),
        })
    }
}

impl PairingCommit {
    #[must_use]
    pub fn unsigned_payload(&self) -> Vec<u8> {
        self.transcript_hash.to_vec()
    }

    fn payload(&self) -> Vec<u8> {
        let mut bytes = self.unsigned_payload();
        bytes.extend_from_slice(&self.signature);
        bytes
    }

    fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        exact_len(payload, PAIRING_COMMIT_LEN)?;
        Ok(Self {
            transcript_hash: read_array(payload, 0),
            signature: read_array(payload, TRANSCRIPT_HASH_LEN),
        })
    }
}

fn exact_len(payload: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if payload.len() != expected {
        return Err(ProtocolError::MessageLength {
            expected,
            actual: payload.len(),
        });
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut output = [0_u8; 8];
    output.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_be_bytes(output)
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    let mut output = [0_u8; N];
    output.copy_from_slice(&bytes[offset..offset + N]);
    output
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProtocolError {
    #[error("pairing frame header is truncated: {0} bytes")]
    TruncatedHeader(usize),
    #[error("pairing frame exceeds the {MAX_FRAME_LEN}-byte limit: {0} bytes")]
    FrameTooLarge(usize),
    #[error("pairing frame magic does not match")]
    InvalidMagic,
    #[error("unsupported pairing wire version {0}")]
    UnsupportedWireVersion(u16),
    #[error("unknown pairing message kind {0}")]
    UnknownMessageKind(u8),
    #[error("pairing frame uses reserved flags 0x{0:02x}")]
    ReservedFlags(u8),
    #[error("pairing payload length is {actual}, frame declared {declared}")]
    PayloadLength { declared: usize, actual: usize },
    #[error("pairing message length is {actual}, expected {expected}")]
    MessageLength { expected: usize, actual: usize },
    #[error("invalid protocol version range {min}..={max}")]
    InvalidVersionRange { min: u16, max: u16 },
    #[error("selected protocol version must be non-zero")]
    InvalidSelectedVersion,
    #[error("invalid encoded boolean {0}")]
    InvalidBoolean(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> ClientHello {
        ClientHello {
            versions: ProtocolRange::new(1, 3)
                .unwrap_or_else(|error| panic!("test version range: {error}")),
            capabilities: Capabilities::from_bits(0x0102_0304_0506_0708),
            nonce: [0x11; NONCE_LEN],
            identity_key: [0x22; IDENTITY_KEY_LEN],
            signature: [0x33; SIGNATURE_LEN],
        }
    }

    #[test]
    fn client_hello_golden_vector_is_stable() {
        let message = PairingMessage::ClientHello(client());
        let encoded = message.encode();
        assert_eq!(&encoded[..12], b"HALO\x00\x01\x01\x00\x00\x00\x00\xad");
        assert_eq!(&encoded[12..16], &[0, 1, 0, 3]);
        assert_eq!(&encoded[16..24], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(PairingMessage::decode(&encoded), Ok(message));
    }

    #[test]
    fn all_message_types_round_trip() {
        let messages = [
            PairingMessage::ClientHello(client()),
            PairingMessage::ServerHello(ServerHello {
                selected_version: 2,
                versions: ProtocolRange::new(2, 4)
                    .unwrap_or_else(|error| panic!("test version range: {error}")),
                capabilities: Capabilities::from_bits(7),
                nonce: [4; NONCE_LEN],
                identity_key: [5; IDENTITY_KEY_LEN],
                client_hello_hash: [6; TRANSCRIPT_HASH_LEN],
                signature: [7; SIGNATURE_LEN],
            }),
            PairingMessage::Decision(PairingDecision {
                transcript_hash: [8; TRANSCRIPT_HASH_LEN],
                accepted: true,
                signature: [9; SIGNATURE_LEN],
            }),
            PairingMessage::Commit(PairingCommit {
                transcript_hash: [10; TRANSCRIPT_HASH_LEN],
                signature: [11; SIGNATURE_LEN],
            }),
        ];
        for message in messages {
            let encoded = message.encode();
            assert_eq!(PairingMessage::decode(&encoded), Ok(message));
        }
    }

    #[test]
    fn rejects_every_truncation_without_panicking() {
        let encoded = PairingMessage::ClientHello(client()).encode();
        for length in 0..encoded.len() {
            assert!(PairingMessage::decode(&encoded[..length]).is_err());
        }
    }

    #[test]
    fn rejects_oversized_unknown_reserved_and_malformed_frames() {
        let mut oversized = vec![0_u8; MAX_FRAME_LEN + 1];
        oversized[..4].copy_from_slice(MAGIC);
        assert_eq!(
            PairingMessage::decode(&oversized),
            Err(ProtocolError::FrameTooLarge(MAX_FRAME_LEN + 1))
        );

        let mut frame = PairingMessage::ClientHello(client()).encode();
        frame[6] = 99;
        assert_eq!(
            PairingMessage::decode(&frame),
            Err(ProtocolError::UnknownMessageKind(99))
        );
        frame[6] = CLIENT_HELLO_KIND;
        frame[7] = 1;
        assert_eq!(
            PairingMessage::decode(&frame),
            Err(ProtocolError::ReservedFlags(1))
        );
    }

    #[test]
    fn negotiates_highest_shared_version_and_rejects_disjoint_ranges() {
        let local =
            ProtocolRange::new(1, 4).unwrap_or_else(|error| panic!("test version range: {error}"));
        let remote =
            ProtocolRange::new(2, 3).unwrap_or_else(|error| panic!("test version range: {error}"));
        assert_eq!(local.negotiate(remote), Some(3));
        let disjoint =
            ProtocolRange::new(5, 6).unwrap_or_else(|error| panic!("test version range: {error}"));
        assert_eq!(local.negotiate(disjoint), None);
    }
}
