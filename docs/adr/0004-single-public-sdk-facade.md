# ADR 0004: One public SDK facade over internal Rust crates

- Status: Accepted
- Date: 2026-07-29
- Owners: Halo maintainers

## Context

Halo has independently testable protocol, cryptography, discovery, and
transport responsibilities. Keeping those dependency boundaries improves wire
compatibility testing, security review, optional transport work, and compile
isolation. Exposing every crate as a product dependency, however, would force
SDK consumers to understand internal orchestration and couple their code to
libraries such as Quinn, rustls, or P-256.

The Flutter bridge had also started to own discovery and pairing workflows.
That would create a second SDK implementation and make non-Flutter Rust clients
reimplement lifecycle, limits, trust policy, and event behavior.

## Decision

`halo-core` is the only public Rust SDK facade. It owns product workflows and
exposes Halo-owned configuration, service, event, consent, and error types.
Its public signatures do not expose types from `halo-protocol`, `halo-crypto`,
`halo-discovery`, `halo-transport`, Quinn, rustls, or cryptographic libraries.

The lower crates remain internal dependency and verification boundaries:

- `halo-protocol` owns deterministic wire compatibility.
- `halo-crypto` owns identity, transcript authentication, and trust storage.
- `halo-discovery` owns providers, candidate aggregation, and ranking.
- `halo-transport` owns QUIC, bounded control I/O, cancellation, and racing.

They are marked `publish = false` while Halo has no SDK release. If a future
crates.io release requires separately published implementation packages, those
packages remain undocumented implementation dependencies; clients still add
only `halo-core`.

`halo-ffi` depends on `halo-core`, maintains opaque numeric handles for the
generated binding runtime, and converts facade models into FFI-safe values. It
must not implement protocol, discovery, trust, or connection state machines.

Native distribution similarly has one product surface per ecosystem: one AAR
for Android, one XCFramework for Apple platforms, and one Flutter plugin. The
internal Rust crate graph is statically linked into those artifacts.

## Consequences

- A Rust consumer configures and drives one Halo service dependency.
- Flutter and native consumers cannot accidentally bypass `halo-core` policy.
- Internal libraries may evolve without forcing source changes in SDK clients,
  provided the facade contract remains compatible.
- `halo-core` needs its own integration and negative tests because it is now
  the workflow boundary.
- Internal crates still appear in build metadata and compile transitively; a
  single public dependency simplifies integration but does not eliminate their
  implementation cost or security review requirements.
