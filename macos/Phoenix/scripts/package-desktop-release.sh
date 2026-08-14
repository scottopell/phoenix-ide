#!/bin/bash
set -euo pipefail

XCODEBUILD=${XCODEBUILD:-/usr/bin/xcodebuild}
LIPO=${LIPO:-/usr/bin/lipo}
CODESIGN=${CODESIGN:-/usr/bin/codesign}
DITTO=${DITTO:-/usr/bin/ditto}
XCRUN=${XCRUN:-/usr/bin/xcrun}
SPCTL=${SPCTL:-/usr/sbin/spctl}
PLISTBUDDY=${PLISTBUDDY:-/usr/libexec/PlistBuddy}
CMP=${CMP:-/usr/bin/cmp}
PYTHON3=${PYTHON3:-/usr/bin/python3}
MKTEMP=${MKTEMP:-/usr/bin/mktemp}

usage() {
  cat >&2 <<'EOF'
usage: package-desktop-release.sh [--unsigned-test] SIDE_CAR TARGET TAG COMMIT OUTPUT_DIR

Builds Phoenix.app with the supplied same-commit phoenix_ide sidecar and writes
Phoenix-macos-TARGET-TAG.zip. Normal mode requires Developer ID signing and
Apple notarization credentials in the environment.
EOF
  exit 2
}

release_build_number() {
  local version=${1#v}
  "$PYTHON3" - "$version" <<'PY'
import re
import sys
version = sys.argv[1]
match = re.fullmatch(r'(\d+)\.(\d+)\.(\d+)', version)
if not match:
    raise SystemExit(f"invalid semantic version: {version}")
major, minor, patch = (int(match.group(i)) for i in range(1, 4))
if major >= 10_000 or minor >= 100 or patch >= 100:
    raise SystemExit("semantic version components exceed CFBundleVersion limits")
print(f"{major}.{minor}.{patch}")
PY
}

info_plist_string() {
  local plist=$1
  local key=$2
  "$PLISTBUDDY" -c "Print :$key" "$plist"
}

unsigned_test=0
if [[ "${1:-}" == "--unsigned-test" ]]; then
  unsigned_test=1
  shift
fi
[[ $# -eq 5 ]] || usage

sidecar=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
target=$2
tag=$3
expected_commit=$4
output_dir=$(mkdir -p "$5" && cd "$5" && pwd)
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
project="$repo_root/macos/Phoenix/Phoenix.xcodeproj"
release_version=${tag#v}
release_build_number=$(release_build_number "$tag")
expected_arch=
case "$target" in
  aarch64-apple-darwin) expected_arch=arm64 ;;
  x86_64-apple-darwin) expected_arch=x86_64 ;;
  *) echo "error: unsupported desktop target: $target" >&2; exit 1 ;;
esac
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "error: release tag must be v-prefixed semantic version: $tag" >&2
  exit 1
}
[[ "$expected_commit" =~ ^[0-9a-f]{40}$ ]] || {
  echo "error: expected commit must be a full lowercase git SHA" >&2
  exit 1
}
[[ -x "$sidecar" ]] || { echo "error: sidecar is not executable: $sidecar" >&2; exit 1; }
sidecar_archs=$($LIPO -archs "$sidecar")
[[ " $sidecar_archs " == *" $expected_arch "* ]] || {
  echo "error: sidecar lacks $expected_arch architecture (has: $sidecar_archs)" >&2
  exit 1
}

if (( unsigned_test == 0 )); then
  : "${MACOS_SIGNING_IDENTITY:?MACOS_SIGNING_IDENTITY is required}"
  : "${APPLE_ID:?APPLE_ID is required}"
  : "${APPLE_TEAM_ID:?APPLE_TEAM_ID is required}"
  : "${APPLE_APP_SPECIFIC_PASSWORD:?APPLE_APP_SPECIFIC_PASSWORD is required}"
fi

tmp_root=${RUNNER_TEMP:-${TMPDIR:-}}
if [[ -n "$tmp_root" ]]; then
  mkdir -p "$tmp_root"
  derived=$($MKTEMP -d "$tmp_root/phoenix-desktop-$target.XXXXXX")
else
  derived=$($MKTEMP -d)
fi
trap 'rm -rf "$derived"' EXIT
PHOENIX_SIDECAR_PATH="$sidecar" \
  "$XCODEBUILD" \
    -project "$project" \
    -scheme Phoenix \
    -configuration Release \
    -derivedDataPath "$derived" \
    ARCHS="$expected_arch" \
    ONLY_ACTIVE_ARCH=YES \
    MARKETING_VERSION="$release_version" \
    CURRENT_PROJECT_VERSION="$release_build_number" \
    CODE_SIGNING_ALLOWED=NO \
    build

app="$derived/Build/Products/Release/Phoenix.app"
helper="$app/Contents/Helpers/phoenix_ide"
info_plist="$app/Contents/Info.plist"
[[ -d "$app" && -x "$helper" && -f "$info_plist" ]] || { echo "error: built app or helper missing" >&2; exit 1; }
[[ " $($LIPO -archs "$helper") " == *" $expected_arch "* ]] || {
  echo "error: packaged helper architecture mismatch" >&2
  exit 1
}
identity_json=$($helper --build-identity)
read -r embedded_version embedded_commit < <("$PYTHON3" -c 'import json,sys; d=json.load(sys.stdin); print(d["version"], d["git_sha"])' <<<"$identity_json")
[[ "v$embedded_version" == "$tag" ]] || {
  echo "error: sidecar version v$embedded_version does not match release $tag" >&2
  exit 1
}
[[ "$embedded_commit" == "$expected_commit" ]] || {
  echo "error: sidecar commit $embedded_commit does not match release commit $expected_commit" >&2
  exit 1
}
[[ "$(info_plist_string "$info_plist" CFBundleShortVersionString)" == "$release_version" ]] || {
  echo "error: built app marketing version does not match release $release_version" >&2
  exit 1
}
[[ "$(info_plist_string "$info_plist" CFBundleVersion)" == "$release_build_number" ]] || {
  echo "error: built app project version does not match release build $release_build_number" >&2
  exit 1
}

if (( unsigned_test == 0 )); then
  "$CODESIGN" --verify --strict --verbose=2 "$sidecar"
fi
"$CMP" -s "$sidecar" "$helper" || {
  echo "error: packaged helper bytes differ from supplied standalone helper" >&2
  exit 1
}

if (( unsigned_test == 0 )); then
  "$CODESIGN" --force --sign "$MACOS_SIGNING_IDENTITY" \
    --entitlements "$repo_root/macos/Phoenix/Phoenix/Phoenix.entitlements" \
    --options runtime --timestamp "$app"
  "$CODESIGN" --verify --deep --strict --verbose=2 "$app"

  pre_notary="$derived/Phoenix-notary.zip"
  "$DITTO" -c -k --keepParent "$app" "$pre_notary"
  "$XCRUN" notarytool submit "$pre_notary" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait
  "$XCRUN" stapler staple "$app"
  "$XCRUN" stapler validate "$app"
  "$SPCTL" --assess --type execute --verbose=2 "$app"
  "$CMP" -s "$sidecar" "$helper" || {
    echo "error: app signing changed the embedded standalone helper bytes" >&2
    exit 1
  }
fi

asset="Phoenix-macos-$target-$tag.zip"
rm -f "$output_dir/$asset"
"$DITTO" -c -k --keepParent "$app" "$output_dir/$asset"
[[ -s "$output_dir/$asset" ]] || { echo "error: desktop archive is empty" >&2; exit 1; }
echo "$output_dir/$asset"
