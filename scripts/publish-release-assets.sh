#!/bin/bash
set -euo pipefail

[[ $# -ge 3 ]] || { echo "usage: publish-release-assets.sh REPO TAG ASSET..." >&2; exit 2; }
repo=$1
tag=$2
shift 2
assets=("$@")

release_metadata=$(gh api "repos/$repo/releases/tags/$tag" 2>/dev/null || true)
if [[ -z "$release_metadata" ]]; then
  gh release create "$tag" --repo "$repo" --title "$tag" --generate-notes "${assets[@]}"
  exit 0
fi
if [[ $(jq -r '.draft // false' <<<"$release_metadata") == true ]]; then
  draft_id=$(jq -er '.id' <<<"$release_metadata")
  gh api --method DELETE "repos/$repo/releases/$draft_id" >/dev/null
  gh release create "$tag" --repo "$repo" --title "$tag" --generate-notes "${assets[@]}"
  exit 0
fi

work=$(mktemp -d)
backup="$work/backup"
staged="$work/staged"
mkdir -p "$backup" "$staged"
trap 'rm -rf "$work"' EXIT

gh release download "$tag" --repo "$repo" --dir "$backup"
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
cleanup_staged() {
  local name id failed=0
  for name in "${staged_names[@]}"; do
    if id=$(asset_id "$name" 2>/dev/null); then
      gh api --method DELETE "repos/$repo/releases/assets/$id" >/dev/null || failed=1
    fi
  done
  ((failed == 0))
}
if ! gh release upload "$tag" --repo "$repo" "$staged"/*; then
  if ! cleanup_staged; then
    echo "error: staged upload failed and staged asset cleanup also failed" >&2
    exit 2
  fi
  echo "staged release asset upload failed; previous asset set remains active" >&2
  exit 1
fi

restore_previous() {
  cleanup_staged || return 1
  local files=()
  local backup_names=()
  while IFS= read -r -d '' file; do
    files+=("$file")
    backup_names+=("$(basename "$file")")
  done < <(find "$backup" -type f -print0)
  if ((${#files[@]})); then
    gh release upload "$tag" --repo "$repo" --clobber "${files[@]}" || return 1
  fi

  local current_names_file="$work/current-names"
  release_json | jq -r '.assets[].name' > "$current_names_file"
  local name file found
  while IFS= read -r name; do
    found=0
    for file in "${backup_names[@]}"; do
      if [[ "$file" == "$name" ]]; then
        found=1
        break
      fi
    done
    if (( !found )); then
      if id=$(asset_id "$name" 2>/dev/null); then
        gh api --method DELETE "repos/$repo/releases/assets/$id" >/dev/null || return 1
      fi
    fi
  done < "$current_names_file"
  for file in "${backup_names[@]}"; do
    asset_id "$file" >/dev/null || return 1
  done
  local sorted_current="$work/current-names-sorted"
  local sorted_backup="$work/backup-names-sorted"
  release_json | jq -r '.assets[].name' | sort > "$sorted_current"
  printf '%s\n' "${backup_names[@]}" | sort > "$sorted_backup"
  cmp -s "$sorted_current" "$sorted_backup"
}

commit_failed=0
for index in "${!assets[@]}"; do
  final_name=$(basename "${assets[$index]}")
  staged_name=${staged_names[$index]}
  if old_id=$(asset_id "$final_name" 2>/dev/null); then
    gh api --method DELETE "repos/$repo/releases/assets/$old_id" >/dev/null || { commit_failed=1; break; }
  fi
  staged_id=$(asset_id "$staged_name") || { commit_failed=1; break; }
  gh api --method PATCH "repos/$repo/releases/assets/$staged_id" -f name="$final_name" >/dev/null || { commit_failed=1; break; }
done

if ((commit_failed)); then
  echo "release asset replacement failed; restoring previous asset set" >&2
  if restore_previous; then
    exit 1
  fi
  echo "error: previous release asset restoration failed" >&2
  exit 2
fi
cleanup_staged || { echo "error: committed assets but failed to remove staged release debris" >&2; exit 2; }
for asset in "${assets[@]}"; do asset_id "$(basename "$asset")" >/dev/null; done
