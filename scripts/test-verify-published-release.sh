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
print(json.dumps({
    "draft": False,
    "assets": [
        {
            "name": path.name,
            "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
        }
        for path in [root / "asset-one", root / "asset-two", root / "SHA256SUMS"]
    ],
}))
PY

cat >"$tmp/bin/gh" <<'PY'
#!/usr/bin/env python3
import os
import shutil
import sys
from pathlib import Path
args = sys.argv[1:]
if args[:1] == ["api"]:
    print(Path(os.environ["FAKE_RELEASE_JSON"]).read_text())
    raise SystemExit(0)
if args[:2] == ["release", "download"]:
    destination = Path(args[args.index("--dir") + 1])
    destination.mkdir(parents=True, exist_ok=True)
    for path in Path(os.environ["FAKE_ASSET_DIR"]).iterdir():
        shutil.copy2(path, destination / path.name)
    raise SystemExit(0)
raise SystemExit(2)
PY
chmod +x "$tmp/bin/gh"
export PATH="$tmp/bin:$PATH"
export FAKE_RELEASE_JSON="$tmp/release.json"
export FAKE_ASSET_DIR="$tmp/source"

bash "$root/scripts/verify-published-release.sh" owner/repo v1.2.3 asset-one asset-two >/dev/null

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
