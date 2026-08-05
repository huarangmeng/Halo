use std::{collections::HashMap, time::Duration};

use crate::{Endpoint, EndpointCandidate, ProtocolRange, ProviderId, ProviderKind};

pub(crate) struct EndpointRecord {
    pub evidence: HashMap<ProviderId, EndpointEvidence>,
    pub successful_connections: u32,
    pub total_failures: u32,
    pub consecutive_failures: u32,
    pub connection_rtt: Option<Duration>,
}

impl EndpointRecord {
    pub fn new() -> Self {
        Self {
            evidence: HashMap::new(),
            successful_connections: 0,
            total_failures: 0,
            consecutive_failures: 0,
            connection_rtt: None,
        }
    }

    pub fn candidate(&self, endpoint: Endpoint, compatible: bool) -> EndpointCandidate {
        let sources = self.evidence.keys().cloned().collect();
        let observed_rtt = self
            .evidence
            .values()
            .filter_map(|evidence| evidence.round_trip_time)
            .min();
        let round_trip_time = self.connection_rtt.or(observed_rtt);
        let score = score_candidate(self, compatible, round_trip_time);

        EndpointCandidate {
            endpoint,
            score,
            sources,
            round_trip_time,
            successful_connections: self.successful_connections,
            consecutive_failures: self.consecutive_failures,
        }
    }
}

pub(crate) struct EndpointEvidence {
    pub expires_at: tokio::time::Instant,
    pub round_trip_time: Option<Duration>,
    pub observations: u32,
}

pub(crate) fn protocol_compatible(local: ProtocolRange, remote: ProtocolRange) -> bool {
    local.overlaps(remote)
}

fn score_candidate(
    record: &EndpointRecord,
    compatible: bool,
    round_trip_time: Option<Duration>,
) -> i32 {
    if !compatible {
        return i32::MIN;
    }

    let mut score = 100;
    for provider in record.evidence.keys() {
        score += source_weight(provider.kind());
    }

    let corroborating_sources = record.evidence.len().saturating_sub(1).min(3) as i32;
    score += corroborating_sources * 35;

    let observations = record
        .evidence
        .values()
        .map(|evidence| evidence.observations.min(5))
        .sum::<u32>() as i32;
    score += observations * 3;

    score += match round_trip_time {
        Some(rtt) if rtt <= Duration::from_millis(20) => 55,
        Some(rtt) if rtt <= Duration::from_millis(100) => 40,
        Some(rtt) if rtt <= Duration::from_millis(500) => 20,
        Some(_) => 5,
        None => 0,
    };

    score += record.successful_connections.min(5) as i32 * 80;
    score -= record.consecutive_failures.min(5) as i32 * 90;
    score -= record.total_failures.min(10) as i32 * 8;
    score
}

fn source_weight(kind: &ProviderKind) -> i32 {
    match kind {
        ProviderKind::Direct => 75,
        ProviderKind::PresenceV4 | ProviderKind::PresenceV6 => 60,
        ProviderKind::WifiAware | ProviderKind::WifiDirect => 60,
        ProviderKind::Mdns => 45,
        ProviderKind::Ble => 20,
        ProviderKind::Custom => 25,
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use super::*;
    use crate::{Endpoint, ProviderId, ProviderKind};

    fn endpoint() -> Endpoint {
        Endpoint::quic(SocketAddr::from(([192, 168, 2, 1], 4433)))
            .unwrap_or_else(|error| panic!("test endpoint: {error}"))
    }

    fn evidence(kind: ProviderKind) -> (ProviderId, EndpointEvidence) {
        (
            ProviderId::new(kind, "test").unwrap_or_else(|error| panic!("provider: {error}")),
            EndpointEvidence {
                expires_at: tokio::time::Instant::now() + Duration::from_secs(10),
                round_trip_time: None,
                observations: 1,
            },
        )
    }

    #[test]
    fn corroboration_and_success_improve_score() {
        let mut record = EndpointRecord::new();
        let (provider, value) = evidence(ProviderKind::Mdns);
        record.evidence.insert(provider, value);
        let first = record.candidate(endpoint(), true).score;

        let (provider, value) = evidence(ProviderKind::PresenceV4);
        record.evidence.insert(provider, value);
        let corroborated = record.candidate(endpoint(), true).score;
        assert!(corroborated > first);

        record.successful_connections = 1;
        record.connection_rtt = Some(Duration::from_millis(10));
        assert!(record.candidate(endpoint(), true).score > corroborated);
    }

    #[test]
    fn repeated_failures_demote_candidate() {
        let mut record = EndpointRecord::new();
        let (provider, value) = evidence(ProviderKind::Direct);
        record.evidence.insert(provider, value);
        let healthy = record.candidate(endpoint(), true).score;
        record.consecutive_failures = 3;
        record.total_failures = 3;
        assert!(record.candidate(endpoint(), true).score < healthy);
    }

    #[test]
    fn incompatible_candidate_cannot_win() {
        let mut record = EndpointRecord::new();
        let (provider, value) = evidence(ProviderKind::Direct);
        record.evidence.insert(provider, value);
        assert_eq!(record.candidate(endpoint(), false).score, i32::MIN);
    }
}
