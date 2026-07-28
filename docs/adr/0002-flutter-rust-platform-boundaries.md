# ADR 0002: One Flutter UI and one Rust-owned capability core

- Status: Accepted
- Date: 2026-07-28
- Owners: Halo maintainers

## Context

Halo targets Android, iOS, Windows, and macOS without allowing each application
to grow an independent implementation of discovery, pairing, or transfer. A
native demonstration UI or a native Presence parser can make one platform look
functional quickly, but it creates a second behavior source and invalidates the
cross-platform protocol claim.

Some operating-system facilities cannot be called portably from Rust. Bluetooth
permissions, scanning, connectable advertising, and GATT callbacks are examples.
Those facilities still need Kotlin, Swift, and Windows platform code, but that
code must not become a second Halo implementation.

## Decision

Halo has one application UI, implemented in Flutter, and one capability core,
implemented in Rust:

```text
Flutter screens and consent UI
              |
      generated Rust FFI
              |
   Rust discovery session
    /                 \
Rust LAN providers    raw platform BLE driver
                      Kotlin / Swift / WinRT
```

The Rust layer owns:

- Presence packet encoding and strict decoding
- discovery session lifetime and cancellation
- provider state normalization
- observation TTL, deduplication, multi-source merge, and endpoint ranking
- the peer snapshot and event model exposed to Flutter
- every future Halo pairing, transport, transfer, and trust workflow

Flutter owns widgets, navigation, permission explanations, accessibility,
localization, and rendering the Rust event model. A small integration service
may start a native driver with opaque bytes supplied by Rust and forward its raw
events to Rust; it must not parse protocol packets or implement discovery rules.

Platform-native drivers own only OS API access:

- request execution after Flutter has obtained user consent
- BLE scan and connectable advertisement lifecycle
- GATT reads, writes, notifications, and bounded connection scheduling
- raw byte and transient radio metadata delivery
- raw permission, hardware, and background capability status delivery

Native drivers must not decode Halo wire data, merge peers, select endpoints,
persist peer identity, or expose a platform-specific product state machine.

`flutter_rust_bridge` is the generated binding layer. Its Dart runtime, Rust
runtime, and generator are pinned to the exact same stable version. Generated
bindings are committed only when a reproducible generation check exists.

## Consequences

Android and macOS use the same Flutter screens and the same Rust discovery
session during interoperability tests. Native BLE unit tests remain useful for
OS lifecycle behavior, but they are not product demos and cannot establish
protocol correctness without the Rust boundary.

The integration has an extra raw-event hop:

```text
native BLE callback -> Dart integration stream -> Rust submit operation
```

This hop is acceptable because BLE carries only small rendezvous descriptors,
not transfer data. File bytes and control-plane protocol messages must never be
routed through a Dart platform channel.

Any duplicated Presence codec or peer aggregation logic in Kotlin, Swift, Dart,
or a platform runner is an architecture violation.
