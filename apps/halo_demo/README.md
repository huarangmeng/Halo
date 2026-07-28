# Halo Flutter demo

This is Halo's single presentation layer for every supported product target.
Android and macOS currently launch this same Flutter application; iOS and
Windows will be added to this project rather than implemented as native product
UIs.

The current Android and macOS demo artifacts are arm64-only (`arm64-v8a` and
Apple Silicon respectively). Release Android APKs should be used for size
comparisons; debug artifacts include the Flutter debugger and validation tools.

Flutter owns rendering, accessibility, permission education, and user actions.
The `halo_ffi` Rust crate owns discovery sessions, Presence encoding and
decoding, LAN discovery, observation aggregation, expiry, and the snapshots
shown by the UI. Kotlin and Swift are narrow drivers for platform Bluetooth
APIs and carry only opaque Presence bytes across platform channels.

The UI ships English and Simplified Chinese ARB resources. `MaterialApp` does
not force a locale, so Flutter follows the operating-system language and falls
back to English for unsupported languages.

## Run

```bash
flutter pub get
flutter analyze
flutter test
flutter run -d macos
flutter run -d <ANDROID_DEVICE_ID>
```

See
[`../../docs/testing/android-macos-discovery.zh-CN.md`](../../docs/testing/android-macos-discovery.zh-CN.md)
for the physical Android-to-macOS discovery procedure and current limitations.

The demo does not yet implement authenticated QUIC sessions or file transfer.
