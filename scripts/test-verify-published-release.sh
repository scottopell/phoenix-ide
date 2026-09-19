#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/source"

printf 'one' >"$tmp/source/asset-one"
printf 'two' >"$tmp/source/asset-two"
(
  cd "$tmp/source"
  shasum -a 256 asset-one asset-two | sed 's/  /  /' >SHA256SUMS
)
python3 - "$tmp/source" >"$tmp/release.json" <<'PY'
import hashlib
import json
import sys
from pathlib import Path
root = Path(sys.argv[1])
paths = [root / "asset-one", root / "asset-two", root / "SHA256SUMS"]
print(json.dumps({
    "id": 42,
    "tag_name": "v1.2.3",
    "draft": False,
    "assets": [
        {
            "id": index,
            "name": path.name,
            "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
            "source": str(path),
        }
        for index, path in enumerate(paths, start=100)
    ],
}))
PY

cat >"$tmp/bin/gh" <<'PY'
#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

args = sys.argv[1:]
if args[:1] != ["api"] or os.environ.get("FAKE_API_FAILURE") == "1":
    raise SystemExit(2)
release = json.loads(Path(os.environ["FAKE_RELEASE_JSON"]).read_text())
if any(arg.endswith("/releases?per_page=100") for arg in args):
    page = [] if os.environ.get("FAKE_RELEASE_ABSENT") == "1" else [release]
    print(json.dumps([page] if "--slurp" in args else page))
    raise SystemExit(0)
endpoint = next((arg for arg in args if "/releases/assets/" in arg), "")
if endpoint:
    asset_id = int(endpoint.rsplit("/", 1)[1])
    asset = next(asset for asset in release["assets"] if asset["id"] == asset_id)
    sys.stdout.buffer.write(Path(asset["source"]).read_bytes())
    raise SystemExit(0)
raise SystemExit(2)
PY
chmod +x "$tmp/bin/gh"
export PATH="$tmp/bin:$PATH"
export FAKE_RELEASE_JSON="$tmp/release.json"
export FAKE_ASSET_DIR="$tmp/source"

bash "$root/scripts/verify-published-release.sh" owner/repo v1.2.3 asset-one asset-two >/dev/null

export FAKE_RELEASE_ABSENT=1
set +e
bash "$root/scripts/verify-published-release.sh" owner/repo v1.2.3 asset-one asset-two >/dev/null 2>&1
status=$?
set -e
test "$status" -eq 3 || { echo "expected absent public release status 3, got $status" >&2; exit 1; }
unset FAKE_RELEASE_ABSENT

export FAKE_API_FAILURE=1
set +e
bash "$root/scripts/verify-published-release.sh" owner/repo v1.2.3 asset-one asset-two >/dev/null 2>&1
status=$?
set -e
test "$status" -ne 0
test "$status" -ne 3 || { echo "API failure must not be classified as release absence" >&2; exit 1; }
unset FAKE_API_FAILURE

python3 - "$tmp/release.json" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
release = json.loads(path.read_text())
release["assets"][0]["digest"] = "sha256:" + "f" * 64
path.write_text(json.dumps(release))
PY
if bash "$root/scripts/verify-published-release.sh" owner/repo v1.2.3 asset-one asset-two >/dev/null 2>&1; then
  echo "expected GitHub digest mismatch to fail" >&2
  exit 1
fi

echo "published release verification checks passed"
