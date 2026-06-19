#!/usr/bin/env bash
# Create and push a release tag. GitHub Actions handles the build.
#
# Usage:
#   ./scripts/tag-release.sh          # auto-bump minor: v0.1.0 -> v0.2.0
#   ./scripts/tag-release.sh v1.0.0   # explicit tag
#
# Bumps Cargo.toml version to match, commits, tags, and pushes.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }
ok()   { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
info() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }

DIRTY=$(git -C "$ROOT" status --porcelain)
[[ -z "$DIRTY" ]] || die "Working tree has uncommitted changes — commit or stash first."

if [[ -n "${1:-}" ]]; then
    TAG="$1"
    [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Tag must be vX.Y.Z format (got: $TAG)"
else
    LATEST=$(git -C "$ROOT" tag --sort=-v:refname | grep -m1 '^v[0-9]' || echo "")
    if [[ -z "$LATEST" ]]; then
        TAG="v0.1.0"
    else
        IFS='.' read -r MAJOR MINOR PATCH <<< "${LATEST#v}"
        TAG="v${MAJOR}.$((MINOR + 1)).0"
    fi
    info "Latest tag: ${LATEST:-none} -> $TAG"
fi

VERSION="${TAG#v}"

git -C "$ROOT" tag | grep -qx "$TAG" && die "Tag $TAG already exists locally."

# Bump version in crates/phoenix-ide/Cargo.toml.
# The root Cargo.toml is workspace-only (no [package]) since the crates/
# restructure — the version lives on the phoenix_ide crate.
CARGO_TOML="$ROOT/crates/phoenix-ide/Cargo.toml"
[[ -f "$CARGO_TOML" ]] || die "Expected $CARGO_TOML to exist."

CURRENT=$(grep -m1 '^version' "$CARGO_TOML" | sed 's/version = "\(.*\)"/\1/')
[[ -n "$CURRENT" ]] || die "Could not read current version from $CARGO_TOML"

if [[ "$CURRENT" != "$VERSION" ]]; then
    info "Bumping crates/phoenix-ide/Cargo.toml: $CURRENT -> $VERSION"
    sed "s/^version = \"$CURRENT\"/version = \"$VERSION\"/" "$CARGO_TOML" > "$CARGO_TOML.tmp" && mv "$CARGO_TOML.tmp" "$CARGO_TOML"
    # Refresh Cargo.lock for the bumped crate.
    if ! (cd "$ROOT" && cargo update -p phoenix_ide --offline); then
        die "Failed to refresh Cargo.lock for phoenix_ide"
    fi
    git -C "$ROOT" add crates/phoenix-ide/Cargo.toml Cargo.lock
    git -C "$ROOT" commit -m "chore: bump version to $VERSION"
    BUMP_COMMITTED=1
    ok "Version bumped"
else
    info "Version already $VERSION — no bump needed"
fi

SHA=$(git -C "$ROOT" rev-parse --short HEAD)
info "Tagging $SHA as $TAG"
git -C "$ROOT" tag -a "$TAG" -m "$TAG"

# Push the TAG ONLY. `main` is branch-protected (changes must go through a
# PR), so a direct `git push origin main` is rejected. The tag ref is not
# covered by branch protection, and pushing it carries the bump commit's
# objects to the remote — so CI checks out the tag and builds the binary
# even though `main` has not advanced yet.
git -C "$ROOT" push origin "$TAG"
ok "Pushed $TAG — GitHub Actions will build and publish the release."
printf '\033[0;90m  https://github.com/scottopell/phoenix-ide/releases/tag/%s\033[0m\n' "$TAG"

# The bump commit still has to land on `main`. It is at local HEAD; move it
# onto its own branch, restore local `main` to the remote, and open a PR.
if [[ -z "${BUMP_COMMITTED:-}" ]]; then
    info "Version was already $VERSION — no bump commit to land on main."
    exit 0
fi

BRANCH="chore/bump-${VERSION}"
info "Landing the bump on main via PR (branch protection blocks direct push)"
git -C "$ROOT" branch "$BRANCH" HEAD
git -C "$ROOT" reset --hard origin/main
git -C "$ROOT" push -u origin "$BRANCH"

if command -v gh >/dev/null 2>&1; then
    (cd "$ROOT" && gh pr create --base main --head "$BRANCH" \
        --title "chore: bump version to $VERSION" \
        --body "Version bump for the $TAG release. Tag is already pushed and CI builds the binary from the bump commit; this PR lands the bump on \`main\` (direct push blocked by branch protection).") \
        && ok "Opened bump PR — merge it to bring main's version up to $VERSION."
else
    info "gh not found — open the bump PR manually:"
    printf '    https://github.com/scottopell/phoenix-ide/pull/new/%s\n' "$BRANCH"
fi
