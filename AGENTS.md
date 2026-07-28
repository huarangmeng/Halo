# AGENTS.md

This file defines the working agreement for humans and coding agents in the
Halo repository. It applies to the whole repository. A more deeply nested
`AGENTS.md` may add component-specific rules, but must not weaken the security,
compatibility, or verification requirements in this file.

## Product intent

Halo is an open, account-free, cross-platform nearby connectivity protocol and
SDK. File transfer is its first reference use case, not the final boundary of
the platform.

The first supported product flow is deliberately narrow:

1. Two devices are on the same local network.
2. Halo is open in the foreground on both devices.
3. The devices discover one another.
4. The receiver explicitly accepts a new peer or trusts a previously paired
   peer.
5. Files move directly between the devices over an encrypted connection.

The initial demo targets Android, iOS, Windows, and macOS. The reusable core is
written in Rust. Flutter is a presentation and integration layer, not the home
of protocol or transfer logic.

Do not describe Halo as an AirDrop-compatible implementation. Do not promise
system-level discovery, background operation, or identical behavior on every
platform. In particular, iOS foreground, local-network, Bluetooth, and
background-execution limits must be reflected honestly in product behavior and
documentation.

## Priorities

When requirements compete, use this order:

1. User safety and data integrity
2. Interoperability and protocol compatibility
3. Correctness and recoverability
4. A simple user experience
5. Performance
6. Additional features

Never trade authentication, integrity checking, or safe path handling for a
benchmark result.

## Intended repository layout

The repository will evolve toward this structure:

```text
Halo/
├── Cargo.toml                 # Rust workspace
├── README.md                  # Default English project overview
├── README.zh-CN.md            # Simplified Chinese translation
├── crates/
│   ├── halo-core/             # Public SDK facade and orchestration
│   ├── halo-protocol/         # Versioned wire messages and state machines
│   ├── halo-discovery/        # Discovery traits and common models
│   ├── halo-transport/        # Connection abstractions and QUIC transport
│   ├── halo-transfer/         # Manifests, chunks, resume, integrity checks
│   ├── halo-crypto/           # Identity, pairing, secure storage interfaces
│   └── halo-ffi/              # Narrow Flutter-facing Rust API
├── platform/
│   ├── android/               # Android discovery/storage adapters
│   ├── ios/                   # iOS discovery/storage adapters
│   ├── macos/                 # macOS discovery/storage adapters
│   └── windows/               # Windows discovery/storage adapters
├── apps/
│   └── halo_demo/             # Flutter reference application
├── protocol/                  # Protocol specification and test vectors
├── docs/                      # Architecture, threat model, ADRs, benchmarks
└── tools/                     # Developer scripts that are safe to rerun
```

Do not create crates merely to match this diagram. Split a crate only when it
has an independently testable responsibility or a useful dependency boundary.

## Architecture boundaries

- `halo-core` owns workflows and exposes the stable SDK surface.
- `halo-protocol` owns wire compatibility. It must not depend on UI, Flutter,
  or platform frameworks.
- Discovery produces candidates; it does not imply identity or trust.
- Transport moves authenticated bytes; it does not decide whether a transfer
  is allowed.
- Transfer code owns manifests, chunking, resume state, integrity validation,
  and safe finalization.
- Platform adapters implement capabilities behind Rust traits where practical.
- `halo-ffi` must expose coarse, asynchronous operations and event streams. Do
  not mirror every internal Rust type across FFI.
- Flutter owns screens, navigation, accessibility, localization, and user
  consent. Business rules must remain in Rust.

Keep the control plane (discovery, pairing, offers, acceptance, cancellation)
separate from the data plane (file streams and chunk acknowledgements). This is
required so transports can change without redesigning the user flow.

## Protocol and compatibility rules

- All wire messages must have an explicit protocol version.
- Use a deterministic, cross-language serialization format selected by an ADR.
- Parsers must reject malformed lengths, impossible states, unknown required
  fields, and resource-exhaustion inputs without panicking.
- Adding an optional field may be compatible. Changing a field's meaning,
  removing a field, or changing state-machine order is a breaking change.
- A protocol change requires specification updates, golden vectors, and tests
  involving at least two independently built peers.
- Never serialize Rust implementation details directly as the public protocol.
- Put experimental messages behind a capability bit and document their expiry
  or stabilization plan.

The first network transport should be QUIC over the local IP network. Discovery
must run the supported providers concurrently: BLE rendezvous, mDNS/DNS-SD,
IPv4 and IPv6 presence multicast, IPv4 directed broadcast, and direct probes to
remembered endpoints. A provider may be unavailable because of hardware,
permission, or network policy, but that state must be explicit and must not stop
the other providers. BLE must never carry file data or be treated as proof of
identity.

## Security and privacy requirements

- Use established, reviewed cryptographic libraries and protocols. Do not invent
  cryptographic primitives or bespoke encryption modes.
- Encrypt all control and file data in transit.
- Treat discovery metadata, filenames, peer names, IP addresses, and logs as
  sensitive.
- A newly seen peer requires explicit receiver consent and a verifiable pairing
  step. Discovery alone never grants trust.
- Persist long-lived identity keys only through OS-backed secure storage adapters
  where the platform supports them.
- Derive paired-peer identity from cryptographic keys, never display names or IP
  addresses.
- Validate every received path. Reject absolute paths, traversal components,
  device names, links, and platform-ambiguous paths.
- Receive into a private staging location, verify size and digest, then finalize
  atomically when the platform allows it.
- Enforce configurable limits for offer count, filename length, file size,
  aggregate transfer size, concurrent streams, and idle time.
- Do not log file contents, secret material, complete filesystem paths, or stable
  peer identifiers. Redact diagnostics by default.
- Zeroization is not a substitute for correct key lifecycle design. Document any
  secret that cannot reliably be erased on a target platform.

Any code touching authentication, key storage, path handling, protocol parsing,
or overwrite behavior requires targeted negative tests and a threat-model update
when the security boundary changes.

## Rust guidance

- Use stable Rust and pin the toolchain in `rust-toolchain.toml` once the
  workspace is created.
- Format with `cargo fmt` and keep `cargo clippy --all-targets --all-features`
  clean. Treat new warnings as failures in CI.
- Avoid `unsafe`. If platform FFI requires it, isolate the smallest possible
  block, document the invariants with a `// SAFETY:` comment, and test the safe
  wrapper.
- Public APIs return structured errors; libraries must not call `unwrap`,
  `expect`, `panic!`, or terminate the process for recoverable input or I/O
  failures.
- Make cancellation explicit and propagate it through discovery, connection,
  and transfer tasks.
- Do not hold blocking filesystem or platform calls on the async runtime.
- Prefer bounded channels and streams. Every queue must have a backpressure or
  drop policy.
- Keep feature flags additive and test meaningful feature combinations.
- Minimize dependencies, disable unused default features, and review additions
  for maintenance, license, platform support, and security posture.

## Flutter and platform guidance

- Keep the demo app thin. Protocol decisions and transfer state transitions do
  not belong in Dart widgets or platform channels.
- Use one generated binding layer (initially evaluate `flutter_rust_bridge`) and
  pin its generator/runtime versions together. Commit generated bindings only
  if the build is reproducible and CI verifies they are current.
- Platform adapters must expose capability status, including permission denied,
  unavailable in background, unsupported transport, and local-network disabled.
- Permission prompts need user-facing context before the OS dialog appears.
- Never request contacts, location, Bluetooth, local-network, or broad storage
  access unless a shipped capability requires it on that platform.
- All four demo targets must share the same transfer state model. Platform
  differences belong in capability adapters and UI messaging.
- UI work must include keyboard navigation on desktop, screen-reader labels,
  scalable text, clear progress/cancellation, and actionable error messages.

## Testing and verification

Use the smallest relevant checks while iterating, then the full applicable set
before declaring work complete. Once the workspace exists, the expected baseline
will be:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
flutter analyze apps/halo_demo
flutter test apps/halo_demo
```

These commands are a target contract until their configuration files exist; do
not claim they ran in an unscaffolded repository.

Test layers should include:

- Unit tests for state machines, policy, chunking, and path normalization
- Property and fuzz tests for untrusted protocol and manifest parsers
- Golden protocol vectors shared across supported versions
- Integration tests with packet loss, duplication, reordering, cancellation,
  low disk space, permission denial, sleep/wake, and interrupted resume
- Cross-version tests between the oldest supported and current protocol
- Device tests on all four target platforms before a release
- Benchmarks that report hardware, OS, network, payload shape, encryption, and
  end-to-end verification settings

A loopback test is useful but is not evidence of cross-device interoperability.

## Change workflow

Before implementing a change:

1. Read this file and the nearest nested `AGENTS.md`.
2. Inspect existing code, tests, ADRs, and the protocol specification.
3. State assumptions when platform behavior or compatibility is uncertain.
4. For a material architecture or dependency decision, add an ADR in
   `docs/adr/NNNN-short-title.md`.

While implementing:

- Keep changes focused and preserve unrelated user changes.
- Update tests alongside behavior.
- Update the protocol spec in the same change as wire behavior.
- Prefer capability detection to scattered OS/version conditionals.
- Use `TODO(owner-or-issue): reason` only for deliberate, tracked follow-up.

Before handing off:

- Run and report relevant verification accurately.
- Call out untested platforms and remaining risks.
- Update README status if a milestone or support claim changed.
- Never mark a feature supported because it compiles; verify its user flow on a
  real device or documented CI environment.

## Documentation language

Code, public identifiers, protocol specifications, ADRs, and canonical technical
documentation use English. User-facing localization may include any supported
language. Prefer precise support labels: `planned`, `experimental`, `beta`, and
`stable`.
