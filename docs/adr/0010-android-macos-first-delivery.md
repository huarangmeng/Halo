# ADR 0010: Android-and-macOS-first delivery

- Status: Accepted for staged implementation
- Date: 2026-08-05
- Owners: Halo maintainers

## Context

Halo ultimately targets Android, iOS/iPadOS, Windows, and macOS. Finishing every
platform and every bearer concurrently would delay the first complete,
measurable product flow. The repository already has the strongest real-device
evidence and the most complete pairing and file-transfer integration on Android
and macOS.

The product also needs an explicit fallback order when two nearby devices do
not initially share a reachable network. A user-created hotspot can provide a
local IP network without changing the Halo protocol. Cellular transfer is a
different problem: peers normally require Internet rendezvous, NAT traversal,
and potentially a relay. Halo does not operate that infrastructure.

## Decision

### Active platform milestone

1. Android and macOS are the only active product platforms. The required pair
   matrix is Android ↔ macOS in both directions and Android ↔ Android. macOS ↔
   macOS uses the same LAN path and remains a regression case.
2. iOS/iPadOS and Windows remain protocol targets. Existing code and portable
   protocol behavior are preserved, but remaining capabilities, product wiring,
   and device-validation matrices are deferred until the active Android/macOS
   pair matrix exits.
3. Deferred work is labelled `planned`; it is not silently presented as current
   product support.
4. Shared Rust protocol changes remain portable. The delivery order must not
   create an Android/macOS-only wire fork.

### Android/macOS path preference

Halo uses these strict tiers:

1. **Shared local network:** an already available, mutually reachable Wi-Fi or
   Ethernet LAN. This is automatic and always preferred.
2. **User-prepared local network:** user-authorized Wi-Fi onboarding or a
   user-created hotspot. Halo may guide setup, but the established file path is
   still local device-to-device IP.
3. **No network path:** report an actionable error and do not transfer.

An existing shared LAN always outranks hotspot setup. Halo does not stripe a
transfer across paths in the initial implementation. Losing a path closes the
session; reconnecting creates a new QUIC connection and repeats Halo
authentication.

### Explicit exclusion of cellular and Internet transfer

Halo does not send control, metadata, or file bytes over cellular, a public
Internet route, or a cloud relay. It does not implement Internet rendezvous,
NAT traversal, or relay allocation. These paths are rejected rather than used
as a fallback.

A hotspot owner may also have cellular service. Halo treats only the local
Wi-Fi association as a candidate and must not migrate onto the owner's uplink.
The first implemented slice has both Halo endpoints join a user-prepared
hotspot. An endpoint-hosted Android `LocalOnlyHotspot` remains a separate gate
because the public host API does not expose the exact `Network` required by the
socket-ownership contract.

## Consequences

- Android ↔ macOS and Android ↔ Android discovery, pairing, bidirectional file
  transfer, interruption, recovery, and hotspot guidance take priority over
  new iOS/iPadOS or Windows work. macOS ↔ macOS remains a same-implementation
  regression case.
- The first complete product can operate without an account, public rendezvous
  service, NAT traversal service, or content relay.
- When shared Wi-Fi is unavailable and hotspot setup fails or is declined,
  Halo reports that no transfer path is available.
- Apple P2P, Wi-Fi Direct, and Wi-Fi Aware remain documented future providers,
  but they do not block the active Android/macOS local matrix.
- A future proposal to support Internet or cellular transfer requires a new ADR
  covering service ownership, privacy, abuse prevention, bandwidth cost,
  authentication, and explicit user consent. It is not implied by this design.

## Exit criteria

- Shared Wi-Fi transfer passes Android → macOS, macOS → Android, Android ↔
  Android, and the macOS ↔ macOS regression case with exact interface binding.
- A user-prepared hotspot passes with a macOS joiner and an Android joiner,
  including both file-transfer directions after association. Android uses
  `WifiNetworkSpecifier`; macOS requires explicit approval of the current Wi-Fi.
- Endpoint-hosted hotspot modes require their own exact-interface proof and
  bidirectional test before they are added to the product matrix.
- Pairing, receiver consent, cancellation, integrity verification, safe
  finalization, restart, and route-loss behavior pass on physical devices.
- With only cellular or public Internet connectivity available, Halo sends no
  control or file data and reports an actionable no-path state.
- iOS/iPadOS and Windows documentation continues to state `planned`, with no
  claim of current milestone support.
