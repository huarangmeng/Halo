use std::{fmt::Debug, sync::Arc, time::Duration};

use async_trait::async_trait;
use thiserror::Error;
use tokio::{task::JoinSet, time::timeout};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionCandidate<E> {
    pub endpoint: E,
    pub start_delay: Duration,
}

#[async_trait]
pub trait SecureConnector<E>: Send + Sync + 'static
where
    E: Clone + Debug + Send + Sync + 'static,
{
    type Connection: Send + 'static;

    async fn connect(
        &self,
        endpoint: E,
        cancellation: CancellationToken,
    ) -> Result<Self::Connection, ConnectAttemptError>;
}

pub struct ConnectionRacer {
    attempt_timeout: Duration,
    max_candidates: usize,
}

impl ConnectionRacer {
    pub fn new(
        attempt_timeout: Duration,
        max_candidates: usize,
    ) -> Result<Self, ConnectAttemptError> {
        if attempt_timeout.is_zero() || max_candidates == 0 || max_candidates > 8 {
            return Err(ConnectAttemptError::new(
                ConnectErrorKind::InvalidConfiguration,
            ));
        }
        Ok(Self {
            attempt_timeout,
            max_candidates,
        })
    }

    pub async fn connect<E, C>(
        &self,
        connector: Arc<C>,
        candidates: Vec<ConnectionCandidate<E>>,
        cancellation: CancellationToken,
    ) -> Result<(E, C::Connection), ConnectAttemptError>
    where
        E: Clone + Debug + Send + Sync + 'static,
        C: SecureConnector<E>,
    {
        if candidates.is_empty() || candidates.len() > self.max_candidates {
            return Err(ConnectAttemptError::new(
                ConnectErrorKind::InvalidConfiguration,
            ));
        }

        let attempts = cancellation.child_token();
        let mut tasks = JoinSet::new();
        for candidate in candidates {
            let connector = Arc::clone(&connector);
            let token = attempts.child_token();
            let external = cancellation.clone();
            let attempt_timeout = self.attempt_timeout;
            tasks.spawn(async move {
                tokio::select! {
                    () = external.cancelled() => Err(ConnectAttemptError::new(ConnectErrorKind::Cancelled)),
                    () = tokio::time::sleep(candidate.start_delay) => {
                        let endpoint = candidate.endpoint;
                        let result = timeout(
                            attempt_timeout,
                            connector.connect(endpoint.clone(), token),
                        ).await;
                        match result {
                            Ok(Ok(connection)) => Ok((endpoint, connection)),
                            Ok(Err(error)) => Err(error),
                            Err(_) => Err(ConnectAttemptError::new(ConnectErrorKind::Timeout)),
                        }
                    }
                }
            });
        }

        let mut last_error = ConnectAttemptError::new(ConnectErrorKind::Unreachable);
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(success)) => {
                    attempts.cancel();
                    tasks.abort_all();
                    return Ok(success);
                }
                Ok(Err(error)) => last_error = error,
                Err(_) => last_error = ConnectAttemptError::new(ConnectErrorKind::InternalTask),
            }
        }
        if cancellation.is_cancelled() {
            Err(ConnectAttemptError::new(ConnectErrorKind::Cancelled))
        } else {
            Err(last_error)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectErrorKind {
    InvalidConfiguration,
    Cancelled,
    Timeout,
    Unreachable,
    Tls,
    Authentication,
    Protocol,
    IdentityChanged,
    NetworkChanged,
    InternalTask,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("secure connection attempt failed: {kind:?}")]
pub struct ConnectAttemptError {
    pub kind: ConnectErrorKind,
}

impl ConnectAttemptError {
    #[must_use]
    pub const fn new(kind: ConnectErrorKind) -> Self {
        Self { kind }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct TestConnector {
        active: Arc<AtomicUsize>,
    }

    struct ActiveGuard(Arc<AtomicUsize>);

    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl SecureConnector<u16> for TestConnector {
        type Connection = u16;

        async fn connect(
            &self,
            endpoint: u16,
            cancellation: CancellationToken,
        ) -> Result<Self::Connection, ConnectAttemptError> {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _guard = ActiveGuard(Arc::clone(&self.active));
            if endpoint == 4433 {
                Ok(endpoint)
            } else {
                tokio::select! {
                    () = cancellation.cancelled() => Err(ConnectAttemptError::new(ConnectErrorKind::Cancelled)),
                    () = tokio::time::sleep(Duration::from_secs(30)) => Err(ConnectAttemptError::new(ConnectErrorKind::Unreachable)),
                }
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn first_success_cancels_losing_attempts() {
        let active = Arc::new(AtomicUsize::new(0));
        let connector = Arc::new(TestConnector {
            active: Arc::clone(&active),
        });
        let racer = ConnectionRacer::new(Duration::from_secs(5), 3)
            .unwrap_or_else(|error| panic!("racer: {error}"));
        let result = racer
            .connect(
                connector,
                vec![
                    ConnectionCandidate {
                        endpoint: 1,
                        start_delay: Duration::ZERO,
                    },
                    ConnectionCandidate {
                        endpoint: 4433,
                        start_delay: Duration::from_millis(50),
                    },
                ],
                CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|error| panic!("race: {error}"));
        assert_eq!(result, (4433, 4433));
        tokio::task::yield_now().await;
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn external_cancellation_stops_the_race() {
        let connector = Arc::new(TestConnector {
            active: Arc::new(AtomicUsize::new(0)),
        });
        let racer = ConnectionRacer::new(Duration::from_secs(5), 1)
            .unwrap_or_else(|error| panic!("racer: {error}"));
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = racer
            .connect(
                connector,
                vec![ConnectionCandidate {
                    endpoint: 1,
                    start_delay: Duration::ZERO,
                }],
                cancellation,
            )
            .await;
        assert_eq!(
            result,
            Err(ConnectAttemptError::new(ConnectErrorKind::Cancelled))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_unresponsive_candidate_hits_the_bounded_timeout() {
        let connector = Arc::new(TestConnector {
            active: Arc::new(AtomicUsize::new(0)),
        });
        let racer = ConnectionRacer::new(Duration::from_secs(2), 1)
            .unwrap_or_else(|error| panic!("racer: {error}"));
        let result = racer
            .connect(
                connector,
                vec![ConnectionCandidate {
                    endpoint: 1,
                    start_delay: Duration::ZERO,
                }],
                CancellationToken::new(),
            )
            .await;
        assert_eq!(
            result,
            Err(ConnectAttemptError::new(ConnectErrorKind::Timeout))
        );
    }
}
