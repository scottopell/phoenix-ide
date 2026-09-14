#!/bin/bash
set -euo pipefail

GIT=${GIT:-git}

[[ $# -ge 4 ]] || { echo "usage: publish-release-assets.sh REPO TAG EXPECTED_COMMIT ASSET..." >&2; exit 2; }
repo=$1
tag=$2
expected_commit=$3
shift 3
assets=("$@")
[[ "$expected_commit" =~ ^[0-9a-f]{40}$ ]] || { echo "error: expected commit must be a full lowercase git SHA" >&2; exit 2; }
"$GIT" fetch --force origin "refs/tags/$tag:refs/tags/$tag" >/dev/null 2>&1 || {
  echo "error: release tag $tag no longer exists on origin" >&2
  exit 1
}
actual_commit=$("$GIT" rev-list -n 1 "$tag")
[[ "$actual_commit" == "$expected_commit" ]] || {
  echo "error: release tag $tag points at $actual_commit, expected $expected_commit" >&2
  exit 1
}

release_metadata=$(gh api "repos/$repo/releases/tags/$tag" 2>/dev/null || true)
if [[ -z "$release_metadata" ]]; then
  gh release create "$tag" --repo "$repo" --verify-tag --title "$tag" --generate-notes "${assets[@]}"
  exit 0
fi
if [[ $(jq -r '.draft // false' <<<"$release_metadata") == true ]]; then
  draft_id=$(jq -er '.id' <<<"$release_metadata")
  gh api --method DELETE "repos/$repo/releases/$draft_id" >/dev/null
  gh release create "$tag" --repo "$repo" --verify-tag --title "$tag" --generate-notes "${assets[@]}"
  exit 0
fi

work=$(mktemp -d)
staged="$work/staged"
mkdir -p "$staged"
trap 'rm -rf "$work"' EXIT
nonce="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
staged_names=()
for asset in "${assets[@]}"; do
  staged_name="$(basename "$asset").staged-$nonce"
  cp "$asset" "$staged/$staged_name"
  staged_names+=("$staged_name")
done

release_json() { gh api "repos/$repo/releases/tags/$tag"; }
asset_id() {
  local name=$1
  release_json | jq -er --arg name "$name" '.assets[] | select(.name == $name) | .id' | head -n 1
}
remove_asset() {
  local name=$1 id
  if id=$(asset_id "$name" 2>/dev/null); then
    gh api --method DELETE "repos/$repo/releases/assets/$id" >/dev/null
  fi
}
remove_staging_assets() {
  local failed=0 name
  while IFS= read -r name; do
    [[ "$name" == *.staged-* ]] || continue
    remove_asset "$name" || failed=1
  done < <(release_json | jq -r '.assets[].name')
  ((failed == 0))
}

# Recover from an interrupted prior run by removing its temporary names. Active
# final names may be mixed, but every retry for this tag is constrained to the
# same commit and converges the complete set below.
remove_staging_assets || { echo "error: failed to clean stale staged assets" >&2; exit 2; }
if ! gh release upload "$tag" --repo "$repo" "$staged"/*; then
  remove_staging_assets || { echo "error: staged upload and cleanup both failed" >&2; exit 2; }
  echo "staged release asset upload failed; retry is safe" >&2
  exit 1
fi

for index in "${!assets[@]}"; do
  final_name=$(basename "${assets[$index]}")
  staged_name=${staged_names[$index]}
  remove_asset "$final_name"
  staged_id=$(asset_id "$staged_name")
  gh api --method PATCH "repos/$repo/releases/assets/$staged_id" -f name="$final_name" >/dev/null
 done

# Success is an exact deterministic asset set. Delete extras left by an older
# release shape or an interrupted run, then verify exact name equality.
desired="$work/desired"
current="$work/current"
printf '%s\n' "${assets[@]##*/}" | sort > "$desired"
while IFS= read -r name; do
  if ! grep -Fx -- "$name" "$desired" >/dev/null; then remove_asset "$name"; fi
 done < <(release_json | jq -r '.assets[].name')
release_json | jq -r '.assets[].name' | sort > "$current"
cmp -s "$current" "$desired" || { echo "error: release asset set did not converge" >&2; exit 2; }
