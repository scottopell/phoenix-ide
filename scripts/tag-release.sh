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

# Bump Cargo.toml version
CARGO_TOML="$ROOT/Cargo.toml"
CURRENT=$(grep -m1 '^version' "$CARGO_TOML" | sed 's/version = "\(.*\)"/\1/')
if [[ "$CURRENT" != "$VERSION" ]]; then
    info "Bumping Cargo.toml: $CURRENT -> $VERSION"
    sed "s/^version = \"$CURRENT\"/version = \"$VERSION\"/" "$CARGO_TOML" > "$CARGO_TOML.tmp" && mv "$CARGO_TOML.tmp" "$CARGO_TOML"
    # Update Cargo.lock
    cargo generate-lockfile --manifest-path "$CARGO_TOML" 2>/dev/null
    git -C "$ROOT" add Cargo.toml Cargo.lock
    git -C "$ROOT" commit -m "chore: bump version to $VERSION"
    ok "Version bumped"
fi

SHA=$(git -C "$ROOT" rev-parse --short HEAD)
info "Tagging $SHA as $TAG"
git -C "$ROOT" tag -a "$TAG" -m "$TAG"
git -C "$ROOT" push origin main "$TAG"
ok "Pushed $TAG — GitHub Actions will build and publish the release."
printf '\033[0;90m  https://github.com/scottopell/phoenix-ide/releases/tag/%s\033[0m\n' "$TAG"
