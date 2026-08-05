use thiserror::Error;

use crate::{HEADER_LEN, MAX_FRAME_LEN, WIRE_VERSION};

pub const TRANSFER_ID_LEN: usize = 16;
pub const CONTENT_DIGEST_LEN: usize = 32;
pub const MAX_FILE_NAME_LEN: usize = 255;
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024 * 1024;
pub const DEFAULT_CHUNK_SIZE: u32 = 64 * 1024;
pub const MAX_CHUNK_SIZE: u32 = 256 * 1024;
pub const DATA_RECORD_HEADER_LEN: usize = 4 + TRANSFER_ID_LEN + 4 + 4 + CONTENT_DIGEST_LEN;
pub const MAX_DATA_RECORD_LEN: usize = DATA_RECORD_HEADER_LEN + MAX_CHUNK_SIZE as usize;

const MAGIC: &[u8; 4] = b"HALO";
const OFFER_KIND: u8 = 16;
const DECISION_KIND: u8 = 17;
const COMPLETE_KIND: u8 = 18;
const CANCEL_KIND: u8 = 19;
const OFFER_FIXED_LEN: usize = TRANSFER_ID_LEN + 8 + 4 + CONTENT_DIGEST_LEN + 2;
const DECISION_LEN: usize = TRANSFER_ID_LEN + 1;
const COMPLETE_LEN: usize = TRANSFER_ID_LEN + CONTENT_DIGEST_LEN;
const CANCEL_LEN: usize = TRANSFER_ID_LEN + 1;
const DATA_MAGIC: &[u8; 4] = b"HDF1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferOffer {
    pub transfer_id: [u8; TRANSFER_ID_LEN],
    pub file_size: u64,
    pub chunk_size: u32,
    pub file_digest: [u8; CONTENT_DIGEST_LEN],
    pub file_name: String,
}

impl TransferOffer {
    pub fn new(
        transfer_id: [u8; TRANSFER_ID_LEN],
        file_size: u64,
        chunk_size: u32,
        file_digest: [u8; CONTENT_DIGEST_LEN],
        file_name: String,
    ) -> Result<Self, TransferProtocolError> {
        validate_offer(file_size, chunk_size, &file_name)?;
        Ok(Self {
            transfer_id,
            file_size,
            chunk_size,
            file_digest,
            file_name,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferDecision {
    pub transfer_id: [u8; TRANSFER_ID_LEN],
    pub accepted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferComplete {
    pub transfer_id: [u8; TRANSFER_ID_LEN],
    pub file_digest: [u8; CONTENT_DIGEST_LEN],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TransferCancelReason {
    User = 1,
    Policy = 2,
    Integrity = 3,
    Storage = 4,
    Protocol = 5,
}

impl TryFrom<u8> for TransferCancelReason {
    type Error = TransferProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::User),
            2 => Ok(Self::Policy),
            3 => Ok(Self::Integrity),
            4 => Ok(Self::Storage),
            5 => Ok(Self::Protocol),
            _ => Err(TransferProtocolError::InvalidCancelReason(value)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferCancel {
    pub transfer_id: [u8; TRANSFER_ID_LEN],
    pub reason: TransferCancelReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferChunk {
    pub transfer_id: [u8; TRANSFER_ID_LEN],
    pub chunk_index: u32,
    pub chunk_digest: [u8; CONTENT_DIGEST_LEN],
    pub payload: Vec<u8>,
}

impl TransferChunk {
    pub fn new(
        transfer_id: [u8; TRANSFER_ID_LEN],
        chunk_index: u32,
        chunk_digest: [u8; CONTENT_DIGEST_LEN],
        payload: Vec<u8>,
    ) -> Result<Self, TransferProtocolError> {
        validate_chunk_payload(payload.len())?;
        Ok(Self {
            transfer_id,
            chunk_index,
            chunk_digest,
            payload,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, TransferProtocolError> {
        validate_chunk_payload(self.payload.len())?;
        let payload_length = u32::try_from(self.payload.len())
            .map_err(|_| TransferProtocolError::InvalidChunkPayloadLength(self.payload.len()))?;
        let mut record = Vec::with_capacity(DATA_RECORD_HEADER_LEN + self.payload.len());
        record.extend_from_slice(DATA_MAGIC);
        record.extend_from_slice(&self.transfer_id);
        record.extend_from_slice(&self.chunk_index.to_be_bytes());
        record.extend_from_slice(&payload_length.to_be_bytes());
        record.extend_from_slice(&self.chunk_digest);
        record.extend_from_slice(&self.payload);
        Ok(record)
    }

    pub fn decode(record: &[u8]) -> Result<Self, TransferProtocolError> {
        if record.len() < DATA_RECORD_HEADER_LEN {
            return Err(TransferProtocolError::TruncatedDataHeader(record.len()));
        }
        if record.len() > MAX_DATA_RECORD_LEN {
            return Err(TransferProtocolError::DataRecordTooLarge(record.len()));
        }
        if &record[..4] != DATA_MAGIC {
            return Err(TransferProtocolError::InvalidDataMagic);
        }
        let payload_length = read_u32(record, 4 + TRANSFER_ID_LEN + 4) as usize;
        validate_chunk_payload(payload_length)?;
        let expected = DATA_RECORD_HEADER_LEN
            .checked_add(payload_length)
            .ok_or(TransferProtocolError::DataRecordTooLarge(usize::MAX))?;
        if record.len() != expected {
            return Err(TransferProtocolError::DataRecordLength {
                declared: expected,
                actual: record.len(),
            });
        }
        Self::new(
            read_array(record, 4),
            read_u32(record, 4 + TRANSFER_ID_LEN),
            read_array(record, 4 + TRANSFER_ID_LEN + 4 + 4),
            record[DATA_RECORD_HEADER_LEN..].to_vec(),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransferMessage {
    Offer(TransferOffer),
    Decision(TransferDecision),
    Complete(TransferComplete),
    Cancel(TransferCancel),
}

impl TransferMessage {
    pub fn encode(&self) -> Result<Vec<u8>, TransferProtocolError> {
        let (kind, payload) = match self {
            Self::Offer(offer) => {
                validate_offer(offer.file_size, offer.chunk_size, &offer.file_name)?;
                let name = offer.file_name.as_bytes();
                let name_len = u16::try_from(name.len())
                    .map_err(|_| TransferProtocolError::InvalidFileNameLength(name.len()))?;
                let mut payload = Vec::with_capacity(OFFER_FIXED_LEN + name.len());
                payload.extend_from_slice(&offer.transfer_id);
                payload.extend_from_slice(&offer.file_size.to_be_bytes());
                payload.extend_from_slice(&offer.chunk_size.to_be_bytes());
                payload.extend_from_slice(&offer.file_digest);
                payload.extend_from_slice(&name_len.to_be_bytes());
                payload.extend_from_slice(name);
                (OFFER_KIND, payload)
            }
            Self::Decision(decision) => {
                let mut payload = Vec::with_capacity(DECISION_LEN);
                payload.extend_from_slice(&decision.transfer_id);
                payload.push(u8::from(decision.accepted));
                (DECISION_KIND, payload)
            }
            Self::Complete(complete) => {
                let mut payload = Vec::with_capacity(COMPLETE_LEN);
                payload.extend_from_slice(&complete.transfer_id);
                payload.extend_from_slice(&complete.file_digest);
                (COMPLETE_KIND, payload)
            }
            Self::Cancel(cancel) => {
                let mut payload = Vec::with_capacity(CANCEL_LEN);
                payload.extend_from_slice(&cancel.transfer_id);
                payload.push(cancel.reason as u8);
                (CANCEL_KIND, payload)
            }
        };
        let payload_len = u32::try_from(payload.len())
            .map_err(|_| TransferProtocolError::FrameTooLarge(payload.len()))?;
        let frame_len = HEADER_LEN
            .checked_add(payload.len())
            .ok_or(TransferProtocolError::FrameTooLarge(usize::MAX))?;
        if frame_len > MAX_FRAME_LEN {
            return Err(TransferProtocolError::FrameTooLarge(frame_len));
        }
        let mut frame = Vec::with_capacity(frame_len);
        frame.extend_from_slice(MAGIC);
        frame.extend_from_slice(&WIRE_VERSION.to_be_bytes());
        frame.push(kind);
        frame.push(0);
        frame.extend_from_slice(&payload_len.to_be_bytes());
        frame.extend_from_slice(&payload);
        Ok(frame)
    }

    pub fn decode(frame: &[u8]) -> Result<Self, TransferProtocolError> {
        if frame.len() < HEADER_LEN {
            return Err(TransferProtocolError::TruncatedHeader(frame.len()));
        }
        if frame.len() > MAX_FRAME_LEN {
            return Err(TransferProtocolError::FrameTooLarge(frame.len()));
        }
        if &frame[..4] != MAGIC {
            return Err(TransferProtocolError::InvalidMagic);
        }
        let version = read_u16(frame, 4);
        if version != WIRE_VERSION {
            return Err(TransferProtocolError::UnsupportedWireVersion(version));
        }
        let kind = frame[6];
        if frame[7] != 0 {
            return Err(TransferProtocolError::ReservedFlags(frame[7]));
        }
        let declared = read_u32(frame, 8) as usize;
        let actual = frame.len() - HEADER_LEN;
        if declared != actual {
            return Err(TransferProtocolError::PayloadLength { declared, actual });
        }
        let payload = &frame[HEADER_LEN..];
        match kind {
            OFFER_KIND => decode_offer(payload).map(Self::Offer),
            DECISION_KIND => decode_decision(payload).map(Self::Decision),
            COMPLETE_KIND => decode_complete(payload).map(Self::Complete),
            CANCEL_KIND => decode_cancel(payload).map(Self::Cancel),
            _ => Err(TransferProtocolError::UnknownMessageKind(kind)),
        }
    }
}

fn decode_offer(payload: &[u8]) -> Result<TransferOffer, TransferProtocolError> {
    if payload.len() < OFFER_FIXED_LEN {
        return Err(TransferProtocolError::MessageLength {
            minimum: OFFER_FIXED_LEN,
            actual: payload.len(),
        });
    }
    let file_size = read_u64(payload, TRANSFER_ID_LEN);
    let chunk_size = read_u32(payload, TRANSFER_ID_LEN + 8);
    let name_len = read_u16(payload, TRANSFER_ID_LEN + 8 + 4 + CONTENT_DIGEST_LEN) as usize;
    let expected = OFFER_FIXED_LEN
        .checked_add(name_len)
        .ok_or(TransferProtocolError::FrameTooLarge(usize::MAX))?;
    if payload.len() != expected {
        return Err(TransferProtocolError::PayloadLength {
            declared: expected,
            actual: payload.len(),
        });
    }
    let name = std::str::from_utf8(&payload[OFFER_FIXED_LEN..])
        .map_err(|_| TransferProtocolError::InvalidUtf8)?
        .to_owned();
    TransferOffer::new(
        read_array(payload, 0),
        file_size,
        chunk_size,
        read_array(payload, TRANSFER_ID_LEN + 8 + 4),
        name,
    )
}

fn decode_decision(payload: &[u8]) -> Result<TransferDecision, TransferProtocolError> {
    exact_len(payload, DECISION_LEN)?;
    let accepted = match payload[TRANSFER_ID_LEN] {
        0 => false,
        1 => true,
        value => return Err(TransferProtocolError::InvalidBoolean(value)),
    };
    Ok(TransferDecision {
        transfer_id: read_array(payload, 0),
        accepted,
    })
}

fn decode_complete(payload: &[u8]) -> Result<TransferComplete, TransferProtocolError> {
    exact_len(payload, COMPLETE_LEN)?;
    Ok(TransferComplete {
        transfer_id: read_array(payload, 0),
        file_digest: read_array(payload, TRANSFER_ID_LEN),
    })
}

fn decode_cancel(payload: &[u8]) -> Result<TransferCancel, TransferProtocolError> {
    exact_len(payload, CANCEL_LEN)?;
    Ok(TransferCancel {
        transfer_id: read_array(payload, 0),
        reason: TransferCancelReason::try_from(payload[TRANSFER_ID_LEN])?,
    })
}

fn validate_offer(
    file_size: u64,
    chunk_size: u32,
    file_name: &str,
) -> Result<(), TransferProtocolError> {
    if file_size > MAX_FILE_SIZE {
        return Err(TransferProtocolError::FileTooLarge(file_size));
    }
    if chunk_size == 0 || chunk_size > MAX_CHUNK_SIZE {
        return Err(TransferProtocolError::InvalidChunkSize(chunk_size));
    }
    let name_len = file_name.len();
    if name_len == 0 || name_len > MAX_FILE_NAME_LEN {
        return Err(TransferProtocolError::InvalidFileNameLength(name_len));
    }
    Ok(())
}

fn validate_chunk_payload(length: usize) -> Result<(), TransferProtocolError> {
    if length == 0 || length > MAX_CHUNK_SIZE as usize {
        return Err(TransferProtocolError::InvalidChunkPayloadLength(length));
    }
    Ok(())
}

fn exact_len(payload: &[u8], expected: usize) -> Result<(), TransferProtocolError> {
    if payload.len() != expected {
        return Err(TransferProtocolError::MessageLength {
            minimum: expected,
            actual: payload.len(),
        });
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut output = [0_u8; 4];
    output.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_be_bytes(output)
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
pub enum TransferProtocolError {
    #[error("transfer frame header is truncated: {0} bytes")]
    TruncatedHeader(usize),
    #[error("transfer frame exceeds the configured limit: {0} bytes")]
    FrameTooLarge(usize),
    #[error("transfer frame magic does not match")]
    InvalidMagic,
    #[error("unsupported transfer wire version {0}")]
    UnsupportedWireVersion(u16),
    #[error("unknown transfer message kind {0}")]
    UnknownMessageKind(u8),
    #[error("transfer frame uses reserved flags 0x{0:02x}")]
    ReservedFlags(u8),
    #[error("transfer payload length is {actual}, expected {declared}")]
    PayloadLength { declared: usize, actual: usize },
    #[error("transfer message length is {actual}, expected at least {minimum}")]
    MessageLength { minimum: usize, actual: usize },
    #[error("transfer boolean has invalid value {0}")]
    InvalidBoolean(u8),
    #[error("transfer cancellation reason has invalid value {0}")]
    InvalidCancelReason(u8),
    #[error("transfer filename is not valid UTF-8")]
    InvalidUtf8,
    #[error("transfer filename length is invalid: {0}")]
    InvalidFileNameLength(usize),
    #[error("transfer file exceeds the v1 size limit: {0}")]
    FileTooLarge(u64),
    #[error("transfer chunk size is invalid: {0}")]
    InvalidChunkSize(u32),
    #[error("transfer data header is truncated: {0} bytes")]
    TruncatedDataHeader(usize),
    #[error("transfer data record exceeds the configured limit: {0} bytes")]
    DataRecordTooLarge(usize),
    #[error("transfer data record magic does not match")]
    InvalidDataMagic,
    #[error("transfer chunk payload length is invalid: {0}")]
    InvalidChunkPayloadLength(usize),
    #[error("transfer data record length is {actual}, expected {declared}")]
    DataRecordLength { declared: usize, actual: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> TransferOffer {
        TransferOffer::new(
            [0x11; TRANSFER_ID_LEN],
            0x0000_0001_0203_0405,
            DEFAULT_CHUNK_SIZE,
            [0x22; CONTENT_DIGEST_LEN],
            "example.txt".to_owned(),
        )
        .unwrap_or_else(|error| panic!("test offer: {error}"))
    }

    #[test]
    fn offer_golden_prefix_and_all_messages_round_trip() {
        let messages = [
            TransferMessage::Offer(offer()),
            TransferMessage::Decision(TransferDecision {
                transfer_id: [1; TRANSFER_ID_LEN],
                accepted: true,
            }),
            TransferMessage::Complete(TransferComplete {
                transfer_id: [2; TRANSFER_ID_LEN],
                file_digest: [3; CONTENT_DIGEST_LEN],
            }),
            TransferMessage::Cancel(TransferCancel {
                transfer_id: [4; TRANSFER_ID_LEN],
                reason: TransferCancelReason::Storage,
            }),
        ];
        let offer_frame = messages[0]
            .encode()
            .unwrap_or_else(|error| panic!("encode: {error}"));
        assert_eq!(&offer_frame[..8], b"HALO\x00\x01\x10\x00");
        for message in messages {
            let encoded = message
                .encode()
                .unwrap_or_else(|error| panic!("encode: {error}"));
            assert_eq!(TransferMessage::decode(&encoded), Ok(message));
        }
    }

    #[test]
    fn rejects_all_offer_truncations_and_malformed_fields() {
        let encoded = TransferMessage::Offer(offer())
            .encode()
            .unwrap_or_else(|error| panic!("encode: {error}"));
        for length in 0..encoded.len() {
            assert!(TransferMessage::decode(&encoded[..length]).is_err());
        }

        let mut invalid_boolean = TransferMessage::Decision(TransferDecision {
            transfer_id: [1; TRANSFER_ID_LEN],
            accepted: true,
        })
        .encode()
        .unwrap_or_else(|error| panic!("encode: {error}"));
        invalid_boolean[HEADER_LEN + TRANSFER_ID_LEN] = 2;
        assert_eq!(
            TransferMessage::decode(&invalid_boolean),
            Err(TransferProtocolError::InvalidBoolean(2))
        );
    }

    #[test]
    fn offer_limits_are_enforced_at_construction_and_encoding() {
        assert!(
            TransferOffer::new(
                [0; TRANSFER_ID_LEN],
                MAX_FILE_SIZE + 1,
                DEFAULT_CHUNK_SIZE,
                [0; CONTENT_DIGEST_LEN],
                "file".to_owned(),
            )
            .is_err()
        );
        assert!(
            TransferOffer::new(
                [0; TRANSFER_ID_LEN],
                0,
                MAX_CHUNK_SIZE + 1,
                [0; CONTENT_DIGEST_LEN],
                "file".to_owned(),
            )
            .is_err()
        );
    }

    #[test]
    fn data_chunk_round_trips_with_the_documented_header() {
        let chunk = TransferChunk::new(
            [0x31; TRANSFER_ID_LEN],
            7,
            [0x42; CONTENT_DIGEST_LEN],
            vec![0x53; 32],
        )
        .unwrap_or_else(|error| panic!("chunk: {error}"));
        let encoded = chunk
            .encode()
            .unwrap_or_else(|error| panic!("encode chunk: {error}"));
        assert_eq!(encoded.len(), DATA_RECORD_HEADER_LEN + 32);
        assert_eq!(&encoded[..4], b"HDF1");
        assert_eq!(&encoded[24..28], &[0, 0, 0, 32]);
        assert_eq!(TransferChunk::decode(&encoded), Ok(chunk));
    }

    #[test]
    fn data_chunk_rejects_truncation_zero_oversize_and_trailing_bytes() {
        let chunk =
            TransferChunk::new([1; TRANSFER_ID_LEN], 0, [2; CONTENT_DIGEST_LEN], vec![3; 4])
                .unwrap_or_else(|error| panic!("chunk: {error}"));
        let encoded = chunk
            .encode()
            .unwrap_or_else(|error| panic!("encode chunk: {error}"));
        for length in 0..encoded.len() {
            assert!(TransferChunk::decode(&encoded[..length]).is_err());
        }
        let mut zero = encoded.clone();
        zero[24..28].copy_from_slice(&0_u32.to_be_bytes());
        assert_eq!(
            TransferChunk::decode(&zero),
            Err(TransferProtocolError::InvalidChunkPayloadLength(0))
        );
        let mut oversized = encoded.clone();
        oversized[24..28].copy_from_slice(&(MAX_CHUNK_SIZE + 1).to_be_bytes());
        assert_eq!(
            TransferChunk::decode(&oversized),
            Err(TransferProtocolError::InvalidChunkPayloadLength(
                MAX_CHUNK_SIZE as usize + 1
            ))
        );
        let mut trailing = encoded;
        trailing.push(0);
        assert!(matches!(
            TransferChunk::decode(&trailing),
            Err(TransferProtocolError::DataRecordLength { .. })
        ));
        assert!(TransferChunk::new([0; 16], 0, [0; 32], Vec::new()).is_err());
    }
}
