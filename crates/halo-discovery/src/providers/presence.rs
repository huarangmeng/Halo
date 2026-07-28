use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use network_interface::{Addr, NetworkInterface, NetworkInterfaceConfig};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::{net::UdpSocket, sync::mpsc, time::Instant};
use tracing::{debug, warn};

use crate::{
    DiscoveryProvider, Endpoint, Observation, ProviderContext, ProviderError, ProviderId,
    ProviderKind, ProviderState,
    wire::{MessageKind, PresenceMessage},
};

pub const PRESENCE_PORT: u16 = 44_721;
pub const PRESENCE_IPV4_GROUP: Ipv4Addr = Ipv4Addr::new(239, 192, 72, 65);
pub const PRESENCE_IPV6_GROUP: Ipv6Addr = Ipv6Addr::new(0xff12, 0, 0, 0, 0, 0, 0x4841, 0x4c4f);

#[derive(Clone, Debug)]
pub struct PresenceV4Config {
    pub group: Ipv4Addr,
    pub port: u16,
    pub announce_interval: Duration,
    pub observation_ttl: Duration,
    pub include_directed_broadcast: bool,
}

impl Default for PresenceV4Config {
    fn default() -> Self {
        Self {
            group: PRESENCE_IPV4_GROUP,
            port: PRESENCE_PORT,
            announce_interval: Duration::from_secs(4),
            observation_ttl: Duration::from_secs(12),
            include_directed_broadcast: true,
        }
    }
}

pub struct PresenceV4Provider {
    id: ProviderId,
    config: PresenceV4Config,
}

impl PresenceV4Provider {
    #[must_use]
    pub fn new(config: PresenceV4Config) -> Self {
        Self {
            id: ProviderId::builtin(ProviderKind::PresenceV4, "presence-v4"),
            config,
        }
    }
}

impl Default for PresenceV4Provider {
    fn default() -> Self {
        Self::new(PresenceV4Config::default())
    }
}

#[async_trait]
impl DiscoveryProvider for PresenceV4Provider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn run(&self, context: ProviderContext) -> Result<(), ProviderError> {
        validate_v4_config(&self.config)?;
        let interfaces = eligible_interfaces()?;
        let addresses = v4_addresses(&interfaces);
        if addresses.is_empty() {
            return Err(ProviderError::Unavailable(
                "no eligible IPv4 interfaces".to_owned(),
            ));
        }

        let receiver = create_v4_receiver(self.config.group, self.config.port, &addresses)?;
        let senders = create_v4_senders(
            self.config.group,
            self.config.port,
            &addresses,
            self.config.include_directed_broadcast,
        )?;
        context.set_state(self.id(), ProviderState::Ready).await?;

        run_v4_loop(&context, &self.id, &self.config, receiver, &senders).await
    }
}

#[derive(Clone, Debug)]
pub struct PresenceV6Config {
    pub group: Ipv6Addr,
    pub port: u16,
    pub announce_interval: Duration,
    pub observation_ttl: Duration,
}

impl Default for PresenceV6Config {
    fn default() -> Self {
        Self {
            group: PRESENCE_IPV6_GROUP,
            port: PRESENCE_PORT,
            announce_interval: Duration::from_secs(4),
            observation_ttl: Duration::from_secs(12),
        }
    }
}

pub struct PresenceV6Provider {
    id: ProviderId,
    config: PresenceV6Config,
}

impl PresenceV6Provider {
    #[must_use]
    pub fn new(config: PresenceV6Config) -> Self {
        Self {
            id: ProviderId::builtin(ProviderKind::PresenceV6, "presence-v6"),
            config,
        }
    }
}

impl Default for PresenceV6Provider {
    fn default() -> Self {
        Self::new(PresenceV6Config::default())
    }
}

#[async_trait]
impl DiscoveryProvider for PresenceV6Provider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn run(&self, context: ProviderContext) -> Result<(), ProviderError> {
        validate_v6_config(&self.config)?;
        let interfaces = eligible_interfaces()?;
        let indexes = v6_interface_indexes(&interfaces);
        if indexes.is_empty() {
            return Err(ProviderError::Unavailable(
                "no eligible IPv6 interfaces".to_owned(),
            ));
        }

        let receiver = create_v6_receiver(self.config.group, self.config.port, &indexes)?;
        let senders = create_v6_senders(self.config.group, self.config.port, &indexes)?;
        context.set_state(self.id(), ProviderState::Ready).await?;

        run_v6_loop(&context, &self.id, &self.config, receiver, &senders).await
    }
}

struct V4InterfaceAddress {
    address: Ipv4Addr,
    broadcast: Option<Ipv4Addr>,
}

struct EgressSocket {
    socket: Arc<UdpSocket>,
    targets: Vec<SocketAddr>,
}

struct ProbeResponse {
    message: PresenceMessage,
    source: SocketAddr,
}

async fn run_v4_loop(
    context: &ProviderContext,
    provider: &ProviderId,
    config: &PresenceV4Config,
    receiver: UdpSocket,
    senders: &[EgressSocket],
) -> Result<(), ProviderError> {
    let cancel = context.cancellation_token();
    let mut ticker = tokio::time::interval(config.announce_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut buffer = [0_u8; 512];
    let mut sequence = 0_u64;
    let mut first_tick = true;
    let mut pending_queries = HashMap::new();
    let reply_cancel = cancel.child_token();
    let _reply_guard = reply_cancel.clone().drop_guard();
    let mut replies = spawn_response_receivers(senders, reply_cancel);

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                let message = PresenceMessage::from_local(
                    context.local(),
                    MessageKind::Goodbye,
                    sequence,
                    0,
                ).encode();
                let _send_result = send_all(senders, &message).await;
                return Ok(());
            }
            _ = ticker.tick() => {
                sequence = sequence.wrapping_add(1);
                let kind = if first_tick { MessageKind::Query } else { MessageKind::Announce };
                let nonce = random_nonce();
                if kind == MessageKind::Query {
                    pending_queries.insert(nonce, Instant::now());
                }
                if kind == MessageKind::Query {
                    send_query_all(senders, context.local(), sequence, nonce).await?;
                } else {
                    let message = PresenceMessage::from_local(context.local(), kind, sequence, nonce).encode();
                    send_all(senders, &message).await?;
                }
                pending_queries.retain(|_, sent_at| sent_at.elapsed() < Duration::from_secs(10));
                first_tick = false;
            }
            received = receiver.recv_from(&mut buffer) => {
                let (length, source) = received?;
                let Ok(message) = PresenceMessage::decode(&buffer[..length]) else {
                    continue;
                };
                if message.presence_id == context.local().presence_id {
                    continue;
                }
                if message.kind == MessageKind::Goodbye {
                    context.withdraw(provider.clone(), message.presence_id).await?;
                    continue;
                }
                if message.kind == MessageKind::Query {
                    let response = PresenceMessage::from_local(
                        context.local(),
                        MessageKind::Response,
                        sequence,
                        message.nonce,
                    ).encode();
                    let target = response_target(source, message.reply_port);
                    if let Err(error) = receiver.send_to(&response, target).await {
                        debug!(%error, %target, "failed to send IPv4 presence response");
                    }
                }
                let round_trip_time = (message.kind == MessageKind::Response)
                    .then(|| pending_queries.get(&message.nonce).map(Instant::elapsed))
                    .flatten();
                if let Some(endpoint) = endpoint_from_source(source, message.quic_port) {
                    context.observe(Observation {
                        provider: provider.clone(),
                        presence_id: message.presence_id,
                        protocol: message.protocol,
                        capabilities: message.capabilities,
                        sequence: message.sequence,
                        endpoints: vec![endpoint],
                        ttl: config.observation_ttl,
                        round_trip_time,
                    }).await?;
                }
            }
            response = replies.recv() => {
                if let Some(response) = response {
                    observe_probe_response(
                        context,
                        provider,
                        config.observation_ttl,
                        &pending_queries,
                        response,
                    ).await?;
                }
            }
        }
    }
}

async fn run_v6_loop(
    context: &ProviderContext,
    provider: &ProviderId,
    config: &PresenceV6Config,
    receiver: UdpSocket,
    senders: &[EgressSocket],
) -> Result<(), ProviderError> {
    let cancel = context.cancellation_token();
    let mut ticker = tokio::time::interval(config.announce_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut buffer = [0_u8; 512];
    let mut sequence = 0_u64;
    let mut first_tick = true;
    let mut pending_queries = HashMap::new();
    let reply_cancel = cancel.child_token();
    let _reply_guard = reply_cancel.clone().drop_guard();
    let mut replies = spawn_response_receivers(senders, reply_cancel);

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                let message = PresenceMessage::from_local(
                    context.local(),
                    MessageKind::Goodbye,
                    sequence,
                    0,
                ).encode();
                let _send_result = send_all(senders, &message).await;
                return Ok(());
            }
            _ = ticker.tick() => {
                sequence = sequence.wrapping_add(1);
                let kind = if first_tick { MessageKind::Query } else { MessageKind::Announce };
                let nonce = random_nonce();
                if kind == MessageKind::Query {
                    pending_queries.insert(nonce, Instant::now());
                }
                if kind == MessageKind::Query {
                    send_query_all(senders, context.local(), sequence, nonce).await?;
                } else {
                    let message = PresenceMessage::from_local(context.local(), kind, sequence, nonce).encode();
                    send_all(senders, &message).await?;
                }
                pending_queries.retain(|_, sent_at| sent_at.elapsed() < Duration::from_secs(10));
                first_tick = false;
            }
            received = receiver.recv_from(&mut buffer) => {
                let (length, source) = received?;
                let Ok(message) = PresenceMessage::decode(&buffer[..length]) else {
                    continue;
                };
                if message.presence_id == context.local().presence_id {
                    continue;
                }
                if message.kind == MessageKind::Goodbye {
                    context.withdraw(provider.clone(), message.presence_id).await?;
                    continue;
                }
                if message.kind == MessageKind::Query {
                    let response = PresenceMessage::from_local(
                        context.local(),
                        MessageKind::Response,
                        sequence,
                        message.nonce,
                    ).encode();
                    let target = response_target(source, message.reply_port);
                    if let Err(error) = receiver.send_to(&response, target).await {
                        debug!(%error, %target, "failed to send IPv6 presence response");
                    }
                }
                let round_trip_time = (message.kind == MessageKind::Response)
                    .then(|| pending_queries.get(&message.nonce).map(Instant::elapsed))
                    .flatten();
                if let Some(endpoint) = endpoint_from_source(source, message.quic_port) {
                    context.observe(Observation {
                        provider: provider.clone(),
                        presence_id: message.presence_id,
                        protocol: message.protocol,
                        capabilities: message.capabilities,
                        sequence: message.sequence,
                        endpoints: vec![endpoint],
                        ttl: config.observation_ttl,
                        round_trip_time,
                    }).await?;
                }
            }
            response = replies.recv() => {
                if let Some(response) = response {
                    observe_probe_response(
                        context,
                        provider,
                        config.observation_ttl,
                        &pending_queries,
                        response,
                    ).await?;
                }
            }
        }
    }
}

fn spawn_response_receivers(
    senders: &[EgressSocket],
    cancel: tokio_util::sync::CancellationToken,
) -> mpsc::Receiver<ProbeResponse> {
    let capacity = senders.len().saturating_mul(8).clamp(8, 128);
    let (sender, receiver) = mpsc::channel(capacity);
    for egress in senders {
        let socket = Arc::clone(&egress.socket);
        let output = sender.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let mut buffer = [0_u8; 512];
            loop {
                let received = tokio::select! {
                    () = cancel.cancelled() => return,
                    received = socket.recv_from(&mut buffer) => received,
                };
                let Ok((length, source)) = received else {
                    return;
                };
                let Ok(message) = PresenceMessage::decode(&buffer[..length]) else {
                    continue;
                };
                if message.kind == MessageKind::Response
                    && output
                        .send(ProbeResponse { message, source })
                        .await
                        .is_err()
                {
                    return;
                }
            }
        });
    }
    drop(sender);
    receiver
}

async fn observe_probe_response(
    context: &ProviderContext,
    provider: &ProviderId,
    ttl: Duration,
    pending_queries: &HashMap<u64, Instant>,
    response: ProbeResponse,
) -> Result<(), ProviderError> {
    if response.message.presence_id == context.local().presence_id {
        return Ok(());
    }
    let Some(sent_at) = pending_queries.get(&response.message.nonce) else {
        return Ok(());
    };
    let Some(endpoint) = endpoint_from_source(response.source, response.message.quic_port) else {
        return Ok(());
    };
    context
        .observe(Observation {
            provider: provider.clone(),
            presence_id: response.message.presence_id,
            protocol: response.message.protocol,
            capabilities: response.message.capabilities,
            sequence: response.message.sequence,
            endpoints: vec![endpoint],
            ttl,
            round_trip_time: Some(sent_at.elapsed()),
        })
        .await
}

async fn send_all(senders: &[EgressSocket], bytes: &[u8]) -> Result<(), ProviderError> {
    let mut successes = 0_usize;
    let mut last_error = None;
    for sender in senders {
        for target in &sender.targets {
            match sender.socket.send_to(bytes, target).await {
                Ok(length) if length == bytes.len() => successes += 1,
                Ok(length) => {
                    last_error = Some(format!(
                        "sent truncated presence datagram: {length}/{} bytes",
                        bytes.len()
                    ));
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
    }
    if successes == 0 {
        Err(ProviderError::Network(
            last_error.unwrap_or_else(|| "no presence egress targets".to_owned()),
        ))
    } else {
        Ok(())
    }
}

async fn send_query_all(
    senders: &[EgressSocket],
    local: &crate::LocalPresence,
    sequence: u64,
    nonce: u64,
) -> Result<(), ProviderError> {
    let mut successes = 0_usize;
    let mut last_error = None;
    for sender in senders {
        let reply_port = match sender.socket.local_addr() {
            Ok(address) => address.port(),
            Err(error) => {
                last_error = Some(error.to_string());
                continue;
            }
        };
        let bytes = PresenceMessage::from_local(local, MessageKind::Query, sequence, nonce)
            .with_reply_port(reply_port)
            .encode();
        for target in &sender.targets {
            match sender.socket.send_to(&bytes, target).await {
                Ok(length) if length == bytes.len() => successes += 1,
                Ok(length) => {
                    last_error = Some(format!(
                        "sent truncated presence query: {length}/{} bytes",
                        bytes.len()
                    ));
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
    }
    if successes == 0 {
        Err(ProviderError::Network(last_error.unwrap_or_else(|| {
            "no presence query egress targets".to_owned()
        })))
    } else {
        Ok(())
    }
}

fn create_v4_receiver(
    group: Ipv4Addr,
    port: u16,
    interfaces: &[V4InterfaceAddress],
) -> Result<UdpSocket, ProviderError> {
    let socket = reusable_socket(Domain::IPV4)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
    let mut joined = 0_usize;
    for interface in interfaces {
        match socket.join_multicast_v4(&group, &interface.address) {
            Ok(()) => joined += 1,
            Err(error) => {
                warn!(%error, address = %interface.address, "cannot join IPv4 presence group")
            }
        }
    }
    if joined == 0 {
        return Err(ProviderError::Unavailable(
            "could not join the IPv4 presence group on any interface".to_owned(),
        ));
    }
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into()).map_err(ProviderError::from)
}

fn create_v4_senders(
    group: Ipv4Addr,
    port: u16,
    interfaces: &[V4InterfaceAddress],
    include_broadcast: bool,
) -> Result<Vec<EgressSocket>, ProviderError> {
    let mut output = Vec::new();
    for interface in interfaces {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_multicast_if_v4(&interface.address)?;
        socket.set_multicast_ttl_v4(1)?;
        socket.set_multicast_loop_v4(true)?;
        socket.set_broadcast(include_broadcast)?;
        socket.bind(&SocketAddrV4::new(interface.address, 0).into())?;
        socket.set_nonblocking(true)?;

        let mut targets = vec![SocketAddr::V4(SocketAddrV4::new(group, port))];
        if include_broadcast
            && let Some(broadcast) = interface.broadcast
            && !broadcast.is_unspecified()
            && !broadcast.is_multicast()
            && broadcast != interface.address
        {
            targets.push(SocketAddr::V4(SocketAddrV4::new(broadcast, port)));
        }
        output.push(EgressSocket {
            socket: Arc::new(UdpSocket::from_std(socket.into())?),
            targets,
        });
    }
    Ok(output)
}

fn create_v6_receiver(
    group: Ipv6Addr,
    port: u16,
    interface_indexes: &[u32],
) -> Result<UdpSocket, ProviderError> {
    let socket = reusable_socket(Domain::IPV6)?;
    socket.set_only_v6(true)?;
    socket.bind(&SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0).into())?;
    let mut joined = 0_usize;
    for &index in interface_indexes {
        match socket.join_multicast_v6(&group, index) {
            Ok(()) => joined += 1,
            Err(error) => warn!(%error, interface_index = index, "cannot join IPv6 presence group"),
        }
    }
    if joined == 0 {
        return Err(ProviderError::Unavailable(
            "could not join the IPv6 presence group on any interface".to_owned(),
        ));
    }
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into()).map_err(ProviderError::from)
}

fn create_v6_senders(
    group: Ipv6Addr,
    port: u16,
    interface_indexes: &[u32],
) -> Result<Vec<EgressSocket>, ProviderError> {
    let mut output = Vec::new();
    for &index in interface_indexes {
        let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_only_v6(true)?;
        socket.set_multicast_if_v6(index)?;
        socket.set_multicast_hops_v6(1)?;
        socket.set_multicast_loop_v6(true)?;
        socket.bind(&SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, index).into())?;
        socket.set_nonblocking(true)?;
        output.push(EgressSocket {
            socket: Arc::new(UdpSocket::from_std(socket.into())?),
            targets: vec![SocketAddr::V6(SocketAddrV6::new(group, port, 0, index))],
        });
    }
    Ok(output)
}

fn reusable_socket(domain: Domain) -> Result<Socket, ProviderError> {
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    Ok(socket)
}

fn eligible_interfaces() -> Result<Vec<NetworkInterface>, ProviderError> {
    NetworkInterface::show()
        .map_err(|error| ProviderError::Network(error.to_string()))
        .map(|interfaces| {
            interfaces
                .into_iter()
                .filter(|interface| !interface.internal && !is_probably_tunnel(&interface.name))
                .collect()
        })
}

fn is_probably_tunnel(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "utun",
        "tun",
        "tap",
        "wg",
        "wireguard",
        "tailscale",
        "ipsec",
        "ppp",
        "awdl",
        "llw",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

pub(crate) fn eligible_ip_addresses() -> Result<Vec<IpAddr>, ProviderError> {
    let interfaces = eligible_interfaces()?;
    let mut addresses = interfaces
        .iter()
        .flat_map(|interface| interface.addr.iter())
        .filter_map(|address| match address {
            Addr::V4(address)
                if !address.ip.is_unspecified()
                    && !address.ip.is_loopback()
                    && !address.ip.is_multicast() =>
            {
                Some(IpAddr::V4(address.ip))
            }
            Addr::V6(address)
                if !address.ip.is_unspecified()
                    && !address.ip.is_loopback()
                    && !address.ip.is_multicast() =>
            {
                Some(IpAddr::V6(address.ip))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

fn v4_addresses(interfaces: &[NetworkInterface]) -> Vec<V4InterfaceAddress> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for interface in interfaces {
        for address in &interface.addr {
            if let Addr::V4(address) = address
                && !address.ip.is_unspecified()
                && !address.ip.is_loopback()
                && !address.ip.is_multicast()
                && seen.insert(address.ip)
            {
                output.push(V4InterfaceAddress {
                    address: address.ip,
                    broadcast: address.broadcast,
                });
            }
        }
    }
    output
}

fn v6_interface_indexes(interfaces: &[NetworkInterface]) -> Vec<u32> {
    let mut indexes = interfaces
        .iter()
        .filter(|interface| {
            interface.addr.iter().any(|address| {
                matches!(address, Addr::V6(address) if !address.ip.is_unspecified() && !address.ip.is_loopback() && !address.ip.is_multicast())
            })
        })
        .map(|interface| interface.index)
        .filter(|index| *index != 0)
        .collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

fn endpoint_from_source(mut source: SocketAddr, quic_port: u16) -> Option<Endpoint> {
    source.set_port(quic_port);
    Endpoint::quic(source).ok()
}

fn response_target(mut source: SocketAddr, reply_port: u16) -> SocketAddr {
    source.set_port(reply_port);
    source
}

fn random_nonce() -> u64 {
    let presence = crate::PresenceId::random();
    let mut seed = [0_u8; 8];
    seed.copy_from_slice(&presence.as_bytes()[..8]);
    u64::from_be_bytes(seed)
}

fn validate_v4_config(config: &PresenceV4Config) -> Result<(), ProviderError> {
    if !config.group.is_multicast()
        || config.port == 0
        || config.announce_interval.is_zero()
        || config.observation_ttl.is_zero()
    {
        return Err(ProviderError::InvalidConfig(
            "invalid IPv4 presence group, port, or timing".to_owned(),
        ));
    }
    Ok(())
}

fn validate_v6_config(config: &PresenceV6Config) -> Result<(), ProviderError> {
    if !config.group.is_multicast()
        || config.port == 0
        || config.announce_interval.is_zero()
        || config.observation_ttl.is_zero()
    {
        return Err(ProviderError::InvalidConfig(
            "invalid IPv6 presence group, port, or timing".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_name_filter_is_conservative() {
        assert!(is_probably_tunnel("utun4"));
        assert!(is_probably_tunnel("WireGuard0"));
        assert!(is_probably_tunnel("awdl0"));
        assert!(!is_probably_tunnel("en0"));
        assert!(!is_probably_tunnel("wlan0"));
    }

    #[test]
    fn nonce_is_randomized() {
        assert_ne!(random_nonce(), random_nonce());
    }

    #[test]
    fn source_address_controls_endpoint_ip() {
        let source = SocketAddr::from(([192, 0, 2, 9], PRESENCE_PORT));
        let endpoint = endpoint_from_source(source, 4433)
            .unwrap_or_else(|| panic!("source should produce endpoint"));
        assert_eq!(endpoint.address(), SocketAddr::from(([192, 0, 2, 9], 4433)));
    }
}
