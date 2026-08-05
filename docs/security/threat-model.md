# Halo security threat model

- Status: Draft for pairing and single-file transfer protocol v1
- Updated: 2026-08-05

## Scope and protected assets

This revision covers discovery, multi-bearer local channel selection, pairing,
and the experimental single-file transfer slice on a hostile local network.
Authenticated resumable manifests, multi-file atomicity, and durable resume
data remain out of scope.

Protected assets are the long-lived device identity key, remembered-peer
records, the user's pairing decision, peer authenticity, control-message
confidentiality and integrity, user-authorized infrastructure Wi-Fi
credentials, file contents and metadata, receiver filesystem integrity, and
sensitive diagnostics.

## Trust boundaries

- Discovery packets, BLE identifiers, DNS records, addresses, ports, device
  names, and capability hints are untrusted.
- BLE advertisements, GATT services, fragments, and link-layer pairing remain
  untrusted until the dedicated bootstrap TLS exporter is bound to a verified
  Halo identity ceremony. Raw GATT never carries a Wi-Fi credential.
- Apple P2P associations, Wi-Fi Direct groups, Wi-Fi Aware pairings, hotspot
  membership, interface metadata, and link-layer credentials are transport
  inputs. None of them establish a Halo identity or authorize file metadata.
- QUIC/TLS encrypts a connection but its ephemeral certificate is not a Halo
  identity until the application handshake is verified.
- Apple native QUIC deliberately accepts the peer's ephemeral transport
  certificate. This does not authorize the peer: Rust must verify the signed
  Halo transcript bound to the exporter from that exact native connection
  before trust or metadata disclosure.
- Rust owns protocol parsing, transcript construction, state transitions, trust
  policy, and persistence decisions.
- Rust owns identity-key creation, signing, verification, and the opaque
  identity-blob format. Android Keystore and Apple Data Protection Keychain
  adapters only protect and persist opaque bytes. Remembered-peer public keys
  and policy metadata remain in app-private files.
- Flutter presents the code and captures consent; it cannot mark a peer trusted.
- Windows/macOS native adapters may retrieve an eligible personal-network
  credential only after an explicit share action and OS authorization. Rust
  owns its one-ceremony envelope and expiry; native mobile adapters receive it
  only to issue the documented OS join request. Flutter never receives it.
- On Apple-managed QUIC paths, Swift passes bounded complete pairing frames
  directly to Rust through a synchronous C ABI. TLS exporters, control frames,
  and file records do not cross the Dart platform channel.

## Attacks and controls

| Threat | Required control | Negative verification |
| --- | --- | --- |
| Endpoint spoofing or active MITM | Signed identities bound to the TLS exporter; compare the same six-digit code on first contact | Different exporter produces a different transcript and an independently distributed code; forged signature is rejected |
| Hello tampering | Strict framing plus signatures over every semantic field and predecessor digest | Flip every field class and verify authentication failure |
| Replay | Fresh TLS exporter, two 256-bit nonces, strict message order, transcript-bound decision and commit | Replay a valid Hello/decision on a second channel and reject it |
| Downgrade | Client signs its version range; server signs the selected intersection; no intersection is fatal | Disjoint ranges and altered selected version are rejected |
| Parser exhaustion | 4096-byte frame cap, fixed v1 message sizes, no attacker-sized allocation before validation | Truncated, oversized, malformed, unknown-kind and reserved-flag inputs are rejected without panic |
| Saved identity substitution | Bind a successfully paired LAN IP (not its changing port) to the complete public key and compare before accepting a new key | A remembered address presenting another valid key returns `IdentityChanged`; IP reuse fails conservatively |
| Consent confusion | Code and consent are bound to one transcript; accepted decision and commit are signed | A decision from another transcript or a mismatched code cannot commit trust |
| Key extraction at rest | OS-backed protection for the Rust identity blob; no plaintext fallback; short-lived zeroized Rust buffers | Platform tests verify protected storage, backup exclusion, deletion, and persistence across restart |
| Stale trust after reinstall or restore | New key is an identity change; no silent repair | Delete/replace local identity and verify peers show a blocking identity-change error |
| Connection resource exhaustion | At most four active ceremonies, one outbound ceremony per discovery reference, a bounded 2-second retry cooldown, at most eight staggered candidates, 8-second connection attempts, 60-second consent, one control stream, 75-second idle timeout, explicit cancellation | Slow peer, duplicate requests, immediate retry, cancellation, disconnect, and network-change tests release tasks and sockets |
| Unexpected background radio use | Discovery continues off-screen only after the user explicitly starts it; Android shows an ongoing foreground-service notification and Stop tears down native and Rust sessions | Put each app in the background, verify disclosure and continuity, then press Stop and verify BLE, LAN sockets, and the Android service are gone |
| Bearer spoofing or confused-deputy selection | Platform adapters return opaque candidates; Rust requires the expected provider class, repeats Halo authentication, and never treats link pairing as trust | Substitute candidates across providers and verify no peer identity, consent, or trust record transfers to the new path |
| Cellular/public-route disclosure | Bind the socket or native QUIC connection to the selected eligible interface/network, disable multipath, reject public candidates, and fail closed on an ineligible path update; address ranges alone are insufficient | Supply public candidates, VPN routes, Wi-Fi-to-cellular transitions, and private addresses routed through an ineligible interface; verify no pairing or file bytes use that path |
| Silent QUIC path migration | Disable server-side active migration, require a provider-attested interface-bound path, and create a new QUIC connection plus Halo authentication after bearer loss | Change the routed interface after authentication and verify the old session cannot continue on cellular, VPN, or another unapproved adapter |
| Stale authenticated session after disconnect | Monitor QUIC closure, remove the session, cancel session transfer tokens, release capacity, and require fresh exporter-bound authentication on reconnect | Close either peer during idle, consent, control, and data phases; the old session disappears and cannot start another transfer |
| P2P resource exhaustion | Bound concurrent link establishment, system prompts, groups, listeners, and candidate lifetime independently per provider | Flood BLE/NAN/Wi-Fi Direct hints and verify bounded work, rate-limited prompts, cancellation, and complete resource teardown |
| Filename traversal or platform ambiguity | Treat the offered name as one conservative leaf; reject absolute/traversal components, separators, controls, Windows device names, and trailing dot/space | Exercise `..`, mixed separators, reserved devices, controls, and ambiguous suffixes; no path may escape the destination |
| Partial, reordered, appended, or corrupted file | Bind control and data streams to the same TLS exporter; require contiguous indices, exact lengths, per-chunk SHA-256, whole-file SHA-256, and stream FIN immediately after the declared bytes | Drop, reorder, duplicate, mutate, truncate, and append data records; remove the private partial and emit no final file |
| Existing-file overwrite or staging substitution | Require caller-provided private directories, reject symlink directories, create staging with `create_new`, and finalize with a same-filesystem no-overwrite hard link | Pre-create final and staging names and substitute symlinks; preserve existing content and never remove an unknown staging entry |
| Source changes after consent | Hash the complete private source before the offer, then re-hash while sending and reject size or digest changes | Truncate, append, and replace the selected source after offer creation; receiver must not finalize it |
| Flutter/native path abuse | File bytes never cross Dart; native pickers copy into app-private outgoing storage, Rust validates file type and destination policy, and native cleanup accepts only canonical `.upload` children of that outgoing directory | Submit arbitrary cleanup and transfer paths through the method channel and verify files outside the private outgoing directory are untouched |
| Transfer resource exhaustion or consent bypass | Permit one active transfer per authenticated session, cap file/chunk/frame sizes and event queues, require an explicit receive decision with a 60-second timeout, and cancel session work on shutdown | Race simultaneous offers, withhold decisions, send oversized offers, and stop during each state; no transfer proceeds without a current accepted request |
| Premature bearer winner | A candidate wins only after eligible-path validation, QUIC, exporter-bound Halo authentication, and peer correlation all succeed | Make the fastest association fail authentication and verify a slower valid bearer wins without metadata disclosure |
| Prompt amplification | Race automatic candidates first; serialize candidates that require system UI, cap the default prompt budget at one, and allow only one active outbound ceremony per discovery reference | Supply many Direct/Aware/P2P candidates and duplicate connect requests; verify at most one ceremony/system-action attempt begins for that peer |
| Duplicate authenticated sessions | Deduplicate retained connections by cryptographic peer ID rather than name, address, or discovery reference | Complete concurrent or repeated authentication for the same key; retain one session and one transfer listener without consuming extra capacity |
| Identity-change fallback confusion | Treat a remembered identity change and explicit user rejection as peer-wide hard stops, while ordinary unauthenticated spoof candidates may fall through | Return each authentication failure class from the first candidate and verify only recoverable authentication failure tries another bearer |
| Silent Wi-Fi credential extraction | Capability probing never reads a secret; Windows/macOS retrieval requires an explicit foreground share action, documented native APIs, and successful OS authorization; Android/iOS saved-password export is unsupported | Probe capability repeatedly and deny/cancel Windows UAC or macOS Keychain access; verify no secret lookup continues and fallback remains available |
| Wi-Fi credential disclosure | Scope the secret to one peer and ceremony; never send it through Dart, advertisements, rendezvous/raw GATT, discovery, unauthenticated IP, logs, diagnostics, clipboard, persistence, backups, or crash reports; use a short-lived QR or authenticated one-use invitation | Instrument every boundary and lifecycle event, force crashes/cancellation, inspect persisted state and logs, and verify the credential is absent |
| BLE bootstrap MITM or plaintext downgrade | Run TLS 1.3 over the bounded GATT stream, bind the signed Halo identity ceremony to its exporter, require short-code comparison on first contact, and reject any raw-GATT credential message | Interpose and replace GATT fragments, exporters, certificates, identities, decisions, and capability flags; no credential is emitted before authentication commits |
| Malicious Wi-Fi QR or invitation | Strictly bind and length-limit the Wi-Fi invitation grammar; reject conflicting fields, controls, unsupported security, profiles, EAP/Passpoint/managed data, stale expiry, replay, or wrong peer binding | Fuzz QR and control payloads, replay invitations across peers/ceremonies, and verify no join request occurs for rejected input |
| Evil twin or network-membership trust | Treat SSID and association as untrusted; require an exact-network nonce probe followed by fresh exporter-bound Halo authentication before metadata or file bytes | Join an AP with the same SSID and password but no expected peer, substitute another Halo identity, and verify the candidate cannot win |
| Destructive Wi-Fi profile handling | Prefer ephemeral joins, track only opaque Halo-created configuration, never overwrite/delete a pre-existing profile, and serialize user-authorized fallback prompts | Pre-create the target profile, cancel at every step, fail joins, and verify user configuration and previous connectivity are preserved |

## Security invariants

1. No discovery observation can create or update a trust record.
2. No unverified peer receives a stable local identifier or file metadata.
3. Trust is written only after both signed Hello messages, the displayed
   transcript code, an accepted signed decision, and a signed commit validate.
4. A known peer with a different public key is a hard error.
5. Pairing data is never sent as QUIC 0-RTT data.
6. Logs redact complete public keys, identity digests, addresses, and TLS
   exporter or transcript values.
7. Link-layer pairing, group membership, or a previously approved platform
   device never creates Halo trust without the exporter-bound handshake.
8. Nearby mode never sends control or file bytes over cellular or an Internet
   path, including after a route change.
9. Flutter platform channels never transport TLS exporter material, Halo
   control frames, or file contents.
10. A received file becomes visible at its final name only after exact stream
    termination, size, chunk order, chunk digests, and whole-file digest pass.
11. Finalization never replaces an existing destination, and cleanup never
    removes a path it did not create for the current transfer.
12. Wi-Fi capability probing never retrieves a password; any retrieval or entry
    is foreground, user-authorized, single-ceremony, and excluded from Flutter,
    advertisements, raw GATT, diagnostics, logs, clipboard, and persistence.
13. An authenticated BLE bootstrap may carry one Wi-Fi invitation but never file
    metadata or bytes. Joining the expected SSID does not authorize a peer; the
    LAN path requires a fresh nonce probe and QUIC/Halo authentication.

## Residual risks and required device validation

- Users can accept matching codes without actually comparing both displays.
- A compromised application process can request signatures and falsify UI.
- Because protocol crypto runs in Rust, a compromised process can read the
  identity after protected storage unlocks it. Non-exportable signing keys are
  deliberately not used to keep protocol behavior cross-platform and shared.
- The current Apple bridge passes a session-only P-256 transport private key
  from Rust through generated Dart/platform-channel buffers to construct the
  native QUIC identity. Those managed-buffer copies cannot be reliably
  zeroized. The key is never persisted or logged and is not a Halo identity;
  direct native/FFI ownership should replace this bootstrap before the Apple
  provider advances beyond `planned`.
- OS backup, biometric policy, key invalidation, lock state, and hardware-backed
  availability differ by device and distribution mode.
- Local denial of service, Wi-Fi roaming, captive portals, VPN routing, and
  aggressive mobile lifecycle suspension can still interrupt pairing.
- The Android Demo now transfers a UDP socket bound to an eligible, unmetered
  Android `Network` directly into Quinn. If native preparation is absent,
  ineligible, or fails, Android uses a loopback-only listener rather than a
  wildcard fallback. A default-route change cannot migrate the socket; if the
  selected Android `Network` disappears or becomes ineligible, the UI requires
  a discovery restart. Apple and Windows LAN sockets
  still need equivalent exact-interface adapters, and Android's no-cellular
  guarantee remains a physical-device validation gate.
- P2P capability depends on OS version, entitlement, hardware, firmware, radio
  coexistence, driver behavior, and system UI. Provider state can change while
  the app is running and must be treated as revocable.
- Android foreground-service policy, vendor battery management, force-stop, and
  process death can still end discovery. macOS sleep or application Quit ends
  availability. iOS background continuity is not supported by this decision.
- Address binding deliberately fails closed when a DHCP lease is reused by a
  different device. Moving a trusted peer to a new address still authenticates
  its stored key, but the current privacy-preserving discovery descriptor does
  not announce a stable identity hint.
- Six decimal digits provide human-comparison usability, not high-entropy
  machine authentication. The implemented cooldown limits repeated use of one
  discovery reference, but a hostile peer can rotate rendezvous identifiers;
  broader durable abuse controls remain a release-hardening requirement.

Before claiming Android ↔ macOS pairing support, physical-device tests must
cover restart persistence, identity replacement, Wi-Fi switching, cancellation,
permission denial, app foreground transitions, and at least one active relay
attempt that produces different codes.
