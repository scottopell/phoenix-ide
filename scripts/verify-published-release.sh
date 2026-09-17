#!/bin/bash
set -euo pipefail

[[ $# -ge 3 ]] || {
  echo "usage: verify-published-release.sh REPO TAG ASSET_NAME..." >&2
  exit 2
}
repo=$1
tag=$2
shift 2
required=("$@")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
metadata="$work/release.json"
gh api "repos/$repo/releases/tags/$tag" >"$metadata"
[[ $(jq -r '.draft // false' "$metadata") == false ]] || {
  echo "error: $tag is not public" >&2
  exit 1
}

printf '%s\n' "${required[@]}" SHA256SUMS | sort >"$work/expected-names"
jq -r '.assets[].name' "$metadata" | sort >"$work/actual-names"
cmp -s "$work/expected-names" "$work/actual-names" || {
  echo "error: public release $tag has an unexpected asset set" >&2
  exit 1
}

gh release download "$tag" --repo "$repo" --dir "$work/assets"
(
  cd "$work/assets"
  sha256sum --check SHA256SUMS
  grep -E '^[0-9a-f]{64} [ *].+$' SHA256SUMS | sed 's/^[0-9a-f]\{64\} [ *]//' | sort >"$work/manifest-names"
)
printf '%s\n' "${required[@]}" | sort >"$work/expected-manifest-names"
cmp -s "$work/expected-manifest-names" "$work/manifest-names" || {
  echo "error: public release $tag has an unexpected checksum manifest" >&2
  exit 1
}

python3 - "$metadata" "$work/assets" <<'PY'
import hashlib
import json
import sys
from pathlib import Path
metadata = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assets = Path(sys.argv[2])
for asset in metadata["assets"]:
    path = assets / asset["name"]
    digest = "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    if asset.get("digest") != digest:
        raise SystemExit(f"error: GitHub digest mismatch for {asset['name']}")
PY

echo "public release $tag is exact"
