use std::{collections::HashSet, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use thiserror::Error;
use tokio::{task::JoinSet, time::timeout};
use tokio_util::sync::CancellationToken;

const MAX_PROVIDERS: usize = 8;
const MAX_CANDIDATES: usize = 8;
const MAX_SYSTEM_PROMPT_ATTEMPTS: usize = 4;
const CANDIDATE_STAGGER: Duration = Duration::from_millis(100);

/// Opaque correlation with one untrusted discovery presence.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DataChannelPeer([u8; 16]);

impl DataChannelPeer {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Opaque identifier meaningful only to the provider that produced it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DataChannelCandidateId([u8; 16]);

impl DataChannelCandidateId {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum DataChannelKind {
    Lan,
    ApplePeerToPeer,
    WifiDirect,
    WifiAware,
}

/// Runtime provider state. States are independent: one unavailable provider
/// never prevents another provider from establishing a channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DataChannelCapabilityState {
    Starting,
    Ready,
    PermissionRequired,
    PermissionDenied,
    HardwareOff,
    Unsupported,
    TemporarilyUnavailable,
    Failed,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataChannelCapability {
    pub kind: DataChannelKind,
    pub state: DataChannelCapabilityState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataChannelPathClass {
    LocalNetwork,
    PeerToPeer,
    Cellular,
    Internet,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataChannelCost {
    Unmetered,
    Metered,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EstablishedPathProperties {
    pub path_class: DataChannelPathClass,
    pub cost: DataChannelCost,
    /// True only when the provider pinned the socket/native connection to the
    /// eligible interface or OS network object it validated.
    pub interface_bound: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataChannelPolicy {
    pub allow_metered: bool,
    pub allow_unknown_cost: bool,
    pub allow_system_prompt: bool,
    pub max_system_prompt_attempts: usize,
    pub require_interface_binding: bool,
}

impl Default for DataChannelPolicy {
    fn default() -> Self {
        Self {
            allow_metered: false,
            allow_unknown_cost: false,
            allow_system_prompt: true,
            max_system_prompt_attempts: 1,
            require_interface_binding: true,
        }
    }
}

impl DataChannelPolicy {
    pub fn validate(self) -> Result<Self, DataChannelError> {
        if self.max_system_prompt_attempts > MAX_SYSTEM_PROMPT_ATTEMPTS {
            return Err(DataChannelError::InvalidConfiguration);
        }
        Ok(self)
    }

    fn allows_cost(self, cost: DataChannelCost) -> bool {
        match cost {
            DataChannelCost::Unmetered => true,
            DataChannelCost::Metered => self.allow_metered,
            DataChannelCost::Unknown => self.allow_unknown_cost,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataChannelCandidateProperties {
    pub path_class: DataChannelPathClass,
    pub cost: DataChannelCost,
    pub already_available: bool,
    pub requires_user_action: bool,
    pub estimated_round_trip_time: Option<Duration>,
}

/// One provider-owned path candidate. It is not authenticated and must never be
/// treated as a trusted peer until the later Halo handshake succeeds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataChannelCandidate {
    id: DataChannelCandidateId,
    peer: DataChannelPeer,
    kind: DataChannelKind,
    path_class: DataChannelPathClass,
    cost: DataChannelCost,
    already_available: bool,
    requires_user_action: bool,
    estimated_round_trip_time: Option<Duration>,
}

impl DataChannelCandidate {
    pub fn new(
        id: DataChannelCandidateId,
        peer: DataChannelPeer,
        kind: DataChannelKind,
        properties: DataChannelCandidateProperties,
    ) -> Result<Self, DataChannelError> {
        let DataChannelCandidateProperties {
            path_class,
            cost,
            already_available,
            requires_user_action,
            estimated_round_trip_time,
        } = properties;
        if !matches!(
            path_class,
            DataChannelPathClass::LocalNetwork | DataChannelPathClass::PeerToPeer
        ) {
            return Err(DataChannelError::ProhibitedPath);
        }
        Ok(Self {
            id,
            peer,
            kind,
            path_class,
            cost,
            already_available,
            requires_user_action,
            estimated_round_trip_time,
        })
    }

    #[must_use]
    pub const fn id(&self) -> DataChannelCandidateId {
        self.id
    }

    #[must_use]
    pub const fn peer(&self) -> DataChannelPeer {
        self.peer
    }

    #[must_use]
    pub const fn kind(&self) -> DataChannelKind {
        self.kind
    }

    #[must_use]
    pub const fn path_class(&self) -> DataChannelPathClass {
        self.path_class
    }

    #[must_use]
    pub const fn cost(&self) -> DataChannelCost {
        self.cost
    }

    #[must_use]
    pub const fn already_available(&self) -> bool {
        self.already_available
    }

    #[must_use]
    pub const fn requires_user_action(&self) -> bool {
        self.requires_user_action
    }

    #[must_use]
    pub const fn estimated_round_trip_time(&self) -> Option<Duration> {
        self.estimated_round_trip_time
    }
}

/// Provider-specific established bearer. This is still unauthenticated. The
/// transport layer must create QUIC and complete Halo pairing before exposing
/// file metadata or a trusted peer.
pub trait EstablishedDataChannel: Send + Sync {
    fn kind(&self) -> DataChannelKind;
    fn peer(&self) -> DataChannelPeer;
    fn path_properties(&self) -> EstablishedPathProperties;
    fn close(&self);
}

/// Completes QUIC plus Halo exporter-bound identity verification. An
/// established bearer is not allowed to win the broker race until this
/// operation succeeds.
#[async_trait]
pub trait DataChannelAuthenticator: Send + Sync + 'static {
    async fn authenticate(
        &self,
        candidate: &DataChannelCandidate,
        channel: &dyn EstablishedDataChannel,
        cancellation: CancellationToken,
    ) -> Result<(), DataChannelError>;
}

#[async_trait]
pub trait DataChannelProvider: Send + Sync + 'static {
    fn capability(&self) -> DataChannelCapability;

    async fn candidates(
        &self,
        peer: DataChannelPeer,
        cancellation: CancellationToken,
    ) -> Result<Vec<DataChannelCandidate>, DataChannelError>;

    async fn establish(
        &self,
        candidate: &DataChannelCandidate,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn EstablishedDataChannel>, DataChannelError>;
}

pub struct EstablishedDataChannelResult {
    pub candidate: DataChannelCandidate,
    pub channel: Box<dyn EstablishedDataChannel>,
}

pub struct AuthenticatedDataChannelResult {
    pub candidate: DataChannelCandidate,
    pub channel: Box<dyn EstablishedDataChannel>,
}

impl fmt::Debug for AuthenticatedDataChannelResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedDataChannelResult")
            .field("candidate", &self.candidate)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for EstablishedDataChannelResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EstablishedDataChannelResult")
            .field("candidate", &self.candidate)
            .finish_non_exhaustive()
    }
}

/// Collects candidates from independent providers and races a bounded ordered
/// set. Product workflows use `establish_authenticated`, where only a bearer
/// that also passes Halo authentication can win.
pub struct DataChannelBroker {
    providers: Vec<Arc<dyn DataChannelProvider>>,
    attempt_timeout: Duration,
    policy: DataChannelPolicy,
}

impl DataChannelBroker {
    pub fn new(
        providers: Vec<Arc<dyn DataChannelProvider>>,
        attempt_timeout: Duration,
    ) -> Result<Self, DataChannelError> {
        if providers.is_empty() || providers.len() > MAX_PROVIDERS || attempt_timeout.is_zero() {
            return Err(DataChannelError::InvalidConfiguration);
        }
        Ok(Self {
            providers,
            attempt_timeout,
            policy: DataChannelPolicy::default(),
        })
    }

    pub fn with_policy(mut self, policy: DataChannelPolicy) -> Result<Self, DataChannelError> {
        self.policy = policy.validate()?;
        Ok(self)
    }

    #[must_use]
    pub fn capabilities(&self) -> Vec<DataChannelCapability> {
        self.providers
            .iter()
            .map(|provider| provider.capability())
            .collect()
    }

    pub async fn establish(
        &self,
        peer: DataChannelPeer,
        cancellation: CancellationToken,
    ) -> Result<EstablishedDataChannelResult, DataChannelError> {
        self.establish_inner(peer, None, cancellation).await
    }

    pub async fn establish_authenticated(
        &self,
        peer: DataChannelPeer,
        authenticator: Arc<dyn DataChannelAuthenticator>,
        cancellation: CancellationToken,
    ) -> Result<AuthenticatedDataChannelResult, DataChannelError> {
        self.establish_inner(peer, Some(authenticator), cancellation)
            .await
            .map(|result| AuthenticatedDataChannelResult {
                candidate: result.candidate,
                channel: result.channel,
            })
    }

    async fn establish_inner(
        &self,
        peer: DataChannelPeer,
        authenticator: Option<Arc<dyn DataChannelAuthenticator>>,
        cancellation: CancellationToken,
    ) -> Result<EstablishedDataChannelResult, DataChannelError> {
        if cancellation.is_cancelled() {
            return Err(DataChannelError::Cancelled);
        }

        let mut discovery_tasks = JoinSet::new();
        for provider in &self.providers {
            if provider.capability().state != DataChannelCapabilityState::Ready {
                continue;
            }
            let provider = Arc::clone(provider);
            let token = cancellation.child_token();
            let external = cancellation.clone();
            let candidate_timeout = self.attempt_timeout;
            discovery_tasks.spawn(async move {
                let candidates = tokio::select! {
                    () = external.cancelled() => Err(DataChannelError::Cancelled),
                    result = timeout(candidate_timeout, provider.candidates(peer, token)) => {
                        match result {
                            Ok(result) => result,
                            Err(_) => Err(DataChannelError::Timeout),
                        }
                    }
                };
                (provider, candidates)
            });
        }

        let mut candidates = Vec::new();
        let mut candidate_ids = HashSet::new();
        let mut last_collection_error = None;
        while let Some(result) = discovery_tasks.join_next().await {
            match result {
                Ok((provider, Ok(provider_candidates))) => {
                    for candidate in provider_candidates.into_iter().take(MAX_CANDIDATES) {
                        if candidate.peer == peer
                            && candidate.kind == provider.capability().kind
                            && matches!(
                                candidate.path_class,
                                DataChannelPathClass::LocalNetwork
                                    | DataChannelPathClass::PeerToPeer
                            )
                            && self.policy.allows_cost(candidate.cost)
                            && (!candidate.requires_user_action
                                || (self.policy.allow_system_prompt
                                    && self.policy.max_system_prompt_attempts > 0))
                            && candidate_ids.insert((candidate.kind, candidate.id))
                        {
                            candidates.push((provider.clone(), candidate));
                        }
                    }
                }
                Ok((_, Err(error))) => last_collection_error = Some(error),
                Err(_) => last_collection_error = Some(DataChannelError::ProviderFailed),
            }
        }
        if cancellation.is_cancelled() {
            return Err(DataChannelError::Cancelled);
        }
        if candidates.is_empty() {
            return Err(last_collection_error.unwrap_or(DataChannelError::NoCandidates));
        }

        candidates.sort_unstable_by(|left, right| {
            candidate_score(&right.1)
                .cmp(&candidate_score(&left.1))
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        candidates.truncate(MAX_CANDIDATES);

        let (automatic, prompted): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|(_, candidate)| !candidate.requires_user_action);
        let mut last_error = DataChannelError::NoCandidates;
        if !automatic.is_empty() {
            match self
                .race_candidates(automatic, authenticator.clone(), cancellation.clone())
                .await
            {
                Ok(result) => return Ok(result),
                Err(DataChannelError::Cancelled) => return Err(DataChannelError::Cancelled),
                Err(
                    error
                    @ (DataChannelError::PeerIdentityChanged | DataChannelError::UserRejected),
                ) => return Err(error),
                Err(error) => last_error = error,
            }
        }

        for candidate in prompted
            .into_iter()
            .take(self.policy.max_system_prompt_attempts)
        {
            match self
                .race_candidates(vec![candidate], authenticator.clone(), cancellation.clone())
                .await
            {
                Ok(result) => return Ok(result),
                Err(DataChannelError::Cancelled) => return Err(DataChannelError::Cancelled),
                Err(
                    error
                    @ (DataChannelError::PeerIdentityChanged | DataChannelError::UserRejected),
                ) => return Err(error),
                Err(error) => last_error = error,
            }
        }
        if cancellation.is_cancelled() {
            Err(DataChannelError::Cancelled)
        } else {
            Err(last_error)
        }
    }

    async fn race_candidates(
        &self,
        candidates: Vec<(Arc<dyn DataChannelProvider>, DataChannelCandidate)>,
        authenticator: Option<Arc<dyn DataChannelAuthenticator>>,
        cancellation: CancellationToken,
    ) -> Result<EstablishedDataChannelResult, DataChannelError> {
        let attempts = cancellation.child_token();
        let mut tasks = JoinSet::new();
        for (index, (provider, candidate)) in candidates.into_iter().enumerate() {
            let token = attempts.child_token();
            let external = cancellation.clone();
            let attempt_timeout = self.attempt_timeout;
            let authenticator = authenticator.clone();
            let policy = self.policy;
            tasks.spawn(async move {
                let delay = CANDIDATE_STAGGER.saturating_mul(index as u32);
                tokio::select! {
                    () = external.cancelled() => Err(DataChannelError::Cancelled),
                    () = tokio::time::sleep(delay) => {
                        tokio::select! {
                            () = external.cancelled() => Err(DataChannelError::Cancelled),
                            result = timeout(
                                attempt_timeout,
                                establish_and_authenticate(
                                    provider,
                                    candidate,
                                    authenticator,
                                    policy,
                                    token,
                                ),
                            ) => match result {
                                Ok(result) => result,
                                Err(_) => Err(DataChannelError::Timeout),
                            },
                        }
                    }
                }
            });
        }

        let mut last_error = DataChannelError::Unavailable;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(channel)) => {
                    attempts.cancel();
                    tasks.abort_all();
                    return Ok(channel);
                }
                Ok(Err(
                    error
                    @ (DataChannelError::PeerIdentityChanged | DataChannelError::UserRejected),
                )) => {
                    attempts.cancel();
                    tasks.abort_all();
                    return Err(error);
                }
                Ok(Err(error)) => last_error = error,
                Err(_) => last_error = DataChannelError::ProviderFailed,
            }
        }
        if cancellation.is_cancelled() {
            Err(DataChannelError::Cancelled)
        } else {
            Err(last_error)
        }
    }
}

async fn establish_and_authenticate(
    provider: Arc<dyn DataChannelProvider>,
    candidate: DataChannelCandidate,
    authenticator: Option<Arc<dyn DataChannelAuthenticator>>,
    policy: DataChannelPolicy,
    cancellation: CancellationToken,
) -> Result<EstablishedDataChannelResult, DataChannelError> {
    let channel = provider
        .establish(&candidate, cancellation.child_token())
        .await?;
    let properties = channel.path_properties();
    if channel.kind() != candidate.kind
        || channel.peer() != candidate.peer
        || properties.path_class != candidate.path_class
        || properties.cost != candidate.cost
        || !matches!(
            properties.path_class,
            DataChannelPathClass::LocalNetwork | DataChannelPathClass::PeerToPeer
        )
        || !policy.allows_cost(properties.cost)
        || (policy.require_interface_binding && !properties.interface_bound)
    {
        channel.close();
        return Err(DataChannelError::ProhibitedPath);
    }
    if let Some(authenticator) = authenticator
        && let Err(error) = authenticator
            .authenticate(&candidate, channel.as_ref(), cancellation)
            .await
    {
        channel.close();
        return Err(error);
    }
    Ok(EstablishedDataChannelResult { candidate, channel })
}

fn candidate_score(candidate: &DataChannelCandidate) -> i32 {
    let mut score = match candidate.path_class {
        DataChannelPathClass::LocalNetwork => 300,
        DataChannelPathClass::PeerToPeer => 250,
        DataChannelPathClass::Cellular
        | DataChannelPathClass::Internet
        | DataChannelPathClass::Unknown => i32::MIN,
    };
    if candidate.already_available {
        score += 200;
    }
    if !candidate.requires_user_action {
        score += 50;
    }
    score += match candidate.cost {
        DataChannelCost::Unmetered => 30,
        DataChannelCost::Metered => 0,
        DataChannelCost::Unknown => -20,
    };
    score += match candidate.estimated_round_trip_time {
        Some(rtt) if rtt <= Duration::from_millis(20) => 40,
        Some(rtt) if rtt <= Duration::from_millis(100) => 25,
        Some(rtt) if rtt <= Duration::from_millis(500) => 10,
        _ => 0,
    };
    score
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DataChannelError {
    #[error("invalid data-channel broker configuration")]
    InvalidConfiguration,
    #[error("data-channel candidate uses a prohibited path")]
    ProhibitedPath,
    #[error("no eligible data-channel candidates are available")]
    NoCandidates,
    #[error("data-channel provider is unavailable")]
    Unavailable,
    #[error("data-channel establishment was cancelled")]
    Cancelled,
    #[error("data-channel establishment timed out")]
    Timeout,
    #[error("data-channel provider failed")]
    ProviderFailed,
    #[error("data-channel authentication failed")]
    AuthenticationFailed,
    #[error("the selected peer presented a changed authenticated identity")]
    PeerIdentityChanged,
    #[error("the user rejected secure connection establishment")]
    UserRejected,
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    struct TestChannel {
        kind: DataChannelKind,
        peer: DataChannelPeer,
        properties: EstablishedPathProperties,
        closed: Arc<AtomicBool>,
    }

    impl EstablishedDataChannel for TestChannel {
        fn kind(&self) -> DataChannelKind {
            self.kind
        }

        fn peer(&self) -> DataChannelPeer {
            self.peer
        }

        fn path_properties(&self) -> EstablishedPathProperties {
            self.properties
        }

        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    struct TestProvider {
        capability: DataChannelCapability,
        candidates: Vec<DataChannelCandidate>,
        delay: Duration,
        succeeds: bool,
        released: Arc<AtomicBool>,
        channel_closed: Arc<AtomicBool>,
        interface_bound: bool,
        attempts: Mutex<usize>,
    }

    struct EstablishGuard(Arc<AtomicBool>);

    impl Drop for EstablishGuard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl DataChannelProvider for TestProvider {
        fn capability(&self) -> DataChannelCapability {
            self.capability
        }

        async fn candidates(
            &self,
            _peer: DataChannelPeer,
            _cancellation: CancellationToken,
        ) -> Result<Vec<DataChannelCandidate>, DataChannelError> {
            Ok(self.candidates.clone())
        }

        async fn establish(
            &self,
            candidate: &DataChannelCandidate,
            cancellation: CancellationToken,
        ) -> Result<Box<dyn EstablishedDataChannel>, DataChannelError> {
            *self
                .attempts
                .lock()
                .map_err(|_| DataChannelError::ProviderFailed)? += 1;
            let _guard = EstablishGuard(Arc::clone(&self.released));
            tokio::select! {
                () = cancellation.cancelled() => {
                    Err(DataChannelError::Cancelled)
                }
                () = tokio::time::sleep(self.delay) => {
                    if !self.succeeds {
                        return Err(DataChannelError::ProviderFailed);
                    }
                    Ok(Box::new(TestChannel {
                        kind: candidate.kind,
                        peer: candidate.peer,
                        properties: EstablishedPathProperties {
                            path_class: candidate.path_class,
                            cost: candidate.cost,
                            interface_bound: self.interface_bound,
                        },
                        closed: Arc::clone(&self.channel_closed),
                    }))
                }
            }
        }
    }

    fn candidate(
        id: u8,
        peer: DataChannelPeer,
        kind: DataChannelKind,
        path: DataChannelPathClass,
        already_available: bool,
    ) -> DataChannelCandidate {
        DataChannelCandidate::new(
            DataChannelCandidateId::from_bytes([id; 16]),
            peer,
            kind,
            DataChannelCandidateProperties {
                path_class: path,
                cost: DataChannelCost::Unmetered,
                already_available,
                requires_user_action: false,
                estimated_round_trip_time: Some(Duration::from_millis(10)),
            },
        )
        .unwrap_or_else(|error| panic!("candidate: {error}"))
    }

    fn provider(
        capability: DataChannelCapability,
        candidates: Vec<DataChannelCandidate>,
        delay: Duration,
        succeeds: bool,
    ) -> Arc<TestProvider> {
        provider_with_binding(capability, candidates, delay, succeeds, true)
    }

    fn provider_with_binding(
        capability: DataChannelCapability,
        candidates: Vec<DataChannelCandidate>,
        delay: Duration,
        succeeds: bool,
        interface_bound: bool,
    ) -> Arc<TestProvider> {
        Arc::new(TestProvider {
            capability,
            candidates,
            delay,
            succeeds,
            released: Arc::new(AtomicBool::new(false)),
            channel_closed: Arc::new(AtomicBool::new(false)),
            interface_bound,
            attempts: Mutex::new(0),
        })
    }

    struct TestAuthenticator {
        rejected: DataChannelCandidateId,
        attempts: Mutex<Vec<DataChannelCandidateId>>,
    }

    #[async_trait]
    impl DataChannelAuthenticator for TestAuthenticator {
        async fn authenticate(
            &self,
            candidate: &DataChannelCandidate,
            _channel: &dyn EstablishedDataChannel,
            cancellation: CancellationToken,
        ) -> Result<(), DataChannelError> {
            if cancellation.is_cancelled() {
                return Err(DataChannelError::Cancelled);
            }
            self.attempts
                .lock()
                .map_err(|_| DataChannelError::ProviderFailed)?
                .push(candidate.id());
            if candidate.id() == self.rejected {
                Err(DataChannelError::AuthenticationFailed)
            } else {
                Ok(())
            }
        }
    }

    struct FixedFailureAuthenticator(DataChannelError);

    #[async_trait]
    impl DataChannelAuthenticator for FixedFailureAuthenticator {
        async fn authenticate(
            &self,
            _candidate: &DataChannelCandidate,
            _channel: &dyn EstablishedDataChannel,
            _cancellation: CancellationToken,
        ) -> Result<(), DataChannelError> {
            Err(self.0.clone())
        }
    }

    #[test]
    fn prohibited_paths_are_rejected_before_the_broker() {
        let peer = DataChannelPeer::from_bytes([1; 16]);
        for path in [
            DataChannelPathClass::Cellular,
            DataChannelPathClass::Internet,
            DataChannelPathClass::Unknown,
        ] {
            assert_eq!(
                DataChannelCandidate::new(
                    DataChannelCandidateId::from_bytes([2; 16]),
                    peer,
                    DataChannelKind::Lan,
                    DataChannelCandidateProperties {
                        path_class: path,
                        cost: DataChannelCost::Unmetered,
                        already_available: true,
                        requires_user_action: false,
                        estimated_round_trip_time: None,
                    },
                ),
                Err(DataChannelError::ProhibitedPath)
            );
        }
    }

    #[tokio::test]
    async fn unavailable_provider_does_not_block_ready_provider() {
        let peer = DataChannelPeer::from_bytes([3; 16]);
        let unavailable = provider(
            DataChannelCapability {
                kind: DataChannelKind::ApplePeerToPeer,
                state: DataChannelCapabilityState::Unsupported,
            },
            vec![],
            Duration::ZERO,
            false,
        );
        let ready = provider(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                4,
                peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )],
            Duration::from_millis(1),
            true,
        );
        let providers: Vec<Arc<dyn DataChannelProvider>> = vec![unavailable, ready];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        let result = broker
            .establish(peer, CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("establish: {error}"));
        assert_eq!(result.candidate.kind(), DataChannelKind::Lan);
        assert_eq!(result.channel.peer(), peer);
    }

    #[tokio::test]
    async fn first_success_releases_slower_candidate() {
        let peer = DataChannelPeer::from_bytes([5; 16]);
        let slow = provider(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                6,
                peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )],
            Duration::from_secs(30),
            true,
        );
        let fast = provider(
            DataChannelCapability {
                kind: DataChannelKind::WifiAware,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                7,
                peer,
                DataChannelKind::WifiAware,
                DataChannelPathClass::PeerToPeer,
                false,
            )],
            Duration::from_millis(1),
            true,
        );
        let providers: Vec<Arc<dyn DataChannelProvider>> = vec![slow.clone(), fast];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(60))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        let result = broker
            .establish(peer, CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("establish: {error}"));
        assert_eq!(result.candidate.kind(), DataChannelKind::WifiAware);
        tokio::task::yield_now().await;
        assert!(slow.released.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn authentication_failure_falls_through_to_another_bearer() {
        let peer = DataChannelPeer::from_bytes([11; 16]);
        let rejected_id = DataChannelCandidateId::from_bytes([12; 16]);
        let rejected = provider(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                12,
                peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )],
            Duration::from_millis(1),
            true,
        );
        let accepted = provider(
            DataChannelCapability {
                kind: DataChannelKind::WifiAware,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                13,
                peer,
                DataChannelKind::WifiAware,
                DataChannelPathClass::PeerToPeer,
                true,
            )],
            Duration::from_millis(1),
            true,
        );
        let authenticator = Arc::new(TestAuthenticator {
            rejected: rejected_id,
            attempts: Mutex::new(Vec::new()),
        });
        let providers: Vec<Arc<dyn DataChannelProvider>> = vec![rejected.clone(), accepted];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        let result = broker
            .establish_authenticated(peer, authenticator.clone(), CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("authenticated establish: {error}"));
        assert_eq!(
            result.candidate.id(),
            DataChannelCandidateId::from_bytes([13; 16])
        );
        assert!(rejected.channel_closed.load(Ordering::SeqCst));
        assert_eq!(
            authenticator
                .attempts
                .lock()
                .unwrap_or_else(|_| panic!("auth attempts"))
                .as_slice(),
            &[rejected_id, DataChannelCandidateId::from_bytes([13; 16])]
        );
    }

    #[tokio::test]
    async fn identity_change_is_a_peer_wide_hard_failure() {
        let peer = DataChannelPeer::from_bytes([20; 16]);
        let first = provider(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                21,
                peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )],
            Duration::from_millis(1),
            true,
        );
        let second = provider(
            DataChannelCapability {
                kind: DataChannelKind::WifiAware,
                state: DataChannelCapabilityState::Ready,
            },
            vec![
                DataChannelCandidate::new(
                    DataChannelCandidateId::from_bytes([22; 16]),
                    peer,
                    DataChannelKind::WifiAware,
                    DataChannelCandidateProperties {
                        path_class: DataChannelPathClass::PeerToPeer,
                        cost: DataChannelCost::Unmetered,
                        already_available: false,
                        requires_user_action: true,
                        estimated_round_trip_time: Some(Duration::from_millis(10)),
                    },
                )
                .unwrap_or_else(|error| panic!("prompted candidate: {error}")),
            ],
            Duration::from_millis(1),
            true,
        );
        let providers: Vec<Arc<dyn DataChannelProvider>> = vec![first.clone(), second.clone()];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        let result = broker
            .establish_authenticated(
                peer,
                Arc::new(FixedFailureAuthenticator(
                    DataChannelError::PeerIdentityChanged,
                )),
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(DataChannelError::PeerIdentityChanged)));
        assert!(first.channel_closed.load(Ordering::SeqCst));
        assert_eq!(
            *second
                .attempts
                .lock()
                .unwrap_or_else(|_| panic!("second attempts")),
            0
        );
    }

    #[tokio::test]
    async fn default_policy_rejects_unbound_and_metered_paths() {
        let peer = DataChannelPeer::from_bytes([14; 16]);
        let metered_candidate = DataChannelCandidate::new(
            DataChannelCandidateId::from_bytes([15; 16]),
            peer,
            DataChannelKind::Lan,
            DataChannelCandidateProperties {
                path_class: DataChannelPathClass::LocalNetwork,
                cost: DataChannelCost::Metered,
                already_available: true,
                requires_user_action: false,
                estimated_round_trip_time: None,
            },
        )
        .unwrap_or_else(|error| panic!("metered candidate: {error}"));
        let metered = provider(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![metered_candidate],
            Duration::from_millis(1),
            true,
        );
        let providers: Vec<Arc<dyn DataChannelProvider>> = vec![metered.clone()];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        assert!(matches!(
            broker.establish(peer, CancellationToken::new()).await,
            Err(DataChannelError::NoCandidates)
        ));
        assert_eq!(
            *metered
                .attempts
                .lock()
                .unwrap_or_else(|_| panic!("metered attempts")),
            0
        );

        let unbound = provider_with_binding(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                16,
                peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )],
            Duration::from_millis(1),
            true,
            false,
        );
        let providers: Vec<Arc<dyn DataChannelProvider>> = vec![unbound.clone()];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        assert!(matches!(
            broker.establish(peer, CancellationToken::new()).await,
            Err(DataChannelError::ProhibitedPath)
        ));
        assert!(unbound.channel_closed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn system_prompt_candidates_are_bounded_and_deferred() {
        let peer = DataChannelPeer::from_bytes([17; 16]);
        let automatic = provider(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                18,
                peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )],
            Duration::from_millis(1),
            false,
        );
        let prompted_candidate = DataChannelCandidate::new(
            DataChannelCandidateId::from_bytes([19; 16]),
            peer,
            DataChannelKind::WifiDirect,
            DataChannelCandidateProperties {
                path_class: DataChannelPathClass::PeerToPeer,
                cost: DataChannelCost::Unmetered,
                already_available: false,
                requires_user_action: true,
                estimated_round_trip_time: Some(Duration::from_millis(10)),
            },
        )
        .unwrap_or_else(|error| panic!("prompted candidate: {error}"));
        let prompted = provider(
            DataChannelCapability {
                kind: DataChannelKind::WifiDirect,
                state: DataChannelCapabilityState::Ready,
            },
            vec![prompted_candidate],
            Duration::from_millis(1),
            true,
        );
        let providers: Vec<Arc<dyn DataChannelProvider>> =
            vec![automatic.clone(), prompted.clone()];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        let result = broker
            .establish(peer, CancellationToken::new())
            .await
            .unwrap_or_else(|error| panic!("prompt fallback: {error}"));
        assert_eq!(result.candidate.kind(), DataChannelKind::WifiDirect);
        assert_eq!(
            *automatic
                .attempts
                .lock()
                .unwrap_or_else(|_| panic!("automatic attempts")),
            1
        );
        assert_eq!(
            *prompted
                .attempts
                .lock()
                .unwrap_or_else(|_| panic!("prompt attempts")),
            1
        );
    }

    #[tokio::test]
    async fn external_cancellation_stops_before_candidate_collection() {
        let peer = DataChannelPeer::from_bytes([8; 16]);
        let ready = provider(
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            },
            vec![candidate(
                9,
                peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )],
            Duration::from_secs(1),
            true,
        );
        let providers: Vec<Arc<dyn DataChannelProvider>> = vec![ready];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = broker.establish(peer, cancellation).await;
        assert!(matches!(result, Err(DataChannelError::Cancelled)));
    }

    struct UnresponsiveCandidateProvider {
        peer: DataChannelPeer,
    }

    #[async_trait]
    impl DataChannelProvider for UnresponsiveCandidateProvider {
        fn capability(&self) -> DataChannelCapability {
            DataChannelCapability {
                kind: DataChannelKind::Lan,
                state: DataChannelCapabilityState::Ready,
            }
        }

        async fn candidates(
            &self,
            _peer: DataChannelPeer,
            _cancellation: CancellationToken,
        ) -> Result<Vec<DataChannelCandidate>, DataChannelError> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(vec![candidate(
                10,
                self.peer,
                DataChannelKind::Lan,
                DataChannelPathClass::LocalNetwork,
                true,
            )])
        }

        async fn establish(
            &self,
            _candidate: &DataChannelCandidate,
            _cancellation: CancellationToken,
        ) -> Result<Box<dyn EstablishedDataChannel>, DataChannelError> {
            Err(DataChannelError::ProviderFailed)
        }
    }

    #[tokio::test]
    async fn external_cancellation_drops_unresponsive_candidate_collection() {
        let peer = DataChannelPeer::from_bytes([10; 16]);
        let providers: Vec<Arc<dyn DataChannelProvider>> =
            vec![Arc::new(UnresponsiveCandidateProvider { peer })];
        let broker = DataChannelBroker::new(providers, Duration::from_secs(60))
            .unwrap_or_else(|error| panic!("broker: {error}"));
        let cancellation = CancellationToken::new();
        let cancelled = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            cancelled.cancel();
        });
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            broker.establish(peer, cancellation),
        )
        .await
        .unwrap_or_else(|_| panic!("broker ignored external cancellation"));
        assert!(matches!(result, Err(DataChannelError::Cancelled)));
    }
}
