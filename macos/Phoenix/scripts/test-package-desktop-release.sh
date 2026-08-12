#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/out" "$tmp/runner" "$tmp/tmpdir"

cat >"$tmp/sidecar" <<'EOF'
#!/bin/sh
if [ "$1" = --build-identity ]; then
  echo '{"version":"1.2.3","git_sha":"0123456789abcdef0123456789abcdef01234567"}'
fi
EOF
chmod +x "$tmp/sidecar"

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
if [[ "${MUTATE_HELPER_ON_APP_SIGN:-}" == 1 && "$*" == *"Phoenix.app"* && "$*" != *"--verify"* ]]; then
  app=${@: -1}
  printf 'mutation' >> "$app/Contents/Helpers/phoenix_ide"
fi
EOF
cat >"$tmp/bin/xcrun" <<'EOF'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "${XCRUN_LOG:?}"
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
grep -Fx "project_version=1002003" "$tmp/xcodebuild.log"
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

if run_unsigned v1.2.3 0123456789abcdef >/dev/null 2>&1; then
  echo "expected short SHA to fail" >&2
  exit 1
fi

if run_unsigned v1.2.3 0123456789abcdef0123456789abcdef01234567-dirty >/dev/null 2>&1; then
  echo "expected dirty SHA to fail" >&2
  exit 1
fi

if run_unsigned v1.2.3 ffffffffffffffffffffffffffffffffffffffff >/dev/null 2>&1; then
  echo "expected commit mismatch to fail" >&2
  exit 1
fi

export TMPDIR="$tmp/tmpdir"
: > "$tmp/codesign.log"
: > "$tmp/xcrun.log"
: > "$tmp/spctl.log"
export MACOS_SIGNING_IDENTITY='Developer ID Application: Phoenix Test'
export APPLE_ID='phoenix@example.com'
export APPLE_TEAM_ID='TEAM1234567'
export APPLE_APP_SPECIFIC_PASSWORD='app-password'
export MUTATE_HELPER_ON_APP_SIGN=1
if "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out" >/dev/null 2>&1; then
  echo "expected app-sign helper mutation to fail byte-identity check" >&2
  exit 1
fi
unset MUTATE_HELPER_ON_APP_SIGN

asset=$("$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out")
[[ -s "$asset" ]]
grep -F -- '--verify --strict --verbose=2' "$tmp/codesign.log" >/dev/null
grep -F -- '--force --sign Developer ID Application: Phoenix Test' "$tmp/codesign.log" >/dev/null
grep -F -- '--verify --deep --strict --verbose=2' "$tmp/codesign.log" >/dev/null
grep -F 'notarytool submit' "$tmp/xcrun.log" >/dev/null
grep -F 'stapler staple' "$tmp/xcrun.log" >/dev/null
grep -F 'stapler validate' "$tmp/xcrun.log" >/dev/null
grep -F -- '--assess --type execute --verbose=2' "$tmp/spctl.log" >/dev/null

echo "desktop release packaging regression checks passed"
