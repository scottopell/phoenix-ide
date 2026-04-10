#!/usr/bin/env bash
# Create and push a release tag. GitHub Actions handles the build.
#
# Usage: ./scripts/tag-release.sh v1.2.3

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }
ok()   { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
info() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }

TAG="${1:-}"
[[ -n "$TAG" ]] || die "Usage: $0 v1.2.3"
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Tag must be vX.Y.Z format (got: $TAG)"

DIRTY=$(git -C "$ROOT" status --porcelain)
[[ -z "$DIRTY" ]] || die "Working tree has uncommitted changes — commit or stash first."

# Check tag doesn't already exist
git -C "$ROOT" tag | grep -qx "$TAG" && die "Tag $TAG already exists locally."

SHA=$(git -C "$ROOT" rev-parse --short HEAD)
info "Tagging $SHA as $TAG"
git -C "$ROOT" tag -a "$TAG" -m "$TAG"
git -C "$ROOT" push origin "$TAG"
ok "Pushed $TAG — GitHub Actions will build and publish the release."
printf '\033[0;90m  https://github.com/scottopell/phoenix-ide/releases/tag/%s\033[0m\n' "$TAG"
