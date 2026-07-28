use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{LocalPresence, Observation, ProviderError, ProviderId, ProviderState};

#[derive(Debug)]
pub(crate) enum ManagerInput {
    Observation(Observation),
    Withdraw {
        provider: ProviderId,
        presence_id: crate::PresenceId,
    },
    ProviderState {
        provider: ProviderId,
        state: ProviderState,
    },
    ConnectionResult {
        presence_id: crate::PresenceId,
        endpoint: crate::Endpoint,
        outcome: crate::ConnectionOutcome,
    },
    Snapshot(tokio::sync::oneshot::Sender<Vec<crate::PeerSnapshot>>),
}

/// Restricted capabilities passed to a discovery provider.
#[derive(Clone)]
pub struct ProviderContext {
    local: LocalPresence,
    input: mpsc::Sender<ManagerInput>,
    cancel: CancellationToken,
}

impl ProviderContext {
    pub(crate) fn new(
        local: LocalPresence,
        input: mpsc::Sender<ManagerInput>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            local,
            input,
            cancel,
        }
    }

    #[must_use]
    pub fn local(&self) -> &LocalPresence {
        &self.local
    }

    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub async fn observe(&self, observation: Observation) -> Result<(), ProviderError> {
        self.input
            .send(ManagerInput::Observation(observation))
            .await
            .map_err(|_| ProviderError::EventStreamClosed)
    }

    pub async fn withdraw(
        &self,
        provider: ProviderId,
        presence_id: crate::PresenceId,
    ) -> Result<(), ProviderError> {
        self.input
            .send(ManagerInput::Withdraw {
                provider,
                presence_id,
            })
            .await
            .map_err(|_| ProviderError::EventStreamClosed)
    }

    pub async fn set_state(
        &self,
        provider: ProviderId,
        state: ProviderState,
    ) -> Result<(), ProviderError> {
        self.input
            .send(ManagerInput::ProviderState { provider, state })
            .await
            .map_err(|_| ProviderError::EventStreamClosed)
    }
}

/// One independently failing source of discovery observations.
#[async_trait]
pub trait DiscoveryProvider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;

    async fn run(&self, context: ProviderContext) -> Result<(), ProviderError>;
}
