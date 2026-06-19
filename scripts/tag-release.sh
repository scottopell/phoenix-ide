#!/usr/bin/env bash
# Open a release bump PR. Merging it triggers the build + tag + publish.
#
# Usage:
#   ./scripts/tag-release.sh          # auto-bump minor: 0.1.0 -> 0.2.0
#   ./scripts/tag-release.sh v1.0.0   # explicit version (v-prefix optional)
#
# This bumps crates/phoenix-ide/Cargo.toml, commits on a branch, and opens a
# PR. It does NOT create or push a tag: the .github/workflows/release.yml
# workflow fires when the bump lands on `main`, then creates the `vX.Y.Z` tag
# at that main commit and builds the release. That keeps the tag always on
# `main` and always `v`-prefixed by construction. Merge the PR to release.
#
# Merge the bump PR with a MERGE COMMIT (not squash/rebase) only if you care
# about a specific authored bump SHA — the workflow tags whatever main HEAD is
# after the merge regardless, so any merge strategy yields a tag-on-main.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }
ok()   { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
info() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }

DIRTY=$(git -C "$ROOT" status --porcelain)
[[ -z "$DIRTY" ]] || die "Working tree has uncommitted changes — commit or stash first."

git -C "$ROOT" fetch origin main --quiet

# Resolve the target version (strip an optional leading v).
if [[ -n "${1:-}" ]]; then
    VERSION="${1#v}"
    [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Version must be X.Y.Z (got: $1)"
else
    LATEST=$(git -C "$ROOT" tag --sort=-v:refname | grep -m1 '^v[0-9]' || echo "")
    if [[ -z "$LATEST" ]]; then
        VERSION="0.1.0"
    else
        IFS='.' read -r MAJOR MINOR PATCH <<< "${LATEST#v}"
        VERSION="${MAJOR}.$((MINOR + 1)).0"
    fi
    info "Latest tag: ${LATEST:-none} -> v$VERSION"
fi

TAG="v$VERSION"
BRANCH="chore/bump-${VERSION}"

# Refuse to re-release an already-tagged version.
git -C "$ROOT" rev-parse -q --verify "refs/tags/$TAG" >/dev/null \
    && die "Tag $TAG already exists — that version was already released."

CARGO_TOML="$ROOT/crates/phoenix-ide/Cargo.toml"
[[ -f "$CARGO_TOML" ]] || die "Expected $CARGO_TOML to exist."

CURRENT=$(grep -m1 '^version' "$CARGO_TOML" | sed 's/version = "\(.*\)"/\1/')
[[ -n "$CURRENT" ]] || die "Could not read current version from $CARGO_TOML"
[[ "$CURRENT" != "$VERSION" ]] || die "Cargo.toml is already at $VERSION — nothing to bump."

# Build the bump on a fresh branch off origin/main (never commit to main: it is
# branch-protected, and the bump must arrive via PR).
info "Branching $BRANCH off origin/main"
git -C "$ROOT" checkout -q -b "$BRANCH" origin/main

info "Bumping crates/phoenix-ide/Cargo.toml: $CURRENT -> $VERSION"
sed "s/^version = \"$CURRENT\"/version = \"$VERSION\"/" "$CARGO_TOML" > "$CARGO_TOML.tmp" && mv "$CARGO_TOML.tmp" "$CARGO_TOML"
if ! (cd "$ROOT" && cargo update -p phoenix_ide --offline); then
    die "Failed to refresh Cargo.lock for phoenix_ide"
fi
git -C "$ROOT" add crates/phoenix-ide/Cargo.toml Cargo.lock
git -C "$ROOT" commit -q -m "chore: bump version to $VERSION"
ok "Version bumped on $BRANCH"

git -C "$ROOT" push -u origin "$BRANCH"

if command -v gh >/dev/null 2>&1; then
    (cd "$ROOT" && gh pr create --base main --head "$BRANCH" \
        --title "chore: bump version to $VERSION" \
        --body "Bump to $VERSION. Merging this fires the release workflow, which tags $TAG at the resulting main commit and builds + publishes the binary.") \
        && ok "Opened bump PR — merge it to release $TAG."
else
    info "gh not found — open the bump PR manually:"
    printf '    https://github.com/scottopell/phoenix-ide/pull/new/%s\n' "$BRANCH"
fi
