//! Single-file transfer workflow for authenticated Halo sessions.

#![forbid(unsafe_code)]

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use getrandom::fill;
use halo_crypto::TlsChannelBinding;
use halo_protocol::{
    CONTENT_DIGEST_LEN, DEFAULT_CHUNK_SIZE, MAX_FILE_NAME_LEN, TRANSFER_ID_LEN, TransferChunk,
    TransferComplete, TransferDecision, TransferMessage, TransferOffer, TransferProtocolError,
};
use halo_transport::{ControlIo, DataIo, DataIoError, FrameIoError};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct PreparedFile {
    source_path: PathBuf,
    offer: TransferOffer,
}

impl PreparedFile {
    #[must_use]
    pub fn offer(&self) -> &TransferOffer {
        &self.offer
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedFile {
    pub transfer_id: [u8; TRANSFER_ID_LEN],
    pub final_path: PathBuf,
    pub size: u64,
    pub digest: [u8; CONTENT_DIGEST_LEN],
}

pub async fn prepare_file(
    source_path: impl Into<PathBuf>,
    advertised_name: Option<String>,
    cancellation: &CancellationToken,
) -> Result<PreparedFile, TransferError> {
    let source_path = source_path.into();
    let metadata = fs::symlink_metadata(&source_path)
        .await
        .map_err(|_| TransferError::Source)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(TransferError::Source);
    }
    let file_name = advertised_name.unwrap_or_else(|| {
        source_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned()
    });
    validate_file_name(&file_name)?;
    let digest = hash_file(&source_path, cancellation).await?;
    let mut transfer_id = [0_u8; TRANSFER_ID_LEN];
    fill(&mut transfer_id).map_err(|_| TransferError::Randomness)?;
    let offer = TransferOffer::new(
        transfer_id,
        metadata.len(),
        DEFAULT_CHUNK_SIZE,
        digest,
        file_name,
    )?;
    Ok(PreparedFile { source_path, offer })
}

pub async fn send_offer(
    control: &mut dyn ControlIo,
    prepared: &PreparedFile,
) -> Result<bool, TransferError> {
    control
        .send_frame(&TransferMessage::Offer(prepared.offer.clone()).encode()?)
        .await?;
    match TransferMessage::decode(&control.receive_frame(4096).await?)? {
        TransferMessage::Decision(decision)
            if decision.transfer_id == prepared.offer.transfer_id =>
        {
            Ok(decision.accepted)
        }
        TransferMessage::Cancel(cancel) if cancel.transfer_id == prepared.offer.transfer_id => {
            Err(TransferError::Rejected)
        }
        _ => Err(TransferError::UnexpectedMessage),
    }
}

pub async fn receive_offer(control: &mut dyn ControlIo) -> Result<TransferOffer, TransferError> {
    match TransferMessage::decode(&control.receive_frame(4096).await?)? {
        TransferMessage::Offer(offer) => {
            validate_file_name(&offer.file_name)?;
            Ok(offer)
        }
        _ => Err(TransferError::UnexpectedMessage),
    }
}

pub async fn send_decision(
    control: &mut dyn ControlIo,
    transfer_id: [u8; TRANSFER_ID_LEN],
    accepted: bool,
) -> Result<(), TransferError> {
    control
        .send_frame(
            &TransferMessage::Decision(TransferDecision {
                transfer_id,
                accepted,
            })
            .encode()?,
        )
        .await?;
    Ok(())
}

pub async fn send_file_data(
    data: &mut dyn DataIo,
    expected_binding: TlsChannelBinding,
    prepared: &PreparedFile,
    cancellation: &CancellationToken,
) -> Result<(), TransferError> {
    send_file_data_with_progress(data, expected_binding, prepared, cancellation, |_| {}).await
}

pub async fn send_file_data_with_progress(
    data: &mut dyn DataIo,
    expected_binding: TlsChannelBinding,
    prepared: &PreparedFile,
    cancellation: &CancellationToken,
    mut progress: impl FnMut(u64) + Send,
) -> Result<(), TransferError> {
    if data.channel_binding() != expected_binding {
        return Err(TransferError::ChannelBinding);
    }
    let mut file = File::open(&prepared.source_path)
        .await
        .map_err(|_| TransferError::Source)?;
    let mut remaining = prepared.offer.file_size;
    let mut chunk_index = 0_u32;
    let mut whole_digest = Sha256::new();
    while remaining > 0 {
        if cancellation.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let chunk_length = usize::try_from(remaining.min(u64::from(prepared.offer.chunk_size)))
            .map_err(|_| TransferError::SourceChanged)?;
        let mut payload = vec![0_u8; chunk_length];
        file.read_exact(&mut payload)
            .await
            .map_err(|_| TransferError::SourceChanged)?;
        whole_digest.update(&payload);
        let chunk_digest: [u8; CONTENT_DIGEST_LEN] = Sha256::digest(&payload).into();
        let record = TransferChunk::new(
            prepared.offer.transfer_id,
            chunk_index,
            chunk_digest,
            payload,
        )?
        .encode()?;
        tokio::select! {
            () = cancellation.cancelled() => return Err(TransferError::Cancelled),
            result = data.send_record(&record) => result?,
        }
        remaining -= u64::try_from(chunk_length).map_err(|_| TransferError::SourceChanged)?;
        progress(prepared.offer.file_size - remaining);
        chunk_index = chunk_index
            .checked_add(1)
            .ok_or(TransferError::SourceChanged)?;
    }
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .await
        .map_err(|_| TransferError::Source)?
        > 0
        || <[u8; CONTENT_DIGEST_LEN]>::from(whole_digest.finalize()) != prepared.offer.file_digest
    {
        return Err(TransferError::SourceChanged);
    }
    data.finish_send().await?;
    Ok(())
}

pub async fn receive_file_data(
    data: &mut dyn DataIo,
    expected_binding: TlsChannelBinding,
    offer: &TransferOffer,
    staging_directory: &Path,
    destination_directory: &Path,
    cancellation: &CancellationToken,
) -> Result<ReceivedFile, TransferError> {
    receive_file_data_with_progress(
        data,
        expected_binding,
        offer,
        staging_directory,
        destination_directory,
        cancellation,
        |_| {},
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn receive_file_data_with_progress(
    data: &mut dyn DataIo,
    expected_binding: TlsChannelBinding,
    offer: &TransferOffer,
    staging_directory: &Path,
    destination_directory: &Path,
    cancellation: &CancellationToken,
    progress: impl FnMut(u64) + Send,
) -> Result<ReceivedFile, TransferError> {
    if data.channel_binding() != expected_binding {
        return Err(TransferError::ChannelBinding);
    }
    validate_file_name(&offer.file_name)?;
    ensure_directory(staging_directory).await?;
    ensure_directory(destination_directory).await?;
    let staging_path = staging_directory.join(staging_name(offer));
    let final_path = destination_directory.join(&offer.file_name);
    receive_file_data_at_paths(
        data,
        offer,
        staging_path,
        final_path,
        cancellation,
        progress,
    )
    .await
}

async fn receive_file_data_at_paths(
    data: &mut dyn DataIo,
    offer: &TransferOffer,
    staging_path: PathBuf,
    final_path: PathBuf,
    cancellation: &CancellationToken,
    progress: impl FnMut(u64) + Send,
) -> Result<ReceivedFile, TransferError> {
    let staging_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging_path)
        .await
        .map_err(|_| TransferError::Staging)?;
    let result = receive_into_staging(data, offer, staging_file, cancellation, progress).await;
    let digest = match result {
        Ok(digest) => digest,
        Err(error) => {
            let _ = fs::remove_file(&staging_path).await;
            return Err(error);
        }
    };
    if fs::hard_link(&staging_path, &final_path).await.is_err() {
        let _ = fs::remove_file(&staging_path).await;
        return Err(if fs::symlink_metadata(&final_path).await.is_ok() {
            TransferError::DestinationExists
        } else {
            TransferError::Finalization
        });
    }
    fs::remove_file(&staging_path)
        .await
        .map_err(|_| TransferError::Finalization)?;
    Ok(ReceivedFile {
        transfer_id: offer.transfer_id,
        final_path,
        size: offer.file_size,
        digest,
    })
}

pub async fn send_complete(
    control: &mut dyn ControlIo,
    received: &ReceivedFile,
) -> Result<(), TransferError> {
    control
        .send_frame(
            &TransferMessage::Complete(TransferComplete {
                transfer_id: received.transfer_id,
                file_digest: received.digest,
            })
            .encode()?,
        )
        .await?;
    Ok(())
}

pub async fn wait_for_complete(
    control: &mut dyn ControlIo,
    prepared: &PreparedFile,
) -> Result<(), TransferError> {
    match TransferMessage::decode(&control.receive_frame(4096).await?)? {
        TransferMessage::Complete(complete)
            if complete.transfer_id == prepared.offer.transfer_id
                && complete.file_digest == prepared.offer.file_digest =>
        {
            Ok(())
        }
        _ => Err(TransferError::UnexpectedMessage),
    }
}

async fn hash_file(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<[u8; CONTENT_DIGEST_LEN], TransferError> {
    let mut file = File::open(path).await.map_err(|_| TransferError::Source)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; DEFAULT_CHUNK_SIZE as usize];
    loop {
        if cancellation.is_cancelled() {
            return Err(TransferError::Cancelled);
        }
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|_| TransferError::Source)?;
        if read == 0 {
            return Ok(digest.finalize().into());
        }
        digest.update(&buffer[..read]);
    }
}

async fn receive_into_staging(
    data: &mut dyn DataIo,
    offer: &TransferOffer,
    mut file: File,
    cancellation: &CancellationToken,
    mut progress: impl FnMut(u64) + Send,
) -> Result<[u8; CONTENT_DIGEST_LEN], TransferError> {
    let mut remaining = offer.file_size;
    let mut chunk_index = 0_u32;
    let mut whole_digest = Sha256::new();
    while remaining > 0 {
        let record = tokio::select! {
            () = cancellation.cancelled() => return Err(TransferError::Cancelled),
            result = data.receive_record() => result?,
        };
        let chunk = TransferChunk::decode(&record)?;
        let expected_length = usize::try_from(remaining.min(u64::from(offer.chunk_size)))
            .map_err(|_| TransferError::Integrity)?;
        if chunk.transfer_id != offer.transfer_id
            || chunk.chunk_index != chunk_index
            || chunk.payload.len() != expected_length
            || <[u8; CONTENT_DIGEST_LEN]>::from(Sha256::digest(&chunk.payload))
                != chunk.chunk_digest
        {
            return Err(TransferError::Integrity);
        }
        file.write_all(&chunk.payload)
            .await
            .map_err(|_| TransferError::Storage)?;
        whole_digest.update(&chunk.payload);
        remaining -= u64::try_from(chunk.payload.len()).map_err(|_| TransferError::Integrity)?;
        progress(offer.file_size - remaining);
        chunk_index = chunk_index.checked_add(1).ok_or(TransferError::Integrity)?;
    }
    data.expect_end().await?;
    file.flush().await.map_err(|_| TransferError::Storage)?;
    file.sync_all().await.map_err(|_| TransferError::Storage)?;
    let digest = whole_digest.finalize().into();
    if digest != offer.file_digest {
        return Err(TransferError::Integrity);
    }
    Ok(digest)
}

async fn ensure_directory(path: &Path) -> Result<(), TransferError> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|_| TransferError::Storage)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(TransferError::Storage)
    }
}

fn staging_name(offer: &TransferOffer) -> String {
    let sequence = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
    let transfer_prefix = offer.transfer_id[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(".halo-{transfer_prefix}-{sequence}.part")
}

pub fn validate_file_name(name: &str) -> Result<(), TransferError> {
    if name.is_empty() || name.len() > MAX_FILE_NAME_LEN || name == "." || name == ".." {
        return Err(TransferError::InvalidFileName);
    }
    if name.ends_with([' ', '.'])
        || name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
    {
        return Err(TransferError::InvalidFileName);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .is_some_and(is_reserved_device_number)
        || stem
            .strip_prefix("LPT")
            .is_some_and(is_reserved_device_number);
    if reserved {
        return Err(TransferError::InvalidFileName);
    }
    Ok(())
}

fn is_reserved_device_number(value: &str) -> bool {
    value.len() == 1 && matches!(value.as_bytes()[0], b'1'..=b'9')
}

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("transfer protocol rejected the message: {0}")]
    Protocol(#[from] TransferProtocolError),
    #[error("transfer control stream failed: {0}")]
    Control(#[from] FrameIoError),
    #[error("transfer data stream failed: {0}")]
    Data(#[from] DataIoError),
    #[error("file name is not a safe cross-platform leaf name")]
    InvalidFileName,
    #[error("source file is unavailable")]
    Source,
    #[error("source file changed after the authenticated offer")]
    SourceChanged,
    #[error("receiver rejected the transfer")]
    Rejected,
    #[error("transfer channel binding does not match the authenticated session")]
    ChannelBinding,
    #[error("peer sent a message that is invalid in the current transfer state")]
    UnexpectedMessage,
    #[error("received file failed size, order, or digest verification")]
    Integrity,
    #[error("private staging file could not be created")]
    Staging,
    #[error("destination already exists")]
    DestinationExists,
    #[error("receiver storage operation failed")]
    Storage,
    #[error("verified file could not be finalized without overwrite")]
    Finalization,
    #[error("secure randomness is unavailable")]
    Randomness,
    #[error("transfer was cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicU64, Ordering},
    };

    use async_trait::async_trait;

    use super::*;

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct MemoryDataIo {
        binding: TlsChannelBinding,
        incoming: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        send_finished: bool,
        trailing_data: bool,
    }

    #[async_trait]
    impl DataIo for MemoryDataIo {
        fn channel_binding(&self) -> TlsChannelBinding {
            self.binding
        }

        async fn send_record(&mut self, record: &[u8]) -> Result<(), DataIoError> {
            self.sent.push(record.to_vec());
            Ok(())
        }

        async fn receive_record(&mut self) -> Result<Vec<u8>, DataIoError> {
            self.incoming.pop_front().ok_or(DataIoError::Truncated)
        }

        async fn finish_send(&mut self) -> Result<(), DataIoError> {
            self.send_finished = true;
            Ok(())
        }

        async fn expect_end(&mut self) -> Result<(), DataIoError> {
            if self.trailing_data || !self.incoming.is_empty() {
                Err(DataIoError::TrailingData)
            } else {
                Ok(())
            }
        }

        async fn close(&mut self) {}
    }

    fn test_directory(name: &str) -> PathBuf {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "halo-transfer-{name}-{}-{sequence}",
            std::process::id()
        ))
    }

    fn offer_and_records(content: &[u8], chunk_size: usize) -> (TransferOffer, VecDeque<Vec<u8>>) {
        let transfer_id = [0x11; TRANSFER_ID_LEN];
        let digest = Sha256::digest(content).into();
        let offer = TransferOffer::new(
            transfer_id,
            content.len() as u64,
            chunk_size as u32,
            digest,
            "received.txt".to_owned(),
        )
        .unwrap_or_else(|error| panic!("offer: {error}"));
        let records = content
            .chunks(chunk_size)
            .enumerate()
            .map(|(index, payload)| {
                TransferChunk::new(
                    transfer_id,
                    index as u32,
                    Sha256::digest(payload).into(),
                    payload.to_vec(),
                )
                .and_then(|chunk| chunk.encode())
                .unwrap_or_else(|error| panic!("chunk: {error}"))
            })
            .collect();
        (offer, records)
    }

    async fn assert_directory_empty(path: &Path) {
        let mut entries = fs::read_dir(path)
            .await
            .unwrap_or_else(|error| panic!("read directory: {error}"));
        assert!(
            entries
                .next_entry()
                .await
                .unwrap_or_else(|error| panic!("read entry: {error}"))
                .is_none()
        );
    }

    #[test]
    fn cross_platform_leaf_names_reject_traversal_and_device_files() {
        for invalid in [
            "",
            ".",
            "..",
            "../escape",
            "folder/file",
            "folder\\file",
            "CON",
            "con.txt",
            "LPT9.log",
            "trailing.",
            "trailing ",
            "bad:name",
            "bad\0name",
        ] {
            assert_eq!(
                validate_file_name(invalid).map_err(|error| error.to_string()),
                Err(TransferError::InvalidFileName.to_string()),
                "accepted {invalid:?}"
            );
        }
        for valid in [
            "photo.jpg",
            "报告 2026.pdf",
            "archive.tar.gz",
            "hello (1).txt",
        ] {
            assert!(validate_file_name(valid).is_ok(), "rejected {valid:?}");
        }
    }

    #[tokio::test]
    async fn verified_chunks_finalize_without_overwrite_and_remove_staging() {
        let root = test_directory("receive");
        let staging = root.join("staging");
        let destination = root.join("destination");
        fs::create_dir_all(&staging)
            .await
            .unwrap_or_else(|error| panic!("staging: {error}"));
        fs::create_dir_all(&destination)
            .await
            .unwrap_or_else(|error| panic!("destination: {error}"));
        let content = b"authenticated file contents across several chunks";
        let (offer, records) = offer_and_records(content, 8);
        let binding = TlsChannelBinding::new([0x42; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: records,
            sent: Vec::new(),
            send_finished: false,
            trailing_data: false,
        };
        let received = receive_file_data(
            &mut data,
            binding,
            &offer,
            &staging,
            &destination,
            &CancellationToken::new(),
        )
        .await
        .unwrap_or_else(|error| panic!("receive: {error}"));
        assert_eq!(
            fs::read(&received.final_path)
                .await
                .unwrap_or_else(|error| panic!("read final: {error}")),
            content
        );
        assert_directory_empty(&staging).await;
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn existing_destination_is_preserved_and_staging_is_removed() {
        let root = test_directory("no-overwrite");
        let staging = root.join("staging");
        let destination = root.join("destination");
        fs::create_dir_all(&staging)
            .await
            .unwrap_or_else(|error| panic!("staging: {error}"));
        fs::create_dir_all(&destination)
            .await
            .unwrap_or_else(|error| panic!("destination: {error}"));
        let (offer, records) = offer_and_records(b"new bytes", 4);
        fs::write(destination.join(&offer.file_name), b"existing bytes")
            .await
            .unwrap_or_else(|error| panic!("existing: {error}"));
        let binding = TlsChannelBinding::new([0x24; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: records,
            sent: Vec::new(),
            send_finished: false,
            trailing_data: false,
        };
        assert!(matches!(
            receive_file_data(
                &mut data,
                binding,
                &offer,
                &staging,
                &destination,
                &CancellationToken::new(),
            )
            .await,
            Err(TransferError::DestinationExists)
        ));
        assert_eq!(
            fs::read(destination.join(&offer.file_name))
                .await
                .unwrap_or_else(|error| panic!("read existing: {error}")),
            b"existing bytes"
        );
        assert_directory_empty(&staging).await;
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn preexisting_staging_entry_is_never_removed() {
        let root = test_directory("staging-substitution");
        let staging = root.join("attacker-owned.part");
        let destination = root.join("received.txt");
        fs::create_dir_all(&root)
            .await
            .unwrap_or_else(|error| panic!("root: {error}"));
        fs::write(&staging, b"must survive")
            .await
            .unwrap_or_else(|error| panic!("preexisting staging: {error}"));
        let (offer, records) = offer_and_records(b"new bytes", 4);
        let binding = TlsChannelBinding::new([0x25; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: records,
            sent: Vec::new(),
            send_finished: false,
            trailing_data: false,
        };
        assert!(matches!(
            receive_file_data_at_paths(
                &mut data,
                &offer,
                staging.clone(),
                destination.clone(),
                &CancellationToken::new(),
                |_| {},
            )
            .await,
            Err(TransferError::Staging)
        ));
        assert_eq!(
            fs::read(&staging)
                .await
                .unwrap_or_else(|error| panic!("read staging: {error}")),
            b"must survive"
        );
        assert!(fs::symlink_metadata(destination).await.is_err());
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn digest_failure_removes_private_partial_file() {
        let root = test_directory("integrity");
        let staging = root.join("staging");
        let destination = root.join("destination");
        fs::create_dir_all(&staging)
            .await
            .unwrap_or_else(|error| panic!("staging: {error}"));
        fs::create_dir_all(&destination)
            .await
            .unwrap_or_else(|error| panic!("destination: {error}"));
        let (offer, mut records) = offer_and_records(b"tampered", 8);
        records[0][28] ^= 1;
        let binding = TlsChannelBinding::new([0x33; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: records,
            sent: Vec::new(),
            send_finished: false,
            trailing_data: false,
        };
        assert!(matches!(
            receive_file_data(
                &mut data,
                binding,
                &offer,
                &staging,
                &destination,
                &CancellationToken::new(),
            )
            .await,
            Err(TransferError::Integrity)
        ));
        assert_directory_empty(&staging).await;
        assert!(
            fs::symlink_metadata(destination.join(&offer.file_name))
                .await
                .is_err()
        );
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn trailing_data_after_declared_file_removes_partial_file() {
        let root = test_directory("trailing-data");
        let staging = root.join("staging");
        let destination = root.join("destination");
        fs::create_dir_all(&staging)
            .await
            .unwrap_or_else(|error| panic!("staging: {error}"));
        fs::create_dir_all(&destination)
            .await
            .unwrap_or_else(|error| panic!("destination: {error}"));
        let (offer, records) = offer_and_records(b"declared bytes", 8);
        let binding = TlsChannelBinding::new([0x35; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: records,
            sent: Vec::new(),
            send_finished: false,
            trailing_data: true,
        };
        assert!(matches!(
            receive_file_data(
                &mut data,
                binding,
                &offer,
                &staging,
                &destination,
                &CancellationToken::new(),
            )
            .await,
            Err(TransferError::Data(DataIoError::TrailingData))
        ));
        assert_directory_empty(&staging).await;
        assert!(
            fs::symlink_metadata(destination.join(&offer.file_name))
                .await
                .is_err()
        );
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn prepared_source_is_hashed_and_sent_as_bounded_records() {
        let root = test_directory("send");
        fs::create_dir_all(&root)
            .await
            .unwrap_or_else(|error| panic!("root: {error}"));
        let source = root.join("source.txt");
        let content = vec![0x7a; DEFAULT_CHUNK_SIZE as usize + 3];
        fs::write(&source, &content)
            .await
            .unwrap_or_else(|error| panic!("source: {error}"));
        let cancellation = CancellationToken::new();
        let prepared = prepare_file(&source, None, &cancellation)
            .await
            .unwrap_or_else(|error| panic!("prepare: {error}"));
        assert_eq!(prepared.offer.file_size, content.len() as u64);
        let expected_digest: [u8; CONTENT_DIGEST_LEN] = Sha256::digest(&content).into();
        assert_eq!(prepared.offer.file_digest, expected_digest);
        let binding = TlsChannelBinding::new([0x55; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: VecDeque::new(),
            sent: Vec::new(),
            send_finished: false,
            trailing_data: false,
        };
        send_file_data(&mut data, binding, &prepared, &cancellation)
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
        assert!(data.send_finished);
        assert_eq!(data.sent.len(), 2);
        assert_eq!(
            data.sent
                .iter()
                .map(|record| TransferChunk::decode(record)
                    .unwrap_or_else(|error| panic!("decode: {error}"))
                    .payload
                    .len())
                .sum::<usize>(),
            content.len()
        );
        let _ = fs::remove_dir_all(root).await;
    }
}
