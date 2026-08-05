use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use halo_protocol::{MAX_FILE_SIZE, TRANSFER_ID_LEN, TransferOffer};
use halo_transfer::{
    TransferError, prepare_file, receive_file_data_with_progress, receive_offer, send_complete,
    send_decision, send_file_data_with_progress, send_offer, wait_for_complete,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferPolicy {
    pub maximum_file_size: u64,
    pub receive_decision_timeout: Duration,
    pub progress_event_step: u64,
}

impl Default for TransferPolicy {
    fn default() -> Self {
        Self {
            maximum_file_size: 10 * 1024 * 1024 * 1024,
            receive_decision_timeout: Duration::from_secs(60),
            progress_event_step: 1024 * 1024,
        }
    }
}

impl TransferPolicy {
    pub fn validate(self) -> Result<Self, TransferServiceError> {
        if self.maximum_file_size == 0
            || self.maximum_file_size > MAX_FILE_SIZE
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferDirection {
    Sending,
    Receiving,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferEventKind {
    OfferReceived,
    AwaitingDecision,
    Transferring,
    Completed,
    Rejected,
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
    pub file_size: u64,
    pub transferred_bytes: u64,
    /// Present after a received file has passed all checks and was finalized.
    pub final_path: Option<PathBuf>,
    /// Stable category only; never contains addresses or filesystem paths.
    pub detail: Option<String>,
}

pub(crate) struct ReceiveTransferDecision {
    pub accepted: bool,
    pub staging_directory: Option<PathBuf>,
    pub destination_directory: Option<PathBuf>,
}

struct TransferSession {
    connection: QuicConnection,
    gate: Arc<Semaphore>,
    cancellation: CancellationToken,
}

pub(crate) struct TransferCoordinator {
    next_event_id: AtomicU64,
    next_request_id: AtomicU64,
    events: Mutex<VecDeque<TransferEvent>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<ReceiveTransferDecision>>>,
    active: Mutex<HashMap<String, CancellationToken>>,
    sessions: Mutex<HashMap<u64, Arc<TransferSession>>>,
    cancellation: CancellationToken,
    policy: TransferPolicy,
}

impl Default for TransferCoordinator {
    fn default() -> Self {
        Self::from_validated(TransferPolicy::default())
    }
}

impl TransferCoordinator {
    pub(crate) fn new(policy: TransferPolicy) -> Result<Self, TransferServiceError> {
        Ok(Self::from_validated(policy.validate()?))
    }

    fn from_validated(policy: TransferPolicy) -> Self {
        Self {
            next_event_id: AtomicU64::new(0),
            next_request_id: AtomicU64::new(0),
            events: Mutex::new(VecDeque::new()),
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            cancellation: CancellationToken::new(),
            policy,
        }
    }

    pub(crate) fn attach_session(
        self: &Arc<Self>,
        session_id: u64,
        connection: QuicConnection,
    ) -> Result<(), TransferServiceError> {
        let session = Arc::new(TransferSession {
            connection,
            gate: Arc::new(Semaphore::new(1)),
            cancellation: self.cancellation.child_token(),
        });
        self.sessions
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .insert(session_id, Arc::clone(&session));
        let coordinator = Arc::clone(self);
        tokio::spawn(async move {
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
        let session = self.session(session_id)?;
        let permit = Arc::clone(&session.gate)
            .try_acquire_owned()
            .map_err(|_| TransferServiceError::Busy)?;
        let cancellation = session.cancellation.child_token();
        let prepared = prepare_file(source_path, advertised_name, &cancellation).await?;
        let offer = prepared.offer().clone();
        if !self.policy.accepts_file_size(offer.file_size) {
            return Err(TransferServiceError::FileRejectedByPolicy);
        }
        let transfer_id = transfer_id_text(offer.transfer_id);
        self.insert_active(transfer_id.clone(), cancellation.clone())?;
        self.emit(event_for_offer(
            session_id,
            &offer,
            TransferDirection::Sending,
            TransferEventKind::AwaitingDecision,
        ))?;

        let coordinator = Arc::clone(self);
        let task_transfer_id = transfer_id.clone();
        tokio::spawn(async move {
            let result = coordinator
                .run_sender(session_id, session, prepared, cancellation, permit)
                .await;
            coordinator.finish_task(
                session_id,
                &offer,
                TransferDirection::Sending,
                &task_transfer_id,
                result,
            );
        });
        Ok(transfer_id)
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

    pub(crate) fn cancel(&self, transfer_id: &str) -> Result<(), TransferServiceError> {
        let cancellation = self
            .active
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .get(transfer_id)
            .cloned()
            .ok_or(TransferServiceError::TransferNotFound)?;
        cancellation.cancel();
        Ok(())
    }

    pub(crate) fn shutdown(&self) {
        self.cancellation.cancel();
        if let Ok(mut active) = self.active.lock() {
            for cancellation in active.values() {
                cancellation.cancel();
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
            self.handle_incoming(session_id, Arc::clone(&session), control)
                .await;
        }
    }

    async fn handle_incoming(
        self: &Arc<Self>,
        session_id: u64,
        session: Arc<TransferSession>,
        mut control: impl ControlIo,
    ) {
        let binding = control.channel_binding();
        let offer = match receive_offer(&mut control).await {
            Ok(offer) => offer,
            Err(_) => {
                control.close().await;
                return;
            }
        };
        let transfer_id = transfer_id_text(offer.transfer_id);
        if !self.policy.accepts_file_size(offer.file_size) {
            let _ = send_decision(&mut control, offer.transfer_id, false).await;
            control.close().await;
            self.emit_ignoring_closed(event_with_detail(
                event_for_offer(
                    session_id,
                    &offer,
                    TransferDirection::Receiving,
                    TransferEventKind::Rejected,
                ),
                "file_size_policy",
            ));
            return;
        }
        let permit = match Arc::clone(&session.gate).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _ = send_decision(&mut control, offer.transfer_id, false).await;
                control.close().await;
                self.emit_ignoring_closed(event_with_detail(
                    event_for_offer(
                        session_id,
                        &offer,
                        TransferDirection::Receiving,
                        TransferEventKind::Rejected,
                    ),
                    "session_busy",
                ));
                return;
            }
        };
        let cancellation = session.cancellation.child_token();
        if self
            .insert_active(transfer_id.clone(), cancellation.clone())
            .is_err()
        {
            let _ = send_decision(&mut control, offer.transfer_id, false).await;
            control.close().await;
            return;
        }
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
        let mut event = event_for_offer(
            session_id,
            &offer,
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
        let result = match decision {
            Some(decision) if decision.accepted => {
                let Some(staging) = decision.staging_directory else {
                    self.remove_active(&transfer_id);
                    control.close().await;
                    return;
                };
                let Some(destination) = decision.destination_directory else {
                    self.remove_active(&transfer_id);
                    control.close().await;
                    return;
                };
                if send_decision(&mut control, offer.transfer_id, true)
                    .await
                    .is_err()
                {
                    Err(TransferError::UnexpectedMessage)
                } else {
                    self.emit_ignoring_closed(event_for_offer(
                        session_id,
                        &offer,
                        TransferDirection::Receiving,
                        TransferEventKind::Transferring,
                    ));
                    self.run_receiver(
                        &session,
                        session_id,
                        &mut control,
                        binding,
                        &offer,
                        staging,
                        destination,
                        &cancellation,
                    )
                    .await
                }
            }
            _ if cancellation.is_cancelled() => Err(TransferError::Cancelled),
            _ => {
                let _ = send_decision(&mut control, offer.transfer_id, false).await;
                Err(TransferError::Rejected)
            }
        };
        control.close().await;
        drop(permit);
        self.finish_task(
            session_id,
            &offer,
            TransferDirection::Receiving,
            &transfer_id,
            result,
        );
    }

    async fn run_sender(
        &self,
        session_id: u64,
        session: Arc<TransferSession>,
        prepared: halo_transfer::PreparedFile,
        cancellation: CancellationToken,
        _permit: OwnedSemaphorePermit,
    ) -> Result<Option<PathBuf>, TransferError> {
        let mut control = tokio::select! {
            () = cancellation.cancelled() => return Err(TransferError::Cancelled),
            result = session.connection.open_control() => result?,
        };
        let binding = control.channel_binding();
        let accepted = tokio::select! {
            () = cancellation.cancelled() => return Err(TransferError::Cancelled),
            result = send_offer(&mut control, &prepared) => result?,
        };
        if !accepted {
            control.close().await;
            return Err(TransferError::Rejected);
        }
        self.emit_ignoring_closed(event_for_offer(
            session_id,
            prepared.offer(),
            TransferDirection::Sending,
            TransferEventKind::Transferring,
        ));
        let mut data = session.connection.open_data().await?;
        let mut last_progress = 0_u64;
        let result = send_file_data_with_progress(
            &mut data,
            binding,
            &prepared,
            &cancellation,
            |transferred_bytes| {
                if transferred_bytes == prepared.offer().file_size
                    || transferred_bytes.saturating_sub(last_progress)
                        >= self.policy.progress_event_step
                {
                    last_progress = transferred_bytes;
                    self.emit_progress(
                        session_id,
                        prepared.offer(),
                        TransferDirection::Sending,
                        transferred_bytes,
                    );
                }
            },
        )
        .await;
        data.close().await;
        result?;
        tokio::select! {
            () = cancellation.cancelled() => return Err(TransferError::Cancelled),
            result = wait_for_complete(&mut control, &prepared) => result?,
        }
        control.close().await;
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_receiver(
        &self,
        session: &TransferSession,
        session_id: u64,
        control: &mut dyn ControlIo,
        binding: halo_crypto::TlsChannelBinding,
        offer: &TransferOffer,
        staging: PathBuf,
        destination: PathBuf,
        cancellation: &CancellationToken,
    ) -> Result<Option<PathBuf>, TransferError> {
        let mut data = tokio::select! {
            () = cancellation.cancelled() => return Err(TransferError::Cancelled),
            result = session.connection.accept_data() => result?,
        };
        let mut last_progress = 0_u64;
        let result = receive_file_data_with_progress(
            &mut data,
            binding,
            offer,
            &staging,
            &destination,
            cancellation,
            |transferred_bytes| {
                if transferred_bytes == offer.file_size
                    || transferred_bytes.saturating_sub(last_progress)
                        >= self.policy.progress_event_step
                {
                    last_progress = transferred_bytes;
                    self.emit_progress(
                        session_id,
                        offer,
                        TransferDirection::Receiving,
                        transferred_bytes,
                    );
                }
            },
        )
        .await;
        data.close().await;
        let received = result?;
        send_complete(control, &received).await?;
        Ok(Some(received.final_path))
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
    ) -> Result<(), TransferServiceError> {
        if self
            .active
            .lock()
            .map_err(|_| TransferServiceError::InternalState)?
            .insert(transfer_id, cancellation)
            .is_some()
        {
            return Err(TransferServiceError::InternalState);
        }
        Ok(())
    }

    fn remove_active(&self, transfer_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(transfer_id);
        }
    }

    fn finish_task(
        &self,
        session_id: u64,
        offer: &TransferOffer,
        direction: TransferDirection,
        transfer_id: &str,
        result: Result<Option<PathBuf>, TransferError>,
    ) {
        self.remove_active(transfer_id);
        let (kind, final_path, detail) = match result {
            Ok(final_path) => (TransferEventKind::Completed, final_path, None),
            Err(TransferError::Rejected) => (
                TransferEventKind::Rejected,
                None,
                Some("rejected".to_owned()),
            ),
            Err(TransferError::Cancelled) => (
                TransferEventKind::Cancelled,
                None,
                Some("cancelled".to_owned()),
            ),
            Err(error) => (
                TransferEventKind::Failed,
                None,
                Some(transfer_error_category(&error).to_owned()),
            ),
        };
        let mut event = event_for_offer(session_id, offer, direction, kind);
        if kind == TransferEventKind::Completed {
            event.transferred_bytes = offer.file_size;
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

    fn emit_progress(
        &self,
        session_id: u64,
        offer: &TransferOffer,
        direction: TransferDirection,
        transferred_bytes: u64,
    ) {
        let mut event = event_for_offer(
            session_id,
            offer,
            direction,
            TransferEventKind::Transferring,
        );
        event.transferred_bytes = transferred_bytes.min(offer.file_size);
        self.emit_ignoring_closed(event);
    }
}

fn event_for_offer(
    session_id: u64,
    offer: &TransferOffer,
    direction: TransferDirection,
    kind: TransferEventKind,
) -> TransferEvent {
    TransferEvent {
        event_id: 0,
        request_id: None,
        authenticated_session_id: session_id,
        transfer_id: transfer_id_text(offer.transfer_id),
        direction,
        kind,
        file_name: offer.file_name.clone(),
        file_size: offer.file_size,
        transferred_bytes: 0,
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

fn transfer_error_category(error: &TransferError) -> &'static str {
    match error {
        TransferError::Protocol(_) | TransferError::UnexpectedMessage => "protocol",
        TransferError::Control(_) | TransferError::Data(_) => "transport",
        TransferError::InvalidFileName => "invalid_file_name",
        TransferError::Source => "source",
        TransferError::SourceChanged => "source_changed",
        TransferError::Rejected => "rejected",
        TransferError::ChannelBinding => "channel_binding",
        TransferError::Integrity => "integrity",
        TransferError::Staging => "staging",
        TransferError::DestinationExists => "destination_exists",
        TransferError::Storage => "storage",
        TransferError::Finalization => "finalization",
        TransferError::Randomness => "randomness",
        TransferError::Cancelled => "cancelled",
    }
}

#[derive(Debug, Error)]
pub enum TransferServiceError {
    #[error("transfer policy configuration is invalid")]
    InvalidPolicy,
    #[error("file exceeds the configured transfer admission policy")]
    FileRejectedByPolicy,
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
    #[error("transfer service internal state is unavailable")]
    InternalState,
    #[error("file could not be prepared: {0}")]
    Prepare(#[from] TransferError),
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
        }
        .validate()
        .unwrap_or_else(|error| panic!("valid policy: {error}"));
        assert!(policy.accepts_file_size(1024));
        assert!(!policy.accepts_file_size(1025));

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
