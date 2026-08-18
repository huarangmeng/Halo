use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::HEADER_LEN;

pub const TRANSFER_WIRE_VERSION: u16 = 1;
pub const TRANSFER_ID_LEN: usize = 16;
pub const CONTENT_DIGEST_LEN: usize = 32;
pub const MAX_FILE_NAME_LEN: usize = 255;
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024 * 1024;
pub const DEFAULT_CHUNK_SIZE: u32 = 256 * 1024;
pub const MAX_CHUNK_SIZE: u32 = 256 * 1024;
pub const MAX_BATCH_FILES: usize = 8;
pub const BATCH_MANIFEST_DIGEST_LEN: usize = 32;
pub const DATA_RECORD_HEADER_LEN: usize =
    4 + crate::TRANSFER_ID_LEN + 2 + 2 + 4 + 4 + CONTENT_DIGEST_LEN;
pub const MAX_DATA_RECORD_LEN: usize = DATA_RECORD_HEADER_LEN + MAX_CHUNK_SIZE as usize;

const MAGIC: &[u8; 4] = b"HALO";
const DATA_MAGIC: &[u8; 4] = b"HDF1";
const OFFER_KIND: u8 = 32;
const DECISION_KIND: u8 = 33;
const COMPLETE_KIND: u8 = 34;
const CANCEL_KIND: u8 = 35;
const PAUSE_KIND: u8 = 36;
const OFFER_FIXED_LEN: usize = crate::TRANSFER_ID_LEN + 4 + 1 + 3 + 8 + BATCH_MANIFEST_DIGEST_LEN;
const FILE_FIXED_LEN: usize = 8 + CONTENT_DIGEST_LEN + 2;
const DECISION_FIXED_LEN: usize = crate::TRANSFER_ID_LEN + BATCH_MANIFEST_DIGEST_LEN + 1 + 1 + 2;
const RESUME_POSITION_LEN: usize = 2 + 4;
const COMPLETE_LEN: usize = crate::TRANSFER_ID_LEN + BATCH_MANIFEST_DIGEST_LEN;
const CANCEL_LEN: usize = crate::TRANSFER_ID_LEN + 1;
const PAUSE_LEN: usize = crate::TRANSFER_ID_LEN + BATCH_MANIFEST_DIGEST_LEN + 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestFile {
    pub file_size: u64,
    pub file_digest: [u8; CONTENT_DIGEST_LEN],
    pub file_name: String,
}

impl ManifestFile {
    pub fn new(
        file_size: u64,
        file_digest: [u8; CONTENT_DIGEST_LEN],
        file_name: String,
    ) -> Result<Self, TransferProtocolError> {
        validate_file(file_size, &file_name)?;
        Ok(Self {
            file_size,
            file_digest,
            file_name,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferManifest {
    pub transfer_id: [u8; crate::TRANSFER_ID_LEN],
    pub chunk_size: u32,
    pub files: Vec<ManifestFile>,
}

impl TransferManifest {
    pub fn new(
        transfer_id: [u8; crate::TRANSFER_ID_LEN],
        chunk_size: u32,
        files: Vec<ManifestFile>,
    ) -> Result<Self, TransferProtocolError> {
        validate_manifest(chunk_size, &files)?;
        Ok(Self {
            transfer_id,
            chunk_size,
            files,
        })
    }

    pub fn aggregate_size(&self) -> Result<u64, TransferProtocolError> {
        aggregate_size(&self.files)
    }

    #[must_use]
    pub fn digest(&self) -> [u8; BATCH_MANIFEST_DIGEST_LEN] {
        let mut digest = Sha256::new();
        digest.update(b"Halo Transfer Manifest v1");
        digest.update(self.transfer_id);
        digest.update(self.chunk_size.to_be_bytes());
        digest.update([u8::try_from(self.files.len()).unwrap_or(u8::MAX)]);
        for file in &self.files {
            digest.update(file.file_size.to_be_bytes());
            digest.update(file.file_digest);
            let name = file.file_name.as_bytes();
            digest.update(u16::try_from(name.len()).unwrap_or(u16::MAX).to_be_bytes());
            digest.update(name);
        }
        digest.finalize().into()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResumePosition {
    pub file_index: u16,
    pub next_chunk_index: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchDecision {
    pub transfer_id: [u8; crate::TRANSFER_ID_LEN],
    pub manifest_digest: [u8; BATCH_MANIFEST_DIGEST_LEN],
    pub accepted: bool,
    pub resume_positions: Vec<ResumePosition>,
}

impl BatchDecision {
    pub fn new(
        transfer_id: [u8; crate::TRANSFER_ID_LEN],
        manifest_digest: [u8; BATCH_MANIFEST_DIGEST_LEN],
        accepted: bool,
        resume_positions: Vec<ResumePosition>,
    ) -> Result<Self, TransferProtocolError> {
        validate_resume_positions(accepted, &resume_positions)?;
        Ok(Self {
            transfer_id,
            manifest_digest,
            accepted,
            resume_positions,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchComplete {
    pub transfer_id: [u8; crate::TRANSFER_ID_LEN],
    pub manifest_digest: [u8; BATCH_MANIFEST_DIGEST_LEN],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BatchCancelReason {
    User = 1,
    Policy = 2,
    Integrity = 3,
    Storage = 4,
    Protocol = 5,
}

impl TryFrom<u8> for BatchCancelReason {
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
pub struct BatchCancel {
    pub transfer_id: [u8; crate::TRANSFER_ID_LEN],
    pub reason: BatchCancelReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BatchPauseReason {
    User = 1,
    RouteLost = 2,
    AppLifecycle = 3,
}

impl TryFrom<u8> for BatchPauseReason {
    type Error = TransferProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::User),
            2 => Ok(Self::RouteLost),
            3 => Ok(Self::AppLifecycle),
            _ => Err(TransferProtocolError::InvalidPauseReason(value)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchPause {
    pub transfer_id: [u8; crate::TRANSFER_ID_LEN],
    pub manifest_digest: [u8; BATCH_MANIFEST_DIGEST_LEN],
    pub reason: BatchPauseReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransferMessage {
    Offer(TransferManifest),
    Decision(BatchDecision),
    Complete(BatchComplete),
    Cancel(BatchCancel),
    Pause(BatchPause),
}

impl TransferMessage {
    pub fn encode(&self) -> Result<Vec<u8>, TransferProtocolError> {
        let (kind, payload) = match self {
            Self::Offer(manifest) => (OFFER_KIND, encode_manifest(manifest)?),
            Self::Decision(decision) => (DECISION_KIND, encode_decision(decision)?),
            Self::Complete(complete) => {
                let mut payload = Vec::with_capacity(COMPLETE_LEN);
                payload.extend_from_slice(&complete.transfer_id);
                payload.extend_from_slice(&complete.manifest_digest);
                (COMPLETE_KIND, payload)
            }
            Self::Cancel(cancel) => {
                let mut payload = Vec::with_capacity(CANCEL_LEN);
                payload.extend_from_slice(&cancel.transfer_id);
                payload.push(cancel.reason as u8);
                (CANCEL_KIND, payload)
            }
            Self::Pause(pause) => {
                let mut payload = Vec::with_capacity(PAUSE_LEN);
                payload.extend_from_slice(&pause.transfer_id);
                payload.extend_from_slice(&pause.manifest_digest);
                payload.push(pause.reason as u8);
                (PAUSE_KIND, payload)
            }
        };
        encode_frame(kind, payload)
    }

    pub fn decode(frame: &[u8]) -> Result<Self, TransferProtocolError> {
        let (kind, payload) = decode_frame(frame)?;
        match kind {
            OFFER_KIND => decode_manifest(payload).map(Self::Offer),
            DECISION_KIND => decode_decision(payload).map(Self::Decision),
            COMPLETE_KIND => {
                exact_len(payload, COMPLETE_LEN)?;
                Ok(Self::Complete(BatchComplete {
                    transfer_id: read_array(payload, 0),
                    manifest_digest: read_array(payload, crate::TRANSFER_ID_LEN),
                }))
            }
            CANCEL_KIND => {
                exact_len(payload, CANCEL_LEN)?;
                Ok(Self::Cancel(BatchCancel {
                    transfer_id: read_array(payload, 0),
                    reason: BatchCancelReason::try_from(payload[crate::TRANSFER_ID_LEN])?,
                }))
            }
            PAUSE_KIND => {
                exact_len(payload, PAUSE_LEN)?;
                Ok(Self::Pause(BatchPause {
                    transfer_id: read_array(payload, 0),
                    manifest_digest: read_array(payload, crate::TRANSFER_ID_LEN),
                    reason: BatchPauseReason::try_from(
                        payload[crate::TRANSFER_ID_LEN + BATCH_MANIFEST_DIGEST_LEN],
                    )?,
                }))
            }
            _ => Err(TransferProtocolError::UnknownMessageKind(kind)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchChunk {
    pub transfer_id: [u8; crate::TRANSFER_ID_LEN],
    pub file_index: u16,
    pub chunk_index: u32,
    pub chunk_digest: [u8; CONTENT_DIGEST_LEN],
    pub payload: Vec<u8>,
}

impl BatchChunk {
    pub fn new(
        transfer_id: [u8; crate::TRANSFER_ID_LEN],
        file_index: u16,
        chunk_index: u32,
        chunk_digest: [u8; CONTENT_DIGEST_LEN],
        payload: Vec<u8>,
    ) -> Result<Self, TransferProtocolError> {
        validate_chunk_payload(payload.len())?;
        Ok(Self {
            transfer_id,
            file_index,
            chunk_index,
            chunk_digest,
            payload,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, TransferProtocolError> {
        let mut record = Vec::with_capacity(DATA_RECORD_HEADER_LEN + self.payload.len());
        self.as_ref().encode_into(&mut record)?;
        Ok(record)
    }

    pub fn decode(record: &[u8]) -> Result<Self, TransferProtocolError> {
        let chunk = BatchChunkRef::decode(record)?;
        Self::new(
            chunk.transfer_id,
            chunk.file_index,
            chunk.chunk_index,
            chunk.chunk_digest,
            chunk.payload.to_vec(),
        )
    }

    #[must_use]
    pub fn as_ref(&self) -> BatchChunkRef<'_> {
        BatchChunkRef {
            transfer_id: self.transfer_id,
            file_index: self.file_index,
            chunk_index: self.chunk_index,
            chunk_digest: self.chunk_digest,
            payload: &self.payload,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchChunkRef<'a> {
    pub transfer_id: [u8; crate::TRANSFER_ID_LEN],
    pub file_index: u16,
    pub chunk_index: u32,
    pub chunk_digest: [u8; CONTENT_DIGEST_LEN],
    pub payload: &'a [u8],
}

impl<'a> BatchChunkRef<'a> {
    pub fn new(
        transfer_id: [u8; crate::TRANSFER_ID_LEN],
        file_index: u16,
        chunk_index: u32,
        chunk_digest: [u8; CONTENT_DIGEST_LEN],
        payload: &'a [u8],
    ) -> Result<Self, TransferProtocolError> {
        validate_chunk_payload(payload.len())?;
        Ok(Self {
            transfer_id,
            file_index,
            chunk_index,
            chunk_digest,
            payload,
        })
    }

    pub fn encode_into(self, record: &mut Vec<u8>) -> Result<(), TransferProtocolError> {
        validate_chunk_payload(self.payload.len())?;
        let payload_length = u32::try_from(self.payload.len())
            .map_err(|_| TransferProtocolError::InvalidChunkPayloadLength(self.payload.len()))?;
        record.clear();
        record.reserve(DATA_RECORD_HEADER_LEN + self.payload.len());
        record.extend_from_slice(DATA_MAGIC);
        record.extend_from_slice(&self.transfer_id);
        record.extend_from_slice(&self.file_index.to_be_bytes());
        record.extend_from_slice(&0_u16.to_be_bytes());
        record.extend_from_slice(&self.chunk_index.to_be_bytes());
        record.extend_from_slice(&payload_length.to_be_bytes());
        record.extend_from_slice(&self.chunk_digest);
        record.extend_from_slice(self.payload);
        Ok(())
    }

    pub fn decode(record: &'a [u8]) -> Result<Self, TransferProtocolError> {
        if record.len() < DATA_RECORD_HEADER_LEN {
            return Err(TransferProtocolError::TruncatedDataHeader(record.len()));
        }
        if record.len() > MAX_DATA_RECORD_LEN {
            return Err(TransferProtocolError::DataRecordTooLarge(record.len()));
        }
        if &record[..4] != DATA_MAGIC {
            return Err(TransferProtocolError::InvalidDataMagic);
        }
        if read_u16(record, 22) != 0 {
            return Err(TransferProtocolError::ReservedDataFlags(read_u16(
                record, 22,
            )));
        }
        let payload_length = read_u32(record, 28) as usize;
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
            read_u16(record, 20),
            read_u32(record, 24),
            read_array(record, 32),
            &record[DATA_RECORD_HEADER_LEN..],
        )
    }
}

fn encode_manifest(manifest: &TransferManifest) -> Result<Vec<u8>, TransferProtocolError> {
    validate_manifest(manifest.chunk_size, &manifest.files)?;
    let file_count = u8::try_from(manifest.files.len())
        .map_err(|_| TransferProtocolError::InvalidFileCount(manifest.files.len()))?;
    let aggregate = manifest.aggregate_size()?;
    let mut payload = Vec::with_capacity(
        OFFER_FIXED_LEN
            + manifest
                .files
                .iter()
                .map(|file| FILE_FIXED_LEN + file.file_name.len())
                .sum::<usize>(),
    );
    payload.extend_from_slice(&manifest.transfer_id);
    payload.extend_from_slice(&manifest.chunk_size.to_be_bytes());
    payload.push(file_count);
    payload.extend_from_slice(&[0; 3]);
    payload.extend_from_slice(&aggregate.to_be_bytes());
    payload.extend_from_slice(&manifest.digest());
    for file in &manifest.files {
        let name = file.file_name.as_bytes();
        let name_length = u16::try_from(name.len())
            .map_err(|_| TransferProtocolError::InvalidFileNameLength(name.len()))?;
        payload.extend_from_slice(&file.file_size.to_be_bytes());
        payload.extend_from_slice(&file.file_digest);
        payload.extend_from_slice(&name_length.to_be_bytes());
        payload.extend_from_slice(name);
    }
    Ok(payload)
}

fn decode_manifest(payload: &[u8]) -> Result<TransferManifest, TransferProtocolError> {
    if payload.len() < OFFER_FIXED_LEN {
        return Err(TransferProtocolError::MessageLength {
            expected: OFFER_FIXED_LEN,
            actual: payload.len(),
        });
    }
    if payload[21..24] != [0; 3] {
        return Err(TransferProtocolError::ReservedManifestFlags);
    }
    let file_count = payload[20] as usize;
    if !(1..=MAX_BATCH_FILES).contains(&file_count) {
        return Err(TransferProtocolError::InvalidFileCount(file_count));
    }
    let mut offset = OFFER_FIXED_LEN;
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        let fixed_end = offset
            .checked_add(FILE_FIXED_LEN)
            .ok_or(TransferProtocolError::FrameTooLarge(usize::MAX))?;
        if fixed_end > payload.len() {
            return Err(TransferProtocolError::TruncatedManifest);
        }
        let name_length = read_u16(payload, offset + 8 + CONTENT_DIGEST_LEN) as usize;
        let name_end = fixed_end
            .checked_add(name_length)
            .ok_or(TransferProtocolError::FrameTooLarge(usize::MAX))?;
        if name_end > payload.len() {
            return Err(TransferProtocolError::TruncatedManifest);
        }
        let file_name = std::str::from_utf8(&payload[fixed_end..name_end])
            .map_err(|_| TransferProtocolError::InvalidUtf8)?
            .to_owned();
        files.push(ManifestFile::new(
            read_u64(payload, offset),
            read_array(payload, offset + 8),
            file_name,
        )?);
        offset = name_end;
    }
    if offset != payload.len() {
        return Err(TransferProtocolError::TrailingManifestBytes(
            payload.len() - offset,
        ));
    }
    let manifest = TransferManifest::new(
        read_array(payload, 0),
        read_u32(payload, crate::TRANSFER_ID_LEN),
        files,
    )?;
    let declared_aggregate = read_u64(payload, 24);
    if manifest.aggregate_size()? != declared_aggregate {
        return Err(TransferProtocolError::AggregateSizeMismatch);
    }
    let declared_digest: [u8; BATCH_MANIFEST_DIGEST_LEN] = read_array(payload, 32);
    if manifest.digest() != declared_digest {
        return Err(TransferProtocolError::ManifestDigestMismatch);
    }
    Ok(manifest)
}

fn encode_decision(decision: &BatchDecision) -> Result<Vec<u8>, TransferProtocolError> {
    validate_resume_positions(decision.accepted, &decision.resume_positions)?;
    let count = u8::try_from(decision.resume_positions.len()).map_err(|_| {
        TransferProtocolError::InvalidResumePositionCount(decision.resume_positions.len())
    })?;
    let mut payload = Vec::with_capacity(DECISION_FIXED_LEN + decision.resume_positions.len() * 6);
    payload.extend_from_slice(&decision.transfer_id);
    payload.extend_from_slice(&decision.manifest_digest);
    payload.push(u8::from(decision.accepted));
    payload.push(count);
    payload.extend_from_slice(&0_u16.to_be_bytes());
    for position in &decision.resume_positions {
        payload.extend_from_slice(&position.file_index.to_be_bytes());
        payload.extend_from_slice(&position.next_chunk_index.to_be_bytes());
    }
    Ok(payload)
}

fn decode_decision(payload: &[u8]) -> Result<BatchDecision, TransferProtocolError> {
    if payload.len() < DECISION_FIXED_LEN {
        return Err(TransferProtocolError::MessageLength {
            expected: DECISION_FIXED_LEN,
            actual: payload.len(),
        });
    }
    let accepted = match payload[crate::TRANSFER_ID_LEN + BATCH_MANIFEST_DIGEST_LEN] {
        0 => false,
        1 => true,
        value => return Err(TransferProtocolError::InvalidBoolean(value)),
    };
    let count = payload[crate::TRANSFER_ID_LEN + BATCH_MANIFEST_DIGEST_LEN + 1] as usize;
    if read_u16(
        payload,
        crate::TRANSFER_ID_LEN + BATCH_MANIFEST_DIGEST_LEN + 2,
    ) != 0
    {
        return Err(TransferProtocolError::ReservedDecisionFlags);
    }
    let expected = DECISION_FIXED_LEN
        .checked_add(
            count
                .checked_mul(RESUME_POSITION_LEN)
                .ok_or(TransferProtocolError::FrameTooLarge(usize::MAX))?,
        )
        .ok_or(TransferProtocolError::FrameTooLarge(usize::MAX))?;
    if payload.len() != expected {
        return Err(TransferProtocolError::MessageLength {
            expected,
            actual: payload.len(),
        });
    }
    let mut positions = Vec::with_capacity(count);
    for index in 0..count {
        let offset = DECISION_FIXED_LEN + index * RESUME_POSITION_LEN;
        positions.push(ResumePosition {
            file_index: read_u16(payload, offset),
            next_chunk_index: read_u32(payload, offset + 2),
        });
    }
    BatchDecision::new(
        read_array(payload, 0),
        read_array(payload, crate::TRANSFER_ID_LEN),
        accepted,
        positions,
    )
}

fn encode_frame(kind: u8, payload: Vec<u8>) -> Result<Vec<u8>, TransferProtocolError> {
    let frame_length = HEADER_LEN
        .checked_add(payload.len())
        .ok_or(TransferProtocolError::FrameTooLarge(usize::MAX))?;
    if frame_length > crate::MAX_FRAME_LEN {
        return Err(TransferProtocolError::FrameTooLarge(frame_length));
    }
    let payload_length = u32::try_from(payload.len())
        .map_err(|_| TransferProtocolError::FrameTooLarge(frame_length))?;
    let mut frame = Vec::with_capacity(frame_length);
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&TRANSFER_WIRE_VERSION.to_be_bytes());
    frame.push(kind);
    frame.push(0);
    frame.extend_from_slice(&payload_length.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn decode_frame(frame: &[u8]) -> Result<(u8, &[u8]), TransferProtocolError> {
    if frame.len() < HEADER_LEN {
        return Err(TransferProtocolError::TruncatedHeader(frame.len()));
    }
    if frame.len() > crate::MAX_FRAME_LEN {
        return Err(TransferProtocolError::FrameTooLarge(frame.len()));
    }
    if &frame[..4] != MAGIC {
        return Err(TransferProtocolError::InvalidMagic);
    }
    let version = read_u16(frame, 4);
    if version != TRANSFER_WIRE_VERSION {
        return Err(TransferProtocolError::UnsupportedWireVersion(version));
    }
    if frame[7] != 0 {
        return Err(TransferProtocolError::ReservedFlags(frame[7]));
    }
    let declared = read_u32(frame, 8) as usize;
    let actual = frame.len() - HEADER_LEN;
    if declared != actual {
        return Err(TransferProtocolError::PayloadLength { declared, actual });
    }
    Ok((frame[6], &frame[HEADER_LEN..]))
}

fn validate_manifest(chunk_size: u32, files: &[ManifestFile]) -> Result<(), TransferProtocolError> {
    if chunk_size == 0 || chunk_size > MAX_CHUNK_SIZE {
        return Err(TransferProtocolError::InvalidChunkSize(chunk_size));
    }
    if !(1..=MAX_BATCH_FILES).contains(&files.len()) {
        return Err(TransferProtocolError::InvalidFileCount(files.len()));
    }
    for file in files {
        validate_file(file.file_size, &file.file_name)?;
    }
    aggregate_size(files)?;
    Ok(())
}

fn validate_file(file_size: u64, file_name: &str) -> Result<(), TransferProtocolError> {
    if file_size > MAX_FILE_SIZE {
        return Err(TransferProtocolError::FileTooLarge(file_size));
    }
    let name_length = file_name.len();
    if name_length == 0 || name_length > MAX_FILE_NAME_LEN {
        return Err(TransferProtocolError::InvalidFileNameLength(name_length));
    }
    Ok(())
}

fn aggregate_size(files: &[ManifestFile]) -> Result<u64, TransferProtocolError> {
    files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.file_size)
            .filter(|sum| *sum <= MAX_FILE_SIZE)
            .ok_or(TransferProtocolError::AggregateTooLarge)
    })
}

fn validate_resume_positions(
    accepted: bool,
    positions: &[ResumePosition],
) -> Result<(), TransferProtocolError> {
    if !accepted && !positions.is_empty() {
        return Err(TransferProtocolError::RejectedWithResumePositions);
    }
    if positions.len() > MAX_BATCH_FILES {
        return Err(TransferProtocolError::InvalidResumePositionCount(
            positions.len(),
        ));
    }
    for (expected, position) in positions.iter().enumerate() {
        if usize::from(position.file_index) != expected {
            return Err(TransferProtocolError::InvalidResumeFileIndex {
                expected,
                actual: position.file_index,
            });
        }
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
            expected,
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
    #[error("transfer message length is {actual}, expected {expected}")]
    MessageLength { expected: usize, actual: usize },
    #[error("transfer manifest is truncated")]
    TruncatedManifest,
    #[error("transfer manifest has {0} trailing bytes")]
    TrailingManifestBytes(usize),
    #[error("transfer manifest uses reserved flags")]
    ReservedManifestFlags,
    #[error("transfer decision uses reserved flags")]
    ReservedDecisionFlags,
    #[error("transfer boolean has invalid value {0}")]
    InvalidBoolean(u8),
    #[error("transfer cancellation reason has invalid value {0}")]
    InvalidCancelReason(u8),
    #[error("transfer pause reason has invalid value {0}")]
    InvalidPauseReason(u8),
    #[error("transfer filename is not valid UTF-8")]
    InvalidUtf8,
    #[error("transfer filename length is invalid: {0}")]
    InvalidFileNameLength(usize),
    #[error("transfer file exceeds the per-file size limit: {0}")]
    FileTooLarge(u64),
    #[error("transfer aggregate size exceeds the configured limit")]
    AggregateTooLarge,
    #[error("transfer aggregate size does not match the file entries")]
    AggregateSizeMismatch,
    #[error("transfer manifest digest does not match its canonical fields")]
    ManifestDigestMismatch,
    #[error("transfer file count is invalid: {0}")]
    InvalidFileCount(usize),
    #[error("transfer chunk size is invalid: {0}")]
    InvalidChunkSize(u32),
    #[error("transfer resume position count is invalid: {0}")]
    InvalidResumePositionCount(usize),
    #[error("a rejected transfer decision cannot contain resume positions")]
    RejectedWithResumePositions,
    #[error("transfer resume file index is {actual}, expected {expected}")]
    InvalidResumeFileIndex { expected: usize, actual: u16 },
    #[error("transfer data header is truncated: {0} bytes")]
    TruncatedDataHeader(usize),
    #[error("transfer data record exceeds the configured limit: {0} bytes")]
    DataRecordTooLarge(usize),
    #[error("transfer data record magic does not match")]
    InvalidDataMagic,
    #[error("transfer data record uses reserved flags 0x{0:04x}")]
    ReservedDataFlags(u16),
    #[error("transfer chunk payload length is invalid: {0}")]
    InvalidChunkPayloadLength(usize),
    #[error("transfer data record length is {actual}, expected {declared}")]
    DataRecordLength { declared: usize, actual: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> TransferManifest {
        TransferManifest::new(
            [0x11; crate::TRANSFER_ID_LEN],
            64 * 1024,
            vec![
                ManifestFile::new(7, [0x21; 32], "first.txt".to_owned())
                    .unwrap_or_else(|error| panic!("first file: {error}")),
                ManifestFile::new(9, [0x22; 32], "second.bin".to_owned())
                    .unwrap_or_else(|error| panic!("second file: {error}")),
            ],
        )
        .unwrap_or_else(|error| panic!("manifest: {error}"))
    }

    #[test]
    fn manifest_and_terminal_messages_round_trip() {
        let manifest = manifest();
        let digest = manifest.digest();
        assert_eq!(
            digest,
            [
                0xf7, 0xa7, 0x90, 0x61, 0xca, 0x4c, 0x58, 0x52, 0xe1, 0xd1, 0xa9, 0x63, 0x28, 0x6b,
                0xec, 0x83, 0x55, 0xbc, 0x68, 0x6b, 0x98, 0x65, 0x22, 0xa8, 0xf5, 0xef, 0x9f, 0x69,
                0xa6, 0xca, 0x34, 0x07,
            ]
        );
        let messages = [
            TransferMessage::Offer(manifest),
            TransferMessage::Decision(
                BatchDecision::new(
                    [0x11; 16],
                    digest,
                    true,
                    vec![
                        ResumePosition {
                            file_index: 0,
                            next_chunk_index: 2,
                        },
                        ResumePosition {
                            file_index: 1,
                            next_chunk_index: 0,
                        },
                    ],
                )
                .unwrap_or_else(|error| panic!("decision: {error}")),
            ),
            TransferMessage::Complete(BatchComplete {
                transfer_id: [0x11; 16],
                manifest_digest: digest,
            }),
            TransferMessage::Cancel(BatchCancel {
                transfer_id: [0x11; 16],
                reason: BatchCancelReason::Storage,
            }),
            TransferMessage::Pause(BatchPause {
                transfer_id: [0x11; 16],
                manifest_digest: digest,
                reason: BatchPauseReason::RouteLost,
            }),
        ];
        for message in messages {
            let encoded = message
                .encode()
                .unwrap_or_else(|error| panic!("encode: {error}"));
            assert_eq!(&encoded[..6], b"HALO\x00\x01");
            assert_eq!(TransferMessage::decode(&encoded), Ok(message));
        }
    }

    #[test]
    fn manifest_rejects_truncation_tampering_and_resource_exhaustion() {
        let encoded = TransferMessage::Offer(manifest())
            .encode()
            .unwrap_or_else(|error| panic!("encode manifest: {error}"));
        for length in 0..encoded.len() {
            assert!(TransferMessage::decode(&encoded[..length]).is_err());
        }

        let mut digest_tampered = encoded;
        digest_tampered[HEADER_LEN + 32] ^= 1;
        assert_eq!(
            TransferMessage::decode(&digest_tampered),
            Err(TransferProtocolError::ManifestDigestMismatch)
        );

        let file = ManifestFile::new(0, [0; 32], "file".to_owned())
            .unwrap_or_else(|error| panic!("file: {error}"));
        assert!(
            TransferManifest::new([0; 16], 64 * 1024, vec![file; MAX_BATCH_FILES + 1]).is_err()
        );
    }

    #[test]
    fn decision_rejects_sparse_or_rejected_resume_maps() {
        let digest = manifest().digest();
        assert!(matches!(
            BatchDecision::new(
                [0; 16],
                digest,
                true,
                vec![ResumePosition {
                    file_index: 1,
                    next_chunk_index: 0,
                }],
            ),
            Err(TransferProtocolError::InvalidResumeFileIndex { .. })
        ));
        assert_eq!(
            BatchDecision::new(
                [0; 16],
                digest,
                false,
                vec![ResumePosition {
                    file_index: 0,
                    next_chunk_index: 0,
                }],
            ),
            Err(TransferProtocolError::RejectedWithResumePositions)
        );
    }

    #[test]
    fn batch_chunk_round_trips_and_rejects_reserved_or_trailing_data() {
        let chunk = BatchChunk::new([0x31; 16], 2, 7, [0x42; 32], vec![0x53; 33])
            .unwrap_or_else(|error| panic!("chunk: {error}"));
        let encoded = chunk
            .encode()
            .unwrap_or_else(|error| panic!("encode chunk: {error}"));
        assert_eq!(encoded.len(), DATA_RECORD_HEADER_LEN + 33);
        assert_eq!(&encoded[..4], b"HDF1");
        let borrowed = BatchChunkRef::decode(&encoded)
            .unwrap_or_else(|error| panic!("decode borrowed chunk: {error}"));
        assert_eq!(borrowed.payload, &encoded[DATA_RECORD_HEADER_LEN..]);
        assert_eq!(
            borrowed.payload.as_ptr(),
            encoded[DATA_RECORD_HEADER_LEN..].as_ptr()
        );
        assert_eq!(BatchChunk::decode(&encoded), Ok(chunk));

        let mut reused = Vec::with_capacity(MAX_DATA_RECORD_LEN);
        borrowed
            .encode_into(&mut reused)
            .unwrap_or_else(|error| panic!("encode borrowed chunk: {error}"));
        assert_eq!(reused, encoded);

        let mut reserved = encoded.clone();
        reserved[22..24].copy_from_slice(&1_u16.to_be_bytes());
        assert_eq!(
            BatchChunk::decode(&reserved),
            Err(TransferProtocolError::ReservedDataFlags(1))
        );
        let mut trailing = encoded;
        trailing.push(0);
        assert!(matches!(
            BatchChunk::decode(&trailing),
            Err(TransferProtocolError::DataRecordLength { .. })
        ));
    }
}
