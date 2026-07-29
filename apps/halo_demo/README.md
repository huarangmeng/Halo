# Halo Flutter demo

This is Halo's single presentation layer for every supported product target.
Android, iOS, and macOS launch this same Flutter application. Windows will be
added to this project rather than implemented as a native product UI.

The current Android, iOS, and macOS demo artifacts are arm64-only (`arm64-v8a`,
arm64 iPhoneOS, and Apple Silicon respectively). Release Android APKs should be
used for size comparisons; debug artifacts include the Flutter debugger and
validation tools.

Flutter owns rendering, accessibility, permission education, and user actions.
The `halo_ffi` Rust crate owns discovery sessions, Presence encoding and
decoding, LAN discovery, observation aggregation, expiry, and the snapshots
shown by the UI. Kotlin and Swift are narrow drivers for platform Bluetooth
APIs and carry only opaque Presence bytes across platform channels.

The diagnostics sheet reads provider health from Rust and reports BLE, mDNS,
IPv4 Presence, and IPv6 Presence independently. A degraded provider does not
silently disable the others.

The UI ships English and Simplified Chinese ARB resources. `MaterialApp` does
not force a locale, so Flutter follows the operating-system language and falls
back to English for unsupported languages.

## Run

```bash
flutter pub get
flutter analyze
flutter test
flutter run -d <ANDROID_DEVICE_ID>
flutter run -d <IOS_DEVICE_ID>
```

Run the signed macOS physical-device peer from the repository root:

```bash
./tools/run-macos-device-validation.sh
```

The script uses the Xcode `Runner` scheme and validates the Data Protection
Keychain entitlements. Do not use `flutter run -d macos` for device validation;
it produces an ad-hoc-signed app that cannot use this identity store.

See
[`../../docs/testing/android-macos-discovery.zh-CN.md`](../../docs/testing/android-macos-discovery.zh-CN.md)
for the physical Android-to-macOS discovery procedure and current limitations.
See
[`../../docs/testing/ios-discovery.zh-CN.md`](../../docs/testing/ios-discovery.zh-CN.md)
for the iOS arm64 build and physical-device procedure. The iOS launcher compiles
for iPhoneOS, but iOS interoperability has not yet been established on a real
device.

The demo does not yet implement authenticated QUIC sessions or file transfer.
