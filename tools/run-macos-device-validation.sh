#!/bin/bash

set -euo pipefail

HALO_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HALO_REPOSITORY_ROOT="$(cd "$HALO_SCRIPT_DIR/.." && pwd)"
HALO_DEMO_DIR="$HALO_REPOSITORY_ROOT/apps/halo_demo"
HALO_SIGNING_CONFIG="$HALO_DEMO_DIR/macos/Runner/Configs/Signing.local.xcconfig"
HALO_WORKSPACE="$HALO_DEMO_DIR/macos/Runner.xcworkspace"
HALO_BUILD_ONLY=false

if [[ "${1:-}" == "--build-only" ]]; then
  HALO_BUILD_ONLY=true
elif [[ -n "${1:-}" ]]; then
  echo "Usage: $0 [--build-only]" >&2
  exit 2
fi

if [[ ! -f "$HALO_SIGNING_CONFIG" ]]; then
  echo "Missing $HALO_SIGNING_CONFIG" >&2
  echo "Copy Signing.local.xcconfig.example and set your Apple Team ID." >&2
  exit 1
fi

if /usr/bin/grep -q "YOUR_TEAM_ID" "$HALO_SIGNING_CONFIG"; then
  echo "Replace YOUR_TEAM_ID in $HALO_SIGNING_CONFIG before building." >&2
  exit 1
fi

cd "$HALO_DEMO_DIR"

/usr/bin/xcodebuild \
  -workspace "$HALO_WORKSPACE" \
  -scheme Runner \
  -configuration Debug \
  -destination "platform=macOS,arch=arm64" \
  -allowProvisioningUpdates \
  -quiet \
  build

HALO_BUILD_DIRECTORY="$({
  /usr/bin/xcodebuild \
    -workspace "$HALO_WORKSPACE" \
    -scheme Runner \
    -configuration Debug \
    -destination "platform=macOS,arch=arm64" \
    -quiet \
    -showBuildSettings \
    -json
} | /usr/bin/plutil -extract 0.buildSettings.TARGET_BUILD_DIR raw -o - -)"
HALO_APP_PATH="$HALO_BUILD_DIRECTORY/halo_demo.app"

if [[ ! -d "$HALO_APP_PATH" ]]; then
  echo "Signed app was not produced at $HALO_APP_PATH" >&2
  exit 1
fi

HALO_ENTITLEMENTS="$(/usr/bin/codesign -d --entitlements - "$HALO_APP_PATH" 2>&1)"
if [[ "$HALO_ENTITLEMENTS" != *"com.apple.application-identifier"* ]] ||
  [[ "$HALO_ENTITLEMENTS" != *"keychain-access-groups"* ]]; then
  echo "The macOS app is missing Data Protection Keychain signing entitlements." >&2
  exit 1
fi

HALO_SIGNATURE="$(/usr/bin/codesign -dv --verbose=4 "$HALO_APP_PATH" 2>&1)"
if [[ "$HALO_SIGNATURE" == *"TeamIdentifier=not set"* ]]; then
  echo "The macOS app was ad-hoc signed; use an Apple Development identity." >&2
  exit 1
fi

echo "Validated signed macOS app: $HALO_APP_PATH"

if [[ "$HALO_BUILD_ONLY" == false ]]; then
  /usr/bin/pkill -x halo_demo >/dev/null 2>&1 || true
  /usr/bin/open "$HALO_APP_PATH"
fi
