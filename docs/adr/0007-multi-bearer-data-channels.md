# ADR 0007: Multi-bearer local data channels

- Status: Accepted for staged implementation
- Date: 2026-07-29
- Owners: Halo maintainers

## Context

Halo needs one authenticated file-transfer protocol across Android, iOS,
Windows, and macOS, but no public radio API creates an infrastructure-free link
between every pair of those platforms.

BLE is broadly available but is not suitable for the file data plane. An
existing LAN is interoperable but is not always present. Apple peer-to-peer
Wi-Fi is Apple-only. Wi-Fi Direct is available on Android and Windows but not as
a general-purpose Apple API. Wi-Fi Aware is an industry standard available on
Android and on supported iOS/iPadOS 26 devices, but it does not provide a
documented four-platform application surface.

The bearer choice must not fork the Halo pairing or transfer protocol, and an
unavailable provider must not make other providers fail.

## Decision

Halo adopts a multi-bearer data-channel layer below authenticated QUIC:

1. Existing LAN IP is the universal baseline on all four platforms.
2. Apple peer-to-peer Wi-Fi is a real Apple-to-Apple provider implemented with
   Network.framework peer-to-peer browsing, listeners, and QUIC.
3. Wi-Fi Direct is a real Android-to-Android, Android-to-Windows, and
   Windows-to-Windows provider, subject to cross-vendor device validation.
4. Wi-Fi Aware is a real Android-to-Android and supported
   Android-to-iOS/iPadOS provider, subject to entitlement, hardware, OS, and
   cross-stack interoperability validation.
5. User-authorized infrastructure Wi-Fi onboarding may prepare a new LAN
   candidate when the peers do not already share one. It is a prompt-requiring
   data-channel setup ceremony, not discovery or a new bearer; saved passwords
   are never scraped. The complete decision is in
   [ADR 0008](0008-user-authorized-wifi-onboarding.md).
6. A user-created hotspot is treated as another LAN, not a distinct protocol.
   Halo may guide setup but does not claim it can create a hotspot on every OS.
7. BLE remains rendezvous and wake-up for discovery and never carries file
   contents. A separate authenticated, TLS-bound BLE bootstrap control channel
   may carry one-use Wi-Fi onboarding credentials as defined by
   [ADR 0009](0009-authenticated-ble-bootstrap-channel.md); raw discovery and
   GATT payloads never do.
8. Cellular, Internet rendezvous, NAT traversal, and relays are not fallback
   bearers. A path change to cellular fails closed and requires a new eligible
   local path and a new authenticated connection.

The active delivery milestone and its path order are defined by
[ADR 0010](0010-android-macos-first-delivery.md): Android/macOS platforms first,
including Android ↔ macOS and Android ↔ Android, shared LAN before a
user-prepared hotspot, and no cellular or Internet fallback.

All providers produce opaque `LinkCandidate` handles and explicit capability
state. `halo-core` races eligible candidates with bounded concurrency. The
winner must establish Halo QUIC, export the standard pairing channel binding,
and authenticate the peer before file metadata is disclosed. Losing attempts
are cancelled and release radio/network resources.

Rust Quinn remains the transport engine for providers that expose a bound UDP
socket and IP endpoints. Apple peer-to-peer and Apple Wi-Fi Aware may use
Network.framework QUIC when the path is not available to a portable socket.
Both engines use the same ALPN, wire messages, limits, TLS-exporter label, and
negative vectors. Cross-engine interoperability is a release gate.

Network.framework QUIC uses one `NWConnectionGroup` as the authenticated tunnel
and separate `NWConnection` objects for pairing and file-data streams. The
Apple adapter exchanges bounded complete frames with Rust through a narrow
synchronous C ABI. Flutter may request a connection and render lifecycle state,
but TLS exporters, protocol frames, and file bytes never cross a Dart platform
channel.

## Consequences

Halo has one product protocol but multiple platform link adapters. Capability
and lifecycle differences are visible to Rust and Flutter rather than hidden.
The common LAN path can ship before every infrastructure-free pair is ready,
while Apple P2P, Wi-Fi Direct, and Wi-Fi Aware remain committed implementation
tracks with explicit exit criteria.

Not every platform pair can offer router-free transfer. In particular, a macOS
peer has no documented public Wi-Fi Direct application API, and Apple currently
documents Wi-Fi Aware host support for selected iPhone and iPad models rather
than macOS. Those pairs fall back to LAN or a user-created hotspot.

Infrastructure onboarding does not make the flow fully automatic. Android and
iOS ordinary apps cannot read the current saved Wi-Fi password. Windows and
macOS may export a personal-network password only through documented native
APIs after explicit user action and successful OS authorization. Otherwise the
user must provide QR/manual input, or an already authenticated Halo channel must
carry a one-use invitation. Enterprise and managed credentials remain excluded.

Two QUIC engines increase test cost. Golden protocol vectors, exporter-binding
tests, cross-version tests, cancellation tests, and real-device matrices are
required before any provider moves from `planned` to `experimental` or `beta`.

The complete design and support matrix are in
[`docs/architecture/data-channels.md`](../architecture/data-channels.md).

iOS/iPadOS and Windows remain protocol targets, but their remaining product
capabilities and device matrices are deferred until the active Android/macOS
local-transfer matrix passes. Deferral does not change existing wire
compatibility or evidence-based support labels.

## Implementation status

The Rust broker now distinguishes automatic unmetered shared LAN from a
lower-priority, explicitly approved local-hotspot scope, while enforcing exact
interface binding, bounded automatic racing, deferred serial system prompts,
and authentication-before-win. Recoverable authentication failure falls through;
identity change and user rejection are peer-wide hard stops. Android and Apple
launchers expose distinct capability states for all four provider kinds.

Quinn exposes constructors that consume a socket already selected and bound by
the platform adapter, and the server disables active address migration. The
core pairing configuration accepts shared LAN only when unmetered, accepts an
expensive/metered hotspot only with the distinct user-approved scope, and
rejects unbound or inconsistent attestations. A closed QUIC transport is
evicted from authenticated sessions and cancels its transfer work; reconnection always creates a new authenticated
session. Concurrent ceremonies are deduplicated by discovery reference, while
retained sessions are deduplicated by authenticated cryptographic peer ID.
The current LAN `PairingService` now retains only an authenticated winner and falls
through to another discovered endpoint after a recoverable candidate failure.
Android now uses the pre-bound-socket contract: Kotlin binds a UDP socket to an
eligible, unmetered Android `Network`, transfers the duplicated descriptor
directly to Rust through JNI, and Rust consumes it once. Missing or failed
native preparation produces a loopback-only listener, never a wildcard
fallback. If the selected Android `Network` disappears or becomes ineligible,
discovery must restart until endpoint hot replacement exists; changing only
the default route does not migrate the bound socket. Android hotspot joiners
use an OS-mediated `WifiNetworkSpecifier`; credentials stay inside native UI,
and only an exact Wi-Fi `Network` without Internet capability is registered as
user-approved. iOS/macOS select an eligible, non-expensive, unconstrained
Network.framework Wi-Fi/Ethernet interface for automatic LAN. macOS may also
explicitly approve the current Wi-Fi as a lower-priority hotspot scope. Both
paths apply `IP_BOUND_IF` to a native IPv4 UDP socket and transfer its descriptor
directly to the same Rust registry. Missing or failed preparation
also produces a loopback-only listener. Apple IPv6 binding, Windows LAN
binding, common-broker integration, and Wi-Fi Direct/Wi-Fi Aware providers
remain incomplete. Creating a hotspot on one Halo endpoint is also not yet a
supported path: Android's public host API does not expose an exact `Network`
that satisfies the current socket-ownership contract. Android ↔ macOS and
Android ↔ Android are the active device-validation targets, with macOS ↔ macOS
retained as a regression path;
iOS/iPadOS and Windows work is deferred. Therefore these host-built integrations
do not advance any provider's support label before the corresponding device
tests.
