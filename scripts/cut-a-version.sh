#!/usr/bin/env bash
# Build a versioned musl release binary and store it in ~/phoenix-builds/.
#
# Output: ~/phoenix-builds/phoenix_ide-x86_64-unknown-linux-musl--<shortsha>
#
# Requires a clean working tree — the binary name encodes the SHA and should
# match exactly what's in git. Use 'git stash' if you have uncommitted work.
#
# Safe to re-run: skips the build if the output file already exists.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_WORKTREE="$(dirname "$ROOT")/.phoenix-ide-build"
OUT_DIR="$HOME/phoenix-builds"
TARGET="x86_64-unknown-linux-musl"

# ── helpers ──────────────────────────────────────────────────────────────────

info() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
ok()   { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
die()  { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

# ── require clean tree ───────────────────────────────────────────────────────

DIRTY=$(git -C "$ROOT" status --porcelain)
if [[ -n "$DIRTY" ]]; then
    die "Working tree has uncommitted changes — stash or commit before cutting a version."
fi

SHA=$(git -C "$ROOT" rev-parse --short HEAD)
OUT="$OUT_DIR/phoenix_ide-${TARGET}--${SHA}"

mkdir -p "$OUT_DIR"

if [[ -f "$OUT" ]]; then
    ok "Already built: $OUT"
    echo "$OUT"
    exit 0
fi

info "Cutting version $SHA"

# ── update build worktree ────────────────────────────────────────────────────

info "Updating build worktree"
if git -C "$ROOT" worktree list | grep -qF "$BUILD_WORKTREE"; then
    # Registered worktree — just checkout the target commit
    git -C "$BUILD_WORKTREE" checkout --force "$(git -C "$ROOT" rev-parse HEAD)"
elif [[ -d "$BUILD_WORKTREE" ]]; then
    # Directory exists but not registered (created by dev.py worktree add previously)
    # dev.py manages this worktree; just checkout the right commit directly
    git -C "$BUILD_WORKTREE" checkout --force "$(git -C "$ROOT" rev-parse HEAD)"
else
    git -C "$ROOT" worktree add --detach "$BUILD_WORKTREE" HEAD
fi
ok "Worktree at $SHA"

# ── build UI ─────────────────────────────────────────────────────────────────

info "Building UI"
npm ci    --prefix "$BUILD_WORKTREE/ui" --silent
npm run build --prefix "$BUILD_WORKTREE/ui"
ok "UI built"

# ── build Rust ───────────────────────────────────────────────────────────────

info "Building Rust ($TARGET)"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CC_x86_64_unknown_linux_musl="x86_64-linux-musl-gcc"
cargo build --release --target "$TARGET" --manifest-path "$BUILD_WORKTREE/Cargo.toml"
ok "Rust built"

# ── copy to output ───────────────────────────────────────────────────────────

BINARY="$BUILD_WORKTREE/target/$TARGET/release/phoenix_ide"
cp "$BINARY" "$OUT"
chmod +x "$OUT"

SIZE=$(du -sh "$OUT" | cut -f1)
ok "Stored: $OUT ($SIZE)"
echo "$OUT"
