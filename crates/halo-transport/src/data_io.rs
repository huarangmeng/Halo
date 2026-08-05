use async_trait::async_trait;
use thiserror::Error;

use halo_crypto::TlsChannelBinding;
use halo_protocol::{DATA_RECORD_HEADER_LEN, MAX_DATA_RECORD_LEN};

/// One ordered file-data stream on an already authenticated QUIC connection.
/// Implementations frame records but leave protocol parsing and digest policy
/// to `halo-transfer`.
#[async_trait]
pub trait DataIo: Send {
    fn channel_binding(&self) -> TlsChannelBinding;
    async fn send_record(&mut self, record: &[u8]) -> Result<(), DataIoError>;
    async fn receive_record(&mut self) -> Result<Vec<u8>, DataIoError>;
    /// Ends the local sending side after the final declared record.
    async fn finish_send(&mut self) -> Result<(), DataIoError>;
    /// Requires the peer to end its sending side without trailing bytes.
    async fn expect_end(&mut self) -> Result<(), DataIoError>;
    async fn close(&mut self);
}

pub(crate) fn data_record_length(header: &[u8]) -> Result<usize, DataIoError> {
    if header.len() != DATA_RECORD_HEADER_LEN {
        return Err(DataIoError::Truncated);
    }
    let payload_length =
        u32::from_be_bytes([header[24], header[25], header[26], header[27]]) as usize;
    let record_length = DATA_RECORD_HEADER_LEN
        .checked_add(payload_length)
        .ok_or(DataIoError::RecordTooLarge(usize::MAX))?;
    if payload_length == 0 || record_length > MAX_DATA_RECORD_LEN {
        return Err(DataIoError::RecordTooLarge(record_length));
    }
    Ok(record_length)
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DataIoError {
    #[error("file-data record exceeds its bounded length: {0} bytes")]
    RecordTooLarge(usize),
    #[error("file-data record ended before its declared length")]
    Truncated,
    #[error("failed to read file-data stream")]
    Read,
    #[error("failed to write file-data stream")]
    Write,
    #[error("file-data stream contains bytes after the declared file")]
    TrailingData,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_framing_is_bounded_before_payload_allocation() {
        let mut header = [0_u8; DATA_RECORD_HEADER_LEN];
        header[24..28].copy_from_slice(&1_u32.to_be_bytes());
        assert_eq!(data_record_length(&header), Ok(DATA_RECORD_HEADER_LEN + 1));

        header[24..28].copy_from_slice(&0_u32.to_be_bytes());
        assert!(matches!(
            data_record_length(&header),
            Err(DataIoError::RecordTooLarge(DATA_RECORD_HEADER_LEN))
        ));
        header[24..28].copy_from_slice(&(256_u32 * 1024 + 1).to_be_bytes());
        assert!(matches!(
            data_record_length(&header),
            Err(DataIoError::RecordTooLarge(_))
        ));
        assert_eq!(
            data_record_length(&header[..59]),
            Err(DataIoError::Truncated)
        );
    }
}
