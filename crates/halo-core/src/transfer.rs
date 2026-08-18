use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use halo_protocol::{
    BatchCancelReason, BatchPauseReason, MAX_BATCH_FILES, MAX_FILE_SIZE, TRANSFER_ID_LEN,
    TransferManifest, TransferMessage,
};
use halo_transfer::{
    BatchResumeStore, BatchSendJobStore, BatchSource, BatchTransferError, PreparedBatch,
    prepare_batch, receive_batch_data_with_progress, receive_manifest, resume_positions,
    send_batch_cancel, send_batch_complete, send_batch_data_with_progress, send_batch_decision,
    send_batch_pause, send_manifest, wait_for_batch_complete,
};
use halo_transport::{ControlIo, DataIo, QuicConnection};
use thiserror::Error;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

const EVENT_LIMIT: usize = 256;
const MAX_DECISION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MIN_PROGRESS_EVENT_STEP: u64 = 64 * 1024;
const MAX_PROGRESS_EVENT_STEP: u64 = 64 * 1024 * 1024;
const TRANSFER_ACTION_RUNNING: u8 = 0;
const TRANSFER_ACTION_PAUSE: u8 = 1;
const TRANSFER_ACTION_CANCEL: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferPolicy {
    pub maximum_file_size: u64,
    pub maximum_aggregate_size: u64,
    pub maximum_file_count: usize,
    pub minimum_free_space_reserve: u64,
    pub receive_decision_timeout: Duration,
    pub progress_event_step: u64,
}

impl Default for TransferPolicy {
    fn default() -> Self {
        Self {
            maximum_file_size: 10 * 1024 * 1024 * 1024,
            maximum_aggregate_size: 10 * 1024 * 1024 * 1024,
            maximum_file_count: MAX_BATCH_FILES,
            minimum_free_space_reserve: 64 * 1024 * 1024,
            receive_decision_timeout: Duration::from_secs(60),
            progress_event_step: 1024 * 1024,
        }
    }
}

impl TransferPolicy {
    pub fn validate(self) -> Result<Self, TransferServiceError> {
        if self.maximum_file_size == 0
            || self.maximum_file_size > MAX_FILE_SIZE
            || self.maximum_aggregate_size == 0
            || self.maximum_aggregate_size > MAX_FILE_SIZE
            || self.maximum_file_count == 0
            || self.maximum_file_count > MAX_BATCH_FILES
            || self.minimum_free_space_reserve > MAX_FILE_SIZE
            || self.receive_decision_timeout.is_zero()
            || self.receive_decision_timeout > MAX_DECISION_TIMEOUT
            || !(MIN_PROGRESS_EVENT_STEP..=MAX_PROGRESS_EVENT_STEP)
                .contains(&self.progress_event_step)
        {
            return Err(TransferServiceError::InvalidPolicy);
        }
        Ok(self)
    }

    #[must_use]
    pub const fn accepts_file_size(self, file_size: u64) -> bool {
        file_size <= self.maximum_file_size
    }

    #[must_use]
    pub fn accepts_manifest(self, manifest: &TransferManifest) -> bool {
        manifest.files.len() <= self.maximum_file_count
            && manifest
                .files
                .iter()
                .all(|file| self.accepts_file_size(file.file_size))
            && manifest
                .aggregate_size()
                .is_ok_and(|size| size <= self.maximum_aggregate_size)
    }

    #[must_use]
    pub const fn has_available_space(self, available_bytes: u64, required_bytes: u64) -> bool {
        available_bytes >= required_bytes.saturating_add(self.minimum_free_space_reserve)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferDirection {
    Sending,
    Receiving,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferFileSource {
    pub source_path: PathBuf,
    pub advertised_name: Option<String>,
}

impl TransferFileSource {
    #[must_use]
    pub fn new(source_path: impl Into<PathBuf>, advertised_name: Option<String>) -> Self {
        Self {
            source_path: source_path.into(),
            advertised_name,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferEventKind {
    OfferReceived,
    AwaitingDecision,
    Transferring,
    Completed,
    Rejected,
    Paused,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferEvent {
    pub event_id: u64,
    pub request_id: Option<u64>,
    pub authenticated_session_id: u64,
    pub transfer_id: String,
    pub direction: TransferDirection,
    pub kind: TransferEventKind,
    pub file_name: String,
    pub file_names: Vec<String>,
    pub file_sizes: Vec<u64>,
    pub file_size: u64,
    pub transferred_bytes: u64,
    pub completed_files: u32,
    pub current_file_index: Option<u32>,
    pub resumable: bool,
    /// Present after a received file has passed all checks and was finalized.
    pub final_path: Option<PathBuf>,
    /// Stable category only; never contains addresses or filesystem paths.
    pub detail: Option<String>,
}

pub(crate) struct ReceiveTransferDecision {
    pub accepted: bool,
    pub staging_directory: Option<PathBuf>,
    pub destination_directory: Option<PathBuf>,
    pub available_bytes: Option<u64>,
}

struct TransferSession {
    connection: QuicConnection,
    peer_binding: [u8; 32],
    gate: Arc<Semaphore>,
    cancellation: CancellationToken,
}

#[derive(Clone)]
struct ActiveTransfer {
    cancellation: CancellationToken,
    action: Arc<AtomicU8>,
}

#[derive(Clone)]
struct PausedBatch {
    prepared: PreparedBatch,
    peer_binding: [u8; 32],
    session_id: u64,
}

struct FinishedSources {
    paths: Vec<PathBuf>,
    peer_binding: [u8; 32],
    transfer_id: [u8; TRANSFER_ID_LEN],
}

pub(crate) struct TransferCoordinator {
    next_event_id: AtomicU64,
    next_request_id: AtomicU64,
    events: Mutex<VecDeque<TransferEvent>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<ReceiveTransferDecision>>>,
    active: Mutex<HashMap<String, ActiveTransfer>>,
    paused_batches: Mutex<HashMap<String, PausedBatch>>,
    finished_sources: Mutex<HashMap<String, FinishedSources>>,
    sessions: Mutex<HashMap<u64, Arc<TransferSession>>>,
    cancellation: CancellationToken,
    policy: TransferPolicy,
    resume_directory: PathBuf,
    send_jobs: BatchSendJobStore,
}

impl Default for TransferCoordinator {
    fn default() -> Self {
        Self::from_validated(
            TransferPolicy::default(),
            std::env::temp_dir().join(format!("halo-core-resume-{}", std::process::id())),
        )
    }
}

impl TransferCoordinator {
    pub(crate) fn new(
        policy: TransferPolicy,
        resume_directory: PathBuf,
    ) -> Result<Self, TransferServiceError> {
        if !resume_directory.is_absolute() {
            return Err(TransferServiceError::InvalidPolicy);
        }
        Ok(Self::from_validated(policy.validate()?, resume_directory))
    }

    fn from_validated(policy: TransferPolicy, resume_directory: PathBuf) -> Self {
        Self {
            next_event_id: AtomicU64::new(0),
            next_request_id: AtomicU64::new(0),
            events: Mutex::new(VecDeque::new()),
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            paused_batches: Mutex::new(HashMap::new()),
            finished_sources: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            cancellation: CancellationToken::new(),
            policy,
            send_jobs: BatchSendJobStore::new(resume_directory.clone()),
            resume_directory,
        }
    }

    pub(crate) fn attach_session(
        self: &Arc<Self>,
        session_id: u64,
        connection: QuicConnection,
        peer_binding: [u8; 32],
    ) -> Result<(), TransferServiceError> {
        let session = Arc::new(TransferSession {
            connection,
            peer_binding,
            gate: Arc::new(Semaphore::new(1)),
            cancellation: self.cancellation.child_token(),
        });
        self.sessions
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .insert(session_id, Arc::clone(&session));
        let coordinator = Arc::clone(self);
        tokio::spawn(async move {
            coordinator
                .restore_sender_jobs(session_id, peer_binding)
                .await;
            coordinator.incoming_loop(session_id, session).await;
        });
        Ok(())
    }

    pub(crate) fn detach_session(&self, session_id: u64) {
        let session = self
            .sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(&session_id));
        if let Some(session) = session {
            session.cancellation.cancel();
        }
    }

    pub(crate) async fn send_file(
        self: &Arc<Self>,
        session_id: u64,
        source_path: PathBuf,
        advertised_name: Option<String>,
    ) -> Result<String, TransferServiceError> {
        self.send_files(
            session_id,
            vec![BatchSource::new(source_path, advertised_name)],
        )
        .await
    }

    pub(crate) async fn send_files(
        self: &Arc<Self>,
        session_id: u64,
        sources: Vec<BatchSource>,
    ) -> Result<String, TransferServiceError> {
        let session = self.session(session_id)?;
        self.send_batch(session_id, session, sources).await
    }

    async fn send_batch(
        self: &Arc<Self>,
        session_id: u64,
        session: Arc<TransferSession>,
        sources: Vec<BatchSource>,
    ) -> Result<String, TransferServiceError> {
        if sources.is_empty() || sources.len() > self.policy.maximum_file_count {
            return Err(TransferServiceError::InvalidSourceCount);
        }
        let permit = Arc::clone(&session.gate)
            .try_acquire_owned()
            .map_err(|_| TransferServiceError::Busy)?;
        let cancellation = session.cancellation.child_token();
        let prepared = prepare_batch(sources, &cancellation).await?;
        if !self.policy.accepts_manifest(prepared.manifest()) {
            return Err(TransferServiceError::FileRejectedByPolicy);
        }
        self.send_jobs
            .persist(session.peer_binding, &prepared)
            .await?;
        let manifest = prepared.manifest().clone();
        let transfer_id = transfer_id_text(manifest.transfer_id);
        let action = self.insert_active(transfer_id.clone(), cancellation.clone())?;
        self.emit(event_for_manifest(
            session_id,
            &manifest,
            TransferDirection::Sending,
            TransferEventKind::AwaitingDecision,
        ))?;

        let coordinator = Arc::clone(self);
        let task_transfer_id = transfer_id.clone();
        let paused_copy = prepared.clone();
        let peer_binding = session.peer_binding;
        tokio::spawn(async move {
            let result = coordinator
                .run_batch_sender(session_id, session, prepared, cancellation, action, permit)
                .await;
            coordinator
                .update_sender_job(
                    &task_transfer_id,
                    paused_copy,
                    peer_binding,
                    session_id,
                    &result,
                )
                .await;
            coordinator.finish_batch_task(
                session_id,
                &manifest,
                TransferDirection::Sending,
                &task_transfer_id,
                result,
            );
        });
        Ok(transfer_id)
    }

    pub(crate) async fn retry(
        self: &Arc<Self>,
        session_id: u64,
        transfer_id: &str,
    ) -> Result<String, TransferServiceError> {
        let session = self.session(session_id)?;
        let memory_paused = self
            .paused_batches
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .get(transfer_id)
            .cloned();
        let paused = match memory_paused {
            Some(paused) => paused,
            None => {
                let transfer_bytes = parse_transfer_id(transfer_id)
                    .ok_or(TransferServiceError::TransferNotPaused)?;
                let prepared = self
                    .send_jobs
                    .load(session.peer_binding, transfer_bytes)
                    .await?
                    .ok_or(TransferServiceError::TransferNotPaused)?;
                PausedBatch {
                    prepared,
                    peer_binding: session.peer_binding,
                    session_id,
                }
            }
        };
        if paused.peer_binding != session.peer_binding {
            return Err(TransferServiceError::PeerMismatch);
        }
        let permit = Arc::clone(&session.gate)
            .try_acquire_owned()
            .map_err(|_| TransferServiceError::Busy)?;
        let cancellation = session.cancellation.child_token();
        let action = self.insert_active(transfer_id.to_owned(), cancellation.clone())?;
        self.paused_batches
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .remove(transfer_id);
        let manifest = paused.prepared.manifest().clone();
        self.emit(event_with_detail(
            event_for_manifest(
                session_id,
                &manifest,
                TransferDirection::Sending,
                TransferEventKind::AwaitingDecision,
            ),
            "retrying",
        ))?;

        let coordinator = Arc::clone(self);
        let task_transfer_id = transfer_id.to_owned();
        let paused_copy = paused.prepared.clone();
        let peer_binding = session.peer_binding;
        tokio::spawn(async move {
            let result = coordinator
                .run_batch_sender(
                    session_id,
                    session,
                    paused.prepared,
                    cancellation,
                    action,
                    permit,
                )
                .await;
            coordinator
                .update_sender_job(
                    &task_transfer_id,
                    paused_copy,
                    peer_binding,
                    session_id,
                    &result,
                )
                .await;
            coordinator.finish_batch_task(
                session_id,
                &manifest,
                TransferDirection::Sending,
                &task_transfer_id,
                result,
            );
        });
        Ok(transfer_id.to_owned())
    }

    pub(crate) fn events_after(
        &self,
        event_id: u64,
    ) -> Result<Vec<TransferEvent>, TransferServiceError> {
        self.events
            .lock()
            .map_err(|_| TransferServiceError::InternalState)
            .map(|events| {
                events
                    .iter()
                    .filter(|event| event.event_id > event_id)
                    .cloned()
                    .collect()
            })
    }

    pub(crate) fn respond(
        &self,
        request_id: u64,
        decision: ReceiveTransferDecision,
    ) -> Result<(), TransferServiceError> {
        if decision.accepted
            && (decision.staging_directory.is_none() || decision.destination_directory.is_none())
        {
            return Err(TransferServiceError::MissingDestination);
        }
        let sender = self
            .pending
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .remove(&request_id)
            .ok_or(TransferServiceError::RequestNotPending)?;
        sender
            .send(decision)
            .map_err(|_| TransferServiceError::RequestNotPending)
    }

    pub(crate) async fn cancel(&self, transfer_id: &str) -> Result<(), TransferServiceError> {
        let active = self
            .active
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .get(transfer_id)
            .cloned();
        if let Some(active) = active {
            active
                .action
                .store(TRANSFER_ACTION_CANCEL, Ordering::Release);
            active.cancellation.cancel();
            return Ok(());
        }
        let paused = self
            .paused_batches
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .remove(transfer_id)
            .ok_or(TransferServiceError::TransferNotFound)?;
        let event = event_with_detail(
            event_for_manifest(
                paused.session_id,
                paused.prepared.manifest(),
                TransferDirection::Sending,
                TransferEventKind::Cancelled,
            ),
            "cancelled",
        );
        let source_paths = paused.prepared.source_paths();
        self.send_jobs
            .remove(paused.peer_binding, paused.prepared.manifest().transfer_id)
            .await?;
        self.insert_finished_sources(
            transfer_id,
            source_paths,
            paused.peer_binding,
            paused.prepared.manifest().transfer_id,
        );
        self.emit(event)
    }

    pub(crate) async fn take_finished_sources(
        &self,
        transfer_id: &str,
    ) -> Result<Vec<PathBuf>, TransferServiceError> {
        let finished = self
            .finished_sources
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .remove(transfer_id)
            .ok_or(TransferServiceError::TransferNotFound)?;
        self.send_jobs
            .remove(finished.peer_binding, finished.transfer_id)
            .await?;
        Ok(finished.paths)
    }

    pub(crate) fn pause(&self, transfer_id: &str) -> Result<(), TransferServiceError> {
        let active = self
            .active
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .get(transfer_id)
            .cloned()
            .ok_or(TransferServiceError::TransferNotFound)?;
        active
            .action
            .store(TRANSFER_ACTION_PAUSE, Ordering::Release);
        active.cancellation.cancel();
        Ok(())
    }

    pub(crate) fn shutdown(&self) {
        self.cancellation.cancel();
        if let Ok(mut active) = self.active.lock() {
            for transfer in active.values() {
                transfer.cancellation.cancel();
            }
            active.clear();
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.clear();
        }
    }

    async fn incoming_loop(self: Arc<Self>, session_id: u64, session: Arc<TransferSession>) {
        loop {
            let control = tokio::select! {
                () = session.cancellation.cancelled() => break,
                result = session.connection.accept_control() => match result {
                    Ok(control) => control,
                    Err(_) => break,
                },
            };
            self.handle_incoming_batch(session_id, Arc::clone(&session), control)
                .await;
        }
    }

    async fn handle_incoming_batch(
        self: &Arc<Self>,
        session_id: u64,
        session: Arc<TransferSession>,
        mut control: impl ControlIo,
    ) {
        let binding = control.channel_binding();
        let manifest = match receive_manifest(&mut control).await {
            Ok(manifest) => manifest,
            Err(_) => {
                control.close().await;
                return;
            }
        };
        let transfer_id = transfer_id_text(manifest.transfer_id);
        if !self.policy.accepts_manifest(&manifest) {
            let _ = send_batch_cancel(&mut control, &manifest, BatchCancelReason::Policy).await;
            control.close().await;
            self.emit_ignoring_closed(event_with_detail(
                event_for_manifest(
                    session_id,
                    &manifest,
                    TransferDirection::Receiving,
                    TransferEventKind::Rejected,
                ),
                "manifest_policy",
            ));
            return;
        }
        let permit = match Arc::clone(&session.gate).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _ = send_batch_cancel(&mut control, &manifest, BatchCancelReason::Policy).await;
                control.close().await;
                self.emit_ignoring_closed(event_with_detail(
                    event_for_manifest(
                        session_id,
                        &manifest,
                        TransferDirection::Receiving,
                        TransferEventKind::Rejected,
                    ),
                    "session_busy",
                ));
                return;
            }
        };
        let cancellation = session.cancellation.child_token();
        let action = match self.insert_active(transfer_id.clone(), cancellation.clone()) {
            Ok(action) => action,
            Err(_) => {
                let _ = send_batch_cancel(&mut control, &manifest, BatchCancelReason::Policy).await;
                control.close().await;
                return;
            }
        };
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = oneshot::channel();
        if self
            .pending
            .lock()
            .map(|mut pending| pending.insert(request_id, sender))
            .is_err()
        {
            self.remove_active(&transfer_id);
            control.close().await;
            return;
        }
        let mut event = event_for_manifest(
            session_id,
            &manifest,
            TransferDirection::Receiving,
            TransferEventKind::OfferReceived,
        );
        event.request_id = Some(request_id);
        self.emit_ignoring_closed(event);

        let decision = tokio::select! {
            () = cancellation.cancelled() => None,
            result = timeout(self.policy.receive_decision_timeout, receiver) => {
                result.ok().and_then(Result::ok)
            },
        };
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&request_id);
        }
        let resume_store =
            BatchResumeStore::new(self.resume_directory.clone(), session.peer_binding);
        let result = match decision {
            Some(decision) if decision.accepted => {
                let required = manifest.aggregate_size().unwrap_or(u64::MAX);
                let available = decision.available_bytes.unwrap_or(0);
                if !self.policy.has_available_space(available, required) {
                    let _ = send_batch_cancel(&mut control, &manifest, BatchCancelReason::Storage)
                        .await;
                    Err(BatchTransferError::InsufficientSpace)
                } else {
                    let Some(destination) = decision.destination_directory else {
                        self.remove_active(&transfer_id);
                        control.close().await;
                        return;
                    };
                    match resume_positions(&manifest, &resume_store).await {
                        Ok(positions) => {
                            if send_batch_decision(&mut control, &manifest, true, positions)
                                .await
                                .is_err()
                            {
                                Err(BatchTransferError::UnexpectedMessage)
                            } else {
                                self.emit_ignoring_closed(event_for_manifest(
                                    session_id,
                                    &manifest,
                                    TransferDirection::Receiving,
                                    TransferEventKind::Transferring,
                                ));
                                self.run_batch_receiver(
                                    &session,
                                    session_id,
                                    &mut control,
                                    binding,
                                    &manifest,
                                    &resume_store,
                                    destination,
                                    &cancellation,
                                    &action,
                                )
                                .await
                            }
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            _ if cancellation.is_cancelled() => {
                let interruption = batch_interruption(&action);
                notify_peer_of_batch_failure(&mut control, &manifest, &interruption).await;
                Err(interruption)
            }
            _ => {
                let _ = send_batch_decision(&mut control, &manifest, false, Vec::new()).await;
                Err(BatchTransferError::Rejected)
            }
        };
        if matches!(
            result,
            Err(BatchTransferError::Cancelled)
                | Err(BatchTransferError::RemoteCancelled(_))
                | Err(BatchTransferError::Integrity)
        ) {
            let _ = resume_store.discard(&manifest).await;
        }
        control.close().await;
        drop(permit);
        self.finish_batch_task(
            session_id,
            &manifest,
            TransferDirection::Receiving,
            &transfer_id,
            result,
        );
    }

    async fn run_batch_sender(
        &self,
        session_id: u64,
        session: Arc<TransferSession>,
        prepared: PreparedBatch,
        cancellation: CancellationToken,
        action: Arc<AtomicU8>,
        _permit: OwnedSemaphorePermit,
    ) -> Result<Vec<PathBuf>, BatchTransferError> {
        let manifest = prepared.manifest();
        let mut control = tokio::select! {
            () = cancellation.cancelled() => return Err(batch_interruption(&action)),
            result = session.connection.open_control() => result?,
        };
        let binding = control.channel_binding();
        let positions = tokio::select! {
            () = cancellation.cancelled() => {
                let interruption = batch_interruption(&action);
                notify_peer_of_batch_failure(&mut control, manifest, &interruption).await;
                control.close().await;
                return Err(interruption);
            }
            result = send_manifest(&mut control, &prepared) => match result {
                Ok(positions) => positions,
                Err(error) => {
                    control.close().await;
                    return Err(error);
                }
            },
        };
        self.emit_ignoring_closed(event_for_manifest(
            session_id,
            manifest,
            TransferDirection::Sending,
            TransferEventKind::Transferring,
        ));
        let mut data = match session.connection.open_data().await {
            Ok(data) => data,
            Err(error) => {
                let error = BatchTransferError::from(error);
                notify_peer_of_batch_failure(&mut control, manifest, &error).await;
                control.close().await;
                return Err(error);
            }
        };
        let result = {
            let mut last_progress = 0_u64;
            send_batch_data_with_progress(
                &mut data,
                binding,
                &prepared,
                &positions,
                &cancellation,
                |file_index, transferred, _| {
                    let aggregate = manifest.files[..file_index]
                        .iter()
                        .map(|file| file.file_size)
                        .sum::<u64>()
                        .saturating_add(transferred);
                    if aggregate.saturating_sub(last_progress) >= self.policy.progress_event_step
                        || aggregate == manifest.aggregate_size().unwrap_or(u64::MAX)
                    {
                        last_progress = aggregate;
                        self.emit_batch_progress(
                            session_id,
                            manifest,
                            TransferDirection::Sending,
                            file_index,
                            aggregate,
                        );
                    }
                },
            )
            .await
        };
        if let Err(error) = result {
            let effective = if matches!(error, BatchTransferError::Paused) {
                batch_interruption(&action)
            } else {
                error
            };
            notify_peer_of_batch_failure(&mut control, manifest, &effective).await;
            data.close().await;
            control.close().await;
            return Err(effective);
        }
        data.close().await;
        let result = tokio::select! {
            () = cancellation.cancelled() => {
                let interruption = batch_interruption(&action);
                notify_peer_of_batch_failure(&mut control, manifest, &interruption).await;
                Err(interruption)
            }
            result = wait_for_batch_complete(&mut control, manifest) => result,
        };
        control.close().await;
        result.map(|()| Vec::new())
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_batch_receiver(
        &self,
        session: &TransferSession,
        session_id: u64,
        control: &mut dyn ControlIo,
        binding: halo_crypto::TlsChannelBinding,
        manifest: &TransferManifest,
        resume_store: &BatchResumeStore,
        destination: PathBuf,
        cancellation: &CancellationToken,
        action: &Arc<AtomicU8>,
    ) -> Result<Vec<PathBuf>, BatchTransferError> {
        let mut data = match tokio::select! {
            () = cancellation.cancelled() => return Err(batch_interruption(action)),
            result = session.connection.accept_data() => result,
        } {
            Ok(data) => data,
            Err(error) => {
                let error = BatchTransferError::from(error);
                notify_peer_of_batch_failure(control, manifest, &error).await;
                return Err(error);
            }
        };
        let result = {
            let mut last_progress = 0_u64;
            let receive = receive_batch_data_with_progress(
                &mut data,
                binding,
                manifest,
                resume_store,
                &destination,
                cancellation,
                |file_index, transferred, _| {
                    let aggregate = manifest.files[..file_index]
                        .iter()
                        .map(|file| file.file_size)
                        .sum::<u64>()
                        .saturating_add(transferred);
                    if aggregate.saturating_sub(last_progress) >= self.policy.progress_event_step
                        || aggregate == manifest.aggregate_size().unwrap_or(u64::MAX)
                    {
                        last_progress = aggregate;
                        self.emit_batch_progress(
                            session_id,
                            manifest,
                            TransferDirection::Receiving,
                            file_index,
                            aggregate,
                        );
                    }
                },
            );
            tokio::pin!(receive);
            tokio::select! {
                biased;
                terminal = receive_batch_interruption(control, manifest) => Err(terminal),
                result = &mut receive => result,
            }
        };
        let result = match result {
            Err(BatchTransferError::Paused) => Err(batch_interruption(action)),
            result => result,
        };
        if let Err(error) = &result {
            notify_peer_of_batch_failure(control, manifest, error).await;
        }
        data.close().await;
        let received = result?;
        send_batch_complete(control, manifest).await?;
        Ok(received.final_paths)
    }

    fn session(&self, session_id: u64) -> Result<Arc<TransferSession>, TransferServiceError> {
        self.sessions
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .get(&session_id)
            .cloned()
            .ok_or(TransferServiceError::SessionNotFound)
    }

    fn insert_active(
        &self,
        transfer_id: String,
        cancellation: CancellationToken,
    ) -> Result<Arc<AtomicU8>, TransferServiceError> {
        let action = Arc::new(AtomicU8::new(TRANSFER_ACTION_RUNNING));
        if self
            .active
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .insert(
                transfer_id,
                ActiveTransfer {
                    cancellation,
                    action: Arc::clone(&action),
                },
            )
            .is_some()
        {
            return Err(TransferServiceError::InternalState);
        }
        Ok(action)
    }

    fn remove_active(&self, transfer_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(transfer_id);
        }
    }

    async fn restore_sender_jobs(&self, session_id: u64, peer_binding: [u8; 32]) {
        let jobs = match self.send_jobs.list(peer_binding).await {
            Ok(jobs) => jobs,
            Err(_) => return,
        };
        for prepared in jobs {
            let transfer_id = transfer_id_text(prepared.manifest().transfer_id);
            let inserted = self
                .paused_batches
                .lock()
                .map(|mut paused| {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        paused.entry(transfer_id)
                    {
                        entry.insert(PausedBatch {
                            prepared: prepared.clone(),
                            peer_binding,
                            session_id,
                        });
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if inserted {
                self.emit_ignoring_closed(event_with_detail(
                    event_for_manifest(
                        session_id,
                        prepared.manifest(),
                        TransferDirection::Sending,
                        TransferEventKind::Paused,
                    ),
                    "restart_recovery",
                ));
            }
        }
    }

    async fn update_sender_job(
        &self,
        transfer_id: &str,
        prepared: PreparedBatch,
        peer_binding: [u8; 32],
        session_id: u64,
        result: &Result<Vec<PathBuf>, BatchTransferError>,
    ) {
        let transfer_bytes = prepared.manifest().transfer_id;
        let source_paths = prepared.source_paths();
        let retryable = matches!(
            result,
            Err(BatchTransferError::Paused)
                | Err(BatchTransferError::LocalPaused(_))
                | Err(BatchTransferError::RemotePaused(_))
                | Err(BatchTransferError::Control(_))
                | Err(BatchTransferError::Data(_))
        );
        if let Ok(mut paused) = self.paused_batches.lock() {
            if retryable {
                paused.insert(
                    transfer_id.to_owned(),
                    PausedBatch {
                        prepared,
                        peer_binding,
                        session_id,
                    },
                );
            } else {
                paused.remove(transfer_id);
            }
        }
        if !retryable {
            self.insert_finished_sources(transfer_id, source_paths, peer_binding, transfer_bytes);
        }
    }

    fn insert_finished_sources(
        &self,
        transfer_id: &str,
        source_paths: Vec<PathBuf>,
        peer_binding: [u8; 32],
        transfer_bytes: [u8; TRANSFER_ID_LEN],
    ) {
        if let Ok(mut finished) = self.finished_sources.lock() {
            if finished.len() >= EVENT_LIMIT
                && !finished.contains_key(transfer_id)
                && let Some(oldest) = finished.keys().next().cloned()
            {
                finished.remove(&oldest);
            }
            finished.insert(
                transfer_id.to_owned(),
                FinishedSources {
                    paths: source_paths,
                    peer_binding,
                    transfer_id: transfer_bytes,
                },
            );
        }
    }

    fn finish_batch_task(
        &self,
        session_id: u64,
        manifest: &TransferManifest,
        direction: TransferDirection,
        transfer_id: &str,
        result: Result<Vec<PathBuf>, BatchTransferError>,
    ) {
        self.remove_active(transfer_id);
        let (kind, final_path, detail) = match result {
            Ok(paths) => (TransferEventKind::Completed, paths.into_iter().next(), None),
            Err(BatchTransferError::Rejected)
            | Err(BatchTransferError::RemoteCancelled(BatchCancelReason::Policy)) => (
                TransferEventKind::Rejected,
                None,
                Some("rejected".to_owned()),
            ),
            Err(BatchTransferError::Paused)
            | Err(BatchTransferError::LocalPaused(_))
            | Err(BatchTransferError::RemotePaused(_)) => {
                (TransferEventKind::Paused, None, Some("paused".to_owned()))
            }
            Err(BatchTransferError::Cancelled)
            | Err(BatchTransferError::RemoteCancelled(BatchCancelReason::User)) => (
                TransferEventKind::Cancelled,
                None,
                Some("cancelled".to_owned()),
            ),
            Err(error) => (
                TransferEventKind::Failed,
                None,
                Some(batch_error_category(&error).to_owned()),
            ),
        };
        let mut event = event_for_manifest(session_id, manifest, direction, kind);
        if kind == TransferEventKind::Completed {
            event.transferred_bytes = manifest.aggregate_size().unwrap_or(0);
            event.completed_files = u32::try_from(manifest.files.len()).unwrap_or(u32::MAX);
        }
        event.final_path = final_path;
        event.detail = detail;
        self.emit_ignoring_closed(event);
    }

    fn emit(&self, mut event: TransferEvent) -> Result<(), TransferServiceError> {
        event.event_id = self.next_event_id.fetch_add(1, Ordering::Relaxed) + 1;
        let mut events = self
            .events
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?;
        if events.len() == EVENT_LIMIT {
            events.pop_front();
        }
        events.push_back(event);
        Ok(())
    }

    fn emit_ignoring_closed(&self, event: TransferEvent) {
        let _ = self.emit(event);
    }

    fn emit_batch_progress(
        &self,
        session_id: u64,
        manifest: &TransferManifest,
        direction: TransferDirection,
        file_index: usize,
        transferred_bytes: u64,
    ) {
        let mut event = event_for_manifest(
            session_id,
            manifest,
            direction,
            TransferEventKind::Transferring,
        );
        event.transferred_bytes = transferred_bytes.min(manifest.aggregate_size().unwrap_or(0));
        event.current_file_index = u32::try_from(file_index).ok();
        let completed = if manifest.files.get(file_index).is_some_and(|file| {
            let prior = manifest.files[..file_index]
                .iter()
                .map(|entry| entry.file_size)
                .sum::<u64>();
            transferred_bytes.saturating_sub(prior) >= file.file_size
        }) {
            file_index.saturating_add(1)
        } else {
            file_index
        };
        event.completed_files = u32::try_from(completed).unwrap_or(u32::MAX);
        self.emit_ignoring_closed(event);
    }
}

fn event_for_manifest(
    session_id: u64,
    manifest: &TransferManifest,
    direction: TransferDirection,
    kind: TransferEventKind,
) -> TransferEvent {
    TransferEvent {
        event_id: 0,
        request_id: None,
        authenticated_session_id: session_id,
        transfer_id: transfer_id_text(manifest.transfer_id),
        direction,
        kind,
        file_name: manifest
            .files
            .first()
            .map(|file| file.file_name.clone())
            .unwrap_or_default(),
        file_names: manifest
            .files
            .iter()
            .map(|file| file.file_name.clone())
            .collect(),
        file_sizes: manifest.files.iter().map(|file| file.file_size).collect(),
        file_size: manifest.aggregate_size().unwrap_or(0),
        transferred_bytes: 0,
        completed_files: 0,
        current_file_index: None,
        resumable: true,
        final_path: None,
        detail: None,
    }
}

fn event_with_detail(mut event: TransferEvent, detail: &str) -> TransferEvent {
    event.detail = Some(detail.to_owned());
    event
}

fn transfer_id_text(transfer_id: [u8; TRANSFER_ID_LEN]) -> String {
    transfer_id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_transfer_id(value: &str) -> Option<[u8; TRANSFER_ID_LEN]> {
    if value.len() != TRANSFER_ID_LEN * 2 || !value.is_ascii() {
        return None;
    }
    let mut output = [0_u8; TRANSFER_ID_LEN];
    for (index, byte) in output.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16).ok()?;
    }
    Some(output)
}

fn batch_interruption(action: &AtomicU8) -> BatchTransferError {
    match action.load(Ordering::Acquire) {
        TRANSFER_ACTION_CANCEL => BatchTransferError::Cancelled,
        TRANSFER_ACTION_PAUSE => BatchTransferError::LocalPaused(BatchPauseReason::User),
        _ => BatchTransferError::LocalPaused(BatchPauseReason::RouteLost),
    }
}

async fn receive_batch_interruption(
    control: &mut dyn ControlIo,
    manifest: &TransferManifest,
) -> BatchTransferError {
    let frame = match control.receive_frame(4096).await {
        Ok(frame) => frame,
        Err(error) => return BatchTransferError::Control(error),
    };
    match TransferMessage::decode(&frame) {
        Ok(TransferMessage::Cancel(cancel)) if cancel.transfer_id == manifest.transfer_id => {
            BatchTransferError::RemoteCancelled(cancel.reason)
        }
        Ok(TransferMessage::Pause(pause))
            if pause.transfer_id == manifest.transfer_id
                && pause.manifest_digest == manifest.digest() =>
        {
            BatchTransferError::RemotePaused(pause.reason)
        }
        Ok(_) => BatchTransferError::UnexpectedMessage,
        Err(error) => BatchTransferError::Protocol(error),
    }
}

async fn notify_peer_of_batch_failure(
    control: &mut dyn ControlIo,
    manifest: &TransferManifest,
    error: &BatchTransferError,
) {
    match error {
        BatchTransferError::LocalPaused(reason) => {
            let _ = send_batch_pause(control, manifest, *reason).await;
        }
        BatchTransferError::Paused => {
            let _ = send_batch_pause(control, manifest, BatchPauseReason::AppLifecycle).await;
        }
        error => {
            if let Some(reason) = batch_cancel_reason(error) {
                let _ = send_batch_cancel(control, manifest, reason).await;
            }
        }
    }
}

fn batch_cancel_reason(error: &BatchTransferError) -> Option<BatchCancelReason> {
    match error {
        BatchTransferError::Cancelled => Some(BatchCancelReason::User),
        BatchTransferError::InvalidSourceCount(_)
        | BatchTransferError::InvalidFileName
        | BatchTransferError::DuplicateFileName
        | BatchTransferError::Source
        | BatchTransferError::SourceChanged
        | BatchTransferError::Rejected
        | BatchTransferError::Randomness => Some(BatchCancelReason::Policy),
        BatchTransferError::Integrity | BatchTransferError::ResumeMismatch => {
            Some(BatchCancelReason::Integrity)
        }
        BatchTransferError::ResumeState
        | BatchTransferError::SendJob
        | BatchTransferError::Storage
        | BatchTransferError::InsufficientSpace
        | BatchTransferError::DestinationExists
        | BatchTransferError::Finalization => Some(BatchCancelReason::Storage),
        BatchTransferError::Protocol(_)
        | BatchTransferError::Data(_)
        | BatchTransferError::ChannelBinding
        | BatchTransferError::UnexpectedMessage
        | BatchTransferError::ProtocolState => Some(BatchCancelReason::Protocol),
        BatchTransferError::Control(_)
        | BatchTransferError::Paused
        | BatchTransferError::LocalPaused(_)
        | BatchTransferError::RemoteCancelled(_)
        | BatchTransferError::RemotePaused(_) => None,
    }
}

fn batch_error_category(error: &BatchTransferError) -> &'static str {
    match error {
        BatchTransferError::Protocol(_)
        | BatchTransferError::UnexpectedMessage
        | BatchTransferError::ProtocolState => "protocol",
        BatchTransferError::Control(_) | BatchTransferError::Data(_) => "transport",
        BatchTransferError::InvalidSourceCount(_) => "invalid_source_count",
        BatchTransferError::InvalidFileName => "invalid_file_name",
        BatchTransferError::DuplicateFileName => "duplicate_file_name",
        BatchTransferError::Source => "source",
        BatchTransferError::SourceChanged => "source_changed",
        BatchTransferError::Rejected => "rejected",
        BatchTransferError::ChannelBinding => "channel_binding",
        BatchTransferError::Integrity => "integrity",
        BatchTransferError::ResumeState => "resume_state",
        BatchTransferError::ResumeMismatch => "resume_mismatch",
        BatchTransferError::SendJob => "send_job",
        BatchTransferError::Storage => "storage",
        BatchTransferError::InsufficientSpace => "insufficient_space",
        BatchTransferError::DestinationExists => "destination_exists",
        BatchTransferError::Finalization => "finalization",
        BatchTransferError::Randomness => "randomness",
        BatchTransferError::Paused | BatchTransferError::LocalPaused(_) => "paused",
        BatchTransferError::Cancelled => "cancelled",
        BatchTransferError::RemoteCancelled(BatchCancelReason::User) => "remote_cancelled",
        BatchTransferError::RemoteCancelled(BatchCancelReason::Policy) => "remote_policy",
        BatchTransferError::RemoteCancelled(BatchCancelReason::Integrity) => "remote_integrity",
        BatchTransferError::RemoteCancelled(BatchCancelReason::Storage) => "remote_storage",
        BatchTransferError::RemoteCancelled(BatchCancelReason::Protocol) => "remote_protocol",
        BatchTransferError::RemotePaused(_) => "remote_paused",
    }
}

#[derive(Debug, Error)]
pub enum TransferServiceError {
    #[error("transfer policy configuration is invalid")]
    InvalidPolicy,
    #[error("file exceeds the configured transfer admission policy")]
    FileRejectedByPolicy,
    #[error("transfer source count is invalid")]
    InvalidSourceCount,
    #[error("authenticated LAN session does not exist")]
    SessionNotFound,
    #[error("authenticated LAN session already has an active transfer")]
    Busy,
    #[error("transfer receive request is no longer pending")]
    RequestNotPending,
    #[error("accepted transfer requires private staging and destination directories")]
    MissingDestination,
    #[error("transfer does not exist")]
    TransferNotFound,
    #[error("transfer is not paused or no longer has retry state")]
    TransferNotPaused,
    #[error("paused transfer belongs to a different authenticated peer")]
    PeerMismatch,
    #[error("transfer service internal state is unavailable")]
    InternalState,
    #[error("transfer could not be prepared: {0}")]
    PrepareBatch(#[from] BatchTransferError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_policy_is_bounded_and_applies_admission_limit() {
        let policy = TransferPolicy {
            maximum_file_size: 1024,
            receive_decision_timeout: Duration::from_secs(30),
            progress_event_step: MIN_PROGRESS_EVENT_STEP,
            ..TransferPolicy::default()
        }
        .validate()
        .unwrap_or_else(|error| panic!("valid policy: {error}"));
        assert!(policy.accepts_file_size(1024));
        assert!(!policy.accepts_file_size(1025));
        assert!(policy.has_available_space(1024 + policy.minimum_free_space_reserve, 1024));
        assert!(!policy.has_available_space(1024 + policy.minimum_free_space_reserve - 1, 1024));

        for invalid in [
            TransferPolicy {
                maximum_file_size: 0,
                ..TransferPolicy::default()
            },
            TransferPolicy {
                maximum_file_size: MAX_FILE_SIZE + 1,
                ..TransferPolicy::default()
            },
            TransferPolicy {
                maximum_aggregate_size: 0,
                ..TransferPolicy::default()
            },
            TransferPolicy {
                maximum_file_count: MAX_BATCH_FILES + 1,
                ..TransferPolicy::default()
            },
            TransferPolicy {
                minimum_free_space_reserve: MAX_FILE_SIZE + 1,
                ..TransferPolicy::default()
            },
            TransferPolicy {
                receive_decision_timeout: Duration::ZERO,
                ..TransferPolicy::default()
            },
            TransferPolicy {
                receive_decision_timeout: MAX_DECISION_TIMEOUT + Duration::from_secs(1),
                ..TransferPolicy::default()
            },
            TransferPolicy {
                progress_event_step: MIN_PROGRESS_EVENT_STEP - 1,
                ..TransferPolicy::default()
            },
        ] {
            assert!(matches!(
                invalid.validate(),
                Err(TransferServiceError::InvalidPolicy)
            ));
        }
    }
}
