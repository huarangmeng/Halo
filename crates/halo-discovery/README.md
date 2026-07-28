# halo-discovery

Experimental multi-provider nearby discovery for Halo.

## Implemented

- Bounded concurrent provider lifecycle and explicit provider health
- Native-provider observation entry point for BLE/platform adapters
- Per-source TTL, goodbye/withdrawal, deduplication, and peer snapshots
- Endpoint ranking using corroboration, RTT, connection history, and failures
- Sticky last-known-good endpoint with fallback after repeated failure
- Authentication-failure quarantine for an entire untrusted presence
- Fixed 58-byte Halo Presence v1 codec with strict validation
- mDNS/DNS-SD browse and publish
- IPv4 multicast plus per-interface directed broadcast
- IPv6 link-local multicast with interface scopes
- Direct UDP probes to remembered discovery addresses
- Shared BLE Presence codec and GATT UUID contract
- Apple CoreBluetooth provider in `platform/apple`

This list means code exists and host tests pass. It does not mean every item has
passed the four-platform real-device acceptance matrix.

## Quick local smoke test

Run this in two terminals with different advertised QUIC ports:

```bash
cargo run --example discover -- 44331 20
cargo run --example discover -- 44332 20
```

Each process starts mDNS, IPv4 Presence, and IPv6 Presence concurrently. A peer
event shows the merged source count, ranked endpoint, score, and measured query
RTT when a response is available. The example advertises a QUIC port but does
not start a QUIC listener or authenticate the peer.

## Library sketch

```rust,ignore
let local = LocalPresence::new(
    PresenceId::random(),
    ProtocolRange::new(1, 1)?,
    Capabilities::default(),
    quic_port,
)?;

let session = DiscoveryManager::new(local)
    .with_provider(MdnsProvider::default())
    .with_provider(PresenceV4Provider::default())
    .with_provider(PresenceV6Provider::default())
    .start()
    .await?;

let handle = session.handle();
let mut events = handle.subscribe();
```

Platform-native BLE adapters validate characteristic bytes with
`halo_discovery::ble::decode_presence`, then submit the resulting observation
through `DiscoveryHandle::submit_observation`.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
(cd platform/apple && swift test)
```

See the repository [discovery architecture](../../docs/architecture/discovery.zh-CN.md),
[LAN wire protocol](../../protocol/discovery-v1.md), and
[BLE rendezvous protocol](../../protocol/ble-rendezvous-v1.md).

