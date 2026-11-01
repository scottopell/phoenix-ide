#!/bin/bash
set -euo pipefail
umask 077

[[ $# -eq 2 ]] || { echo 'usage: sign-notarize-browser-cli.sh EXECUTABLE TARGET' >&2; exit 2; }
browser=$1
target=$2
case "$target" in
  aarch64-apple-darwin) expected_arch=arm64 ;;
  x86_64-apple-darwin) expected_arch=x86_64 ;;
  *) echo 'Unsupported browser CLI target' >&2; exit 1 ;;
esac
: "${MACOS_SIGNING_IDENTITY:?}"
: "${APPLE_TEAM_ID:?}"
: "${APP_STORE_CONNECT_API_KEY_PATH:?}"
: "${APP_STORE_CONNECT_KEY_ID:?}"
: "${APP_STORE_CONNECT_ISSUER_ID:?}"
: "${RUNNER_TEMP:?}"
[[ "$APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ && -f "$APP_STORE_CONNECT_API_KEY_PATH" && -x "$browser" ]]
CODESIGN=${CODESIGN:-/usr/bin/codesign}
LIPO=${LIPO:-/usr/bin/lipo}
DITTO=${DITTO:-/usr/bin/ditto}
XCRUN=${XCRUN:-/usr/bin/xcrun}
PYTHON3=${PYTHON3:-/usr/bin/python3}
[[ "$("$LIPO" -archs "$browser")" == "$expected_arch" ]] || {
  echo 'Browser CLI architecture mismatch' >&2; exit 1;
}
"$CODESIGN" --force --sign "$MACOS_SIGNING_IDENTITY" --options runtime --timestamp "$browser"
"$CODESIGN" --verify --strict --verbose=2 "$browser"
signature=$("$CODESIGN" --display --verbose=4 "$browser" 2>&1)
grep -E '^Authority=Developer ID Application:' <<<"$signature" >/dev/null
grep -Fx "TeamIdentifier=$APPLE_TEAM_ID" <<<"$signature" >/dev/null
grep -E '^CodeDirectory .*flags=.*runtime' <<<"$signature" >/dev/null
grep -E '^Timestamp=.+$' <<<"$signature" >/dev/null

notary_dir=$(mktemp -d "$RUNNER_TEMP/phoenix-browser-notary.XXXXXX")
trap 'rm -rf "$notary_dir"' EXIT
"$DITTO" -c -k --keepParent "$browser" "$notary_dir/browser.zip"
"$XCRUN" notarytool submit "$notary_dir/browser.zip" \
  --key "$APP_STORE_CONNECT_API_KEY_PATH" \
  --key-id "$APP_STORE_CONNECT_KEY_ID" \
  --issuer "$APP_STORE_CONNECT_ISSUER_ID" \
  --wait --output-format json > "$notary_dir/result.json"
"$PYTHON3" - "$notary_dir/result.json" <<'PY'
import json
import sys
from pathlib import Path

result = json.loads(Path(sys.argv[1]).read_text())
if result.get('status') != 'Accepted':
    raise SystemExit('Browser CLI notarization was not Accepted')
PY
"$browser" --help
"$browser" --json schema snapshot
