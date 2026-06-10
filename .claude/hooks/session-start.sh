#!/bin/bash
# SessionStart hook for Claude Code on the web.
#
# Warms the build cache and installs the fast-path toolchain so `./dev.py check`
# runs at warm speed in web sessions instead of paying a cold full-workspace
# compile every time. The remote container's filesystem is snapshotted after
# this hook completes, so the one-time cost here is amortized across every
# later session that starts from the snapshot.
#
# Runs in async mode (see the control JSON below): the warm build is minutes
# long, so blocking session startup on it is not worth it. The session starts
# immediately and this work completes in the background. `./dev.py check`
# degrades gracefully if a tool or warm target is not ready yet, so the only
# cost of the race is a one-off slower check early in the very first session.
#
# Every step is best-effort and idempotent: a missing registry or an
# already-installed tool must never wedge the run, so we do NOT use `set -e`
# and we always exit 0.
set -uo pipefail

# Only run in the remote (web) environment. Local sessions already have a warm
# checkout and the developer's own toolchain.
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "${CLAUDE_PROJECT_DIR:-.}" || exit 0

# Run the rest in the background so session startup is not blocked on the
# multi-minute warm build. asyncTimeout is generous: a cold install + warm of
# the whole workspace can take ~15-20 min on a fresh snapshot.
echo '{"async": true, "asyncTimeout": 1200000}'

log() { printf '[session-start] %s\n' "$*"; }

# --- 1. UI dependencies -----------------------------------------------------
# The tsc / eslint / stylelint / vitest lanes need ui/node_modules. dev.py's
# ensure_ui_deps() pins pnpm via Corepack; mirror that here so a fresh snapshot
# has deps ready. --prefer-offline reuses the pnpm store when it is warm.
log "installing UI dependencies (pnpm)..."
corepack enable >/dev/null 2>&1
( cd ui && pnpm install --prefer-offline ) || log "pnpm install failed (non-fatal)"

# --- 2. Fast-path toolchain -------------------------------------------------
# cargo-nextest  splits cargo test's build and run phases, so the e2e bin build
#                overlaps the test run and each step gets its own timeout budget
#                (dev.py auto-detects nextest and uses it when present).
# sccache        shares dependency object files across `cargo clean` cycles and
#                across the test/clippy target dirs (dev.py sets RUSTC_WRAPPER).
# allium         enables the spec-validation lane (skipped when not installed).
# @ast-grep/cli  enables the structural-lint lane (skipped when not installed).
install_cargo_tool() { # $1 = binary name on PATH, $2 = crate to install
  if command -v "$1" >/dev/null 2>&1; then
    log "$1 already present"
    return
  fi
  log "installing $1 (cargo install $2)..."
  cargo install "$2" --locked || log "cargo install $2 failed (non-fatal)"
}
install_cargo_tool cargo-nextest cargo-nextest
install_cargo_tool sccache sccache
install_cargo_tool allium allium-cli

if command -v ast-grep >/dev/null 2>&1; then
  log "ast-grep already present"
else
  log "installing ast-grep (npm -g @ast-grep/cli)..."
  npm install -g @ast-grep/cli || log "npm install @ast-grep/cli failed (non-fatal)"
fi

# --- 3. Warm the Rust build caches ------------------------------------------
# dev.py's check builds through two target dirs: the shared target/ (test
# binaries) and clippy's dedicated target/clippy. Warming both — plus the
# phoenix_ide bin the e2e lane links — leaves the snapshot past the cold
# full-workspace compile that otherwise dominates check wall time. Enable
# sccache for these builds so its cache is populated in the snapshot too.
if command -v sccache >/dev/null 2>&1 && [ -z "${RUSTC_WRAPPER:-}" ]; then
  export RUSTC_WRAPPER=sccache
fi
log "warming test build (shared target/)..."
cargo test --workspace --no-run --locked || log "test-build warm failed (non-fatal)"
log "warming e2e bin (target/debug/phoenix_ide)..."
cargo build --bin phoenix_ide --locked || log "bin warm failed (non-fatal)"
log "warming clippy build (target/clippy)..."
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace || log "clippy warm failed (non-fatal)"

log "done."
exit 0
