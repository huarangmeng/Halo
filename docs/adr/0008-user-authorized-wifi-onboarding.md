# ADR 0008: User-authorized infrastructure Wi-Fi onboarding

- Status: Accepted design; implementation planned
- Date: 2026-08-05
- Owners: Halo maintainers

## Context

Two nearby devices may not initially share a usable LAN even though one device
is attached to an infrastructure Wi-Fi network that could carry a faster and
more stable file transfer. Halo may help the other device join that network,
then use the ordinary authenticated LAN/QUIC data channel.

The operating system's Wi-Fi sharing screen is not evidence that a normal app
can read a saved passphrase. For example, Android Settings is a privileged
system component. Android 10 and later return no configured-network list to a
normal target-SDK application, while system apps and device-policy controllers
have exceptions. Android's supported application APIs instead let an app that
already has network parameters request or suggest a connection with system and
user mediation. iOS similarly accepts supplied Wi-Fi configurations without
exposing the saved password of the current network to the app.

Desktop systems expose a narrower, authorization-dependent source capability.
Windows can return plaintext profile key material through `WlanGetProfile` only
when the caller has the plaintext-key and profile-read permissions; the default
plaintext permission is limited to local administrators. macOS CoreWLAN exposes
`CWKeychainFindWiFiPassword`, but Keychain access control and the user's
authorization decide whether the item is returned. Neither result is a silent,
portable application entitlement.

Enterprise EAP, Passpoint, managed, captive-portal, and policy-controlled
networks may contain personal certificates, user identities, device posture,
or non-transferable organization credentials. Copying such a configuration is
not a valid general-purpose onboarding mechanism and may violate network
policy.

## Decision

Halo treats infrastructure Wi-Fi onboarding as an optional, user-authorized
data-channel setup ceremony. It is not a discovery provider, a Halo identity,
or a new file-transfer protocol. A successful ceremony produces an ordinary
LAN candidate; that candidate still has to establish QUIC and authenticate the
expected Halo peer before it can win.

### Responsibility boundary

- Rust owns eligibility policy, ceremony state, strict invitation parsing,
  secret lifetime, peer correlation, fallback decisions, and the requirement
  to reauthenticate on the resulting LAN path.
- Native platform adapters report redacted Wi-Fi capability and request an
  OS-mediated join. They bind the later UDP socket to the selected network or
  interface and report revocation and network changes.
- Flutter explains the operation and captures explicit user intent. It does not
  receive or persist Wi-Fi passphrases, raw configuration profiles, Halo
  protocol frames, or file bytes.
- No layer uses private APIs, shell commands, keychain scraping, or silent
  privilege elevation. A documented Windows WLAN or macOS CoreWLAN/Keychain
  call is made only after an explicit share action and any OS authorization.

### Suitability states

The current network is never declared suitable from SSID presence or Internet
access alone. The onboarding provider exposes three distinct evidence levels:

1. `ineligible`: not infrastructure Wi-Fi, VPN-only, metered when nearby policy
   forbids it, captive, managed, unsupported security, missing local addressing,
   or denied by platform/network policy;
2. `locally_eligible`: a foreground platform adapter can identify and bind the
   Wi-Fi path, and local multicast/broadcast/unicast sockets can start, but no
   claim is made about AP isolation, cross-VLAN routing, or peer reachability;
3. `peer_verified`: after the invited peer joins, both sides exchange a
   session-nonce-bound reachability probe on that exact network and complete a
   fresh QUIC plus exporter-bound Halo authentication.

Only `peer_verified` is a transferable data path. mDNS visibility, pinging a
gateway, obtaining a private address, or passing an Internet connectivity check
is insufficient.

### Credential sources

The initial version supports only open or personal Wi-Fi configurations that
the target platform can join through a documented public API. A credential may
enter the ceremony only through one of these explicit paths:

1. the receiver scans a Wi-Fi QR code that the user deliberately displayed
   from an operating-system sharing screen;
2. the user enters the SSID and passphrase for this one ceremony;
3. an already authenticated Halo peer sends a short-lived onboarding
   invitation over an existing encrypted control channel, including the
   control-only BLE bootstrap defined by
   [ADR 0009](0009-authenticated-ble-bootstrap-channel.md), to establish or
   migrate a later file transfer to infrastructure Wi-Fi; or
4. a Windows or macOS sender retrieves the current personal-network passphrase
   through a documented native credential API after an explicit user action
   and successful OS authorization, then displays a short-lived QR invitation
   or sends it over an already authenticated Halo control channel.

A QR code is an out-of-band user-consent ceremony, not automatic password
access. The parser accepts only a bounded, documented Wi-Fi invitation grammar
and rejects unknown security modes, oversized fields, duplicate/conflicting
fields, control characters, and configuration-profile payloads.

Saved-password export is capability-gated rather than assumed. Android and
iOS/iPadOS ordinary-app adapters always report `saved_credential_export` as
`unsupported`. Windows reports it as available only when the current profile is
eligible and the calling security context can request plaintext key material;
macOS reports it as permission-required until CoreWLAN/Keychain authorization
succeeds. Denial, cancellation, a missing item, an encrypted-only Windows
result, or an unsupported profile immediately skips this source without asking
the user to weaken OS security. Device-owner, system-app, or organization-wide
policy deployments are outside the public SDK baseline.

Windows must not elevate the whole Halo UI or Rust process. If administrator
authorization is required, a minimal signed one-shot native broker requests the
exact current profile and returns one bounded result over authenticated local
IPC without writing a profile export to disk. Broker cancellation, caller
identity, profile identity, and response size are validated before Rust accepts
the result.

Enterprise EAP, Passpoint, SIM-based, certificate-based, captive-portal,
hidden-network, and managed profiles are excluded from automatic sharing in the
initial version. The UI may direct users to normal system network enrollment,
but Halo does not serialize those credentials.

### Secret handling

- A passphrase is scoped to one foreground ceremony and one selected peer.
- Desktop retrieval occurs only after a `Share current Wi-Fi` action; capability
  probing never reads the secret. A later peer or retry requires a new action.
- It is never logged, included in diagnostics, written to Flutter state,
  persisted by Halo, backed up, placed on the clipboard, or reused for another
  peer.
- Native join APIs receive the secret through a narrow in-process interface.
  Rust and native buffers are released promptly; best-effort zeroization does
  not replace the rule against persistence and unnecessary copies.
- A control-channel invitation is accepted only after Halo authentication and
  is bound to the sender identity, receiver identity, ceremony nonce, expiry,
  network descriptor, and one use. BLE advertisements, rendezvous GATT, raw or
  unauthenticated GATT, discovery packets, and unauthenticated IP channels never
  carry credentials. An authenticated BLE bootstrap is a separate control
  channel, not part of discovery and never part of the file data plane.
- QR transfer necessarily exposes the credential to cameras and anyone who can
  view the display. A platform-native protected share view renders the QR; Dart
  receives neither its payload nor credential-bearing pixels. The UI warns the
  user, uses a short display lifetime, blocks screenshots where the platform
  supports it, and requires explicit reveal.
- The user must confirm that they are authorized to share the selected network.
  Halo does not infer permission from possession of an administrator account or
  Keychain item.

### Establishment and fallback

1. Rust requests redacted local eligibility from the platform adapter.
2. The user selects Wi-Fi onboarding and supplies an allowed credential source.
   On Windows/macOS this may invoke the documented native password lookup and
   OS authorization; failure is a normal capability outcome.
3. The receiver's OS presents any required network-selection or join consent.
4. The receiver joins ephemerally when the OS supports it. Halo does not delete
   or overwrite a pre-existing saved profile and records enough opaque state to
   request cleanup only for configuration it created.
5. Both peers perform the nonce-bound reachability probe on the selected path.
6. A new bound QUIC connection completes Halo authentication. Only then does
   the broker expose `Ready to transfer` and allow file offers.
7. Join timeout, user denial, AP isolation, VLAN separation, stale invitation,
   authentication failure, or path loss closes the candidate and removes only
   Halo-created ephemeral configuration.

Failure never sends file bytes over BLE, cellular, a VPN-only path, or the
public Internet. After a recoverable failure, the UI may explicitly continue
with an eligible Apple P2P, Wi-Fi Direct, or Wi-Fi Aware provider. A
user-created hotspot remains the last guided LAN fallback. Each additional
system prompt requires a new user action; fallback does not silently churn the
device through networks.

When Halo guides hotspot setup, the host or OS should generate a new temporary
personal-network credential where a documented API permits it. Otherwise the
user performs setup in system UI. The hotspot is subjected to the same
`peer_verified` test because hotspot client isolation is possible.

## Consequences

This design improves cross-platform transfer setup without treating network
membership as peer trust. Windows/macOS can act as credential-providing peers
when their documented APIs and the user authorize it; Android/iOS can receive
that invitation and request a join. If credential export is unavailable and the
user cannot provide QR/manual input, Halo skips infrastructure onboarding and
tries another eligible data-channel provider.

Joining infrastructure Wi-Fi can interrupt the receiver's current network and
may remove Internet access. The UI must disclose that effect, keep the app in
the foreground where required, and restore or release only state created by the
ceremony. OS behavior remains platform-specific and revocable.

## Required validation

- Android, iOS/iPadOS, Windows, and macOS: QR/manual input, user denial, timeout,
  wrong password, stale invitation, cancellation, and restoration behavior.
- Windows: standard-user/encrypted-only result, administrator-authorized
  plaintext result, profile ACL denial, UAC cancellation, and group-policy
  profiles. The least-privilege broker never writes export XML or elevates the
  main process. macOS: allowed, denied, cancelled, locked, and missing Keychain
  items through `CWKeychainFindWiFiPassword`.
- WPA2/WPA3 Personal and open networks where publicly supported; all excluded
  enterprise/managed/profile payloads fail closed.
- AP isolation, guest Wi-Fi, cross-VLAN, multicast-disabled, captive-portal,
  metered, VPN, and Wi-Fi-to-cellular transition cases.
- The invited peer must fail before metadata disclosure when it joins an evil
  twin with the same SSID or authenticates with the wrong Halo identity.
- No credential appears in Dart/platform-channel traces, crash reports, logs,
  diagnostics, persistence, clipboard history, or backups.
- Cleanup never removes a network that existed before the ceremony.
- Fallback prompts are serialized, bounded, and require explicit continuation.
- Authenticated BLE bootstrap passes the security, framing, lifecycle, and
  cross-platform gates in ADR 0009; plain GATT never receives a credential.

## References

- Android: [Privacy changes in Android 10](https://developer.android.com/about/versions/10/privacy/changes)
- Android: [Wi-Fi infrastructure overview](https://developer.android.com/develop/connectivity/wifi/wifi-infrastructure)
- Apple: [Wi-Fi configuration](https://developer.apple.com/documentation/networkextension/wi-fi-configuration)
- Apple: [`NEHotspotConfiguration`](https://developer.apple.com/documentation/networkextension/nehotspotconfiguration)
- Apple: [`CWKeychainFindWiFiPassword`](https://developer.apple.com/documentation/corewlan/cwkeychainfindwifipassword(_:_:_:))
- Windows: [About the Native Wi-Fi API](https://learn.microsoft.com/en-us/windows/win32/nativewifi/about-the-native-wifi-api)
- Windows: [`WlanGetProfile`](https://learn.microsoft.com/en-us/windows/win32/api/wlanapi/nf-wlanapi-wlangetprofile)
- Halo: [ADR 0009 authenticated BLE bootstrap](0009-authenticated-ble-bootstrap-channel.md)
