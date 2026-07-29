# Halo security threat model

- Status: Draft for pairing protocol v1
- Updated: 2026-07-29

## Scope and protected assets

This revision covers discovery-to-pairing on a hostile local network. It does
not yet cover file manifests, receive paths, resume data, or finalization.

Protected assets are the long-lived device identity key, remembered-peer
records, the user's pairing decision, peer authenticity, control-message
confidentiality and integrity, and sensitive diagnostics.

## Trust boundaries

- Discovery packets, BLE identifiers, DNS records, addresses, ports, device
  names, and capability hints are untrusted.
- QUIC/TLS encrypts a connection but its ephemeral certificate is not a Halo
  identity until the application handshake is verified.
- Rust owns protocol parsing, transcript construction, state transitions, trust
  policy, and persistence decisions.
- Rust owns identity-key creation, signing, verification, and the opaque
  identity-blob format. Android Keystore and macOS Keychain adapters only
  protect and persist opaque bytes.
- Flutter presents the code and captures consent; it cannot mark a peer trusted.

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
| Connection resource exhaustion | At most four active ceremonies, at most eight staggered candidates, 8-second connection attempts, 60-second consent, one control stream, 75-second idle timeout, explicit cancellation | Slow peer, cancellation, disconnect, and network-change tests release tasks and sockets |

## Security invariants

1. No discovery observation can create or update a trust record.
2. No unverified peer receives a stable local identifier or file metadata.
3. Trust is written only after both signed Hello messages, the displayed
   transcript code, an accepted signed decision, and a signed commit validate.
4. A known peer with a different public key is a hard error.
5. Pairing data is never sent as QUIC 0-RTT data.
6. Logs redact complete public keys, identity digests, addresses, and TLS
   exporter or transcript values.

## Residual risks and required device validation

- Users can accept matching codes without actually comparing both displays.
- A compromised application process can request signatures and falsify UI.
- Because protocol crypto runs in Rust, a compromised process can read the
  identity after protected storage unlocks it. Non-exportable signing keys are
  deliberately not used to keep protocol behavior cross-platform and shared.
- OS backup, biometric policy, key invalidation, lock state, and hardware-backed
  availability differ by device and distribution mode.
- Local denial of service, Wi-Fi roaming, captive portals, VPN routing, and
  aggressive mobile lifecycle suspension can still interrupt pairing.
- Address binding deliberately fails closed when a DHCP lease is reused by a
  different device. Moving a trusted peer to a new address still authenticates
  its stored key, but the current privacy-preserving discovery descriptor does
  not announce a stable identity hint.
- Six decimal digits provide human-comparison usability, not high-entropy
  machine authentication; repeated failed ceremonies must be rate-limited.

Before claiming Android ↔ macOS pairing support, physical-device tests must
cover restart persistence, identity replacement, Wi-Fi switching, cancellation,
permission denial, app foreground transitions, and at least one active relay
attempt that produces different codes.
