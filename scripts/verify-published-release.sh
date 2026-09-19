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
releases="$work/releases.json"
gh api --paginate --slurp "repos/$repo/releases?per_page=100" >"$releases"
if jq -ce --arg tag "$tag" \
  '[.[][] | select(.draft == false and .tag_name == $tag)] | if length == 1 then .[0] elif length == 0 then empty else error("multiple public releases for tag") end' \
  "$releases" >"$metadata"
then
  :
else
  status=$?
  if [[ $status -eq 4 ]]; then
    echo "public release $tag is absent" >&2
    exit 3
  fi
  exit "$status"
fi

printf '%s\n' "${required[@]}" SHA256SUMS | sort >"$work/expected-names"
jq -r '.assets[].name' "$metadata" | sort >"$work/actual-names"
cmp -s "$work/expected-names" "$work/actual-names" || {
  echo "error: public release $tag has an unexpected asset set" >&2
  exit 1
}

mkdir -p "$work/assets"
while IFS=$'\t' read -r asset_id asset_name; do
  [[ "$asset_id" =~ ^[0-9]+$ ]] || { echo "error: invalid GitHub asset id for $asset_name" >&2; exit 1; }
  [[ "$asset_name" != */* && "$asset_name" != . && "$asset_name" != .. ]] || {
    echo "error: unsafe GitHub asset name: $asset_name" >&2
    exit 1
  }
  gh api "repos/$repo/releases/assets/$asset_id" \
    -H 'Accept: application/octet-stream' >"$work/assets/$asset_name"
done < <(jq -r '.assets[] | [.id, .name] | @tsv' "$metadata")
python3 - "$work/assets" "$work/manifest-names" <<'PY'
import hashlib
import re
import sys
from pathlib import Path

assets = Path(sys.argv[1])
manifest = {}
for line in (assets / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
    match = re.fullmatch(r"([0-9a-f]{64}) [ *](.+)", line)
    if not match:
        raise SystemExit(f"error: malformed SHA256SUMS line: {line!r}")
    digest, name = match.groups()
    if name in manifest:
        raise SystemExit(f"error: duplicate SHA256SUMS member: {name}")
    manifest[name] = digest
for name, expected in manifest.items():
    path = assets / name
    if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        raise SystemExit(f"error: SHA256SUMS digest mismatch for {name}")
Path(sys.argv[2]).write_text("".join(f"{name}\n" for name in sorted(manifest)), encoding="utf-8")
PY
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
