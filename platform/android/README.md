# Halo Android discovery adapter

Status: experimental and build-tested; real Android-to-macOS BLE testing is
still required before this adapter can be called beta or supported.

The Android project contains:

- `halo-discovery-android`: a foreground BLE scanner, connectable advertiser,
  GATT client, and GATT server using the Halo BLE Rendezvous v1 UUIDs.
- `demo`: an intentionally small native Android test application for validating
  Android-to-macOS rendezvous before Flutter integration.

The adapter exchanges only an opaque, fixed 58-byte value supplied by Rust. It
does not parse Presence, authenticate the peer, merge observations, transfer
files over BLE, or treat a Bluetooth address as identity. Every received value
must be forwarded to the Rust `halo-discovery` codec and aggregation pipeline.

## Toolchain

- Android Gradle Plugin 9.3.0
- Gradle 9.5.0
- compileSdk / targetSdk 37 (Android 17)
- minSdk 31 (Android 12)
- JDK 17 or newer for local builds

## Build

```bash
./gradlew :halo-discovery-android:assembleDebug
```

The product UI and installable application live in `apps/halo_demo`; this
directory intentionally contains no native Android UI. An emulator cannot
validate BLE advertising or physical proximity behavior.
