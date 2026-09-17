#!/bin/bash
set -euo pipefail

GIT=${GIT:-git}
PYTHON3=${PYTHON3:-python3}

[[ $# -ge 4 ]] || {
  echo "usage: publish-release-assets.sh REPO TAG EXPECTED_COMMIT ASSET..." >&2
  exit 2
}
repo=$1
tag=$2
expected_commit=$3
shift 3
assets=("$@")
[[ "$expected_commit" =~ ^[0-9a-f]{40}$ ]] || {
  echo "error: expected commit must be a full lowercase git SHA" >&2
  exit 2
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
expected_digests="$work/expected-digests.tsv"
"$PYTHON3" - "$expected_digests" "${assets[@]}" <<'PY'
import hashlib
import re
import sys
from pathlib import Path

output = Path(sys.argv[1])
paths = [Path(value) for value in sys.argv[2:]]
by_name = {}
for path in paths:
    if not path.is_file():
        raise SystemExit(f"error: release asset is not a file: {path}")
    if "\t" in path.name or "\n" in path.name:
        raise SystemExit(f"error: unsupported release asset name: {path.name!r}")
    if path.name in by_name:
        raise SystemExit(f"error: duplicate release asset name: {path.name}")
    by_name[path.name] = path

checksum = by_name.get("SHA256SUMS")
if checksum is None:
    raise SystemExit("error: release asset set must contain SHA256SUMS")
expected_members = set(by_name) - {"SHA256SUMS"}
manifest = {}
for line in checksum.read_text(encoding="utf-8").splitlines():
    match = re.fullmatch(r"([0-9a-f]{64}) [ *](.+)", line)
    if not match:
        raise SystemExit(f"error: malformed SHA256SUMS line: {line!r}")
    digest, name = match.groups()
    if name in manifest:
        raise SystemExit(f"error: duplicate SHA256SUMS member: {name}")
    manifest[name] = digest
if set(manifest) != expected_members:
    raise SystemExit("error: SHA256SUMS membership does not match release assets")

rows = []
for name, path in by_name.items():
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if name != "SHA256SUMS" and manifest[name] != digest:
        raise SystemExit(f"error: SHA256SUMS digest mismatch for {name}")
    rows.append((name, digest))
output.write_text(
    "".join(f"{name}\t{digest}\n" for name, digest in sorted(rows)),
    encoding="utf-8",
)
PY

verify_tag() {
  "$GIT" fetch --force origin "refs/tags/$tag:refs/tags/$tag" >/dev/null 2>&1 || {
    echo "error: release tag $tag no longer exists on origin" >&2
    return 1
  }
  local actual_commit
  actual_commit=$("$GIT" rev-list -n 1 "$tag")
  [[ "$actual_commit" == "$expected_commit" ]] || {
    echo "error: release tag $tag points at $actual_commit, expected $expected_commit" >&2
    return 1
  }
}

published_release_metadata() {
  gh api "repos/$repo/releases/tags/$tag"
}

draft_release_metadata() {
  gh api --paginate --slurp "repos/$repo/releases?per_page=100" \
    | jq -ce --arg tag "$tag" \
      '[.[][] | select(.draft == true and .tag_name == $tag)] | if length == 1 then .[0] elif length == 0 then empty else error("multiple drafts for release tag") end'
}

release_metadata_by_id() {
  local release_id=$1
  gh api "repos/$repo/releases/$release_id"
}

current_release_metadata() {
  published_release_metadata 2>/dev/null || draft_release_metadata
}

verify_release_assets() {
  local expected_draft=$1
  local metadata="$work/release-$expected_draft.json"
  current_release_metadata >"$metadata"
  "$PYTHON3" - "$expected_digests" "$metadata" "$expected_draft" <<'PY'
import json
import sys
from pathlib import Path

expected = {}
for line in Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    name, digest = line.split("\t", 1)
    expected[name] = f"sha256:{digest}"
release = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
expected_draft = sys.argv[3] == "true"
if bool(release.get("draft")) != expected_draft:
    raise SystemExit("error: release visibility does not match the required state")
assets = release.get("assets", [])
actual = {asset.get("name"): asset.get("digest") for asset in assets}
if len(actual) != len(assets) or set(actual) != set(expected):
    raise SystemExit("error: release asset names do not match the exact required set")
for name, digest in expected.items():
    if actual[name] != digest:
        raise SystemExit(f"error: published digest mismatch for {name}")
PY
}

verify_draft_subset_and_list_missing() {
  local metadata=$1
  local missing=$2
  "$PYTHON3" - "$expected_digests" "$metadata" "$missing" <<'PY'
import json
import sys
from pathlib import Path

expected = {}
for line in Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    name, digest = line.split("\t", 1)
    expected[name] = f"sha256:{digest}"
release = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
if not release.get("draft"):
    raise SystemExit("error: release became public before exact verification")
assets = release.get("assets", [])
actual = {asset.get("name"): asset.get("digest") for asset in assets}
if len(actual) != len(assets) or not set(actual).issubset(expected):
    raise SystemExit("error: draft contains duplicate or unexpected assets")
for name, digest in actual.items():
    if digest != expected[name]:
        raise SystemExit(f"error: draft digest mismatch for {name}")
Path(sys.argv[3]).write_text(
    "".join(f"{name}\n" for name in sorted(set(expected) - set(actual))),
    encoding="utf-8",
)
PY
}

asset_path_by_name() {
  local wanted=$1
  local path
  for path in "${assets[@]}"; do
    if [[ $(basename "$path") == "$wanted" ]]; then
      printf '%s\n' "$path"
      return 0
    fi
  done
  echo "error: no local asset path for $wanted" >&2
  return 1
}

upload_asset_to_draft() {
  local release_id=$1
  local path=$2
  local name
  name=$(basename "$path")
  gh api --hostname uploads.github.com --method POST \
    "repos/$repo/releases/$release_id/assets?name=$name" \
    -H 'Content-Type: application/octet-stream' \
    --input "$path" >/dev/null
}

verify_tag
metadata=$(published_release_metadata 2>/dev/null || true)
if [[ -n "$metadata" ]]; then
  verify_release_assets false
  verify_tag
  echo "release $tag is already published with the exact asset set"
  exit 0
fi

if metadata=$(draft_release_metadata); then
  :
else
  status=$?
  if [[ $status -eq 4 ]]; then
    metadata=
  else
    exit "$status"
  fi
fi
if [[ -z "$metadata" ]]; then
  verify_tag
  gh release create "$tag" --repo "$repo" --verify-tag --draft --title "$tag" --generate-notes
  metadata=$(draft_release_metadata)
fi

release_id=$(jq -er '.id' <<<"$metadata")
metadata_file="$work/draft.json"
missing_file="$work/missing-assets"
release_metadata_by_id "$release_id" >"$metadata_file"
verify_draft_subset_and_list_missing "$metadata_file" "$missing_file"
while IFS= read -r name; do
  [[ -n "$name" ]] || continue
  verify_tag
  release_metadata_by_id "$release_id" >"$metadata_file"
  verify_draft_subset_and_list_missing "$metadata_file" "$missing_file.current"
  grep -Fx "$name" "$missing_file.current" >/dev/null || {
    echo "error: draft changed while preparing $name" >&2
    exit 1
  }
  upload_asset_to_draft "$release_id" "$(asset_path_by_name "$name")"
done <"$missing_file"

verify_tag
release_metadata_by_id "$release_id" >"$metadata_file"
verify_draft_subset_and_list_missing "$metadata_file" "$missing_file"
[[ ! -s "$missing_file" ]] || { echo "error: draft remains incomplete" >&2; exit 1; }
gh api --method PATCH "repos/$repo/releases/$release_id" -F draft=false >/dev/null
verify_release_assets false
verify_tag
