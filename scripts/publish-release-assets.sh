#!/bin/bash
set -euo pipefail

[[ $# -ge 3 ]] || { echo "usage: publish-release-assets.sh REPO TAG ASSET..." >&2; exit 2; }
repo=$1
tag=$2
shift 2
assets=("$@")

if ! gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
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
  local name id
  for name in "${staged_names[@]}"; do
    if id=$(asset_id "$name" 2>/dev/null); then
      gh api --method DELETE "repos/$repo/releases/assets/$id" >/dev/null || true
    fi
  done
}
if ! gh release upload "$tag" --repo "$repo" "$staged"/*; then
  cleanup_staged
  echo "staged release asset upload failed; previous asset set remains active" >&2
  exit 1
fi

restore_previous() {
  cleanup_staged
  local files=()
  local backup_names=()
  while IFS= read -r -d '' file; do
    files+=("$file")
    backup_names+=("$(basename "$file")")
  done < <(find "$backup" -type f -print0)
  if ((${#files[@]})); then
    gh release upload "$tag" --repo "$repo" --clobber "${files[@]}" || return 1
  fi

  local current_names=()
  mapfile -t current_names < <(release_json | jq -r '.assets[].name')
  local name file found
  for name in "${current_names[@]}"; do
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
  done
  for file in "${backup_names[@]}"; do
    asset_id "$file" >/dev/null || return 1
  done
  mapfile -t current_names < <(release_json | jq -r '.assets[].name' | sort)
  mapfile -t backup_names < <(printf '%s
' "${backup_names[@]}" | sort)
  [[ "${current_names[*]}" == "${backup_names[*]}" ]]
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
  restore_previous || { echo "error: previous release asset restoration failed" >&2; exit 2; }
  exit 1
fi
cleanup_staged
for asset in "${assets[@]}"; do asset_id "$(basename "$asset")" >/dev/null; done
