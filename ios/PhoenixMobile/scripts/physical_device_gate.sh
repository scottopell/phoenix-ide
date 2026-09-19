#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -z "${PHOENIX_IOS_DEVELOPMENT_TEAM:-}" ]]; then
  cat >&2 <<'EOF'
PHOENIX_IOS_DEVELOPMENT_TEAM is required for a signed physical-device build.
Set it to the Apple Developer Team ID that should sign the iOS app.
EOF
  exit 2
fi

if [[ -z "${PHOENIX_IOS_DEVICE_DESTINATION:-}" ]]; then
  cat >&2 <<'EOF'
PHOENIX_IOS_DEVICE_DESTINATION is required.
Example: PHOENIX_IOS_DEVICE_DESTINATION='platform=iOS,id=<device-udid>'
List connected devices with: xcrun devicectl list devices
EOF
  exit 2
fi

xcodebuild build \
  -project PhoenixMobile.xcodeproj \
  -scheme PhoenixMobile \
  -destination "${PHOENIX_IOS_DEVICE_DESTINATION}" \
  -allowProvisioningUpdates \
  CODE_SIGN_STYLE=Automatic \
  DEVELOPMENT_TEAM="${PHOENIX_IOS_DEVELOPMENT_TEAM}"
