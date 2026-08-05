# ADR 0009: Authenticated BLE bootstrap control channel

- Status: Accepted design; protocol and implementation planned
- Date: 2026-08-05
- Owners: Halo maintainers

## Context

Infrastructure Wi-Fi onboarding can require a small secret to cross between two
nearby devices before they share an IP path. Windows or macOS may obtain the
current personal-network credential after explicit user and OS authorization,
while Android or iOS/iPadOS can use supplied credentials in an OS-mediated join
flow. A Wi-Fi passphrase is small enough for BLE, but BLE discovery observations,
advertisements, GATT handles, link-layer pairing, and device names are untrusted.

Sending a password in an advertisement, plaintext GATT characteristic, or a
channel selected only from discovery evidence would expose the network to a
nearby attacker. BLE link encryption is platform-dependent and does not bind the
connection to a Halo cryptographic identity. Halo therefore needs a control-only
bootstrap channel with the same authentication standard as its IP data paths.

## Decision

Halo may carry a Wi-Fi onboarding invitation over BLE only through a distinct
`AuthenticatedBootstrapChannel`. The ordinary BLE rendezvous provider remains
discovery-only. The bootstrap channel is a bounded, foreground, bidirectional
control transport and never carries file contents, file metadata, manifests, or
transfer chunks.

Any non-BLE path may implement the same bootstrap interface only after it has
completed equivalent encryption and Halo peer authentication. A discovered LAN
endpoint, mDNS record, Presence packet, broadcast response, BLE advertisement,
or unauthenticated platform association is not a bootstrap channel.

### Security construction

Halo does not invent a BLE-specific encryption mode. The target design adapts
GATT writes and notifications into a bounded reliable byte stream, runs TLS 1.3
with a dedicated `halo-bootstrap/1` ALPN over that stream, and binds the normal
signed Halo identity ceremony to the TLS exporter from that exact connection.
QUIC is not required for this small bootstrap stream, but the authentication
properties remain the same:

- ephemeral transport certificates are not peer identities;
- TLS 1.3 0-RTT and session resumption are disabled;
- the exporter label and bootstrap transcript domain are versioned and distinct
  from the QUIC data channel;
- known peers must prove the remembered Halo public key;
- first-contact peers require the same user-verifiable short-code comparison and
  signed acceptance/commit semantics before a credential is sent;
- a remembered identity change is a hard stop across every fallback path.

Before implementation, this design requires a versioned protocol specification,
independently generated golden vectors, cross-language framing tests, and review
of the TLS-over-GATT adapter. Reusing TLS and the Halo identity ceremony does not
permit silently reusing QUIC wire assumptions; transport-domain separation must
be explicit.

### GATT record layer

The native adapter owns BLE scanning, advertising, connection, MTU observation,
write/notification scheduling, and platform lifecycle. Rust owns a bounded
record layer above opaque GATT fragments:

- fixed protocol magic and version;
- ceremony ID, direction, monotonically increasing record sequence, declared
  ciphertext length, and final-fragment marker;
- hard limits on total handshake bytes, record size, buffered fragments,
  retransmission, idle time, and concurrent bootstrap sessions;
- duplicate, gap, reorder, oversize, timeout, disconnect, and cancellation
  handling that fails closed and releases all buffers.

The native layer never parses a Wi-Fi credential. Flutter receives only redacted
state and consent requests; it never receives GATT payloads, TLS bytes, exporter
material, Wi-Fi passphrases, or the bootstrap invitation.

### Credential envelope

After authentication, Rust may send one bounded `WifiOnboardingInvitation`
containing only the information required by ADR 0008: protocol version,
ceremony ID, intended sender and receiver identities, one-use nonce, expiry,
SSID, supported personal security mode, and passphrase when required. The
envelope is accepted only on the authenticated channel for the expected peer
and current ceremony.

The receiver acknowledges exactly one terminal result: accepted for OS join,
rejected, unsupported, expired, or cancelled. Receipt does not prove association
or create a data path. The secret is released after it is handed once to the
native join adapter or when any terminal result occurs.

### Establishment flow

1. BLE rendezvous reports an untrusted candidate and rotating Presence ID.
2. The user selects the peer or accepts an incoming bootstrap request.
3. Native adapters establish the GATT byte stream with bounded time and prompts.
4. TLS 1.3 and the exporter-bound Halo identity ceremony authenticate the peer.
5. The credential source performs its separate user-authorized retrieval or
   input flow.
6. Rust sends the one-use invitation and receives a bounded acknowledgement.
7. The receiving platform requests the Wi-Fi join through its documented API.
8. Both devices close the BLE bootstrap session and release secret buffers.
9. On the new LAN, peers perform the nonce-bound reachability probe and establish
   a new authenticated QUIC connection before any file metadata or bytes move.

The bootstrap authentication cannot be reused as the LAN transport binding. A
fresh QUIC/TLS exporter prevents a BLE attacker, stale association, evil twin,
or route change from inheriting the authenticated session.

### Fallback

BLE bootstrap failure does not imply that the peer is malicious unless identity
verification fails or the user rejects it. Permission denial, radio loss,
timeout, MTU/framing failure, or lifecycle suspension closes only that candidate.
The user may explicitly try an already authenticated P2P control path, direct
platform provider, QR ceremony, or guided hotspot. Prompts remain serialized and
bounded; Halo never loops through system dialogs automatically.

## Consequences

This enables an authorized Windows/macOS sender to provision an Android or
iOS/iPadOS receiver without first sharing an IP network or exposing the password
as a visible QR. It adds a second TLS carrier, a GATT record layer, and another
cross-platform security matrix. That complexity is justified only for small
bootstrap control messages; BLE remains intentionally excluded from the file
data plane.

Both applications must be in a platform-supported foreground state. Background
continuity, BLE availability, and simultaneous central/peripheral roles are
capability results rather than promises.

## Required validation

- Android, iOS/iPadOS, Windows, and macOS central/peripheral role combinations,
  including role reversal and at least two BLE controller/vendor families.
- TLS implementation interoperability, exporter equality, identity binding,
  first-contact short code, remembered peers, identity change, and cancellation.
- Fragment loss, duplication, reorder, truncation, oversize, MTU changes,
  notification backpressure, disconnect, radio toggle, and foreground loss.
- Nearby active attacker, malicious GATT server/client, replayed invitation,
  wrong peer, stale ceremony, and credential substitution cases.
- No password or protocol byte appears in advertisements, Flutter channels,
  logs, diagnostics, crash reports, persistence, clipboard, or backups.
- After onboarding, file transfer cannot begin until the new LAN path completes
  fresh QUIC and Halo authentication.
