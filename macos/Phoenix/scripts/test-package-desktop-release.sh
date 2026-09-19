#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/out" "$tmp/runner" "$tmp/tmpdir"

cat >"$tmp/sidecar" <<'EOF'
#!/bin/sh
if [ "$1" = --build-identity ]; then
  printf '{"version":"1.2.3","git_sha":"%s"}\n' "${FAKE_EMBEDDED_SHA:-0123456789abcdef0123456789abcdef01234567}"
fi
EOF
chmod +x "$tmp/sidecar"

cat >"$tmp/bin/git" <<'EOF'
#!/bin/bash
set -euo pipefail
if [[ "$*" == *"rev-parse HEAD" ]]; then
  printf '%s\n' "${FAKE_CHECKOUT_COMMIT:-0123456789abcdef0123456789abcdef01234567}"
else
  exit 2
fi
EOF
cat >"$tmp/bin/lipo" <<'EOF'
#!/bin/sh
echo arm64
EOF
cat >"$tmp/bin/xcodebuild" <<'EOF'
#!/bin/bash
set -euo pipefail
derived=
marketing=
project_version=
log_file=${XCODEBUILD_LOG:?}
while (($#)); do
  case "$1" in
    -derivedDataPath) derived=$2; shift 2 ;;
    MARKETING_VERSION=*) marketing=${1#MARKETING_VERSION=} ; shift ;;
    CURRENT_PROJECT_VERSION=*) project_version=${1#CURRENT_PROJECT_VERSION=} ; shift ;;
    *) shift ;;
  esac
done
printf 'derived=%s\nmarketing=%s\nproject_version=%s\n' "$derived" "$marketing" "$project_version" > "$log_file"
app="$derived/Build/Products/Release/Phoenix.app"
mkdir -p "$app/Contents/Helpers" "$app/Contents"
cp "$PHOENIX_SIDECAR_PATH" "$app/Contents/Helpers/phoenix_ide"
chmod +x "$app/Contents/Helpers/phoenix_ide"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key>
  <string>${marketing}</string>
  <key>CFBundleVersion</key>
  <string>${project_version}</string>
</dict>
</plist>
PLIST
EOF
cat >"$tmp/bin/ditto" <<'EOF'
#!/bin/bash
set -euo pipefail
output=${@: -1}
printf 'zip-fixture' > "$output"
EOF
cat >"$tmp/bin/PlistBuddy" <<'EOF'
#!/usr/bin/env python3
import plistlib
import sys
cmd = sys.argv[2]
plist_path = sys.argv[3]
key = cmd.split(':', 1)[1]
with open(plist_path, 'rb') as fh:
    data = plistlib.load(fh)
print(data[key])
EOF
cat >"$tmp/bin/cmp" <<'EOF'
#!/bin/sh
exec /usr/bin/cmp "$@"
EOF
cat >"$tmp/bin/mktemp" <<'EOF'
#!/bin/sh
exec /usr/bin/mktemp "$@"
EOF
cat >"$tmp/bin/codesign" <<'EOF'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "${CODESIGN_LOG:?}"
if [[ "$*" == *"--display --verbose=4"* ]]; then
  cat >&2 <<SIG
Authority=Developer ID Application: Phoenix Test
TeamIdentifier=${FAKE_SIGNED_TEAM_ID:-${APPLE_TEAM_ID:?}}
CodeDirectory v=20500 size=100 flags=0x10000(runtime) hashes=1+1 location=embedded
Timestamp=Sep 19, 2026
SIG
  exit 0
fi
if [[ "${MUTATE_HELPER_ON_APP_SIGN:-}" == 1 && "$*" == *"Phoenix.app"* && "$*" != *"--verify"* ]]; then
  app=${@: -1}
  printf 'mutation' >> "$app/Contents/Helpers/phoenix_ide"
fi
EOF
cat >"$tmp/bin/xcrun" <<'EOF'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "${XCRUN_LOG:?}"
if [[ "${1:-}" == notarytool && "${2:-}" == submit ]]; then
  printf '{"id":"submission-fixture","status":"%s"}\n' "${FAKE_NOTARY_STATUS:-Accepted}"
  exit "${FAKE_NOTARY_EXIT:-0}"
fi
if [[ "${1:-}" == notarytool && "${2:-}" == log ]]; then
  printf '{"issues":["fixture rejection"]}\n' > "$4"
fi
EOF
cat >"$tmp/bin/spctl" <<'EOF'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "${SPCTL_LOG:?}"
EOF
chmod +x "$tmp/bin"/*

export XCODEBUILD="$tmp/bin/xcodebuild"
export XCODEBUILD_LOG="$tmp/xcodebuild.log"
export LIPO="$tmp/bin/lipo"
export DITTO="$tmp/bin/ditto"
export PLISTBUDDY="$tmp/bin/PlistBuddy"
export CMP="$tmp/bin/cmp"
export MKTEMP="$tmp/bin/mktemp"
export CODESIGN="$tmp/bin/codesign"
export XCRUN="$tmp/bin/xcrun"
export SPCTL="$tmp/bin/spctl"
export GIT="$tmp/bin/git"
export CODESIGN_LOG="$tmp/codesign.log"
export XCRUN_LOG="$tmp/xcrun.log"
export SPCTL_LOG="$tmp/spctl.log"

run_unsigned() {
  "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
    --unsigned-test "$tmp/sidecar" aarch64-apple-darwin "$1" \
    "$2" "$tmp/out"
}

export TMPDIR="$tmp/tmpdir"
unset RUNNER_TEMP
asset=$(run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567)
[[ "$asset" == "$tmp/out/Phoenix-macos-aarch64-apple-darwin-v1.2.3.zip" ]]
[[ -s "$asset" ]]
grep -Fx "marketing=1.2.3" "$tmp/xcodebuild.log"
grep -Fx "project_version=2.2.3" "$tmp/xcodebuild.log"
case $(grep '^derived=' "$tmp/xcodebuild.log") in
  "derived=$tmp/tmpdir/phoenix-desktop-aarch64-apple-darwin."*) ;;
  *) echo "expected derived data to be created under TMPDIR fallback" >&2; exit 1 ;;
esac

unset TMPDIR RUNNER_TEMP
: > "$tmp/xcodebuild.log"
asset=$(run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567)
derived_line=$(grep '^derived=' "$tmp/xcodebuild.log")
derived_path=${derived_line#derived=}
parent_dir=$(dirname "$derived_path")
case "$parent_dir" in
  /private/tmp|/tmp|/var/folders/*/T) ;;
  *) echo "expected bare mktemp fallback under system temp directory" >&2; exit 1 ;;
esac
[[ -s "$asset" ]]

if run_unsigned v1.2.3-rc1 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1; then
  echo "expected prerelease tag to fail" >&2
  exit 1
fi

if run_unsigned v1.1000.0 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1; then
  echo "expected colliding bundle-version components to fail" >&2
  exit 1
fi

if run_unsigned v1.2.3 0123456789abcdef >/dev/null 2>&1; then
  echo "expected short SHA to fail" >&2
  exit 1
fi

if run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567-dirty >/dev/null 2>&1; then
  echo "expected dirty SHA to fail" >&2
  exit 1
fi

export FAKE_EMBEDDED_SHA=0123456789ab
if run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1; then
  echo "expected 12-character embedded SHA to fail" >&2
  exit 1
fi
unset FAKE_EMBEDDED_SHA

export FAKE_EMBEDDED_SHA=0123456789abcdef0123456789abcdef01234567-dirty
if run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1; then
  echo "expected dirty embedded SHA to fail" >&2
  exit 1
fi
unset FAKE_EMBEDDED_SHA

export FAKE_EMBEDDED_SHA=unknown
if run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1; then
  echo "expected unknown embedded SHA to fail" >&2
  exit 1
fi
unset FAKE_EMBEDDED_SHA

export FAKE_EMBEDDED_SHA=0123456789abffffffffffffffffffffffffffff
if run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1; then
  echo "expected same-prefix sidecar commit with a mismatched tail to fail" >&2
  exit 1
fi
unset FAKE_EMBEDDED_SHA

export FAKE_CHECKOUT_COMMIT=ffffffffffffffffffffffffffffffffffffffff
if run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1; then
  echo "expected checkout commit mismatch to fail" >&2
  exit 1
fi
unset FAKE_CHECKOUT_COMMIT

export TMPDIR="$tmp/tmpdir"
: > "$tmp/codesign.log"
: > "$tmp/xcrun.log"
: > "$tmp/spctl.log"
export MACOS_SIGNING_IDENTITY='Developer ID Application: Phoenix Test'
export APPLE_TEAM_ID='TEAM123456'
printf '%s\n' 'private-key-fixture' > "$tmp/notary-key.p8"
export APP_STORE_CONNECT_API_KEY_PATH="$tmp/notary-key.p8"
export APP_STORE_CONNECT_KEY_ID='KEYID12345'
export APP_STORE_CONNECT_ISSUER_ID='00000000-0000-0000-0000-000000000000'
export MUTATE_HELPER_ON_APP_SIGN=1
if "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out" >/dev/null 2>&1; then
  echo "expected app-sign helper mutation to fail byte-identity check" >&2
  exit 1
fi
unset MUTATE_HELPER_ON_APP_SIGN

export FAKE_SIGNED_TEAM_ID=OTHER12345
if "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out" >/dev/null 2>&1; then
  echo "expected TeamIdentifier mismatch to fail" >&2
  exit 1
fi
unset FAKE_SIGNED_TEAM_ID

export FAKE_NOTARY_STATUS=Invalid
if "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out" >/dev/null 2>&1; then
  echo "expected non-Accepted notarization status to fail" >&2
  exit 1
fi
grep -F 'notarytool log submission-fixture' "$tmp/xcrun.log" >/dev/null
unset FAKE_NOTARY_STATUS
: > "$tmp/xcrun.log"

export FAKE_NOTARY_EXIT=1
if "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out" >/dev/null 2>&1; then
  echo "expected failed notarytool submission to fail" >&2
  exit 1
fi
grep -F 'notarytool log submission-fixture' "$tmp/xcrun.log" >/dev/null
unset FAKE_NOTARY_EXIT
: > "$tmp/xcrun.log"

asset=$("$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out")
[[ -s "$asset" ]]
grep -F -- '--verify --strict --verbose=2' "$tmp/codesign.log" >/dev/null
grep -F -- '--force --sign Developer ID Application: Phoenix Test' "$tmp/codesign.log" >/dev/null
grep -F -- '--verify --deep --strict --verbose=2' "$tmp/codesign.log" >/dev/null
grep -F 'notarytool submit' "$tmp/xcrun.log" >/dev/null
grep -F -- '--key ' "$tmp/xcrun.log" >/dev/null
grep -F -- '--key-id KEYID12345' "$tmp/xcrun.log" >/dev/null
grep -F -- '--issuer 00000000-0000-0000-0000-000000000000' "$tmp/xcrun.log" >/dev/null
grep -F -- '--output-format json' "$tmp/xcrun.log" >/dev/null
grep -F 'TeamIdentifier=TEAM123456' < <("$tmp/bin/codesign" --display --verbose=4 "$asset" 2>&1) >/dev/null
if grep -F -- '--apple-id' "$tmp/xcrun.log" >/dev/null; then
  echo "notarization must not use a human Apple ID" >&2
  exit 1
fi
grep -F 'stapler staple' "$tmp/xcrun.log" >/dev/null
grep -F 'stapler validate' "$tmp/xcrun.log" >/dev/null
grep -F -- '--assess --type execute --verbose=2' "$tmp/spctl.log" >/dev/null

echo "desktop release packaging regression checks passed"
