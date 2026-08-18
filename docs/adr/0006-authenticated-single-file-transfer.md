# ADR 0006: Authenticated single-file transfer slice

- Status: Superseded by ADR 0011 before release
- Date: 2026-07-29

## Context

This ADR records the unpublished single-file proof slice. ADR 0011 replaced its
wire format and product workflow before any Halo release.

Halo can discover and pair Android and macOS peers, but the current pairing
service closes the QUIC connection immediately after trust is established. File
transfer needs to prove that application data is carried only by the connection
whose TLS exporter was bound to the verified device identities.

The first transfer slice must remain small enough to test negative cases before
multi-file offers and resumability add durable state and scheduling complexity.

## Decision

- A successfully paired QUIC connection becomes an authenticated session owned
  by `halo-core`. It is not exposed across FFI.
- The file-transfer session must use a local-network endpoint. BLE is never a
  file bearer; the separately authenticated control-only onboarding bootstrap
  in [ADR 0009](0009-authenticated-ble-bootstrap-channel.md) may only prepare a
  later LAN. No public or cellular fallback is attempted when Wi-Fi/Ethernet is
  unavailable.
- Transfer control and file bytes use separate bidirectional QUIC streams on the
  authenticated connection. Pairing messages never share the data stream.
- The first slice permits one active, single-file transfer per authenticated
  session. Bounded concurrency across sessions remains possible.
- Product admission is configurable below the protocol maximum. The Demo
  defaults to 10 GiB, a 60-second receiver decision, and 1 MiB progress-event
  granularity; both sender and receiver enforce the size limit independently.
- The sender hashes the complete source with SHA-256 before offering it. Each
  data chunk also carries a SHA-256 digest. The receiver verifies both levels.
- Filenames are protocol data, not paths. Transfer v1 accepts one conservative
  cross-platform leaf name and rejects separators, traversal, control
  characters, Windows device names, and trailing spaces or dots.
- The receiver writes with `create_new` into a caller-provided private staging
  directory on the destination filesystem. After size and digest verification,
  it creates the final name with an atomic no-overwrite hard link and removes
  the staging name. Platforms that cannot provide this guarantee must report an
  explicit finalization error instead of falling back to overwrite.
- Cancellation is explicit. Partial staging files are removed on every handled
  failure or cancellation; an existing destination is never replaced.

## Consequences

The initial implementation reads each source twice, once to build the offer and
once to send it. This is acceptable for the correctness slice and keeps the
manifest authenticated before receiver consent. A later resumable-manifest ADR
may introduce durable hashing state.

Hard-link finalization requires staging and destination to be on the same
filesystem. Platform storage adapters must arrange that layout or report the
capability as unavailable.

Multi-file atomicity, pause/resume, overwrite policies, directory transfer, and
parallel chunks are deliberately excluded from this slice.

## Implementation status

The Rust LAN path now retains authenticated Quinn connections and can open a
separate bounded data stream after its pairing stream ends. `halo-transfer`
implements source preparation, offer/decision helpers, verified chunk I/O,
cancellation cleanup, stream-end validation, and atomic no-overwrite
finalization. `halo-core` now coordinates one transfer per authenticated LAN
session, and the FFI/Flutter Demo exposes send, destination consent, cancel,
and terminal events. Android and macOS native adapters provide private paths
without moving file bytes through Dart. Host loopback and both platform builds
pass; physical Android ↔ macOS transfer validation is still pending.
