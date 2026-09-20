#!/bin/bash
set -euo pipefail

GIT=${GIT:-git}
PYTHON3=${PYTHON3:-python3}
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

[[ $# -ge 5 ]] || {
  echo "usage: publish-release-assets.sh REPO TAG EXPECTED_COMMIT CHANNEL ASSET..." >&2
  exit 2
}
repo=$1
tag=$2
expected_commit=$3
channel=$4
shift 4
assets=("$@")
[[ "$expected_commit" =~ ^[0-9a-f]{40}$ ]] || {
  echo "error: expected commit must be a full lowercase git SHA" >&2
  exit 2
}
release_version=$("$PYTHON3" "$SCRIPT_DIR/release_version.py" validate-tag "$tag") || exit 2
case "$release_version" in
  *-rc.*) tag_channel=rc ;;
  *) tag_channel=stable ;;
esac
[[ "$channel" == stable || "$channel" == rc ]] || {
  echo "error: release channel must be stable or rc" >&2
  exit 2
}
[[ "$channel" == "$tag_channel" ]] || {
  echo "error: release channel $channel does not match tag $tag" >&2
  exit 2
}
if [[ "$channel" == rc ]]; then
  expected_prerelease=true
  make_latest=false
else
  expected_prerelease=false
  make_latest=true
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
expected_digests="$work/expected-digests.tsv"
release_inventory="$work/releases.json"
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
    if not re.fullmatch(r"[A-Za-z0-9._-]+", path.name):
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
  gh api --paginate --slurp "repos/$repo/releases?per_page=100" >"$release_inventory" || return $?
  jq -ce --arg tag "$tag" --argjson prerelease "$expected_prerelease" \
    '[.[][] | select(.tag_name == $tag)] as $matching | if any($matching[]; .prerelease != $prerelease) then error("release publication channel does not match requested channel") else [$matching[] | select(.draft == false)] | if length == 1 then .[0] elif length == 0 then empty else error("multiple public releases for tag") end end' \
    "$release_inventory"
}

draft_release_metadata() {
  gh api --paginate --slurp "repos/$repo/releases?per_page=100" >"$release_inventory" || return $?
  jq -ce --arg tag "$tag" --argjson prerelease "$expected_prerelease" \
    '[.[][] | select(.tag_name == $tag)] as $matching | if any($matching[]; .prerelease != $prerelease) then error("release publication channel does not match requested channel") else [$matching[] | select(.draft == true)] | if length == 1 then .[0] elif length == 0 then empty else error("multiple drafts for release tag") end end' \
    "$release_inventory"
}

release_metadata_by_id() {
  local release_id=$1
  gh api "repos/$repo/releases/$release_id"
}

current_release_metadata() {
  local metadata status
  if metadata=$(published_release_metadata); then
    printf '%s\n' "$metadata"
    return 0
  else
    status=$?
    [[ $status -eq 4 ]] || return "$status"
  fi
  draft_release_metadata
}

verify_release_assets() {
  local expected_draft=$1
  local metadata="$work/release-$expected_draft.json"
  current_release_metadata >"$metadata"
  "$PYTHON3" - "$expected_digests" "$metadata" "$expected_draft" "$expected_prerelease" <<'PY'
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
expected_prerelease = sys.argv[4] == "true"
if release.get("prerelease") is not expected_prerelease:
    raise SystemExit("error: release publication channel does not match the required state")
assets = release.get("assets", [])
actual = {asset.get("name"): asset.get("digest") for asset in assets}
if len(actual) != len(assets) or set(actual) != set(expected):
    raise SystemExit("error: release asset names do not match the exact required set")
for name, digest in expected.items():
    if actual[name] != digest:
        raise SystemExit(f"error: published digest mismatch for {name}")
PY
}

verify_latest_relationship() {
  local latest_tag
  latest_tag=$(gh api "repos/$repo/releases/latest" --jq .tag_name)
  if [[ "$channel" == stable ]]; then
    [[ "$latest_tag" == "$tag" ]] || {
      echo "error: newly published stable release $tag is not the repository latest release" >&2
      return 1
    }
  else
    [[ "$latest_tag" != "$tag" ]] || {
      echo "error: release candidate $tag replaced the repository latest stable release" >&2
      return 1
    }
  fi
}

verify_public_rc_is_not_latest() {
  [[ "$channel" != rc ]] || verify_latest_relationship
}

assert_stable_is_newest_before_publication() {
  [[ "$channel" != stable ]] && return 0
  local public_stable_tags="$work/public-stable-tags"
  gh api --paginate --slurp "repos/$repo/releases?per_page=100" \
    | jq -er '.[][] | select(.draft == false and .prerelease == false) | .tag_name' \
      >"$public_stable_tags" || {
        local statuses=("${PIPESTATUS[@]}")
        [[ ${statuses[0]} -eq 0 && ${statuses[1]} -eq 4 ]] || return 1
      }
  [[ ! -s "$public_stable_tags" ]] && return 0
  "$SCRIPT_DIR/release_version.py" validate-new-from-tags "$version" \
    <"$public_stable_tags" >/dev/null || {
      echo "error: refusing to make older stable release $tag latest over a newer public stable release" >&2
      return 1
    }
}

assert_private_release() {
  local metadata=$1
  "$PYTHON3" - "$metadata" "$tag" "$expected_prerelease" <<'PY'
import json
import sys
from pathlib import Path
release = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
tag = sys.argv[2]
expected_prerelease = sys.argv[3] == "true"
if not release.get("draft"):
    raise SystemExit("error: release became public before exact verification")
if release.get("prerelease") is not expected_prerelease:
    raise SystemExit("error: release draft channel changed before exact verification")
if release.get("tag_name") != tag:
    raise SystemExit("error: release draft no longer targets the exact tag")
PY
}

verify_complete_private_release() {
  local metadata=$1
  "$PYTHON3" - "$expected_digests" "$metadata" <<'PY'
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
if len(actual) != len(assets) or actual != expected:
    raise SystemExit("error: private release asset names or digests are not exact")
PY
}

upload_asset_to_draft() {
  local release_id=$1
  local path=$2
  local name
  name=$(basename "$path")
  gh api --method POST \
    "https://uploads.github.com/repos/$repo/releases/$release_id/assets?name=$name" \
    -H 'Content-Type: application/octet-stream' \
    --input "$path" >/dev/null
}

verify_tag
if metadata=$(published_release_metadata); then
  verify_release_assets false
  verify_tag
  verify_public_rc_is_not_latest
  echo "release $tag is already published with the exact asset set"
  exit 0
else
  status=$?
  [[ $status -eq 4 ]] || exit "$status"
  metadata=
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
  gh api --method POST "repos/$repo/releases" \
    -f tag_name="$tag" \
    -f name="$tag" \
    -f target_commitish="$expected_commit" \
    -F draft=true \
    -F prerelease="$expected_prerelease" \
    -f make_latest="$make_latest" \
    -F generate_release_notes=true >/dev/null
  metadata=$(draft_release_metadata)
fi

release_id=$(jq -er '.id' <<<"$metadata")
metadata_file="$work/draft.json"
release_metadata_by_id "$release_id" >"$metadata_file"
assert_private_release "$metadata_file"
while IFS= read -r asset_id; do
  [[ -n "$asset_id" ]] || continue
  verify_tag
  release_metadata_by_id "$release_id" >"$metadata_file"
  assert_private_release "$metadata_file"
  jq -e --argjson id "$asset_id" '.assets[] | select(.id == $id)' "$metadata_file" >/dev/null || {
    echo "error: private release changed while removing asset $asset_id" >&2
    exit 1
  }
  gh api --method DELETE "repos/$repo/releases/assets/$asset_id" >/dev/null
done < <(jq -r '.assets[]?.id' "$metadata_file")

for path in "${assets[@]}"; do
  verify_tag
  release_metadata_by_id "$release_id" >"$metadata_file"
  assert_private_release "$metadata_file"
  upload_asset_to_draft "$release_id" "$path"
done

verify_tag
release_metadata_by_id "$release_id" >"$metadata_file"
verify_complete_private_release "$metadata_file"
assert_private_release "$metadata_file"
assert_stable_is_newest_before_publication
gh api --method PATCH "repos/$repo/releases/$release_id" \
  -F draft=false \
  -F prerelease="$expected_prerelease" \
  -f make_latest="$make_latest" >/dev/null
verify_release_assets false
verify_tag
verify_latest_relationship
