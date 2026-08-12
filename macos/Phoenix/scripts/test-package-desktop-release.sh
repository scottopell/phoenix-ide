#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/out" "$tmp/runner"

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
while (($#)); do
  if [[ "$1" == -derivedDataPath ]]; then derived=$2; shift 2; else shift; fi
done
app="$derived/Build/Products/Release/Phoenix.app"
mkdir -p "$app/Contents/Helpers"
cp "$PHOENIX_SIDECAR_PATH" "$app/Contents/Helpers/phoenix_ide"
chmod +x "$app/Contents/Helpers/phoenix_ide"
EOF
cat >"$tmp/bin/ditto" <<'EOF'
#!/bin/bash
set -euo pipefail
output=${@: -1}
printf 'zip-fixture' > "$output"
EOF
chmod +x "$tmp/bin"/*

export RUNNER_TEMP="$tmp/runner"
export XCODEBUILD="$tmp/bin/xcodebuild"
export LIPO="$tmp/bin/lipo"
export DITTO="$tmp/bin/ditto"

asset=$("$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  --unsigned-test "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  0123456789abcdef0123456789abcdef01234567 "$tmp/out")
[[ "$asset" == "$tmp/out/Phoenix-macos-aarch64-apple-darwin-v1.2.3.zip" ]]
[[ -s "$asset" ]]

if "$root/macos/Phoenix/scripts/package-desktop-release.sh" \
  --unsigned-test "$tmp/sidecar" aarch64-apple-darwin v1.2.3 \
  ffffffffffffffffffffffffffffffffffffffff "$tmp/out" >/dev/null 2>&1; then
  echo "expected commit mismatch to fail" >&2
  exit 1
fi

echo "desktop release packaging regression checks passed"
