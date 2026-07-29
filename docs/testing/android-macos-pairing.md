# Android ↔ macOS authenticated pairing test

This checklist validates the integrated foreground pairing flow on physical
devices. A successful build or host loopback test is not cross-device evidence.

## Preconditions

- Copy `apps/halo_demo/macos/Runner/Configs/Signing.local.xcconfig.example`
  to `Signing.local.xcconfig`, set `HALO_DEVELOPMENT_TEAM` to the Apple Team
  selected in Xcode, and build the macOS app through the `Runner` scheme. The
  local file is ignored so a personal Team ID is never committed.
- Both devices run the same revision and are on a mutually reachable LAN.
- Halo is open in the foreground on both devices.
- Discovery shows a LAN endpoint for the other device.
- Record the Android model/version, macOS version, network topology, build
  revision, and permission state with the result.

## First contact

1. Tap **Connect securely** on one device.
2. Confirm that QUIC uses the discovered dynamic port; no UI or platform code
   assumes port 4433.
3. Confirm that both devices show the same six-digit code and short key
   fingerprint.
4. Reject once. Neither side may report the peer as trusted.
5. Retry, compare both displays, and accept on the receiver.
6. Stop and fully restart both applications. Connect again. Both sides must
   recognize the stored key without another receiver-consent prompt.

Android stores only an AES-GCM-wrapped opaque Rust identity blob in
`noBackupFilesDir`; the wrapping key is non-exportable from Android Keystore.
Apple platforms store the same opaque blob as a device-only, non-synchronizing
Data Protection Keychain item. Normal use must not request the user's login
keychain password.
Rust owns the identity format, signatures, trust records, and atomic trust-file
writes.

## Required negative cases

- Decline or let the 60-second confirmation window expire.
- Stop the app during connect and while the confirmation is visible.
- Disable Wi-Fi, switch access points, and restore connectivity.
- Run peers with incompatible protocol versions.
- Send truncated, oversized, unknown-kind, replayed, or signature-tampered
  control frames using the Rust negative-test harness.
- Replace one side's protected identity while keeping the other side's trust
  directory. Connecting from the remembered LAN address must show the blocking
  identity-change state and must not offer first-contact acceptance.
- Corrupt the protected blob or a Rust trust record. Startup/pairing must fail
  closed; the app must not silently generate a replacement identity.

For a development-only Android identity replacement, remove
`no_backup/halo-identity-v1.bin` with `adb shell run-as` while the app is stopped.
For a development-only macOS replacement, delete the app's Data Protection
Keychain item whose service is `org.halo.identity` and account is
`device-identity-v1` through the debug identity-reset operation. Do not use the
`security` command-line tool as a substitute: it targets file-based keychains.
These are destructive test operations; preserve the opposite device's trust
data and do not perform them on a profile whose identity must be retained.

## Pass criteria

- First-contact codes match, explicit receiver consent is required, and a
  rejection never writes trust.
- Restarted apps recognize the same full public keys automatically.
- A remembered address presenting a different valid identity is explicitly
  rejected as `IdentityChanged`.
- Timeouts, cancellation, disconnects, and Wi-Fi changes release the ceremony
  and leave discovery independently usable.
- No logs contain identity blobs, complete keys, transcript/exporter values,
  stable peer identifiers, or full filesystem paths.
