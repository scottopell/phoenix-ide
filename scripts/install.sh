#!/usr/bin/env bash
# Phoenix IDE — DD-internal install script
#
# Usage:
#   curl -fsSL https://sopell3.example/phoenix-ide/install.sh | bash
#
# Re-running is safe — it updates the clone and redeploys.
# Requirements: Linux x86_64, sudo access (for apt installs on first run only)

set -euo pipefail

REPO_URL="https://github.com/scottopell/phoenix-ide.git"
BUILD_DIR="$HOME/.phoenix-ide-build"
PORT=8031

# DD auth config — uv-based dd-internal-authentication helper
LLM_API_KEY_HELPER="uv run --with 'dd-internal-authentication @ https://binaries.ddbuild.io/dd-source/python/dd_internal_authentication-1.8.0-py2.py3-none-any.whl' python3 -c 'import os; os.environ.setdefault(\"DD_TRACE_ENABLED\",\"false\"); os.environ.setdefault(\"DD_TRACE_STARTUP_LOGS\",\"false\"); from dd_internal_authentication.client import JWTDDToolAuthClientTokenManager as M; print(M.instance(name=\"rapid-ai-platform\",datacenter=\"us1.ddbuild.io\").get_token(\"rapid-ai-platform\"))'"
LLM_API_KEY_HELPER_TTL_MS=7200000

# ── helpers ──────────────────────────────────────────────────────────────────

info()  { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
item()  { printf '\033[0;37m     - %s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m  ! %s\033[0m\n' "$*"; }
die()   { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

need_cmd() { command -v "$1" &>/dev/null || die "Required command not found: $1"; }

run() {
    printf '\033[0;90m  $ %s\033[0m\n' "$*"
    "$@"
}

# ── os check ─────────────────────────────────────────────────────────────────

[[ "$(uname -s)" == "Linux" ]]  || die "Linux only (got $(uname -s))"
[[ "$(uname -m)" == "x86_64" ]] || die "x86_64 only (got $(uname -m))"

need_cmd git
need_cmd curl

# ── phase 1: detect ───────────────────────────────────────────────────────────

NEED_MUSL=false
NEED_NODE=false
NEED_UV=false
NEED_RUST=false
NEED_MUSL_TARGET=false

command -v x86_64-linux-musl-gcc &>/dev/null || NEED_MUSL=true
command -v node                  &>/dev/null || NEED_NODE=true
command -v uv                    &>/dev/null || NEED_UV=true

if command -v cargo &>/dev/null; then
    # cargo present but may still need musl target
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
    rustup target list --installed 2>/dev/null | grep -q x86_64-unknown-linux-musl || NEED_MUSL_TARGET=true
else
    NEED_RUST=true
    NEED_MUSL_TARGET=true
fi

NEED_APT=$( [[ "$NEED_MUSL" == true || "$NEED_NODE" == true ]] && echo true || echo false )

# ── phase 2: confirm ──────────────────────────────────────────────────────────

printf '\n'
printf '\033[1;37mPhoenix IDE installer\033[0m\n'
printf '\n'

if [[ "$NEED_APT" == false && "$NEED_UV" == false && "$NEED_RUST" == false && "$NEED_MUSL_TARGET" == false ]]; then
    info "All dependencies already installed — updating and redeploying"
else
    info "The following will be installed:"
    [[ "$NEED_NODE"        == true ]] && item "Node.js LTS (via NodeSource apt repo)"
    [[ "$NEED_MUSL"        == true ]] && item "musl-tools (apt)"
    [[ "$NEED_UV"          == true ]] && item "uv (Python toolchain runner, to ~/.local/bin)"
    [[ "$NEED_RUST"        == true ]] && item "Rust toolchain via rustup (to ~/.cargo)"
    [[ "$NEED_MUSL_TARGET" == true ]] && item "Rust target: x86_64-unknown-linux-musl"
    [[ "$NEED_APT"         == true ]] && item "  (apt installs require sudo)"
    printf '\n'
    printf '\033[0;37mAlso: clone/update %s → %s, then build (~2-5 min) and start on port %s.\033[0m\n' \
        "$REPO_URL" "$BUILD_DIR" "$PORT"
    printf '\n'
    read -r -p "Continue? [y/N] " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || { printf 'Aborted.\n'; exit 0; }
fi

printf '\n'

# ── phase 3: install ──────────────────────────────────────────────────────────

if [[ "$NEED_NODE" == true ]]; then
    info "Adding NodeSource LTS repo"
    curl -fsSL https://deb.nodesource.com/setup_lts.x | run sudo -E bash -
fi

if [[ "$NEED_APT" == true ]]; then
    APT_PKGS=()
    [[ "$NEED_MUSL" == true ]] && APT_PKGS+=(musl-tools)
    [[ "$NEED_NODE" == true ]] && APT_PKGS+=(nodejs)
    info "apt install: ${APT_PKGS[*]}"
    run sudo apt-get install -y "${APT_PKGS[@]}"
    ok "System packages installed"
fi

if [[ "$NEED_UV" == true ]]; then
    info "Installing uv"
    run curl -LsSf https://astral.sh/uv/install.sh | sh
    export PATH="$HOME/.local/bin:$PATH"
fi
export PATH="$HOME/.local/bin:$PATH"
ok "uv $(uv --version)"

if [[ "$NEED_RUST" == true ]]; then
    info "Installing Rust toolchain"
    run curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
fi
# shellcheck source=/dev/null
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
ok "Rust $(rustc --version)"

if [[ "$NEED_MUSL_TARGET" == true ]]; then
    info "Adding musl Rust target"
    run rustup target add x86_64-unknown-linux-musl
fi
ok "musl target present"

# ── clone / update repo ───────────────────────────────────────────────────────

info "Fetching phoenix-ide source"
if [[ -d "$BUILD_DIR/.git" ]]; then
    run git -C "$BUILD_DIR" pull --ff-only --depth 1
else
    run git clone --depth 1 "$REPO_URL" "$BUILD_DIR"
fi
ok "Repo at $(git -C "$BUILD_DIR" rev-parse --short HEAD)"

# ── write DD auth config ──────────────────────────────────────────────────────

info "Writing auth config"
ENV_FILE="$BUILD_DIR/.phoenix-ide.env"
cat > "$ENV_FILE" <<EOF
LLM_API_KEY_HELPER=$LLM_API_KEY_HELPER
LLM_API_KEY_HELPER_TTL_MS=$LLM_API_KEY_HELPER_TTL_MS
EOF
ok "Auth config written to $ENV_FILE"

# ── deploy ────────────────────────────────────────────────────────────────────

info "Building and deploying (2-5 min on first run)"
cd "$BUILD_DIR"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
run ./dev.py prod deploy

# ── done ─────────────────────────────────────────────────────────────────────

printf '\n'
printf '\033[1;32m Phoenix IDE is running at http://localhost:%s\033[0m\n' "$PORT"
printf '\033[0;90m   Logs:   ~/.phoenix-ide/prod.log\033[0m\n'
printf '\033[0;90m   Stop:   cd %s && ./dev.py prod stop\033[0m\n' "$BUILD_DIR"
printf '\033[0;90m   Update: curl -fsSL <this-url> | bash\033[0m\n'
printf '\n'
