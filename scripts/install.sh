#!/usr/bin/env bash
# Phoenix IDE — DD-internal install script
#
# Usage:
#   curl -fsSL https://sopell3.example/phoenix-ide/install.sh | bash
#
# What this does:
#   1. Installs system deps (musl-tools, nodejs) via apt if missing
#   2. Installs uv (Python toolchain runner) if missing
#   3. Installs rustup/cargo if missing, adds musl target
#   4. Shallow-clones the repo to ~/.phoenix-ide-build/ (or pulls if already cloned)
#   5. Writes ~/.phoenix-ide-build/.phoenix-ide.env with DD auth config
#   6. Runs ./dev.py prod deploy (builds + starts daemon on port 8031)
#
# Re-running this script is safe — it updates the clone and redeploys.
#
# Requirements: Linux x86_64, sudo access (for apt installs on first run only)

set -euo pipefail

REPO_URL="git@github.com:scottopell/phoenix-ide.git"
BUILD_DIR="$HOME/.phoenix-ide-build"
PORT=8031

# DD auth config — uv-based dd-internal-authentication helper
LLM_API_KEY_HELPER="uv run --with 'dd-internal-authentication @ https://binaries.ddbuild.io/dd-source/python/dd_internal_authentication-1.8.0-py2.py3-none-any.whl' python3 -c 'import os; os.environ.setdefault(\"DD_TRACE_ENABLED\",\"false\"); os.environ.setdefault(\"DD_TRACE_STARTUP_LOGS\",\"false\"); from dd_internal_authentication.client import JWTDDToolAuthClientTokenManager as M; print(M.instance(name=\"rapid-ai-platform\",datacenter=\"us1.ddbuild.io\").get_token(\"rapid-ai-platform\"))'"
# 2-hour credential TTL
LLM_API_KEY_HELPER_TTL_MS=7200000

# ── helpers ──────────────────────────────────────────────────────────────────

info()  { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
ok()    { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m  ! %s\033[0m\n' "$*"; }
die()   { printf '\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

need_cmd() {
    command -v "$1" &>/dev/null || die "Required command not found: $1"
}

# ── os check ─────────────────────────────────────────────────────────────────

[[ "$(uname -s)" == "Linux" ]]  || die "Linux only (got $(uname -s))"
[[ "$(uname -m)" == "x86_64" ]] || die "x86_64 only (got $(uname -m))"

need_cmd git
need_cmd curl

# ── system packages ───────────────────────────────────────────────────────────

info "Checking system packages"

MISSING_PKGS=()

if ! command -v x86_64-linux-musl-gcc &>/dev/null; then
    MISSING_PKGS+=(musl-tools)
fi

if ! command -v node &>/dev/null; then
    MISSING_PKGS+=(nodejs npm)
fi

if [[ ${#MISSING_PKGS[@]} -gt 0 ]]; then
    info "Installing: ${MISSING_PKGS[*]}"
    need_cmd apt-get
    # Add NodeSource LTS repo if nodejs is missing
    if ! command -v node &>/dev/null; then
        curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash - >/dev/null 2>&1
    fi
    sudo apt-get install -y "${MISSING_PKGS[@]}" >/dev/null 2>&1
    ok "System packages installed"
else
    ok "System packages present"
fi

# ── uv ───────────────────────────────────────────────────────────────────────

info "Checking uv"
if ! command -v uv &>/dev/null; then
    info "Installing uv"
    curl -LsSf https://astral.sh/uv/install.sh | sh
    # uv installs to ~/.local/bin — make sure it's on PATH for this session
    export PATH="$HOME/.local/bin:$PATH"
    ok "uv installed"
else
    ok "uv present ($(uv --version))"
fi

# ── rust / cargo ──────────────────────────────────────────────────────────────

info "Checking Rust toolchain"
if ! command -v cargo &>/dev/null; then
    info "Installing rustup"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
    # Source for this session
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
    ok "Rust installed ($(rustc --version))"
else
    # Source cargo env in case this shell doesn't have it yet
    # shellcheck source=/dev/null
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
    ok "Rust present ($(rustc --version))"
fi

info "Checking musl target"
if ! rustup target list --installed | grep -q x86_64-unknown-linux-musl; then
    rustup target add x86_64-unknown-linux-musl
    ok "musl target added"
else
    ok "musl target present"
fi

# ── clone / update repo ───────────────────────────────────────────────────────

info "Fetching phoenix-ide source"
if [[ -d "$BUILD_DIR/.git" ]]; then
    git -C "$BUILD_DIR" pull --ff-only --depth 1
    ok "Repo updated ($(git -C "$BUILD_DIR" rev-parse --short HEAD))"
else
    git clone --depth 1 "$REPO_URL" "$BUILD_DIR"
    ok "Repo cloned ($(git -C "$BUILD_DIR" rev-parse --short HEAD))"
fi

# ── write DD auth config ──────────────────────────────────────────────────────

info "Writing auth config"
ENV_FILE="$BUILD_DIR/.phoenix-ide.env"
cat > "$ENV_FILE" <<EOF
LLM_API_KEY_HELPER=$LLM_API_KEY_HELPER
LLM_API_KEY_HELPER_TTL_MS=$LLM_API_KEY_HELPER_TTL_MS
EOF
ok "Auth config written to $ENV_FILE"

# ── deploy ────────────────────────────────────────────────────────────────────

info "Building and deploying (this takes ~60s on first run)"
cd "$BUILD_DIR"
# Ensure cargo + uv are on PATH inside this subshell
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
./dev.py prod deploy

# ── done ─────────────────────────────────────────────────────────────────────

printf '\n'
printf '\033[1;32m Phoenix IDE is running at http://localhost:%s\033[0m\n' "$PORT"
printf '\033[0;90m   Logs:   ~/.phoenix-ide/prod.log\033[0m\n'
printf '\033[0;90m   Stop:   cd %s && ./dev.py prod stop\033[0m\n' "$BUILD_DIR"
printf '\033[0;90m   Update: curl -fsSL <this-url> | bash\033[0m\n'
printf '\n'
