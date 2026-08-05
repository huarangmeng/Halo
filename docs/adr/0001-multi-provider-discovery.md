# ADR 0001: Multi-provider discovery with evidence-based endpoint selection

- Status: Accepted for experimental implementation
- Date: 2026-07-28
- Owners: Halo maintainers

## Context

No single nearby-discovery mechanism is reliable across home Wi-Fi, enterprise
networks, mobile power policies, VPNs, multiple active interfaces, firewalls,
and stale resolver caches. mDNS offers broad zero-configuration interoperability
but can be delayed or filtered. A custom multicast signal can react quickly and
carry a tightly bounded protocol, but multicast itself can also be filtered.
Previously paired devices may still be reachable at a remembered address even
when all multicast traffic is unavailable.

Discovery is untrusted. A discovered identifier, address, service record, or
display label cannot establish peer identity. Authentication belongs to the
subsequent secure connection and pairing transcript.

## Decision

Halo runs independent discovery providers concurrently and merges their
observations by a rotating `PresenceId`:

1. Native BLE advertise, scan, and GATT rendezvous on all four target platforms
2. mDNS/DNS-SD browser and publisher using `_halo._udp.local.`
3. Halo Presence Protocol over IPv4 and IPv6 scoped multicast
4. Halo Presence Protocol over each eligible IPv4 directed-broadcast address
5. Direct presence probes to remembered discovery endpoints
6. Platform proximity providers that can also establish data paths, including
   Apple peer-to-peer Wi-Fi, Wi-Fi Direct, and Wi-Fi Aware, when runtime
   capability and interoperability are proven

The first five form the required discovery baseline. Rust implements the LAN
providers, BLE descriptor codec, and aggregation. Platform-native adapters only
drive BLE operating-system APIs and forward opaque bytes and capability state as
defined by [ADR 0002](0002-flutter-rust-platform-boundaries.md). No provider may
be reported as supported until its real implementation passes device tests.

The manager retains all viable endpoint candidates. It assigns a deterministic
score using:

- independent source corroboration
- source reliability weight
- freshness and observation count
- measured probe or connection latency
- successful connection history
- consecutive and total connection failures

The highest-scoring compatible endpoint is presented as `best_endpoint`, with
the ordered alternatives retained for connection racing or fallback. The
transport layer reports actual connection outcomes back to discovery so that
observed-but-unreachable addresses are demoted rapidly.

Provider tasks have independent failure states. One provider failing does not
stop the others. Events and provider inputs are bounded; lagging consumers must
resubscribe and request a fresh snapshot rather than causing unbounded memory
growth.

## Privacy and security

- `PresenceId` is random per application presence lifetime. It is a correlation
  key, not a trusted device identity.
- Announcements omit filenames, account identifiers, stable device keys, and
  human-readable device names.
- Packet source addresses are used instead of advertised IP addresses.
- The custom presence packet is fixed at 58 bytes and has strict version,
  reserved-byte, port, and protocol-range validation.
- Presence responses are no larger than requests, limiting reflection
  amplification. Receivers rate-limit announcements through fixed intervals;
  per-source abuse controls remain required before a stable release.
- A QUIC/TLS and pairing handshake must authenticate the selected endpoint
  before any peer is trusted or file metadata is disclosed.

## Consequences

The design uses more sockets and produces more state than a single mDNS browser,
but tolerates partial network failures and exposes provider health explicitly.
The Flutter layer receives one coherent peer list rather than reconciling
platform-specific discovery streams.

Network-change-driven socket rebuilding, Android multicast-lock ownership, iOS
Bonjour declarations, BLE adapters, and per-source packet rate limits are
required implementation work. The current crate must be described as an
experimental core until those behaviors pass real-device tests.

Provider-specific data-path behavior is defined by
[ADR 0007](0007-multi-bearer-data-channels.md); this ADR remains limited to
rendezvous observations and endpoint evidence.
