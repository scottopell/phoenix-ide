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
chmod +x "$tmp/bin"/*

export TMPDIR="$tmp/tmpdir"
unset RUNNER_TEMP
export XCODEBUILD="$tmp/bin/xcodebuild"
export XCODEBUILD_LOG="$tmp/xcodebuild.log"
export LIPO="$tmp/bin/lipo"
export DITTO="$tmp/bin/ditto"
export PLISTBUDDY="$tmp/bin/PlistBuddy"
export CMP="$tmp/bin/cmp"
export MKTEMP="$tmp/bin/mktemp"

asset=$("$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  --unsigned-test "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out")
[[ "$asset" == "$tmp/out/Phoenix-macos-aarch64-apple-darwin-v1.2.3.zip" ]]
[[ -s "$asset" ]]
grep -Fx "marketing=1.2.3" "$tmp/xcodebuild.log"
grep -Fx "project_version=1002003" "$tmp/xcodebuild.log"
case $(grep '^derived=' "$tmp/xcodebuild.log") in
  "derived=$tmp/tmpdir/phoenix-desktop-aarch64-apple-darwin."*) ;;
  *) echo "expected derived data to be created under TMPDIR fallback" >&2; exit 1 ;;
esac

if "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  --unsigned-test "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  ffffffffffffffffffffffffffffffffffffffff "$tmp/out" >/dev/null 2>&1; then
  echo "expected commit mismatch to fail" >&2
  exit 1
fi

echo "desktop release packaging regression checks passed"
