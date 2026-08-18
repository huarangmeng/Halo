use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use getrandom::fill;
use halo_crypto::TlsChannelBinding;
use halo_protocol::{
    BATCH_MANIFEST_DIGEST_LEN, BatchCancel, BatchCancelReason, BatchChunkRef, BatchComplete,
    BatchDecision, BatchPause, BatchPauseReason, CONTENT_DIGEST_LEN, DEFAULT_CHUNK_SIZE,
    MAX_BATCH_FILES, ManifestFile, ResumePosition, TRANSFER_ID_LEN, TransferManifest,
    TransferMessage, TransferProtocolError,
};
use halo_transport::{ControlIo, DataIo, DataIoError, FrameIoError};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
};
use tokio_util::sync::CancellationToken;

use crate::validate_file_name;

const RESUME_MAGIC: &[u8; 4] = b"HRS1";
const RESUME_VERSION: u16 = 1;
const RESUME_FIXED_LEN: usize = 4 + 2 + 32 + TRANSFER_ID_LEN + 32 + 4 + 1 + 3;
const RESUME_FILE_LEN: usize = 4 + 32;
const RESUME_CHECKSUM_LEN: usize = 32;
const SEND_JOB_MAGIC: &[u8; 4] = b"HSJ1";
const SEND_JOB_VERSION: u16 = 1;
const MAX_SOURCE_PATH_LEN: usize = 4096;
const MAX_SEND_JOB_LEN: usize = 64 * 1024;
const MAX_SEND_JOBS_PER_PEER: usize = 32;
const RESUME_CHECKPOINT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchSource {
    pub source_path: PathBuf,
    pub advertised_name: Option<String>,
}

impl BatchSource {
    #[must_use]
    pub fn new(source_path: impl Into<PathBuf>, advertised_name: Option<String>) -> Self {
        Self {
            source_path: source_path.into(),
            advertised_name,
        }
    }
}

#[derive(Clone, Debug)]
struct PreparedBatchFile {
    source_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct PreparedBatch {
    manifest: TransferManifest,
    files: Vec<PreparedBatchFile>,
}

impl PreparedBatch {
    #[must_use]
    pub fn manifest(&self) -> &TransferManifest {
        &self.manifest
    }

    #[must_use]
    pub fn source_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|file| file.source_path.clone())
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct BatchSendJobStore {
    root: PathBuf,
}

impl BatchSendJobStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub async fn persist(
        &self,
        peer_id: [u8; 32],
        prepared: &PreparedBatch,
    ) -> Result<(), BatchTransferError> {
        ensure_directory(&self.root).await?;
        let bytes = encode_send_job(peer_id, prepared)?;
        let mut random = [0_u8; 8];
        fill(&mut random).map_err(|_| BatchTransferError::Randomness)?;
        let temporary = self.root.join(format!(
            ".send-job-{}.tmp",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
        let final_path = self.path(peer_id, prepared.manifest.transfer_id);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await
            .map_err(|_| BatchTransferError::SendJob)?;
        let result = async {
            file.write_all(&bytes)
                .await
                .map_err(|_| BatchTransferError::SendJob)?;
            file.sync_all()
                .await
                .map_err(|_| BatchTransferError::SendJob)?;
            fs::rename(&temporary, final_path)
                .await
                .map_err(|_| BatchTransferError::SendJob)
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(temporary).await;
        }
        result
    }

    pub async fn load(
        &self,
        peer_id: [u8; 32],
        transfer_id: [u8; TRANSFER_ID_LEN],
    ) -> Result<Option<PreparedBatch>, BatchTransferError> {
        ensure_directory(&self.root).await?;
        let bytes = match fs::read(self.path(peer_id, transfer_id)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(BatchTransferError::SendJob),
        };
        decode_send_job(&bytes, peer_id, transfer_id).map(Some)
    }

    pub async fn list(&self, peer_id: [u8; 32]) -> Result<Vec<PreparedBatch>, BatchTransferError> {
        ensure_directory(&self.root).await?;
        let prefix = format!("{}-", binding_text(peer_id));
        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|_| BatchTransferError::SendJob)?;
        let mut jobs = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| BatchTransferError::SendJob)?
        {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(&prefix) || !name.ends_with(".send") {
                continue;
            }
            if jobs.len() >= MAX_SEND_JOBS_PER_PEER {
                return Err(BatchTransferError::SendJob);
            }
            let file_type = entry
                .file_type()
                .await
                .map_err(|_| BatchTransferError::SendJob)?;
            let metadata = entry
                .metadata()
                .await
                .map_err(|_| BatchTransferError::SendJob)?;
            if !file_type.is_file()
                || file_type.is_symlink()
                || metadata.len() as usize > MAX_SEND_JOB_LEN
            {
                return Err(BatchTransferError::SendJob);
            }
            let bytes = fs::read(entry.path())
                .await
                .map_err(|_| BatchTransferError::SendJob)?;
            if bytes.len() < 54 {
                return Err(BatchTransferError::SendJob);
            }
            let transfer_id = read_array::<TRANSFER_ID_LEN>(&bytes, 38);
            jobs.push(decode_send_job(&bytes, peer_id, transfer_id)?);
        }
        jobs.sort_by_key(|job| job.manifest.transfer_id);
        Ok(jobs)
    }

    pub async fn remove(
        &self,
        peer_id: [u8; 32],
        transfer_id: [u8; TRANSFER_ID_LEN],
    ) -> Result<(), BatchTransferError> {
        ensure_directory(&self.root).await?;
        match fs::remove_file(self.path(peer_id, transfer_id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(BatchTransferError::SendJob),
        }
    }

    fn path(&self, peer_id: [u8; 32], transfer_id: [u8; TRANSFER_ID_LEN]) -> PathBuf {
        self.root.join(format!(
            "{}-{}.send",
            binding_text(peer_id),
            transfer_id_text(transfer_id)
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedBatch {
    pub transfer_id: [u8; TRANSFER_ID_LEN],
    pub final_paths: Vec<PathBuf>,
    pub aggregate_size: u64,
    pub manifest_digest: [u8; BATCH_MANIFEST_DIGEST_LEN],
}

#[derive(Clone, Debug)]
pub struct BatchResumeStore {
    root: PathBuf,
    peer_id: [u8; 32],
}

impl BatchResumeStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, peer_id: [u8; 32]) -> Self {
        Self {
            root: root.into(),
            peer_id,
        }
    }

    pub async fn discard(&self, manifest: &TransferManifest) -> Result<(), BatchTransferError> {
        ensure_directory(&self.root).await?;
        let state_path = self.state_path(manifest.transfer_id);
        let state = match self.load_state(manifest).await {
            Ok(Some(state)) => state,
            Ok(None) => return Ok(()),
            Err(error) => return Err(error),
        };
        for index in 0..state.next_chunks.len() {
            let path = self.partial_path(manifest.transfer_id, index)?;
            match fs::symlink_metadata(&path).await {
                Ok(metadata)
                    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() =>
                {
                    fs::remove_file(path)
                        .await
                        .map_err(|_| BatchTransferError::Storage)?;
                }
                Ok(_) => return Err(BatchTransferError::ResumeState),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(BatchTransferError::Storage),
            }
        }
        match fs::remove_file(state_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(BatchTransferError::Storage),
        }
    }

    async fn load_or_create(
        &self,
        manifest: &TransferManifest,
    ) -> Result<ResumeState, BatchTransferError> {
        ensure_directory(&self.root).await?;
        if let Some(state) = self.load_state(manifest).await? {
            self.verify_partials(manifest, &state).await?;
            return Ok(state);
        }
        let state = ResumeState {
            next_chunks: vec![0; manifest.files.len()],
            chains: vec![[0; 32]; manifest.files.len()],
        };
        self.persist_state(manifest, &state).await?;
        Ok(state)
    }

    async fn load_state(
        &self,
        manifest: &TransferManifest,
    ) -> Result<Option<ResumeState>, BatchTransferError> {
        let bytes = match fs::read(self.state_path(manifest.transfer_id)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(BatchTransferError::ResumeState),
        };
        decode_resume_state(&bytes, self.peer_id, manifest).map(Some)
    }

    async fn persist_state(
        &self,
        manifest: &TransferManifest,
        state: &ResumeState,
    ) -> Result<(), BatchTransferError> {
        let bytes = encode_resume_state(self.peer_id, manifest, state)?;
        let mut random = [0_u8; 8];
        fill(&mut random).map_err(|_| BatchTransferError::Randomness)?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let temporary = self.root.join(format!(".resume-{suffix}.tmp"));
        let final_path = self.state_path(manifest.transfer_id);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await
            .map_err(|_| BatchTransferError::ResumeState)?;
        let result = async {
            file.write_all(&bytes)
                .await
                .map_err(|_| BatchTransferError::ResumeState)?;
            file.sync_all()
                .await
                .map_err(|_| BatchTransferError::ResumeState)?;
            fs::rename(&temporary, &final_path)
                .await
                .map_err(|_| BatchTransferError::ResumeState)
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&temporary).await;
        }
        result
    }

    async fn verify_partials(
        &self,
        manifest: &TransferManifest,
        state: &ResumeState,
    ) -> Result<(), BatchTransferError> {
        for (index, file) in manifest.files.iter().enumerate() {
            let next_chunk = state.next_chunks[index];
            validate_resume_position(file.file_size, manifest.chunk_size, next_chunk)?;
            let path = self.partial_path(manifest.transfer_id, index)?;
            if next_chunk == 0 {
                if let Ok(metadata) = fs::symlink_metadata(&path).await {
                    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                        return Err(BatchTransferError::ResumeState);
                    }
                    OpenOptions::new()
                        .write(true)
                        .open(path)
                        .await
                        .map_err(|_| BatchTransferError::ResumeState)?
                        .set_len(0)
                        .await
                        .map_err(|_| BatchTransferError::ResumeState)?;
                }
                if state.chains[index] != [0; 32] {
                    return Err(BatchTransferError::ResumeState);
                }
                continue;
            }
            let metadata = fs::symlink_metadata(&path)
                .await
                .map_err(|_| BatchTransferError::ResumeState)?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(BatchTransferError::ResumeState);
            }
            let expected_length =
                durable_prefix_length(file.file_size, manifest.chunk_size, next_chunk)?;
            if metadata.len() < expected_length {
                return Err(BatchTransferError::ResumeState);
            }
            if metadata.len() > expected_length {
                OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .await
                    .map_err(|_| BatchTransferError::ResumeState)?
                    .set_len(expected_length)
                    .await
                    .map_err(|_| BatchTransferError::ResumeState)?;
            }
            let (chain, digest) = hash_partial(
                &path,
                index,
                next_chunk,
                manifest.chunk_size,
                file.file_size,
            )
            .await?;
            if chain != state.chains[index] {
                return Err(BatchTransferError::ResumeMismatch);
            }
            if next_chunk == chunk_count(file.file_size, manifest.chunk_size)?
                && digest != file.file_digest
            {
                return Err(BatchTransferError::ResumeMismatch);
            }
        }
        Ok(())
    }

    fn state_path(&self, transfer_id: [u8; TRANSFER_ID_LEN]) -> PathBuf {
        self.root.join(format!(
            "{}-{}.resume",
            binding_text(self.peer_id),
            transfer_id_text(transfer_id)
        ))
    }

    fn partial_path(
        &self,
        transfer_id: [u8; TRANSFER_ID_LEN],
        file_index: usize,
    ) -> Result<PathBuf, BatchTransferError> {
        if file_index >= MAX_BATCH_FILES {
            return Err(BatchTransferError::ResumeState);
        }
        Ok(self.root.join(format!(
            ".halo-{}-{}-{file_index}.part",
            binding_text(self.peer_id),
            transfer_id_text(transfer_id)
        )))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResumeState {
    next_chunks: Vec<u32>,
    chains: Vec<[u8; 32]>,
}

pub async fn prepare_batch(
    sources: Vec<BatchSource>,
    cancellation: &CancellationToken,
) -> Result<PreparedBatch, BatchTransferError> {
    if !(1..=MAX_BATCH_FILES).contains(&sources.len()) {
        return Err(BatchTransferError::InvalidSourceCount(sources.len()));
    }
    let mut transfer_id = [0_u8; TRANSFER_ID_LEN];
    fill(&mut transfer_id).map_err(|_| BatchTransferError::Randomness)?;
    prepare_batch_with_id(transfer_id, sources, cancellation).await
}

pub async fn prepare_batch_with_id(
    transfer_id: [u8; TRANSFER_ID_LEN],
    sources: Vec<BatchSource>,
    cancellation: &CancellationToken,
) -> Result<PreparedBatch, BatchTransferError> {
    if !(1..=MAX_BATCH_FILES).contains(&sources.len()) {
        return Err(BatchTransferError::InvalidSourceCount(sources.len()));
    }
    let mut names = HashSet::new();
    let mut manifest_files = Vec::with_capacity(sources.len());
    let mut prepared_files = Vec::with_capacity(sources.len());
    for source in sources {
        if cancellation.is_cancelled() {
            return Err(BatchTransferError::Paused);
        }
        let metadata = fs::symlink_metadata(&source.source_path)
            .await
            .map_err(|_| BatchTransferError::Source)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(BatchTransferError::Source);
        }
        let file_name = source.advertised_name.unwrap_or_else(|| {
            source
                .source_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned()
        });
        validate_file_name(&file_name).map_err(|_| BatchTransferError::InvalidFileName)?;
        if !names.insert(file_name.to_ascii_lowercase()) {
            return Err(BatchTransferError::DuplicateFileName);
        }
        let digest = hash_file(&source.source_path, cancellation).await?;
        manifest_files.push(ManifestFile::new(metadata.len(), digest, file_name)?);
        prepared_files.push(PreparedBatchFile {
            source_path: source.source_path,
        });
    }
    let manifest = TransferManifest::new(transfer_id, DEFAULT_CHUNK_SIZE, manifest_files)?;
    Ok(PreparedBatch {
        manifest,
        files: prepared_files,
    })
}

pub async fn send_manifest(
    control: &mut dyn ControlIo,
    prepared: &PreparedBatch,
) -> Result<Vec<ResumePosition>, BatchTransferError> {
    control
        .send_frame(&TransferMessage::Offer(prepared.manifest.clone()).encode()?)
        .await?;
    match TransferMessage::decode(&control.receive_frame(4096).await?)? {
        TransferMessage::Decision(decision)
            if decision.transfer_id == prepared.manifest.transfer_id
                && decision.manifest_digest == prepared.manifest.digest() =>
        {
            if !decision.accepted {
                return Err(BatchTransferError::Rejected);
            }
            validate_positions(&prepared.manifest, &decision.resume_positions)?;
            Ok(decision.resume_positions)
        }
        TransferMessage::Cancel(cancel) if cancel.transfer_id == prepared.manifest.transfer_id => {
            Err(BatchTransferError::RemoteCancelled(cancel.reason))
        }
        TransferMessage::Pause(pause)
            if pause.transfer_id == prepared.manifest.transfer_id
                && pause.manifest_digest == prepared.manifest.digest() =>
        {
            Err(BatchTransferError::RemotePaused(pause.reason))
        }
        _ => Err(BatchTransferError::UnexpectedMessage),
    }
}

pub async fn receive_manifest(
    control: &mut dyn ControlIo,
) -> Result<TransferManifest, BatchTransferError> {
    match TransferMessage::decode(&control.receive_frame(4096).await?)? {
        TransferMessage::Offer(manifest) => {
            let mut names = HashSet::new();
            for file in &manifest.files {
                validate_file_name(&file.file_name)
                    .map_err(|_| BatchTransferError::InvalidFileName)?;
                if !names.insert(file.file_name.to_ascii_lowercase()) {
                    return Err(BatchTransferError::DuplicateFileName);
                }
            }
            Ok(manifest)
        }
        _ => Err(BatchTransferError::UnexpectedMessage),
    }
}

pub async fn send_batch_decision(
    control: &mut dyn ControlIo,
    manifest: &TransferManifest,
    accepted: bool,
    resume_positions: Vec<ResumePosition>,
) -> Result<(), BatchTransferError> {
    if accepted {
        validate_positions(manifest, &resume_positions)?;
    }
    let decision = BatchDecision::new(
        manifest.transfer_id,
        manifest.digest(),
        accepted,
        resume_positions,
    )?;
    control
        .send_frame(&TransferMessage::Decision(decision).encode()?)
        .await?;
    Ok(())
}

pub async fn send_batch_cancel(
    control: &mut dyn ControlIo,
    manifest: &TransferManifest,
    reason: BatchCancelReason,
) -> Result<(), BatchTransferError> {
    control
        .send_frame(
            &TransferMessage::Cancel(BatchCancel {
                transfer_id: manifest.transfer_id,
                reason,
            })
            .encode()?,
        )
        .await?;
    Ok(())
}

pub async fn send_batch_pause(
    control: &mut dyn ControlIo,
    manifest: &TransferManifest,
    reason: BatchPauseReason,
) -> Result<(), BatchTransferError> {
    control
        .send_frame(
            &TransferMessage::Pause(BatchPause {
                transfer_id: manifest.transfer_id,
                manifest_digest: manifest.digest(),
                reason,
            })
            .encode()?,
        )
        .await?;
    Ok(())
}

pub async fn send_batch_complete(
    control: &mut dyn ControlIo,
    manifest: &TransferManifest,
) -> Result<(), BatchTransferError> {
    control
        .send_frame(
            &TransferMessage::Complete(BatchComplete {
                transfer_id: manifest.transfer_id,
                manifest_digest: manifest.digest(),
            })
            .encode()?,
        )
        .await?;
    Ok(())
}

pub async fn wait_for_batch_complete(
    control: &mut dyn ControlIo,
    manifest: &TransferManifest,
) -> Result<(), BatchTransferError> {
    match TransferMessage::decode(&control.receive_frame(4096).await?)? {
        TransferMessage::Complete(complete)
            if complete.transfer_id == manifest.transfer_id
                && complete.manifest_digest == manifest.digest() =>
        {
            Ok(())
        }
        TransferMessage::Cancel(cancel) if cancel.transfer_id == manifest.transfer_id => {
            Err(BatchTransferError::RemoteCancelled(cancel.reason))
        }
        TransferMessage::Pause(pause)
            if pause.transfer_id == manifest.transfer_id
                && pause.manifest_digest == manifest.digest() =>
        {
            Err(BatchTransferError::RemotePaused(pause.reason))
        }
        _ => Err(BatchTransferError::UnexpectedMessage),
    }
}

pub async fn send_batch_data_with_progress(
    data: &mut dyn DataIo,
    expected_binding: TlsChannelBinding,
    prepared: &PreparedBatch,
    resume_positions: &[ResumePosition],
    cancellation: &CancellationToken,
    mut progress: impl FnMut(usize, u64, u64) + Send,
) -> Result<(), BatchTransferError> {
    if data.channel_binding() != expected_binding {
        return Err(BatchTransferError::ChannelBinding);
    }
    validate_positions(&prepared.manifest, resume_positions)?;
    for (index, (prepared_file, manifest_file)) in prepared
        .files
        .iter()
        .zip(&prepared.manifest.files)
        .enumerate()
    {
        verify_source_metadata(prepared_file, manifest_file).await?;
        let next_chunk = resume_positions[index].next_chunk_index;
        let offset = durable_prefix_length(
            manifest_file.file_size,
            prepared.manifest.chunk_size,
            next_chunk,
        )?;
        let mut file = File::open(&prepared_file.source_path)
            .await
            .map_err(|_| BatchTransferError::Source)?;
        let mut payload = vec![0_u8; prepared.manifest.chunk_size as usize];
        let mut whole_digest = Sha256::new();
        let mut prefix_remaining = offset;
        while prefix_remaining > 0 {
            if cancellation.is_cancelled() {
                return Err(BatchTransferError::Paused);
            }
            let length = usize::try_from(
                prefix_remaining.min(u64::try_from(payload.len()).unwrap_or(u64::MAX)),
            )
            .map_err(|_| BatchTransferError::SourceChanged)?;
            file.read_exact(&mut payload[..length])
                .await
                .map_err(|_| BatchTransferError::SourceChanged)?;
            whole_digest.update(&payload[..length]);
            prefix_remaining -=
                u64::try_from(length).map_err(|_| BatchTransferError::SourceChanged)?;
        }
        let mut chunk_index = next_chunk;
        let mut transferred = offset;
        let mut record = Vec::with_capacity(
            halo_protocol::DATA_RECORD_HEADER_LEN + prepared.manifest.chunk_size as usize,
        );
        while transferred < manifest_file.file_size {
            if cancellation.is_cancelled() {
                return Err(BatchTransferError::Paused);
            }
            let length = usize::try_from(
                (manifest_file.file_size - transferred)
                    .min(u64::from(prepared.manifest.chunk_size)),
            )
            .map_err(|_| BatchTransferError::SourceChanged)?;
            file.read_exact(&mut payload[..length])
                .await
                .map_err(|_| BatchTransferError::SourceChanged)?;
            whole_digest.update(&payload[..length]);
            let chunk_digest = Sha256::digest(&payload[..length]).into();
            BatchChunkRef::new(
                prepared.manifest.transfer_id,
                u16::try_from(index).map_err(|_| BatchTransferError::ProtocolState)?,
                chunk_index,
                chunk_digest,
                &payload[..length],
            )?
            .encode_into(&mut record)?;
            tokio::select! {
                () = cancellation.cancelled() => return Err(BatchTransferError::Paused),
                result = data.send_record(&record) => result?,
            }
            transferred += u64::try_from(length).map_err(|_| BatchTransferError::SourceChanged)?;
            chunk_index = chunk_index
                .checked_add(1)
                .ok_or(BatchTransferError::ProtocolState)?;
            progress(index, transferred, manifest_file.file_size);
        }
        let mut extra = [0_u8; 1];
        if file
            .read(&mut extra)
            .await
            .map_err(|_| BatchTransferError::Source)?
            != 0
            || <[u8; CONTENT_DIGEST_LEN]>::from(whole_digest.finalize())
                != manifest_file.file_digest
        {
            return Err(BatchTransferError::SourceChanged);
        }
    }
    data.finish_send().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn receive_batch_data_with_progress(
    data: &mut dyn DataIo,
    expected_binding: TlsChannelBinding,
    manifest: &TransferManifest,
    resume_store: &BatchResumeStore,
    destination_directory: &Path,
    cancellation: &CancellationToken,
    mut progress: impl FnMut(usize, u64, u64) + Send,
) -> Result<ReceivedBatch, BatchTransferError> {
    if data.channel_binding() != expected_binding {
        return Err(BatchTransferError::ChannelBinding);
    }
    ensure_directory(destination_directory).await?;
    let mut state = resume_store.load_or_create(manifest).await?;
    for (index, file) in manifest.files.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(BatchTransferError::Paused);
        }
        let partial_path = resume_store.partial_path(manifest.transfer_id, index)?;
        let mut partial = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&partial_path)
            .await
            .map_err(|_| BatchTransferError::Storage)?;
        let next_chunk = state.next_chunks[index];
        let mut transferred =
            durable_prefix_length(file.file_size, manifest.chunk_size, next_chunk)?;
        let mut whole_digest = hash_file_prefix(&partial_path, transferred, cancellation).await?;
        partial
            .seek(SeekFrom::Start(transferred))
            .await
            .map_err(|_| BatchTransferError::Storage)?;
        let mut chunk_index = next_chunk;
        let mut checkpointed = transferred;
        while transferred < file.file_size {
            let record = tokio::select! {
                () = cancellation.cancelled() => {
                    if transferred > checkpointed {
                        persist_resume_checkpoint(
                            &mut partial,
                            resume_store,
                            manifest,
                            &state,
                        )
                        .await?;
                    }
                    return Err(BatchTransferError::Paused);
                },
                result = data.receive_record() => match result {
                    Ok(record) => record,
                    Err(error) => {
                        if transferred > checkpointed {
                            persist_resume_checkpoint(
                                &mut partial,
                                resume_store,
                                manifest,
                                &state,
                            )
                            .await?;
                        }
                        return Err(error.into());
                    }
                },
            };
            let chunk = BatchChunkRef::decode(&record)?;
            let expected_length =
                usize::try_from((file.file_size - transferred).min(u64::from(manifest.chunk_size)))
                    .map_err(|_| BatchTransferError::Integrity)?;
            if chunk.transfer_id != manifest.transfer_id
                || usize::from(chunk.file_index) != index
                || chunk.chunk_index != chunk_index
                || chunk.payload.len() != expected_length
                || <[u8; CONTENT_DIGEST_LEN]>::from(Sha256::digest(chunk.payload))
                    != chunk.chunk_digest
            {
                return Err(BatchTransferError::Integrity);
            }
            whole_digest.update(chunk.payload);
            partial
                .write_all(chunk.payload)
                .await
                .map_err(|_| BatchTransferError::Storage)?;
            state.chains[index] =
                next_chain(state.chains[index], index, chunk_index, chunk.chunk_digest)?;
            chunk_index = chunk_index
                .checked_add(1)
                .ok_or(BatchTransferError::Integrity)?;
            state.next_chunks[index] = chunk_index;
            transferred +=
                u64::try_from(expected_length).map_err(|_| BatchTransferError::Integrity)?;
            if checkpoint_due(checkpointed, transferred, transferred == file.file_size) {
                persist_resume_checkpoint(&mut partial, resume_store, manifest, &state).await?;
                checkpointed = transferred;
            }
            progress(index, transferred, file.file_size);
        }
        partial
            .flush()
            .await
            .map_err(|_| BatchTransferError::Storage)?;
        partial
            .sync_all()
            .await
            .map_err(|_| BatchTransferError::Storage)?;
        drop(partial);
        if <[u8; CONTENT_DIGEST_LEN]>::from(whole_digest.finalize()) != file.file_digest {
            return Err(BatchTransferError::Integrity);
        }
    }
    data.expect_end().await?;
    finalize_batch(manifest, resume_store, destination_directory, &state).await
}

pub async fn resume_positions(
    manifest: &TransferManifest,
    resume_store: &BatchResumeStore,
) -> Result<Vec<ResumePosition>, BatchTransferError> {
    let state = resume_store.load_or_create(manifest).await?;
    state
        .next_chunks
        .iter()
        .enumerate()
        .map(|(index, next_chunk_index)| {
            Ok(ResumePosition {
                file_index: u16::try_from(index).map_err(|_| BatchTransferError::ProtocolState)?,
                next_chunk_index: *next_chunk_index,
            })
        })
        .collect()
}

async fn finalize_batch(
    manifest: &TransferManifest,
    resume_store: &BatchResumeStore,
    destination_directory: &Path,
    state: &ResumeState,
) -> Result<ReceivedBatch, BatchTransferError> {
    for (index, file) in manifest.files.iter().enumerate() {
        if state.next_chunks[index] != chunk_count(file.file_size, manifest.chunk_size)? {
            return Err(BatchTransferError::Integrity);
        }
        if fs::symlink_metadata(destination_directory.join(&file.file_name))
            .await
            .is_ok()
        {
            return Err(BatchTransferError::DestinationExists);
        }
    }
    let mut created = Vec::with_capacity(manifest.files.len());
    for (index, file) in manifest.files.iter().enumerate() {
        let partial = resume_store.partial_path(manifest.transfer_id, index)?;
        let final_path = destination_directory.join(&file.file_name);
        if fs::hard_link(&partial, &final_path).await.is_err() {
            rollback_created_links(&created).await;
            return Err(if fs::symlink_metadata(&final_path).await.is_ok() {
                BatchTransferError::DestinationExists
            } else {
                BatchTransferError::Finalization
            });
        }
        created.push((partial, final_path));
    }
    for (partial, _) in &created {
        let _ = fs::remove_file(partial).await;
    }
    let _ = fs::remove_file(resume_store.state_path(manifest.transfer_id)).await;
    Ok(ReceivedBatch {
        transfer_id: manifest.transfer_id,
        final_paths: created
            .into_iter()
            .map(|(_, final_path)| final_path)
            .collect(),
        aggregate_size: manifest.aggregate_size()?,
        manifest_digest: manifest.digest(),
    })
}

async fn rollback_created_links(created: &[(PathBuf, PathBuf)]) {
    for (partial, final_path) in created.iter().rev() {
        if paths_reference_same_file(partial, final_path).await {
            let _ = fs::remove_file(final_path).await;
        }
    }
}

#[cfg(unix)]
async fn paths_reference_same_file(first: &Path, second: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (
        fs::symlink_metadata(first).await,
        fs::symlink_metadata(second).await,
    ) {
        (Ok(first), Ok(second)) => first.dev() == second.dev() && first.ino() == second.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
async fn paths_reference_same_file(_first: &Path, _second: &Path) -> bool {
    false
}

async fn verify_source_metadata(
    prepared: &PreparedBatchFile,
    manifest: &ManifestFile,
) -> Result<(), BatchTransferError> {
    let metadata = fs::symlink_metadata(&prepared.source_path)
        .await
        .map_err(|_| BatchTransferError::Source)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() != manifest.file_size
    {
        return Err(BatchTransferError::SourceChanged);
    }
    Ok(())
}

fn checkpoint_due(checkpointed: u64, transferred: u64, file_complete: bool) -> bool {
    file_complete || transferred.saturating_sub(checkpointed) >= RESUME_CHECKPOINT_BYTES
}

async fn persist_resume_checkpoint(
    partial: &mut File,
    resume_store: &BatchResumeStore,
    manifest: &TransferManifest,
    state: &ResumeState,
) -> Result<(), BatchTransferError> {
    partial
        .flush()
        .await
        .map_err(|_| BatchTransferError::Storage)?;
    partial
        .sync_data()
        .await
        .map_err(|_| BatchTransferError::Storage)?;
    resume_store.persist_state(manifest, state).await
}

async fn hash_file(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<[u8; CONTENT_DIGEST_LEN], BatchTransferError> {
    let mut file = File::open(path)
        .await
        .map_err(|_| BatchTransferError::Source)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; DEFAULT_CHUNK_SIZE as usize];
    loop {
        if cancellation.is_cancelled() {
            return Err(BatchTransferError::Paused);
        }
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|_| BatchTransferError::Source)?;
        if read == 0 {
            return Ok(digest.finalize().into());
        }
        digest.update(&buffer[..read]);
    }
}

async fn hash_file_prefix(
    path: &Path,
    length: u64,
    cancellation: &CancellationToken,
) -> Result<Sha256, BatchTransferError> {
    let mut file = File::open(path)
        .await
        .map_err(|_| BatchTransferError::Storage)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; DEFAULT_CHUNK_SIZE as usize];
    let mut remaining = length;
    while remaining > 0 {
        if cancellation.is_cancelled() {
            return Err(BatchTransferError::Paused);
        }
        let read_length = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| BatchTransferError::Storage)?;
        file.read_exact(&mut buffer[..read_length])
            .await
            .map_err(|_| BatchTransferError::Storage)?;
        digest.update(&buffer[..read_length]);
        remaining -= u64::try_from(read_length).map_err(|_| BatchTransferError::Storage)?;
    }
    Ok(digest)
}

async fn hash_partial(
    path: &Path,
    file_index: usize,
    next_chunk: u32,
    chunk_size: u32,
    file_size: u64,
) -> Result<([u8; 32], [u8; 32]), BatchTransferError> {
    let mut file = File::open(path)
        .await
        .map_err(|_| BatchTransferError::ResumeState)?;
    let mut chain = [0_u8; 32];
    let mut whole = Sha256::new();
    let mut transferred = 0_u64;
    for chunk_index in 0..next_chunk {
        let length = usize::try_from((file_size - transferred).min(u64::from(chunk_size)))
            .map_err(|_| BatchTransferError::ResumeState)?;
        if length == 0 {
            return Err(BatchTransferError::ResumeState);
        }
        let mut payload = vec![0_u8; length];
        file.read_exact(&mut payload)
            .await
            .map_err(|_| BatchTransferError::ResumeState)?;
        let chunk_digest = Sha256::digest(&payload).into();
        chain = next_chain(chain, file_index, chunk_index, chunk_digest)?;
        whole.update(payload);
        transferred += u64::try_from(length).map_err(|_| BatchTransferError::ResumeState)?;
    }
    Ok((chain, whole.finalize().into()))
}

fn next_chain(
    previous: [u8; 32],
    file_index: usize,
    chunk_index: u32,
    chunk_digest: [u8; 32],
) -> Result<[u8; 32], BatchTransferError> {
    let file_index = u16::try_from(file_index).map_err(|_| BatchTransferError::ProtocolState)?;
    let mut digest = Sha256::new();
    digest.update(b"Halo Resume Chunk Chain v1");
    digest.update(previous);
    digest.update(file_index.to_be_bytes());
    digest.update(chunk_index.to_be_bytes());
    digest.update(chunk_digest);
    Ok(digest.finalize().into())
}

fn validate_positions(
    manifest: &TransferManifest,
    positions: &[ResumePosition],
) -> Result<(), BatchTransferError> {
    if positions.len() != manifest.files.len() {
        return Err(BatchTransferError::ResumeMismatch);
    }
    for (index, (position, file)) in positions.iter().zip(&manifest.files).enumerate() {
        if usize::from(position.file_index) != index {
            return Err(BatchTransferError::ResumeMismatch);
        }
        validate_resume_position(
            file.file_size,
            manifest.chunk_size,
            position.next_chunk_index,
        )?;
    }
    Ok(())
}

fn validate_resume_position(
    file_size: u64,
    chunk_size: u32,
    next_chunk: u32,
) -> Result<(), BatchTransferError> {
    if next_chunk > chunk_count(file_size, chunk_size)? {
        return Err(BatchTransferError::ResumeMismatch);
    }
    Ok(())
}

fn chunk_count(file_size: u64, chunk_size: u32) -> Result<u32, BatchTransferError> {
    if chunk_size == 0 {
        return Err(BatchTransferError::ProtocolState);
    }
    let count = file_size.div_ceil(u64::from(chunk_size));
    u32::try_from(count).map_err(|_| BatchTransferError::ProtocolState)
}

fn durable_prefix_length(
    file_size: u64,
    chunk_size: u32,
    next_chunk: u32,
) -> Result<u64, BatchTransferError> {
    validate_resume_position(file_size, chunk_size, next_chunk)?;
    Ok(u64::from(next_chunk)
        .saturating_mul(u64::from(chunk_size))
        .min(file_size))
}

async fn ensure_directory(path: &Path) -> Result<(), BatchTransferError> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|_| BatchTransferError::Storage)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(BatchTransferError::Storage)
    }
}

fn encode_send_job(
    peer_id: [u8; 32],
    prepared: &PreparedBatch,
) -> Result<Vec<u8>, BatchTransferError> {
    if prepared.files.len() != prepared.manifest.files.len()
        || prepared.files.is_empty()
        || prepared.files.len() > MAX_BATCH_FILES
    {
        return Err(BatchTransferError::SendJob);
    }
    let manifest = TransferMessage::Offer(prepared.manifest.clone()).encode()?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(SEND_JOB_MAGIC);
    bytes.extend_from_slice(&SEND_JOB_VERSION.to_be_bytes());
    bytes.extend_from_slice(&peer_id);
    bytes.extend_from_slice(&prepared.manifest.transfer_id);
    bytes.extend_from_slice(
        &u32::try_from(manifest.len())
            .map_err(|_| BatchTransferError::SendJob)?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&manifest);
    bytes.push(u8::try_from(prepared.files.len()).map_err(|_| BatchTransferError::SendJob)?);
    for file in &prepared.files {
        let path = file
            .source_path
            .to_str()
            .filter(|path| !path.is_empty())
            .ok_or(BatchTransferError::SendJob)?;
        if !file.source_path.is_absolute() || path.len() > MAX_SOURCE_PATH_LEN {
            return Err(BatchTransferError::SendJob);
        }
        bytes.extend_from_slice(
            &u16::try_from(path.len())
                .map_err(|_| BatchTransferError::SendJob)?
                .to_be_bytes(),
        );
        bytes.extend_from_slice(path.as_bytes());
    }
    if bytes.len().saturating_add(32) > MAX_SEND_JOB_LEN {
        return Err(BatchTransferError::SendJob);
    }
    let checksum = send_job_checksum(&bytes);
    bytes.extend_from_slice(&checksum);
    Ok(bytes)
}

fn decode_send_job(
    bytes: &[u8],
    peer_id: [u8; 32],
    transfer_id: [u8; TRANSFER_ID_LEN],
) -> Result<PreparedBatch, BatchTransferError> {
    const FIXED_LEN: usize = 4 + 2 + 32 + TRANSFER_ID_LEN + 4;
    if bytes.len() < FIXED_LEN + 1 + 32
        || bytes.len() > MAX_SEND_JOB_LEN
        || &bytes[..4] != SEND_JOB_MAGIC
        || read_u16(bytes, 4) != SEND_JOB_VERSION
        || read_array::<32>(bytes, 6) != peer_id
        || read_array::<TRANSFER_ID_LEN>(bytes, 38) != transfer_id
    {
        return Err(BatchTransferError::SendJob);
    }
    let checksum_offset = bytes.len() - 32;
    if send_job_checksum(&bytes[..checksum_offset]) != read_array::<32>(bytes, checksum_offset) {
        return Err(BatchTransferError::SendJob);
    }
    let manifest_length = read_u32(bytes, 54) as usize;
    let manifest_end = FIXED_LEN
        .checked_add(manifest_length)
        .ok_or(BatchTransferError::SendJob)?;
    if manifest_end >= checksum_offset {
        return Err(BatchTransferError::SendJob);
    }
    let manifest = match TransferMessage::decode(&bytes[FIXED_LEN..manifest_end])? {
        TransferMessage::Offer(manifest) if manifest.transfer_id == transfer_id => manifest,
        _ => return Err(BatchTransferError::SendJob),
    };
    let source_count = bytes[manifest_end] as usize;
    if source_count != manifest.files.len() || !(1..=MAX_BATCH_FILES).contains(&source_count) {
        return Err(BatchTransferError::SendJob);
    }
    let mut offset = manifest_end + 1;
    let mut files = Vec::with_capacity(source_count);
    for _ in 0..source_count {
        let length_end = offset.checked_add(2).ok_or(BatchTransferError::SendJob)?;
        if length_end > checksum_offset {
            return Err(BatchTransferError::SendJob);
        }
        let path_length = read_u16(bytes, offset) as usize;
        if path_length == 0 || path_length > MAX_SOURCE_PATH_LEN {
            return Err(BatchTransferError::SendJob);
        }
        let path_end = length_end
            .checked_add(path_length)
            .ok_or(BatchTransferError::SendJob)?;
        if path_end > checksum_offset {
            return Err(BatchTransferError::SendJob);
        }
        let path = std::str::from_utf8(&bytes[length_end..path_end])
            .map_err(|_| BatchTransferError::SendJob)?;
        let source_path = PathBuf::from(path);
        if !source_path.is_absolute() {
            return Err(BatchTransferError::SendJob);
        }
        files.push(PreparedBatchFile { source_path });
        offset = path_end;
    }
    if offset != checksum_offset {
        return Err(BatchTransferError::SendJob);
    }
    for file in &manifest.files {
        validate_file_name(&file.file_name).map_err(|_| BatchTransferError::SendJob)?;
    }
    Ok(PreparedBatch { manifest, files })
}

fn send_job_checksum(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"Halo Sender Job v1");
    digest.update(bytes);
    digest.finalize().into()
}

fn encode_resume_state(
    peer_id: [u8; 32],
    manifest: &TransferManifest,
    state: &ResumeState,
) -> Result<Vec<u8>, BatchTransferError> {
    if state.next_chunks.len() != manifest.files.len()
        || state.chains.len() != manifest.files.len()
        || state.next_chunks.len() > MAX_BATCH_FILES
    {
        return Err(BatchTransferError::ResumeState);
    }
    let file_count =
        u8::try_from(state.next_chunks.len()).map_err(|_| BatchTransferError::ResumeState)?;
    let mut bytes = Vec::with_capacity(
        RESUME_FIXED_LEN + state.next_chunks.len() * RESUME_FILE_LEN + RESUME_CHECKSUM_LEN,
    );
    bytes.extend_from_slice(RESUME_MAGIC);
    bytes.extend_from_slice(&RESUME_VERSION.to_be_bytes());
    bytes.extend_from_slice(&peer_id);
    bytes.extend_from_slice(&manifest.transfer_id);
    bytes.extend_from_slice(&manifest.digest());
    bytes.extend_from_slice(&manifest.chunk_size.to_be_bytes());
    bytes.push(file_count);
    bytes.extend_from_slice(&[0; 3]);
    for (next_chunk, chain) in state.next_chunks.iter().zip(&state.chains) {
        bytes.extend_from_slice(&next_chunk.to_be_bytes());
        bytes.extend_from_slice(chain);
    }
    let checksum = resume_checksum(&bytes);
    bytes.extend_from_slice(&checksum);
    Ok(bytes)
}

fn decode_resume_state(
    bytes: &[u8],
    peer_id: [u8; 32],
    manifest: &TransferManifest,
) -> Result<ResumeState, BatchTransferError> {
    if bytes.len() < RESUME_FIXED_LEN + RESUME_CHECKSUM_LEN
        || &bytes[..4] != RESUME_MAGIC
        || read_u16(bytes, 4) != RESUME_VERSION
        || read_array::<32>(bytes, 6) != peer_id
        || read_array::<TRANSFER_ID_LEN>(bytes, 38) != manifest.transfer_id
        || read_array::<32>(bytes, 54) != manifest.digest()
        || read_u32(bytes, 86) != manifest.chunk_size
        || bytes[91..94] != [0; 3]
    {
        return Err(BatchTransferError::ResumeMismatch);
    }
    let file_count = bytes[90] as usize;
    if file_count != manifest.files.len() || file_count > MAX_BATCH_FILES {
        return Err(BatchTransferError::ResumeMismatch);
    }
    let expected = RESUME_FIXED_LEN
        .checked_add(
            file_count
                .checked_mul(RESUME_FILE_LEN)
                .ok_or(BatchTransferError::ResumeState)?,
        )
        .and_then(|length| length.checked_add(RESUME_CHECKSUM_LEN))
        .ok_or(BatchTransferError::ResumeState)?;
    if bytes.len() != expected {
        return Err(BatchTransferError::ResumeState);
    }
    let checksum_offset = bytes.len() - RESUME_CHECKSUM_LEN;
    if resume_checksum(&bytes[..checksum_offset])
        != read_array::<RESUME_CHECKSUM_LEN>(bytes, checksum_offset)
    {
        return Err(BatchTransferError::ResumeState);
    }
    let mut next_chunks = Vec::with_capacity(file_count);
    let mut chains = Vec::with_capacity(file_count);
    for index in 0..file_count {
        let offset = RESUME_FIXED_LEN + index * RESUME_FILE_LEN;
        let next_chunk = read_u32(bytes, offset);
        validate_resume_position(
            manifest.files[index].file_size,
            manifest.chunk_size,
            next_chunk,
        )?;
        next_chunks.push(next_chunk);
        chains.push(read_array(bytes, offset + 4));
    }
    Ok(ResumeState {
        next_chunks,
        chains,
    })
}

fn resume_checksum(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"Halo Resume State v1");
    digest.update(bytes);
    digest.finalize().into()
}

fn transfer_id_text(transfer_id: [u8; TRANSFER_ID_LEN]) -> String {
    transfer_id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn binding_text(binding: [u8; 32]) -> String {
    binding[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut output = [0_u8; 4];
    output.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_be_bytes(output)
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    let mut output = [0_u8; N];
    output.copy_from_slice(&bytes[offset..offset + N]);
    output
}

#[derive(Debug, Error)]
pub enum BatchTransferError {
    #[error("transfer protocol rejected the message: {0}")]
    Protocol(#[from] TransferProtocolError),
    #[error("transfer control stream failed: {0}")]
    Control(#[from] FrameIoError),
    #[error("transfer data stream failed: {0}")]
    Data(#[from] DataIoError),
    #[error("transfer source count is invalid: {0}")]
    InvalidSourceCount(usize),
    #[error("file name is not a safe cross-platform leaf name")]
    InvalidFileName,
    #[error("a transfer manifest contains duplicate file names")]
    DuplicateFileName,
    #[error("source file is unavailable")]
    Source,
    #[error("source file changed after the authenticated manifest")]
    SourceChanged,
    #[error("receiver rejected the transfer")]
    Rejected,
    #[error("transfer channel binding does not match the authenticated session")]
    ChannelBinding,
    #[error("peer sent a message that is invalid in the current transfer state")]
    UnexpectedMessage,
    #[error("received data failed size, order, or digest verification")]
    Integrity,
    #[error("resume state is malformed or unavailable")]
    ResumeState,
    #[error("resume state does not match the authenticated peer and manifest")]
    ResumeMismatch,
    #[error("sender retry job is malformed or unavailable")]
    SendJob,
    #[error("receiver storage operation failed")]
    Storage,
    #[error("receiver does not have enough free space")]
    InsufficientSpace,
    #[error("a destination file already exists")]
    DestinationExists,
    #[error("verified files could not be finalized without overwrite")]
    Finalization,
    #[error("secure randomness is unavailable")]
    Randomness,
    #[error("transfer is paused with resumable state retained")]
    Paused,
    #[error("local transfer pause requested: {0:?}")]
    LocalPaused(BatchPauseReason),
    #[error("transfer was cancelled and partial state must be removed")]
    Cancelled,
    #[error("peer cancelled the transfer: {0:?}")]
    RemoteCancelled(BatchCancelReason),
    #[error("peer paused the transfer: {0:?}")]
    RemotePaused(BatchPauseReason),
    #[error("transfer state is impossible for the negotiated protocol")]
    ProtocolState,
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
        ended: bool,
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
            self.ended = true;
            Ok(())
        }

        async fn expect_end(&mut self) -> Result<(), DataIoError> {
            if self.incoming.is_empty() {
                Ok(())
            } else {
                Err(DataIoError::TrailingData)
            }
        }

        async fn close(&mut self) {}
    }

    fn test_directory(name: &str) -> PathBuf {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "halo-batch-{name}-{}-{sequence}",
            std::process::id()
        ))
    }

    async fn prepared_batch(root: &Path) -> PreparedBatch {
        let first = root.join("first.txt");
        let second = root.join("second.bin");
        fs::write(&first, b"first file contents")
            .await
            .unwrap_or_else(|error| panic!("first source: {error}"));
        fs::write(&second, vec![0x42; DEFAULT_CHUNK_SIZE as usize + 7])
            .await
            .unwrap_or_else(|error| panic!("second source: {error}"));
        prepare_batch_with_id(
            [0x11; 16],
            vec![
                BatchSource::new(first, None),
                BatchSource::new(second, None),
            ],
            &CancellationToken::new(),
        )
        .await
        .unwrap_or_else(|error| panic!("prepare batch: {error}"))
    }

    #[tokio::test]
    async fn prepared_batch_sends_ordered_file_and_chunk_records() {
        let root = test_directory("send");
        fs::create_dir_all(&root)
            .await
            .unwrap_or_else(|error| panic!("root: {error}"));
        let prepared = prepared_batch(&root).await;
        let positions = prepared
            .manifest()
            .files
            .iter()
            .enumerate()
            .map(|(index, _)| ResumePosition {
                file_index: index as u16,
                next_chunk_index: 0,
            })
            .collect::<Vec<_>>();
        let binding = TlsChannelBinding::new([0x21; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: VecDeque::new(),
            sent: Vec::new(),
            ended: false,
        };
        send_batch_data_with_progress(
            &mut data,
            binding,
            &prepared,
            &positions,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap_or_else(|error| panic!("send batch: {error}"));
        assert!(data.ended);
        let chunks = data
            .sent
            .iter()
            .map(|record| {
                BatchChunkRef::decode(record)
                    .unwrap_or_else(|error| panic!("decode chunk: {error}"))
            })
            .collect::<Vec<_>>();
        assert_eq!(chunks[0].file_index, 0);
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[1].file_index, 1);
        assert_eq!(chunks[1].chunk_index, 0);
        assert_eq!(chunks[2].file_index, 1);
        assert_eq!(chunks[2].chunk_index, 1);
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn sender_rejects_same_size_source_mutation_during_streaming() {
        let root = test_directory("source-mutation");
        fs::create_dir_all(&root)
            .await
            .unwrap_or_else(|error| panic!("root: {error}"));
        let prepared = prepared_batch(&root).await;
        let first_path = root.join("first.txt");
        let original_length = fs::metadata(&first_path)
            .await
            .unwrap_or_else(|error| panic!("metadata: {error}"))
            .len();
        fs::write(
            &first_path,
            vec![
                0x91;
                usize::try_from(original_length)
                    .unwrap_or_else(|error| panic!("source length: {error}"))
            ],
        )
        .await
        .unwrap_or_else(|error| panic!("mutate source: {error}"));
        let positions = prepared
            .manifest()
            .files
            .iter()
            .enumerate()
            .map(|(index, _)| ResumePosition {
                file_index: index as u16,
                next_chunk_index: 0,
            })
            .collect::<Vec<_>>();
        let binding = TlsChannelBinding::new([0x92; 32]);
        let mut data = MemoryDataIo {
            binding,
            incoming: VecDeque::new(),
            sent: Vec::new(),
            ended: false,
        };

        let result = send_batch_data_with_progress(
            &mut data,
            binding,
            &prepared,
            &positions,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await;

        assert!(matches!(result, Err(BatchTransferError::SourceChanged)));
        assert!(!data.ended);
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn receiver_persists_resume_prefix_and_finishes_without_overwrite() {
        let root = test_directory("receive");
        let source = root.join("source");
        let resume = root.join("resume");
        let destination = root.join("destination");
        for directory in [&source, &resume, &destination] {
            fs::create_dir_all(directory)
                .await
                .unwrap_or_else(|error| panic!("directory: {error}"));
        }
        let prepared = prepared_batch(&source).await;
        let positions = prepared
            .manifest()
            .files
            .iter()
            .enumerate()
            .map(|(index, _)| ResumePosition {
                file_index: index as u16,
                next_chunk_index: 0,
            })
            .collect::<Vec<_>>();
        let binding = TlsChannelBinding::new([0x31; 32]);
        let mut sender = MemoryDataIo {
            binding,
            incoming: VecDeque::new(),
            sent: Vec::new(),
            ended: false,
        };
        send_batch_data_with_progress(
            &mut sender,
            binding,
            &prepared,
            &positions,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap_or_else(|error| panic!("send: {error}"));
        let store = BatchResumeStore::new(&resume, [0x41; 32]);
        let mut receiver = MemoryDataIo {
            binding,
            incoming: sender.sent.into(),
            sent: Vec::new(),
            ended: false,
        };
        let received = receive_batch_data_with_progress(
            &mut receiver,
            binding,
            prepared.manifest(),
            &store,
            &destination,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap_or_else(|error| panic!("receive: {error}"));
        assert_eq!(received.final_paths.len(), 2);
        assert_eq!(
            fs::read(destination.join("first.txt"))
                .await
                .unwrap_or_else(|error| panic!("read first: {error}")),
            b"first file contents"
        );
        assert!(
            fs::read_dir(&resume)
                .await
                .unwrap_or_else(|error| panic!("resume directory: {error}"))
                .next_entry()
                .await
                .unwrap_or_else(|error| panic!("resume entry: {error}"))
                .is_none()
        );
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn receiver_rejects_valid_chunk_digests_with_wrong_whole_file_digest() {
        let root = test_directory("whole-digest");
        let source = root.join("source");
        let resume = root.join("resume");
        let destination = root.join("destination");
        for directory in [&source, &resume, &destination] {
            fs::create_dir_all(directory)
                .await
                .unwrap_or_else(|error| panic!("directory: {error}"));
        }
        let prepared = prepared_batch(&source).await;
        let positions = prepared
            .manifest()
            .files
            .iter()
            .enumerate()
            .map(|(index, _)| ResumePosition {
                file_index: index as u16,
                next_chunk_index: 0,
            })
            .collect::<Vec<_>>();
        let binding = TlsChannelBinding::new([0x93; 32]);
        let mut sender = MemoryDataIo {
            binding,
            incoming: VecDeque::new(),
            sent: Vec::new(),
            ended: false,
        };
        send_batch_data_with_progress(
            &mut sender,
            binding,
            &prepared,
            &positions,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap_or_else(|error| panic!("send: {error}"));
        let mut corrupted = halo_protocol::BatchChunk::decode(&sender.sent[0])
            .unwrap_or_else(|error| panic!("decode: {error}"));
        corrupted.payload[0] ^= 1;
        corrupted.chunk_digest = Sha256::digest(&corrupted.payload).into();
        sender.sent[0] = corrupted
            .encode()
            .unwrap_or_else(|error| panic!("encode: {error}"));
        let store = BatchResumeStore::new(&resume, [0x94; 32]);
        let mut receiver = MemoryDataIo {
            binding,
            incoming: sender.sent.into(),
            sent: Vec::new(),
            ended: false,
        };

        let result = receive_batch_data_with_progress(
            &mut receiver,
            binding,
            prepared.manifest(),
            &store,
            &destination,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await;

        assert!(matches!(result, Err(BatchTransferError::Integrity)));
        assert!(
            fs::symlink_metadata(destination.join("first.txt"))
                .await
                .is_err()
        );
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn interrupted_receiver_requests_only_missing_chunks_on_retry() {
        let root = test_directory("resume");
        let source = root.join("source");
        let resume = root.join("resume");
        let destination = root.join("destination");
        for directory in [&source, &resume, &destination] {
            fs::create_dir_all(directory)
                .await
                .unwrap_or_else(|error| panic!("directory: {error}"));
        }
        let prepared = prepared_batch(&source).await;
        let initial = prepared
            .manifest()
            .files
            .iter()
            .enumerate()
            .map(|(index, _)| ResumePosition {
                file_index: index as u16,
                next_chunk_index: 0,
            })
            .collect::<Vec<_>>();
        let binding = TlsChannelBinding::new([0x39; 32]);
        let mut first_sender = MemoryDataIo {
            binding,
            incoming: VecDeque::new(),
            sent: Vec::new(),
            ended: false,
        };
        send_batch_data_with_progress(
            &mut first_sender,
            binding,
            &prepared,
            &initial,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap_or_else(|error| panic!("initial send: {error}"));
        let final_record = first_sender
            .sent
            .pop()
            .unwrap_or_else(|| panic!("final record"));
        let store = BatchResumeStore::new(&resume, [0x49; 32]);
        let mut interrupted = MemoryDataIo {
            binding,
            incoming: first_sender.sent.into(),
            sent: Vec::new(),
            ended: false,
        };
        assert!(matches!(
            receive_batch_data_with_progress(
                &mut interrupted,
                binding,
                prepared.manifest(),
                &store,
                &destination,
                &CancellationToken::new(),
                |_, _, _| {},
            )
            .await,
            Err(BatchTransferError::Data(DataIoError::Truncated))
        ));

        let positions = resume_positions(prepared.manifest(), &store)
            .await
            .unwrap_or_else(|error| panic!("resume positions: {error}"));
        assert_eq!(positions[0].next_chunk_index, 1);
        assert_eq!(positions[1].next_chunk_index, 1);
        let mut retry_sender = MemoryDataIo {
            binding,
            incoming: VecDeque::new(),
            sent: Vec::new(),
            ended: false,
        };
        send_batch_data_with_progress(
            &mut retry_sender,
            binding,
            &prepared,
            &positions,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap_or_else(|error| panic!("retry send: {error}"));
        assert_eq!(retry_sender.sent, vec![final_record]);
        let mut retry_receiver = MemoryDataIo {
            binding,
            incoming: retry_sender.sent.into(),
            sent: Vec::new(),
            ended: false,
        };
        receive_batch_data_with_progress(
            &mut retry_receiver,
            binding,
            prepared.manifest(),
            &store,
            &destination,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap_or_else(|error| panic!("retry receive: {error}"));
        assert!(
            fs::symlink_metadata(destination.join("second.bin"))
                .await
                .is_ok()
        );
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn sender_job_survives_restart_and_rejects_another_peer() {
        let root = test_directory("sender-job");
        let source = root.join("source");
        let jobs = root.join("jobs");
        fs::create_dir_all(&source)
            .await
            .unwrap_or_else(|error| panic!("source directory: {error}"));
        fs::create_dir_all(&jobs)
            .await
            .unwrap_or_else(|error| panic!("job directory: {error}"));
        let prepared = prepared_batch(&source).await;
        let store = BatchSendJobStore::new(&jobs);
        let peer = [0x44; 32];
        store
            .persist(peer, &prepared)
            .await
            .unwrap_or_else(|error| panic!("persist job: {error}"));

        let restored = store
            .load(peer, prepared.manifest().transfer_id)
            .await
            .unwrap_or_else(|error| panic!("load job: {error}"))
            .unwrap_or_else(|| panic!("stored job"));
        assert_eq!(restored.manifest(), prepared.manifest());
        let listed = store
            .list(peer)
            .await
            .unwrap_or_else(|error| panic!("list jobs: {error}"));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest(), prepared.manifest());
        assert!(
            store
                .load([0x45; 32], prepared.manifest().transfer_id)
                .await
                .unwrap_or_else(|error| panic!("other peer lookup: {error}"))
                .is_none()
        );
        store
            .remove(peer, prepared.manifest().transfer_id)
            .await
            .unwrap_or_else(|error| panic!("remove job: {error}"));
        assert!(
            store
                .load(peer, prepared.manifest().transfer_id)
                .await
                .unwrap_or_else(|error| panic!("load removed job: {error}"))
                .is_none()
        );
        let _ = fs::remove_dir_all(root).await;
    }

    #[test]
    fn resume_state_rejects_peer_manifest_and_checksum_substitution() {
        let manifest = TransferManifest::new(
            [0x51; 16],
            DEFAULT_CHUNK_SIZE,
            vec![
                ManifestFile::new(7, [0x61; 32], "file.txt".to_owned())
                    .unwrap_or_else(|error| panic!("manifest file: {error}")),
            ],
        )
        .unwrap_or_else(|error| panic!("manifest: {error}"));
        let state = ResumeState {
            next_chunks: vec![0],
            chains: vec![[0; 32]],
        };
        let encoded = encode_resume_state([0x71; 32], &manifest, &state)
            .unwrap_or_else(|error| panic!("encode state: {error}"));
        assert_eq!(
            decode_resume_state(&encoded, [0x72; 32], &manifest).map_err(|error| error.to_string()),
            Err(BatchTransferError::ResumeMismatch.to_string())
        );
        let mut tampered = encoded;
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert_eq!(
            decode_resume_state(&tampered, [0x71; 32], &manifest)
                .map_err(|error| error.to_string()),
            Err(BatchTransferError::ResumeState.to_string())
        );
    }

    #[test]
    fn resume_checkpoint_interval_is_bounded_and_flushes_file_end() {
        assert!(!checkpoint_due(0, DEFAULT_CHUNK_SIZE.into(), false));
        assert!(!checkpoint_due(
            0,
            RESUME_CHECKPOINT_BYTES - u64::from(DEFAULT_CHUNK_SIZE),
            false
        ));
        assert!(checkpoint_due(0, RESUME_CHECKPOINT_BYTES, false));
        assert!(checkpoint_due(0, 1, true));
    }

    #[tokio::test]
    async fn resume_load_truncates_bytes_beyond_durable_checkpoint() {
        let root = test_directory("checkpoint-tail");
        fs::create_dir_all(&root)
            .await
            .unwrap_or_else(|error| panic!("root: {error}"));
        let first = vec![0x81; DEFAULT_CHUNK_SIZE as usize];
        let second = vec![0x82; DEFAULT_CHUNK_SIZE as usize];
        let mut complete = first.clone();
        complete.extend_from_slice(&second);
        let manifest = TransferManifest::new(
            [0x83; 16],
            DEFAULT_CHUNK_SIZE,
            vec![
                ManifestFile::new(
                    complete.len() as u64,
                    Sha256::digest(&complete).into(),
                    "checkpoint.bin".to_owned(),
                )
                .unwrap_or_else(|error| panic!("manifest file: {error}")),
            ],
        )
        .unwrap_or_else(|error| panic!("manifest: {error}"));
        let peer = [0x84; 32];
        let store = BatchResumeStore::new(&root, peer);
        let first_digest = Sha256::digest(&first).into();
        let state = ResumeState {
            next_chunks: vec![1],
            chains: vec![
                next_chain([0; 32], 0, 0, first_digest)
                    .unwrap_or_else(|error| panic!("chain: {error}")),
            ],
        };
        store
            .persist_state(&manifest, &state)
            .await
            .unwrap_or_else(|error| panic!("persist state: {error}"));
        let partial = store
            .partial_path(manifest.transfer_id, 0)
            .unwrap_or_else(|error| panic!("partial path: {error}"));
        fs::write(&partial, &complete)
            .await
            .unwrap_or_else(|error| panic!("partial: {error}"));

        let positions = resume_positions(&manifest, &store)
            .await
            .unwrap_or_else(|error| panic!("resume positions: {error}"));

        assert_eq!(positions[0].next_chunk_index, 1);
        assert_eq!(
            fs::metadata(&partial)
                .await
                .unwrap_or_else(|error| panic!("metadata: {error}"))
                .len(),
            u64::from(DEFAULT_CHUNK_SIZE)
        );
        let _ = fs::remove_dir_all(root).await;
    }
}
