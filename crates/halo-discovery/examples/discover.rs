use std::{error::Error, time::Duration};

use halo_discovery::{
    Capabilities, DiscoveryEvent, DiscoveryManager, LocalPresence, PeerSnapshot, PresenceId,
    ProtocolRange,
    providers::{MdnsProvider, PresenceV4Provider, PresenceV6Provider},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let quic_port = std::env::args()
        .nth(1)
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(44_330);
    let seconds = std::env::args()
        .nth(2)
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(20);
    let protocol = ProtocolRange::new(1, 1)?;
    let local = LocalPresence::new(
        PresenceId::random(),
        protocol,
        Capabilities::default(),
        quic_port,
    )?;
    println!("local presence={} quic_port={quic_port}", local.presence_id);

    let session = DiscoveryManager::new(local)
        .with_provider(MdnsProvider::default())
        .with_provider(PresenceV4Provider::default())
        .with_provider(PresenceV6Provider::default())
        .start()
        .await?;
    let handle = session.handle();
    let mut events = handle.subscribe();
    let deadline = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            () = &mut deadline => break,
            event = events.recv() => match event {
                Ok(event) => print_event(&event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    println!("event receiver lagged by {skipped}; snapshot={:?}", handle.snapshot().await?);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    let snapshot = handle.snapshot().await?;
    println!("final peer count={}", snapshot.len());
    for peer in &snapshot {
        print_peer("final", peer);
    }
    session.shutdown().await?;
    Ok(())
}

fn print_event(event: &DiscoveryEvent) {
    match event {
        DiscoveryEvent::PeerAppeared(peer) => print_peer("appeared", peer),
        DiscoveryEvent::PeerChanged(peer) => print_peer("changed", peer),
        DiscoveryEvent::PeerExpired(presence) => println!("peer expired presence={presence}"),
        DiscoveryEvent::PeerQuarantined(presence) => {
            println!("peer quarantined presence={presence}")
        }
        DiscoveryEvent::ProviderChanged { provider, state } => {
            println!("provider={provider} state={state:?}")
        }
        _ => println!("event={event:?}"),
    }
}

fn print_peer(change: &str, peer: &PeerSnapshot) {
    let best = peer.candidates.first();
    println!(
        "peer {change} presence={} sources={} candidates={} best={:?} score={:?} rtt={:?}",
        peer.presence_id,
        peer.sources.len(),
        peer.candidates.len(),
        peer.best_endpoint.map(|endpoint| endpoint.address()),
        best.map(|candidate| candidate.score),
        best.and_then(|candidate| candidate.round_trip_time),
    );
}
