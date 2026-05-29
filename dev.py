#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "taskmd>=1.0,<2",
# ]
# ///
"""Development tasks for phoenix-ide."""

import argparse
import dataclasses
import datetime
import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path


ROOT = Path(__file__).parent.resolve()

UI_DIR = ROOT / "ui"
PHOENIX_PID_FILE = ROOT / ".phoenix.pid"
VITE_PID_FILE = ROOT / ".vite.pid"
VITE_PROXY_FILE = ROOT / ".vite.proxy"
LOG_FILE = ROOT / "phoenix.log"

# ANSI/terminal control sequences: CSI escapes (colour, cursor) plus the
# SO/SI shift bytes rustfmt emits around its diff colours. Used to scrub
# captured subprocess output before it is buffered and reprinted.
_CONTROL_SEQ_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|[\x0e\x0f]")


def _node_env() -> dict:
    """Return an env dict with the correct Node.js binary prepended to PATH.

    Reads the `.node-version` file at the repo root, then searches
    `~/.local/share/node`, `~/node`, and `/usr/local` for a matching
    venv-style installation.  Falls back to the ambient PATH if no match
    is found (safe: the check will simply use whatever `node` is on PATH).
    """
    env = os.environ.copy()
    node_version_file = ROOT / ".node-version"
    if not node_version_file.exists():
        return env
    requested = node_version_file.read_text().strip()  # e.g. "22" or "22.14"
    major = requested.split(".")[0]
    candidates = [
        Path.home() / "node",
        Path.home() / ".local" / "share" / "node",
        Path("/usr/local"),
    ]
    for base in candidates:
        node_bin = base / "bin" / "node"
        if node_bin.exists():
            try:
                import subprocess as _sp
                ver_out = _sp.check_output([str(node_bin), "--version"],
                                           text=True).strip()  # e.g. "v24.1.0"
                found_major = ver_out.lstrip("v").split(".")[0]
                # Accept the candidate if it meets or exceeds the requested major
                if int(found_major) >= int(major):
                    env["PATH"] = str(base / "bin") + ":" + env.get("PATH", "")
                    return env
            except Exception:
                continue
    return env


_NODE_ENV: dict | None = None


def node_env() -> dict:
    """Cached result of _node_env()."""
    global _NODE_ENV
    if _NODE_ENV is None:
        _NODE_ENV = _node_env()
    return _NODE_ENV


# Earlier corepack versions reject pnpm 11.x's signing keys with
# "Cannot find matching keyid" (key rotation). 0.30.x ships keys that
# cover pnpm 11.x; older releases do not.
_MIN_COREPACK = (0, 30)
_PNPM_READY: str | None = None


def _read_pnpm_pin() -> str:
    """Return the pnpm version pinned in ui/package.json#packageManager."""
    pkg = json.loads((UI_DIR / "package.json").read_text())
    pm = pkg.get("packageManager", "")
    if not pm.startswith("pnpm@"):
        raise SystemExit(
            f"ui/package.json#packageManager is '{pm}', expected 'pnpm@X.Y.Z'.\n"
            "This repo uses pnpm via Corepack — fix the packageManager field."
        )
    # Format may include an optional "+sha512-..." integrity hash suffix.
    return pm[len("pnpm@"):].split("+", 1)[0]


def ensure_corepack_pnpm() -> None:
    """Pin and activate the project's pnpm version via Corepack.

    Single source of truth: `ui/package.json#packageManager`.
    Idempotent and cheap to call repeatedly — the verified pin is cached.
    Hard-fails (no fallback to system pnpm) if Corepack is missing, too old,
    or the pnpm version on PATH does not match the pin. The whole point of
    going through Corepack is that there is exactly one pnpm.
    """
    global _PNPM_READY
    pinned = _read_pnpm_pin()
    if _PNPM_READY == pinned:
        return

    env = node_env()

    min_version = ".".join(str(x) for x in _MIN_COREPACK)
    upgrade_hint = (
        f"Upgrade corepack to >= {min_version}. Pick whichever fits your setup:\n"
        "  - Volta-managed Node:   volta install corepack\n"
        "  - nvm/asdf/system Node: npm i -g corepack@latest\n"
        "  - Homebrew Node:        brew upgrade node"
    )

    try:
        cp_out = subprocess.run(
            ["corepack", "--version"],
            capture_output=True, text=True, env=env, check=True,
        ).stdout.strip()
    except FileNotFoundError:
        raise SystemExit("corepack not found on PATH.\n" + upgrade_hint)
    except subprocess.CalledProcessError as e:
        raise SystemExit(f"`corepack --version` failed:\n{e.stderr}\n" + upgrade_hint)

    cp_version = tuple(int(x) for x in cp_out.split(".") if x.isdigit())
    if cp_version < _MIN_COREPACK:
        raise SystemExit(
            f"corepack {cp_out} is too old (minimum: {min_version}).\n"
            "Older versions reject pnpm's current signing keys.\n" + upgrade_hint
        )

    subprocess.run(["corepack", "enable"], check=True, env=env, capture_output=True)
    subprocess.run(
        ["corepack", "prepare", f"pnpm@{pinned}", "--activate"],
        check=True, env=env, capture_output=True,
    )

    try:
        actual = subprocess.run(
            ["pnpm", "--version"],
            capture_output=True, text=True, env=env, check=True,
        ).stdout.strip()
    except FileNotFoundError:
        raise SystemExit(
            "pnpm not found on PATH after `corepack enable`.\n"
            "Corepack shims should land on PATH automatically; check that "
            "your node version manager exposes them."
        )
    except subprocess.CalledProcessError as e:
        raise SystemExit(f"`pnpm --version` failed:\n{e.stderr or e.stdout}")

    if actual != pinned:
        raise SystemExit(
            f"pnpm version mismatch: expected {pinned}, got {actual}.\n"
            f"Run `corepack prepare pnpm@{pinned} --activate`. "
            f"If a different pnpm wins on PATH, adjust your node version "
            f"manager so the corepack shim is found first."
        )

    _PNPM_READY = pinned

# Production paths
PROD_SERVICE_NAME = "phoenix-ide"
PROD_INSTALL_DIR = Path("/opt/phoenix-ide")
PROD_DB_PATH = Path.home() / ".phoenix-ide" / "prod.db"
PROD_ENV_FILE = Path("/etc/phoenix-ide/phoenix.env")
PROD_PORT = 8031

# launchd (native macOS) configuration
LAUNCHD_LABEL = "com.phoenix-ide.server"
LAUNCHD_PLIST_PATH = Path.home() / "Library" / "LaunchAgents" / f"{LAUNCHD_LABEL}.plist"
LAUNCHD_INSTALL_DIR = Path.home() / ".phoenix-ide"
LAUNCHD_LOG_PATH = Path.home() / ".phoenix-ide" / "prod.log"
PROD_SHA_PATH = Path.home() / ".phoenix-ide" / "deployed.sha"
NEWSYSLOG_CONF_PATH = Path("/etc/newsyslog.d") / f"{LAUNCHD_LABEL}.conf"

# exe.dev LLM gateway configuration
EXE_DEV_CONFIG = Path("/exe.dev/shelley.json")
DEFAULT_GATEWAY = "http://169.254.169.254/gateway/llm"
LOCAL_AI_PROXY = "http://127.0.0.1:8462"

# Dev ports: 8030-8050 range, offset by worktree path hash to avoid collisions.
# 8031 is reserved for prod. Dev uses two blocks offset by worktree hash:
#   Phoenix API: 8032-8040  (PORT_RANGE=9, offsets 0-8)
#   Vite:        8041-8049  (PORT_RANGE=9, offsets 0-8)
BASE_PHOENIX_PORT = 8032
BASE_VITE_PORT = 8041
PORT_RANGE = 9
DEV_PORT_MIN = 8030
DEV_PORT_MAX = 8050

# Database directory
DB_DIR = Path.home() / ".phoenix-ide"
TLS_CA_DIR = DB_DIR / "tls"
TLS_BUNDLE_DIR = DB_DIR / "tls-bundles"
TLS_INSTALL_DIR = DB_DIR / "tls"


def _gateway_is_reachable(url: str) -> bool:
    """Probe a gateway with a quick HTTP request. Any response means it's up."""
    import urllib.request
    import urllib.error
    # Prefer /_proxy/status (ai-proxy health endpoint) — responds instantly without
    # touching ddtool or upstream. Fall back to bare URL for other gateway types.
    probe_url = f"{url.rstrip('/')}/_proxy/status"
    for candidate in (probe_url, url):
        try:
            urllib.request.urlopen(candidate, timeout=0.5)
            return True
        except urllib.error.HTTPError:
            return True  # 404, 405, etc. — server is listening
        except Exception:
            continue
    return False


def _discover_gateway_candidates() -> list[str]:
    """Build an ordered list of gateway URLs to try."""
    candidates = [LOCAL_AI_PROXY]
    if EXE_DEV_CONFIG.exists():
        try:
            config = json.loads(EXE_DEV_CONFIG.read_text())
            if gw := config.get("llm_gateway"):
                candidates.append(gw)
        except (json.JSONDecodeError, KeyError):
            pass
    candidates.append(DEFAULT_GATEWAY)
    return candidates


def write_deployed_sha():
    """Write the current HEAD SHA to ~/.phoenix-ide/deployed.sha."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT, capture_output=True, text=True,
    )
    sha = result.stdout.strip()
    if sha:
        PROD_SHA_PATH.parent.mkdir(parents=True, exist_ok=True)
        PROD_SHA_PATH.write_text(sha + "\n")


def read_deployed_sha() -> str | None:
    """Read the deployed SHA, return short hash with staleness hint or None."""
    if not PROD_SHA_PATH.exists():
        return None
    deployed = PROD_SHA_PATH.read_text().strip()
    if not deployed:
        return None
    short = deployed[:7]
    current = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout.strip()
    if current and current != deployed:
        return f"{short} (HEAD is now {current[:7]})"
    return f"{short} (current)"


def get_llm_gateway() -> str | None:
    """Get LLM gateway URL from env or by probing candidates. Returns None if none reachable."""
    if val := os.environ.get("LLM_GATEWAY"):
        return val
    for url in _discover_gateway_candidates():
        if _gateway_is_reachable(url):
            return url
    return None


def get_worktree_hash() -> str:
    """Get a short hash of the worktree path for unique identification."""
    return hashlib.md5(str(ROOT).encode()).hexdigest()[:8]


def get_port_offset() -> int:
    """Get deterministic port offset from worktree path hash."""
    return int(get_worktree_hash()[:4], 16) % PORT_RANGE


def get_default_ports() -> tuple[int, int]:
    """Get default Phoenix and Vite ports for this worktree."""
    offset = get_port_offset()
    phoenix = BASE_PHOENIX_PORT + offset
    vite = BASE_VITE_PORT + offset
    for name, port in [("Phoenix", phoenix), ("Vite", vite)]:
        if port == PROD_PORT:
            print(f"ERROR: {name} port {port} collides with prod port {PROD_PORT}.", file=sys.stderr)
            print(f"  Worktree hash produced offset {offset}. Use --port to override.", file=sys.stderr)
            sys.exit(1)
        if not (DEV_PORT_MIN <= port <= DEV_PORT_MAX):
            print(f"ERROR: {name} port {port} outside allowed range {DEV_PORT_MIN}-{DEV_PORT_MAX}.", file=sys.stderr)
            print(f"  Worktree hash produced offset {offset}. Use --port to override.", file=sys.stderr)
            sys.exit(1)
    return (phoenix, vite)


def get_db_path() -> Path:
    """Get database path unique to this worktree."""
    worktree_hash = get_worktree_hash()
    return DB_DIR / f"phoenix-{worktree_hash}.db"


def get_lock_path() -> Path:
    """Get lock file path for this worktree's database."""
    worktree_hash = get_worktree_hash()
    return DB_DIR / f"phoenix-{worktree_hash}.lock"


class DatabaseLock:
    """Context manager for exclusive database access."""
    
    def __init__(self):
        self.lock_path = get_lock_path()
        self.lock_file = None
        self.fd = None
    
    def acquire(self) -> bool:
        """Acquire exclusive lock. Returns False if already locked."""
        # Ensure directory exists
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        
        # Open lock file
        self.fd = os.open(str(self.lock_path), os.O_RDWR | os.O_CREAT)
        
        try:
            # Try to acquire exclusive lock (non-blocking)
            fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            # Write PID to lock file for debugging
            os.ftruncate(self.fd, 0)
            os.write(self.fd, f"{os.getpid()}\n".encode())
            return True
        except OSError:
            # Lock is held by another process
            os.close(self.fd)
            self.fd = None
            return False
    
    def release(self):
        """Release the lock."""
        if self.fd is not None:
            fcntl.flock(self.fd, fcntl.LOCK_UN)
            os.close(self.fd)
            self.fd = None
            # Clean up lock file
            try:
                self.lock_path.unlink()
            except OSError:
                pass
    
    def __enter__(self):
        if not self.acquire():
            raise RuntimeError(
                f"Database is locked by another process.\n"
                f"Lock file: {self.lock_path}\n"
                f"Run './dev.py down' in the other instance first."
            )
        return self
    
    def __exit__(self, *args):
        self.release()


# Global lock instance - held while Phoenix is running
_db_lock: DatabaseLock | None = None


def is_process_running(pid: int) -> bool:
    """Check if a process is running."""
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def get_pid(pid_file: Path) -> int | None:
    """Get PID from file if process is still running."""
    if not pid_file.exists():
        return None
    pid = int(pid_file.read_text().strip())
    if is_process_running(pid):
        return pid
    pid_file.unlink()  # Clean up stale PID file
    return None


def stop_process(pid_file: Path, name: str) -> bool:
    """Stop a process by PID file. Returns True if was running."""
    global _db_lock
    
    pid = get_pid(pid_file)
    if pid is None:
        return False
    try:
        # Kill the entire process group to catch child workers (e.g., Vite
        # spawns node child processes that survive if only the parent is killed)
        try:
            pgid = os.getpgid(pid)
            os.killpg(pgid, signal.SIGTERM)
        except (OSError, ProcessLookupError):
            os.kill(pid, signal.SIGTERM)
        # Wait briefly for graceful shutdown
        for _ in range(10):
            if not is_process_running(pid):
                break
            time.sleep(0.1)
        else:
            try:
                pgid = os.getpgid(pid)
                os.killpg(pgid, signal.SIGKILL)
            except (OSError, ProcessLookupError):
                os.kill(pid, signal.SIGKILL)
        print(f"Stopped {name} (PID {pid})")
    except OSError as e:
        print(f"Could not stop {name}: {e}")
    finally:
        if pid_file.exists():
            pid_file.unlink()
        if name == "Vite" and VITE_PROXY_FILE.exists():
            VITE_PROXY_FILE.unlink()
        # Release database lock if stopping Phoenix
        if name == "Phoenix" and _db_lock is not None:
            _db_lock.release()
            _db_lock = None
    return True


def ensure_ui_deps():
    """Ensure UI dependencies are installed."""
    ensure_corepack_pnpm()
    # `node_modules/.modules.yaml` is pnpm's success marker; checking its
    # presence (rather than just `node_modules/`) avoids treating a phantom
    # empty directory left behind by a failed install as already-installed.
    if not (UI_DIR / "node_modules" / ".modules.yaml").exists():
        print("Installing UI dependencies...")
        subprocess.run(["pnpm", "install"], cwd=UI_DIR, check=True, env=node_env())
    # rust-embed in crates/phoenix-ide/src/api/assets.rs reads ui/dist/ at
    # proc-macro expansion. The folder must exist for cargo to build, even
    # if empty — runtime serve_static() falls back to filesystem reads.
    (UI_DIR / "dist").mkdir(exist_ok=True)


def build_rust(release: bool = True):
    """Build the Rust backend."""
    # RustEmbed requires ui/dist to exist at compile time, even if empty.
    # In dev mode Vite serves assets, so an empty dir is fine.
    (UI_DIR / "dist").mkdir(exist_ok=True)

    args = ["cargo", "build"]
    if release:
        args.append("--release")
    print("Building Rust backend...")
    subprocess.run(args, check=True, cwd=ROOT)


def tls_enabled_from_env(env: dict[str, str]) -> bool:
    """Return whether the supplied Phoenix env config enables HTTPS."""
    mode = env.get("PHOENIX_TLS", "").strip().lower()
    has_manual_paths = bool(env.get("PHOENIX_TLS_CERT_PATH") and env.get("PHOENIX_TLS_KEY_PATH"))
    return has_manual_paths or mode in {"1", "true", "on", "auto", "manual"}


def maybe_enable_auto_tls(env: dict[str, str], tls: bool) -> bool:
    """Apply the dev HTTPS shortcut without overriding explicit TLS config."""
    if tls and not tls_enabled_from_env(env):
        env["PHOENIX_TLS"] = "auto"
    return tls_enabled_from_env(env)


def _probe_phoenix_scheme(port: int) -> str | None:
    import ssl
    import urllib.request

    for scheme in ("https", "http"):
        context = ssl._create_unverified_context() if scheme == "https" else None
        try:
            with urllib.request.urlopen(
                f"{scheme}://localhost:{port}/version",
                timeout=1,
                context=context,
            ):
                return scheme
        except Exception:
            continue
    return None


def start_phoenix(port: int, release: bool = True, tls: bool = False) -> bool:
    """Start the Phoenix server."""
    global _db_lock

    db_path = get_db_path()
    env = os.environ.copy()
    # Load .phoenix-ide.env overrides (LLM_API_KEY_HELPER, base URLs, etc.)
    env_file = _load_env_file(env)
    # Dev-only overrides on top of the prod env. Lets dev disable
    # PHOENIX_PASSWORD (or override anything else) without polluting the
    # prod env file used by `./dev.py prod deploy`.
    dev_env_file = _load_env_file(env, ".phoenix-ide.dev.env")
    # Auto-detect gateway only if .phoenix-ide.env didn't provide LLM config
    if not env.get("LLM_API_KEY_HELPER") and not env.get("LLM_GATEWAY"):
        if gateway := get_llm_gateway():
            env["LLM_GATEWAY"] = gateway
    env["PHOENIX_PORT"] = str(port)
    env["PHOENIX_DB_PATH"] = str(db_path)
    phoenix_tls = maybe_enable_auto_tls(env, tls)

    if get_pid(PHOENIX_PID_FILE):
        desired_scheme = "https" if phoenix_tls else "http"
        current_scheme = _probe_phoenix_scheme(port)
        if current_scheme == desired_scheme or (current_scheme is None and desired_scheme == "http"):
            print("Phoenix server already running")
            return phoenix_tls
        print("Restarting Phoenix server for TLS mode change")
        stop_process(PHOENIX_PID_FILE, "Phoenix")
        time.sleep(0.5)

    binary = ROOT / "target" / ("release" if release else "debug") / "phoenix_ide"
    if not binary.exists():
        print(f"Binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    # Acquire database lock
    _db_lock = DatabaseLock()
    if not _db_lock.acquire():
        print(f"ERROR: Database is locked by another process.", file=sys.stderr)
        print(f"  Lock file: {get_lock_path()}", file=sys.stderr)
        print(f"  Run './dev.py down' in the other instance first.", file=sys.stderr)
        sys.exit(1)

    if env_file:
        print(f"  Loaded env from {env_file}")
    if dev_env_file:
        print(f"  Loaded dev overrides from {dev_env_file}")
    # Default to debug logging in dev, can be overridden via RUST_LOG env var
    if "RUST_LOG" not in env:
        env["RUST_LOG"] = "phoenix_ide=debug,tower_http=debug"

    with open(LOG_FILE, "w") as log:
        proc = subprocess.Popen(
            [str(binary)],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        PHOENIX_PID_FILE.write_text(str(proc.pid))

    # Verify it started
    time.sleep(0.5)
    if not is_process_running(proc.pid):
        print("Phoenix failed to start. Check phoenix.log", file=sys.stderr)
        PHOENIX_PID_FILE.unlink()
        _db_lock.release()
        _db_lock = None
        sys.exit(1)

    print(f"Started Phoenix server (PID {proc.pid}, port {port})")
    print(f"  Database: {db_path}")
    if phoenix_tls:
        tls_dir = env.get("PHOENIX_TLS_DIR", str(db_path.parent / "tls"))
        print(f"  TLS: auto-managed local CA ({tls_dir})")
    return phoenix_tls


def start_vite(port: int, phoenix_port: int, phoenix_tls: bool = False):
    """Start the Vite dev server."""
    scheme = "https" if phoenix_tls else "http"
    desired_proxy = f"{scheme}://localhost:{phoenix_port}"
    if get_pid(VITE_PID_FILE):
        current_proxy = VITE_PROXY_FILE.read_text().strip() if VITE_PROXY_FILE.exists() else ""
        if current_proxy == desired_proxy:
            print("Vite dev server already running")
            return
        print("Restarting Vite dev server for API proxy change")
        stop_process(VITE_PID_FILE, "Vite")

    ensure_ui_deps()

    env = node_env()
    # Pass Phoenix port to Vite for proxy configuration
    env["VITE_API_PORT"] = str(phoenix_port)
    if phoenix_tls:
        env["VITE_API_SCHEME"] = "https"
        env.setdefault("VITE_API_PROXY_SECURE", "false")
    
    # Start Vite in background (bind to 0.0.0.0 for external access).
    # pnpm passes args after the script name directly — no `--` separator.
    proc = subprocess.Popen(
        ["pnpm", "run", "dev", "--port", str(port), "--host", "0.0.0.0"],
        cwd=UI_DIR,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    VITE_PID_FILE.write_text(str(proc.pid))

    time.sleep(1)
    if not is_process_running(proc.pid):
        print("Vite failed to start", file=sys.stderr)
        VITE_PID_FILE.unlink()
        sys.exit(1)

    print(f"Started Vite dev server (PID {proc.pid}, port {port})")
    VITE_PROXY_FILE.write_text(desired_proxy + "\n")
    print(f"  Proxying /api to Phoenix at {desired_proxy}")


# =============================================================================
# Commands
# =============================================================================

def cmd_up(
    phoenix_port: int | None = None,
    vite_port: int | None = None,
    no_seed: bool = False,
    tls: bool = False,
):
    """Build and start Phoenix + Vite dev servers."""
    default_phoenix, default_vite = get_default_ports()
    phoenix_port = phoenix_port or default_phoenix
    vite_port = vite_port or default_vite
    
    print(f"Worktree: {ROOT}")
    print(f"  Hash: {get_worktree_hash()}, Port offset: +{get_port_offset()}")
    print()
    
    build_rust(release=True)

    # Seed BEFORE Phoenix starts so the seeder runs offline against the DB
    # (no contention with a live runtime that owns the same conversation rows).
    # On a fresh DB the seeder bootstraps the schema itself; subsequent ups
    # just see an idempotent no-op.
    if not no_seed:
        cmd_seed(quiet_if_populated=True)
        print()

    phoenix_tls = start_phoenix(port=phoenix_port, tls=tls)
    start_vite(port=vite_port, phoenix_port=phoenix_port, phoenix_tls=phoenix_tls)
    api_scheme = "https" if phoenix_tls else "http"
    print()
    print(f"Ready! UI: http://localhost:{vite_port}")
    print(f"        API: {api_scheme}://localhost:{phoenix_port}")
    print(f"        Log: {LOG_FILE}")


# ---------------------------------------------------------------------------
# Seed (offline)
# ---------------------------------------------------------------------------
#
# The seed runs OFFLINE: writes directly to SQLite, requires Phoenix NOT to
# be running against this DB. Earlier versions ran against a live Phoenix
# via HTTP and used a direct SQL UPDATE to force ContextExhausted state on
# rows the runtime had already loaded into memory; the runtime's next
# checkpoint flushed its in-memory state back over the seed write, leaving
# `/continue` rejecting the parent with `parent_not_context_exhausted`.
#
# Stop the runtime, write, restart. That's the rule: the system is the
# authority for the data when it's running, so the seeder can't share
# that authority. Either Phoenix is running and the seed defers to the
# API (which has no API for forcing ContextExhausted -- by design), or
# Phoenix is down and the seed has unambiguous ownership of the DB.
#
# `./dev.py up` calls this BEFORE starting Phoenix; `./dev.py seed`
# refuses if a Phoenix PID file is alive on the worktree.
#
# Representative conversations:
#   - Direct mode (no git required): standalones + 3-member and 2-member chains
#   - Explore mode (managed, read-only) chain
#   - Branch mode: NOT seeded -- requires real `git worktree add` plumbing.
#     Use the UI's "Branch mode" picker for those scenarios.
#   - Work mode: NOT seeded -- only reachable via propose_task / approval
#     (human-in-the-loop by design; cannot be faked without a real commit).
#
# Idempotent: if any active conversations exist the seeder skips.

_SEED_DIRECT_STANDALONES = [
    "Review the recent changes to the authentication middleware and identify any security concerns",
    "Update the README with installation instructions for the new Docker-based dev setup",
    "Investigate why the integration test test_user_login is flaky in CI — fails ~30% of runs",
]

_SEED_CHAIN_3_TEXT = (
    "Refactor the database connection pool — current impl leaks connections under sustained load"
)
_SEED_CHAIN_2_TEXT = "Debug memory leak in the background worker service"
_SEED_EXPLORE_TEXT = "Analyze the project structure and summarise the key architectural components"

_SEED_CONTEXT_SUMMARY = "Context limit reached after extended session"

# Minimal schema bootstrap for the seeder. Mirrors the idempotent CREATE/ALTER
# sequence Phoenix runs at startup (see crates/phoenix-ide/src/db/schema.rs and
# crates/phoenix-ide/src/db.rs::run_migrations). When Phoenix later starts it
# re-runs everything; CREATE TABLE IF NOT EXISTS and ADD COLUMN are no-ops on
# an already-bootstrapped DB. If a column the seeder INSERTs into is later
# renamed/removed, the seeder fails loud with `no such column` on the next run
# -- that's the canary, not a bug.
_SEED_SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    slug TEXT UNIQUE,
    cwd TEXT NOT NULL,
    parent_conversation_id TEXT,
    user_initiated BOOLEAN NOT NULL,
    state TEXT NOT NULL DEFAULT '{"type":"idle"}',
    state_data TEXT,
    state_updated_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    archived BOOLEAN NOT NULL DEFAULT 0,
    model TEXT,
    FOREIGN KEY (parent_conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS messages (
    message_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    sequence_id INTEGER NOT NULL,
    message_type TEXT NOT NULL,
    content TEXT NOT NULL,
    display_data TEXT,
    usage_data TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    canonical_path TEXT UNIQUE NOT NULL,
    main_ref TEXT NOT NULL DEFAULT 'main',
    created_at TEXT NOT NULL
);
"""

# Versioned migrations from crates/phoenix-ide/src/db/migrations.rs that the
# seeder pre-applies (because the seeder INSERTs into the columns they create).
# Pre-stamping the `_migrations` row prevents Phoenix's `run_pending_migrations`
# from re-applying them at next startup, which would fail with "duplicate column
# name" since the column is already there.
#
# Only stamp a versioned migration when the seeder reproduces ALL of its
# side-effects -- not just the column-add. (Migration 5, for example, adds
# `conversations.chain_name` AND creates the `chain_qa` table; pre-stamping
# it would skip the table creation and Phoenix would crash on first
# `chain_qa` query. So we don't pre-add `chain_name` either; let migration 5
# run normally on first startup.)
_SEED_PRESTAMPED_MIGRATIONS = [
    (3, "add_continued_in_conv_id_column"),
]

# Each ALTER TABLE may already be applied by a prior Phoenix startup. We catch
# OperationalError ("duplicate column name") and continue -- same pattern as the
# Rust side (`let _ = sqlx::raw_sql(...).await;`).
#
# Only list ALTERs that correspond to UNCONDITIONAL idempotent ALTERs in
# Phoenix's `run_migrations` (the ones using `let _ = sqlx::raw_sql(...).await;`)
# OR to versioned migrations whose ENTIRE effect we replicate here (currently
# only migration 3, the single-column add for `continued_in_conv_id`). Adding
# columns from versioned migrations whose other side-effects we don't reproduce
# is unsafe -- see `_SEED_PRESTAMPED_MIGRATIONS`.
_SEED_SCHEMA_ALTERS = [
    "ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects(id)",
    "ALTER TABLE conversations ADD COLUMN conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}'",
    "ALTER TABLE conversations ADD COLUMN title TEXT",
    "ALTER TABLE conversations ADD COLUMN desired_base_branch TEXT",
    "ALTER TABLE conversations ADD COLUMN seed_parent_id TEXT",
    "ALTER TABLE conversations ADD COLUMN seed_label TEXT",
    "ALTER TABLE conversations ADD COLUMN continued_in_conv_id TEXT",
    "ALTER TABLE conversations ADD COLUMN steering_queue TEXT NOT NULL DEFAULT '[]'",
]


def _slug_from_text(text: str, max_words: int = 6) -> str:
    """Mirror crates/phoenix-ide/src/api/handlers.rs::slugify_label.

    Lowercase, alphanumerics-only, dash-separated, trimmed. Keep first
    `max_words` to keep the slug compact for UI display.
    """
    out: list[str] = []
    word: list[str] = []
    for ch in text:
        if ch.isascii() and ch.isalnum():
            word.append(ch.lower())
        elif word:
            out.append("".join(word))
            word = []
            if len(out) >= max_words:
                break
    if word and len(out) < max_words:
        out.append("".join(word))
    return "-".join(out)


def _title_from_slug(slug: str) -> str:
    """Mirror crates/phoenix-ide/src/db/schema.rs::title_from_slug."""
    return " ".join(w[:1].upper() + w[1:] for w in slug.split("-") if w)


def cmd_seed(quiet_if_populated: bool = False) -> None:
    """Populate the dev DB with representative conversations.

    Runs OFFLINE -- writes directly to SQLite while Phoenix is not running
    against this worktree's DB. Refuses if a live Phoenix is detected on
    the PID file (its in-memory runtime would clobber seed writes).

    Idempotent: if any active conversations exist the seeder is a no-op.
    """
    import sqlite3
    import uuid as _uuid

    db_path = get_db_path()

    # ----------------------------------------------------- liveness guard

    live_pid = get_pid(PHOENIX_PID_FILE)
    if live_pid is not None:
        print(
            f"✗ Phoenix is running (pid {live_pid}) against this DB.\n"
            f"  The seeder is offline-only — a live runtime would clobber seed\n"
            f"  writes when it next checkpoints in-memory state to disk.\n"
            f"  Run './dev.py down' first, or use './dev.py up' (which\n"
            f"  seeds before starting Phoenix).",
            file=sys.stderr,
        )
        sys.exit(1)

    # ----------------------------------------------------- helpers

    db_path.parent.mkdir(parents=True, exist_ok=True)
    now = datetime.datetime.now(datetime.timezone.utc).isoformat()

    def _ensure_schema(conn: sqlite3.Connection) -> None:
        conn.executescript(_SEED_SCHEMA_SQL)
        for stmt in _SEED_SCHEMA_ALTERS:
            try:
                conn.execute(stmt)
            except sqlite3.OperationalError as e:
                # "duplicate column name" -- column already exists. Anything
                # else is a real schema problem worth surfacing.
                if "duplicate column name" not in str(e):
                    raise
        # Stamp the versioned migrations whose columns we pre-added, so
        # Phoenix's `run_pending_migrations` skips them on next startup
        # (re-applying would fail with "duplicate column name" because the
        # versioned migrations don't swallow errors).
        conn.execute(
            "CREATE TABLE IF NOT EXISTS _migrations ("
            " version INTEGER PRIMARY KEY,"
            " name TEXT NOT NULL,"
            " applied_at TEXT NOT NULL DEFAULT (datetime('now')))"
        )
        for version, name in _SEED_PRESTAMPED_MIGRATIONS:
            conn.execute(
                "INSERT OR IGNORE INTO _migrations (version, name) VALUES (?, ?)",
                (version, name),
            )
        conn.commit()

    def _existing_active_count(conn: sqlite3.Connection) -> int:
        return conn.execute(
            "SELECT COUNT(*) FROM conversations WHERE archived = 0"
        ).fetchone()[0]

    def _find_or_create_project(conn: sqlite3.Connection, canonical_path: str) -> str:
        row = conn.execute(
            "SELECT id FROM projects WHERE canonical_path = ?", (canonical_path,)
        ).fetchone()
        if row is not None:
            return row[0]
        proj_id = str(_uuid.uuid4())
        conn.execute(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at)"
            " VALUES (?, ?, 'main', ?)",
            (proj_id, canonical_path, now),
        )
        return proj_id

    def _insert_conv(
        conn: sqlite3.Connection,
        *,
        text: str,
        conv_mode: dict,
        cwd: str,
        project_id: str | None,
        state: dict | None = None,
    ) -> tuple[str, str]:
        """Insert one conversation row + its single user message. Returns (id, slug)."""
        conv_id = str(_uuid.uuid4())
        base_slug = _slug_from_text(text) or "seed-conv"
        # Random suffix to dodge the slug UNIQUE constraint without a retry loop.
        # (Production code retries on collision; the seed runs once per fresh
        # DB so a single suffix is plenty.)
        slug = f"{base_slug}-{conv_id[:4]}"
        title = _title_from_slug(base_slug)
        state_json = json.dumps(state if state is not None else {"type": "idle"})
        mode_json = json.dumps(conv_mode)
        conn.execute(
            "INSERT INTO conversations ("
            " id, slug, title, cwd, parent_conversation_id, user_initiated,"
            " state, state_updated_at, created_at, updated_at, archived,"
            " model, project_id, conv_mode, desired_base_branch,"
            " seed_parent_id, seed_label"
            ") VALUES (?, ?, ?, ?, NULL, 1, ?, ?, ?, ?, 0, 'mock', ?, ?, NULL, NULL, NULL)",
            (conv_id, slug, title, cwd, state_json, now, now, now, project_id, mode_json),
        )
        # One user message so the conv looks lived-in (message_count > 0).
        msg_id = str(_uuid.uuid4())
        user_content = json.dumps({"text": text, "images": []})
        conn.execute(
            "INSERT INTO messages ("
            " message_id, conversation_id, sequence_id, message_type,"
            " content, created_at"
            ") VALUES (?, ?, 0, 'user', ?, ?)",
            (msg_id, conv_id, user_content, now),
        )
        return conv_id, slug

    def _link_continuation(
        conn: sqlite3.Connection,
        *,
        parent_id: str,
        parent_slug: str,
        chain_index: int,
        conv_mode: dict,
        cwd: str,
        project_id: str | None,
    ) -> tuple[str, str]:
        """Insert a continuation conversation and update parent.continued_in_conv_id.

        Mirrors crates/phoenix-ide/src/db.rs::continue_conversation: the parent
        is forced to ContextExhausted by `_exhaust_state`, then a child row is
        inserted with a sequential `{root_slug}-{N}` slug.
        """
        new_id = str(_uuid.uuid4())
        # `parent_slug` already includes a random suffix from `_insert_conv`
        # for the chain root, so use the chain index against the raw root
        # slug for human-readable continuation slugs (root, root-2, root-3...).
        # Strip the trailing -<hex> suffix added by `_insert_conv` to find the root.
        root_slug = "-".join(parent_slug.split("-")[:-1]) or parent_slug
        new_slug = f"{root_slug}-{chain_index}-{new_id[:4]}"
        new_title = _title_from_slug(root_slug)
        idle_state = json.dumps({"type": "idle"})
        mode_json = json.dumps(conv_mode)
        conn.execute(
            "INSERT INTO conversations ("
            " id, slug, title, cwd, parent_conversation_id, user_initiated,"
            " state, state_updated_at, created_at, updated_at, archived,"
            " model, project_id, conv_mode, desired_base_branch,"
            " seed_parent_id, seed_label, continued_in_conv_id"
            ") VALUES (?, ?, ?, ?, NULL, 1, ?, ?, ?, ?, 0, 'mock', ?, ?, NULL, NULL, NULL, NULL)",
            (new_id, new_slug, new_title, cwd, idle_state, now, now, now, project_id, mode_json),
        )
        # Continuation summary message bridges parent -> child in the UI.
        msg_id = str(_uuid.uuid4())
        cont_content = json.dumps({"summary": _SEED_CONTEXT_SUMMARY})
        conn.execute(
            "INSERT INTO messages ("
            " message_id, conversation_id, sequence_id, message_type,"
            " content, created_at"
            ") VALUES (?, ?, 0, 'continuation', ?, ?)",
            (msg_id, new_id, cont_content, now),
        )
        # Wire parent -> child.
        conn.execute(
            "UPDATE conversations SET continued_in_conv_id = ? WHERE id = ?",
            (new_id, parent_id),
        )
        return new_id, new_slug

    def _exhaust_state(conn: sqlite3.Connection, conv_id: str) -> None:
        state_json = json.dumps(
            {"type": "context_exhausted", "summary": _SEED_CONTEXT_SUMMARY}
        )
        conn.execute(
            "UPDATE conversations SET state = ?, state_updated_at = ?, updated_at = ?"
            " WHERE id = ?",
            (state_json, now, now, conv_id),
        )

    def _perf_text(n_words: int) -> str:
        words = [
            "phoenix", "runtime", "state", "message", "stream", "context",
            "render", "profile", "deterministic", "fixture", "markdown", "token",
            "conversation", "agent", "tool", "result", "baseline", "sample",
        ]
        return " ".join(words[i % len(words)] for i in range(n_words))

    def _ensure_conversation_load_fixture(
        conn: sqlite3.Connection,
        *,
        project_id: str,
        conv_mode: dict,
        cwd: str,
    ) -> bool:
        """Ensure the deterministic large conversation used by perf scenarios exists."""
        slug = "fixture-turn-one"
        existing = conn.execute(
            "SELECT id, archived FROM conversations WHERE slug = ?",
            (slug,),
        ).fetchone()
        if existing is not None:
            conv_id, archived = existing
            message_count = conn.execute(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = ?",
                (conv_id,),
            ).fetchone()[0]
            if archived == 0 and message_count == 47:
                return False
            conn.execute("DELETE FROM messages WHERE conversation_id = ?", (conv_id,))
            conn.execute("DELETE FROM conversations WHERE id = ?", (conv_id,))

        conv_id = str(_uuid.uuid4())
        state_json = json.dumps({"type": "idle"})
        mode_json = json.dumps(conv_mode)
        conn.execute(
            "INSERT INTO conversations ("
            " id, slug, title, cwd, parent_conversation_id, user_initiated,"
            " state, state_updated_at, created_at, updated_at, archived,"
            " model, project_id, conv_mode, desired_base_branch,"
            " seed_parent_id, seed_label"
            ") VALUES (?, ?, ?, ?, NULL, 1, ?, ?, ?, ?, 0, 'mock', ?, ?, NULL, NULL, ?)",
            (
                conv_id,
                slug,
                "Fixture Turn One",
                cwd,
                state_json,
                now,
                now,
                now,
                project_id,
                mode_json,
                "perf:conversation-load",
            ),
        )

        for seq in range(47):
            msg_id = str(_uuid.uuid4())
            if seq % 2 == 0:
                content = {
                    "text": f"Fixture user turn {seq // 2 + 1}: {_perf_text(80)}",
                    "images": [],
                }
                message_type = "user"
            else:
                content = [
                    {
                        "type": "text",
                        "text": (
                            f"Fixture agent turn {seq // 2 + 1}.\n\n"
                            f"{_perf_text(650)}\n\n"
                            "```ts\n"
                            "export function fixtureTurn(input: string): string {\n"
                            "  return input.trim().toUpperCase();\n"
                            "}\n"
                            "```"
                        ),
                    }
                ]
                message_type = "agent"
            conn.execute(
                "INSERT INTO messages ("
                " message_id, conversation_id, sequence_id, message_type,"
                " content, created_at"
                ") VALUES (?, ?, ?, ?, ?, ?)",
                (msg_id, conv_id, seq, message_type, json.dumps(content), now),
            )
        return True

    def _ensure_heavy_prod_shape_fixture(
        conn: sqlite3.Connection,
        *,
        project_id: str,
        conv_mode: dict,
        cwd: str,
    ) -> bool:
        """Ensure a sanitized 484-message fixture matching a real prod shape.

        Shape derived from read-only aggregate inspection of prod conversation
        `check-open-pr-development` (no raw prod text copied):
        18 user messages, 233 agent messages, 233 tool messages; mostly
        agent→tool pairs with a few zero/multi-tool agents and large user/tool
        payload outliers. Used for MessageList virtualization profiling.
        """
        slug = "fixture-heavy-prod-shape"
        expected_count = 484
        existing = conn.execute(
            "SELECT id, archived FROM conversations WHERE slug = ?",
            (slug,),
        ).fetchone()
        if existing is not None:
            conv_id, archived = existing
            message_count = conn.execute(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = ?",
                (conv_id,),
            ).fetchone()[0]
            if archived == 0 and message_count == expected_count:
                return False
            conn.execute("DELETE FROM messages WHERE conversation_id = ?", (conv_id,))
            conn.execute("DELETE FROM conversations WHERE id = ?", (conv_id,))

        conv_id = str(_uuid.uuid4())
        state_json = json.dumps({"type": "idle"})
        mode_json = json.dumps(conv_mode)
        conn.execute(
            "INSERT INTO conversations ("
            " id, slug, title, cwd, parent_conversation_id, user_initiated,"
            " state, state_updated_at, created_at, updated_at, archived,"
            " model, project_id, conv_mode, desired_base_branch,"
            " seed_parent_id, seed_label"
            ") VALUES (?, ?, ?, ?, NULL, 1, ?, ?, ?, ?, 0, 'mock', ?, ?, NULL, NULL, ?)",
            (
                conv_id,
                slug,
                "Fixture Heavy Prod Shape",
                cwd,
                state_json,
                now,
                now,
                now,
                project_id,
                mode_json,
                "perf:message-list-heavy-prod-shape",
            ),
        )

        zero_tool_agents = {5, 17, 41, 68, 93, 119, 151, 188, 229}
        two_tool_agents = {12, 37, 74, 106, 143, 177, 214}
        three_tool_agents = {201}

        def _tool_count(agent_idx: int) -> int:
            if agent_idx in zero_tool_agents:
                return 0
            if agent_idx in two_tool_agents:
                return 2
            if agent_idx in three_tool_agents:
                return 3
            return 1

        def _markdown_table(rows: int) -> str:
            lines = ["| metric | value | note |", "| --- | ---: | --- |"]
            for i in range(rows):
                lines.append(f"| sample-{i} | {i * 17} | deterministic fixture row |")
            return "\n".join(lines)

        def _insert_message(
            seq: int,
            message_type: str,
            content: object,
            display_data: object | None = None,
            usage_data: object | None = None,
        ) -> None:
            conn.execute(
                "INSERT INTO messages ("
                " message_id, conversation_id, sequence_id, message_type,"
                " content, display_data, usage_data, created_at"
                ") VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    str(_uuid.uuid4()),
                    conv_id,
                    seq,
                    message_type,
                    json.dumps(content),
                    json.dumps(display_data) if display_data is not None else None,
                    json.dumps(usage_data) if usage_data is not None else None,
                    now,
                ),
            )

        seq = 0
        agent_idx = 0
        tool_idx = 0
        for turn in range(18):
            if turn == 0:
                user_words = 12000
            elif turn == 8:
                user_words = 6000
            elif turn == 15:
                user_words = 2500
            else:
                user_words = 420
            _insert_message(
                seq,
                "user",
                {
                    "text": (
                        f"Heavy fixture user turn {turn + 1}.\n\n"
                        f"{_perf_text(user_words)}"
                    ),
                    "images": [],
                },
            )
            seq += 1

            agents_this_turn = 13 if turn < 17 else 12
            for _ in range(agents_this_turn):
                count = _tool_count(agent_idx)
                tool_blocks = []
                for local_tool in range(count):
                    tool_blocks.append({
                        "type": "tool_use",
                        "id": f"heavy-tool-{agent_idx}-{local_tool}",
                        "name": "bash" if (agent_idx + local_tool) % 3 else "read_file",
                        "input": {
                            "cmd": f"echo heavy-fixture-{agent_idx}-{local_tool}",
                            "path": f"/tmp/heavy-fixture-{agent_idx}-{local_tool}.txt",
                        },
                        "display": f"heavy fixture tool {agent_idx}.{local_tool}",
                    })

                text_words = 620 if agent_idx in {22, 88, 166, 220} else 64
                blocks = [
                    {
                        "type": "text",
                        "text": (
                            f"Heavy fixture agent step {agent_idx}.\n\n"
                            f"{_perf_text(text_words)}\n\n"
                            f"{_markdown_table(4 if agent_idx % 11 == 0 else 1)}\n\n"
                            "```ts\n"
                            "export function heavyFixture(value: string): string {\n"
                            "  return value.trim().toUpperCase();\n"
                            "}\n"
                            "```"
                        ),
                    },
                    *tool_blocks,
                ]
                _insert_message(
                    seq,
                    "agent",
                    blocks,
                    usage_data={
                        "input_tokens": 200 + agent_idx,
                        "output_tokens": 80 + (agent_idx % 40),
                    },
                )
                seq += 1

                for local_tool in range(count):
                    if tool_idx in {7, 101, 180}:
                        result_words = 3200
                    elif tool_idx % 17 == 0:
                        result_words = 900
                    else:
                        result_words = 90
                    tool_use_id = f"heavy-tool-{agent_idx}-{local_tool}"
                    _insert_message(
                        seq,
                        "tool",
                        {
                            "tool_use_id": tool_use_id,
                            "content": (
                                f"Sanitized heavy fixture tool result {tool_idx}.\n"
                                f"{_perf_text(result_words)}"
                            ),
                            "is_error": False,
                        },
                        display_data={"duration_ms": 125 + (tool_idx % 5000)},
                    )
                    seq += 1
                    tool_idx += 1

                agent_idx += 1

        if seq != expected_count or agent_idx != 233 or tool_idx != 233:
            raise RuntimeError(
                "heavy prod-shape fixture generated unexpected counts: "
                f"messages={seq}, agents={agent_idx}, tools={tool_idx}"
            )
        return True

    # ----------------------------------------------------- the seed

    direct_mode = {"mode": "Direct"}
    explore_mode = {"mode": "Explore"}

    with sqlite3.connect(str(db_path), timeout=10) as conn:
        conn.execute("PRAGMA foreign_keys = ON")
        _ensure_schema(conn)

        project_id = _find_or_create_project(conn, str(ROOT))

        if _existing_active_count(conn) > 0:
            created_fixture = _ensure_conversation_load_fixture(
                conn,
                project_id=project_id,
                conv_mode=direct_mode,
                cwd=str(ROOT),
            )
            created_heavy_fixture = _ensure_heavy_prod_shape_fixture(
                conn,
                project_id=project_id,
                conv_mode=direct_mode,
                cwd=str(ROOT),
            )
            conn.commit()
            if not quiet_if_populated:
                count = _existing_active_count(conn)
                suffixes = []
                if created_fixture:
                    suffixes.append("repaired perf fixture")
                if created_heavy_fixture:
                    suffixes.append("repaired heavy fixture")
                suffix = f" + {', '.join(suffixes)}" if suffixes else ""
                print(f"✓ Dev DB already populated ({count} conversations) — skipping seed{suffix}.")
            return

        print("Seeding dev DB with representative conversations...")

        # [1/3] Direct standalones
        print("  [1/3] Direct standalones")
        for text in _SEED_DIRECT_STANDALONES:
            _insert_conv(
                conn,
                text=text,
                conv_mode=direct_mode,
                cwd=str(ROOT),
                project_id=project_id,
            )

        # [2/3] Direct chains (3-member + 2-member)
        print("  [2/3] Direct chains (3-member + 2-member)")
        id1, slug1 = _insert_conv(
            conn, text=_SEED_CHAIN_3_TEXT, conv_mode=direct_mode,
            cwd=str(ROOT), project_id=project_id,
        )
        _exhaust_state(conn, id1)
        id2, slug2 = _link_continuation(
            conn, parent_id=id1, parent_slug=slug1, chain_index=2,
            conv_mode=direct_mode, cwd=str(ROOT), project_id=project_id,
        )
        _exhaust_state(conn, id2)
        _link_continuation(
            conn, parent_id=id2, parent_slug=slug2, chain_index=3,
            conv_mode=direct_mode, cwd=str(ROOT), project_id=project_id,
        )

        id_a, slug_a = _insert_conv(
            conn, text=_SEED_CHAIN_2_TEXT, conv_mode=direct_mode,
            cwd=str(ROOT), project_id=project_id,
        )
        _exhaust_state(conn, id_a)
        _link_continuation(
            conn, parent_id=id_a, parent_slug=slug_a, chain_index=2,
            conv_mode=direct_mode, cwd=str(ROOT), project_id=project_id,
        )

        # [3/3] Explore mode chain
        print("  [3/3] Explore mode chain (2-member)")
        eid1, eslug1 = _insert_conv(
            conn, text=_SEED_EXPLORE_TEXT, conv_mode=explore_mode,
            cwd=str(ROOT), project_id=project_id,
        )
        _exhaust_state(conn, eid1)
        _link_continuation(
            conn, parent_id=eid1, parent_slug=eslug1, chain_index=2,
            conv_mode=explore_mode, cwd=str(ROOT), project_id=project_id,
        )

        _ensure_conversation_load_fixture(
            conn,
            project_id=project_id,
            conv_mode=direct_mode,
            cwd=str(ROOT),
        )
        _ensure_heavy_prod_shape_fixture(
            conn,
            project_id=project_id,
            conv_mode=direct_mode,
            cwd=str(ROOT),
        )

        conn.commit()


# ---------------------------------------------------------------------------
# TLS
# ---------------------------------------------------------------------------

def _tls_helper(args: list[str]) -> str:
    result = subprocess.run(
        ["cargo", "run", "--quiet", "--bin", "phoenix-tls", "--", *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(result.stdout, end="")
        print(result.stderr, end="", file=sys.stderr)
        sys.exit(result.returncode)
    return result.stdout


def _sanitize_tls_name(name: str) -> str:
    safe = "".join(c if c.isalnum() or c in ".-" else "_" for c in name)
    return safe.strip("._-") or "phoenix"


def _open_private_write(path: Path):
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW

    fd = os.open(path, flags, 0o600)
    try:
        os.fchmod(fd, 0o600)
        return os.fdopen(fd, "wb")
    except Exception:
        os.close(fd)
        raise


def _default_tls_hosts(primary_host: str, extra_hosts: list[str] | None = None) -> list[str]:
    hosts = [primary_host, "localhost", "127.0.0.1", "::1"]
    if extra_hosts:
        hosts.extend(extra_hosts)
    seen: set[str] = set()
    deduped: list[str] = []
    for host in hosts:
        host = host.strip()
        if host and host not in seen:
            seen.add(host)
            deduped.append(host)
    return deduped


def _update_env_file(path: Path, updates: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    existing = path.read_text().splitlines() if path.exists() else []
    seen: set[str] = set()
    output: list[str] = []

    for line in existing:
        stripped = line.strip()
        key, sep, _value = stripped.partition("=")
        if sep and key in updates and not stripped.startswith("#"):
            output.append(f"{key}={updates[key]}")
            seen.add(key)
        else:
            output.append(line)

    missing = [key for key in updates if key not in seen]
    if missing and output and output[-1].strip():
        output.append("")
    for key in missing:
        output.append(f"{key}={updates[key]}")

    path.write_text("\n".join(output) + "\n")


def cmd_tls_ca(ca_dir: Path = TLS_CA_DIR) -> None:
    """Create or show the Phoenix private CA."""
    ca_dir.mkdir(parents=True, exist_ok=True)
    out = _tls_helper(["ca", "--dir", str(ca_dir)])
    print(out, end="")
    print("Trust the cert path above on browser machines. Keep the key path private.")
    print("Do not copy the CA key to remote Phoenix hosts; use './dev.py tls issue <host>'.")


def cmd_tls_issue(
    host: str,
    extra_hosts: list[str] | None = None,
    ca_dir: Path = TLS_CA_DIR,
    out_dir: Path = TLS_BUNDLE_DIR,
    port: int = PROD_PORT,
) -> None:
    """Issue a per-host Phoenix TLS bundle from the local CA."""
    import tarfile
    import tempfile

    hosts = _default_tls_hosts(host, extra_hosts)
    bundle_name = _sanitize_tls_name(host)
    out_dir.mkdir(parents=True, exist_ok=True)
    bundle_path = out_dir / f"{bundle_name}.tar.gz"

    with tempfile.TemporaryDirectory(prefix="phoenix-tls-") as tmp:
        tmp_path = Path(tmp)
        cert_path = tmp_path / "server.pem"
        key_path = tmp_path / "server-key.pem"
        args = [
            "issue",
            "--ca-dir",
            str(ca_dir),
            "--cert",
            str(cert_path),
            "--key",
            str(key_path),
        ]
        for item in hosts:
            args.extend(["--host", item])
        _tls_helper(args)

        metadata = {
            "host": host,
            "hosts": hosts,
            "port": port,
            "cert": "server.pem",
            "key": "server-key.pem",
        }
        metadata_path = tmp_path / "phoenix-tls.json"
        metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")

        with _open_private_write(bundle_path) as bundle_file:
            with tarfile.open(fileobj=bundle_file, mode="w:gz") as tar:
                tar.add(cert_path, arcname="server.pem")
                tar.add(key_path, arcname="server-key.pem")
                tar.add(metadata_path, arcname="phoenix-tls.json")

    print(f"Bundle: {bundle_path}")
    print(f"Hosts:  {', '.join(hosts)}")
    print("Contains: server cert/key only; the CA private key stays local.")
    print(f"Copy to host, then run: ./dev.py tls install ~/{bundle_path.name}")


def cmd_tls_install(
    bundle: Path,
    install_dir: Path = TLS_INSTALL_DIR,
    env_file: Path = ROOT / ".phoenix-ide.env",
) -> None:
    """Install a Phoenix TLS bundle and update repo-local env config."""
    import tarfile
    import tempfile

    if not bundle.exists():
        print(f"ERROR: bundle not found: {bundle}", file=sys.stderr)
        sys.exit(1)

    with tempfile.TemporaryDirectory(prefix="phoenix-tls-install-") as tmp:
        tmp_path = Path(tmp)
        with tarfile.open(bundle, "r:gz") as tar:
            members = tar.getmembers()
            names = {member.name for member in members}
            expected = {"server.pem", "server-key.pem", "phoenix-tls.json"}
            if names != expected or not all(member.isfile() for member in members):
                print(f"ERROR: invalid TLS bundle contents: {sorted(names)}", file=sys.stderr)
                sys.exit(1)
            tar.extractall(tmp_path, filter="data")

        metadata = json.loads((tmp_path / "phoenix-tls.json").read_text())
        host = metadata["host"]
        port = int(metadata.get("port", PROD_PORT))
        name = _sanitize_tls_name(host)
        install_dir.mkdir(parents=True, exist_ok=True)

        cert_dest = install_dir / f"{name}.pem"
        key_dest = install_dir / f"{name}-key.pem"
        shutil.copy2(tmp_path / "server.pem", cert_dest)
        shutil.copy2(tmp_path / "server-key.pem", key_dest)
        cert_dest.chmod(0o644)
        key_dest.chmod(0o600)

    _update_env_file(
        env_file,
        {
            "PHOENIX_TLS": "manual",
            "PHOENIX_TLS_CERT_PATH": str(cert_dest),
            "PHOENIX_TLS_KEY_PATH": str(key_dest),
            "PHOENIX_PUBLIC_URL": f"https://{host}:{port}",
        },
    )

    print(f"Installed cert: {cert_dest}")
    print(f"Installed key:  {key_dest}")
    print(f"Updated env:    {env_file}")
    print("Run: ./dev.py prod deploy")


def cmd_down():
    """Stop all servers."""
    stopped_any = False
    stopped_any |= stop_process(VITE_PID_FILE, "Vite")
    stopped_any |= stop_process(PHOENIX_PID_FILE, "Phoenix")
    
    # Clean up lock file if it exists and process is gone
    lock_path = get_lock_path()
    if lock_path.exists():
        try:
            lock_path.unlink()
        except OSError:
            pass
    
    if not stopped_any:
        print("Nothing running")


def cmd_restart(phoenix_port: int | None = None, tls: bool = False):
    """Rebuild Rust and restart Phoenix (Vite stays for hot reload)."""
    default_phoenix, default_vite = get_default_ports()
    phoenix_port = phoenix_port or default_phoenix
    vite_was_running = get_pid(VITE_PID_FILE) is not None

    build_rust(release=True)
    stop_process(PHOENIX_PID_FILE, "Phoenix")
    time.sleep(0.5)
    phoenix_tls = start_phoenix(port=phoenix_port, tls=tls)
    api_scheme = "https" if phoenix_tls else "http"

    if vite_was_running:
        start_vite(port=default_vite, phoenix_port=phoenix_port, phoenix_tls=phoenix_tls)
        print(f"Phoenix restarted. Vite ready for UI hot reload.")
        print(f"  UI:  http://localhost:{default_vite}")
        print(f"  API: {api_scheme}://localhost:{phoenix_port}")
    else:
        print(f"Phoenix restarted. Vite not running (start with ./dev.py up).")
        print(f"  API: {api_scheme}://localhost:{phoenix_port}")


def cmd_status():
    """Check what's running."""
    phoenix_pid = get_pid(PHOENIX_PID_FILE)
    vite_pid = get_pid(VITE_PID_FILE)
    default_phoenix, default_vite = get_default_ports()
    
    print(f"Worktree: {ROOT}")
    print(f"  Hash: {get_worktree_hash()}")
    print(f"  Default ports: Phoenix={default_phoenix}, Vite={default_vite}")
    print(f"  Database: {get_db_path()}")
    print()

    if phoenix_pid:
        print(f"Phoenix: running (PID {phoenix_pid})")
        if scheme := _probe_phoenix_scheme(default_phoenix):
            print(f"  URL: {scheme}://localhost:{default_phoenix}")
    else:
        print("Phoenix: stopped")

    if vite_pid:
        print(f"Vite:    running (PID {vite_pid})")
    else:
        print("Vite:    stopped")

    if phoenix_pid:
        try:
            import ssl
            import urllib.request

            scheme = _probe_phoenix_scheme(default_phoenix) or "http"
            context = ssl._create_unverified_context() if scheme == "https" else None
            with urllib.request.urlopen(
                f"{scheme}://localhost:{default_phoenix}/api/models",
                timeout=2,
                context=context,
            ) as resp:
                data = json.loads(resp.read())
                print(f"Models:  {', '.join(data.get('models', []))}")
        except Exception:
            pass


def _find_chromium_binary() -> Path | None:
    """Locate a usable Chromium/Chrome binary, in this order:

    1. `PATH` (`google-chrome`, `chromium`, `chromium-browser`, `chrome`).
    2. Playwright's standard install dir at `/opt/pw-browsers/chromium-*/chrome-linux/chrome`.
    3. Playwright's per-user cache at `~/.cache/ms-playwright/chromium-*/chrome-linux/chrome`.
    4. Puppeteer's per-user cache at `~/.cache/puppeteer/chrome/*/chrome-linux*/chrome`.
    5. chromiumoxide's own cache at `~/.local/share/chromiumoxide/`.

    Returns the first existing executable, or `None`. The result flows
    into `PHOENIX_CHROME_EXECUTABLE` so `BrowserSession::new()` can use
    the binary directly without invoking the fetcher.
    """
    import shutil
    for name in ("google-chrome", "chromium", "chromium-browser", "chrome", "google-chrome-stable"):
        p = shutil.which(name)
        if p:
            return Path(p)
    home = Path.home()
    glob_roots: list[tuple[Path, str]] = [
        (Path("/opt/pw-browsers"), "chromium-*/chrome-linux/chrome"),
        (home / ".cache" / "ms-playwright", "chromium-*/chrome-linux/chrome"),
        (home / ".cache" / "puppeteer" / "chrome", "*/chrome-linux*/chrome"),
        (home / ".local" / "share" / "chromiumoxide", "*/chrome-linux/chrome"),
        (home / ".local" / "share" / "chromiumoxide", "chrome-linux/chrome"),
    ]
    for root, pattern in glob_roots:
        if not root.is_dir():
            continue
        for hit in sorted(root.glob(pattern)):
            if hit.is_file() and os.access(hit, os.X_OK):
                return hit
    return None


def _fetcher_download_reachable() -> bool:
    """Probe whether chromiumoxide's fetcher could reach its CDN.

    The fetcher first hits a JSON manifest on `googlechromelabs.github.io`
    then downloads the binary from `storage.googleapis.com`. We HEAD the
    manifest and require 200 OK — any other response (403/404/cert
    failure/timeout) means the fetcher path is unreliable in this env
    and the test harness should skip rather than waste a minute hitting
    the same wall the Rust-side rustls TLS stack will hit.

    1.5s budget is enough on a healthy network and short enough not to
    slow `check` when offline.
    """
    import urllib.request as ureq
    import urllib.error as uerr
    url = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions.json"
    req = ureq.Request(url, method="HEAD")
    try:
        resp = ureq.urlopen(req, timeout=1.5)
    except (uerr.HTTPError, uerr.URLError, OSError):
        return False
    return resp.status == 200


def _outbound_https_reachable() -> bool:
    """Probe whether outbound HTTPS to public hosts works.

    Some browser tests (currently `test_browser_navigate_remote`)
    actually navigate to real websites; they need real internet, not
    just a working Chromium. Sandbox networks often block this. We
    probe `https://example.com` itself — that's the exact host the
    test hits, so a green probe means the test will work and a red
    probe means it would fail for environmental reasons (signal we
    skip via `PHOENIX_SKIP_NETWORK_TESTS=1`).
    """
    import urllib.request as ureq
    import urllib.error as uerr
    req = ureq.Request("https://example.com/", method="HEAD")
    try:
        resp = ureq.urlopen(req, timeout=1.5)
    except (uerr.HTTPError, uerr.URLError, OSError):
        return False
    return 200 <= resp.status < 400


def _classify_browser_env() -> None:
    """Pick a Chromium binary or auto-skip browser tests.

    Resolution order:
      1. `PHOENIX_SKIP_BROWSER_TESTS` already set → user opt-out, honour.
      2. Chromium found via PATH or cache dirs → set
         `PHOENIX_CHROME_EXECUTABLE` so the test harness launches it
         directly (bypassing chromiumoxide's lookup + fetcher chain).
      3. No binary BUT the fetcher CDN is reachable → don't skip,
         let the auto-fetcher download.
      4. No binary AND fetcher unreachable → set
         `PHOENIX_SKIP_BROWSER_TESTS=1` so all browser tests skip
         instead of failing with launch errors.

    Per the cmd_check probe rule: print only when classification
    changes test behavior. Branches 2 and 3 are silent (happy paths);
    branch 4 prints because it skips a class of tests.
    """
    if "PHOENIX_SKIP_BROWSER_TESTS" in os.environ:
        return
    chrome_path = _find_chromium_binary()
    if chrome_path is not None:
        os.environ.setdefault("PHOENIX_CHROME_EXECUTABLE", str(chrome_path))
        return
    if _fetcher_download_reachable():
        return
    os.environ["PHOENIX_SKIP_BROWSER_TESTS"] = "1"
    print("  i  Chromium unavailable — skipping browser tests")


def _classify_network_env() -> None:
    """Auto-skip tests that need outbound HTTPS when the env can't make
    those requests. The probe targets the same host the affected tests
    hit (currently example.com) so a green probe means the test will
    work and a red probe means it would fail for environmental reasons.

    Sets `PHOENIX_SKIP_NETWORK_TESTS=1` on probe failure; the Rust
    `require_network!()` macro reads this and short-circuits each
    network-dependent test.

    Per the cmd_check probe rule: silent on the green path; prints
    only when skipping the network-dependent test class.
    """
    if "PHOENIX_SKIP_NETWORK_TESTS" in os.environ:
        return
    if not _outbound_https_reachable():
        os.environ["PHOENIX_SKIP_NETWORK_TESTS"] = "1"
        print("  i  outbound HTTPS: unavailable — skipping remote-network tests")


def cmd_check():
    """Run lint, format check, tests, and task validation in parallel."""
    results = []  # (name, returncode, elapsed, output)
    results_lock = threading.Lock()
    t_start = time.monotonic()

    # Per-step kill-and-fail budget. 600s covers cold `cargo test compile`
    # on a 4-vCPU GitHub Actions runner without sccache (observed ~5min);
    # local M-class hardware finishes in ~90s. Bump only if a single step
    # legitimately takes longer — never to mask flakes.
    CHECK_TIMEOUT = 600

    def run_step(name, cmd, cwd=ROOT):
        # Stream stdout+stderr line-by-line into a bounded buffer so that on
        # timeout we keep the last N lines of output instead of throwing it
        # away. Process is launched in its own session so SIGKILL can target
        # the whole group \u2014 vitest, eslint, etc. fork worker children; killing
        # only the parent leaves orphans whose pipes the harness can still
        # be blocked on, masking the original hang.
        from collections import deque
        TAIL_LINES = 200
        t0 = time.monotonic()
        # Captured lanes buffer stdout into `buf` and reprint it via plain
        # print() on failure, so terminal color codes are never wanted here —
        # they only mangle the buffered text (cargo clippy disables color on a
        # pipe, but rustfmt's --check diff and some node tools key off
        # COLORTERM/TERM rather than isatty). Force color off explicitly and
        # neutralise any inherited FORCE_COLOR override. node_env() returns a
        # cached shared dict, so copy before mutating.
        env = dict(node_env()) if Path(cwd) == UI_DIR else os.environ.copy()
        env["CARGO_TERM_COLOR"] = "never"
        env["NO_COLOR"] = "1"
        env.pop("FORCE_COLOR", None)
        env.pop("CLICOLOR_FORCE", None)
        buf: deque[str] = deque(maxlen=TAIL_LINES)
        truncated = False

        proc = subprocess.Popen(
            cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, env=env, start_new_session=True, bufsize=1,
        )

        def reader():
            nonlocal truncated
            assert proc.stdout is not None
            for line in proc.stdout:
                if len(buf) == buf.maxlen:
                    truncated = True
                # Strip terminal control sequences before buffering. The env
                # above disables color for tools that honour it, but rustfmt's
                # --check diff colours via the `term` crate keyed on $TERM and
                # ignores both NO_COLOR and CARGO_TERM_COLOR; this is the
                # tool-agnostic backstop so the reprinted buffer stays clean.
                buf.append(_CONTROL_SEQ_RE.sub("", line).rstrip("\n"))
            proc.stdout.close()

        rt = threading.Thread(target=reader, daemon=True)
        rt.start()

        timed_out = False
        try:
            rc = proc.wait(timeout=CHECK_TIMEOUT)
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                rc = proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                rc = proc.wait(timeout=5)
        # Reader exits when stdout closes (proc termination drops the pipe).
        rt.join(timeout=5)
        elapsed = time.monotonic() - t0

        tail = "\n".join(buf).strip()
        if timed_out:
            header = f"TIMEOUT after {CHECK_TIMEOUT}s \u2014 last {len(buf)} lines of output"
            if truncated:
                header += " (earlier lines dropped)"
            output = header + ("\n" + tail if tail else "\n(no output captured before timeout)")
            rc = rc if rc != 0 else 1
        else:
            output = tail

        with results_lock:
            ok = "\u2713" if rc == 0 else "\u2717"
            results.append((name, rc, elapsed, output))
            print(f"  {ok} {name:<18s} ({elapsed:.1f}s)")

    def lane_rust():
        """Rust lane: clippy → [musl smoke check] → test compile → test run → codegen staleness check.

        The musl smoke check is **macOS-only and conditional** on the
        `x86_64-linux-musl-gcc` cross toolchain being on PATH; it never
        runs on Linux (clippy already covers the same surface there).
        Expect the timing breakdown to be missing the musl line on Linux
        and on macOS hosts without the cross toolchain installed.

        vite build is intentionally NOT run here. `#[derive(Embed)]` in
        crates/phoenix-ide/src/api/assets.rs reads ui/dist/ during proc-macro
        expansion, and that read tolerates an empty (or just `.gitkeep`)
        directory: rust-embed enumerates whatever files are there. The
        runtime fallback in serve_static() reads from the filesystem when
        Assets::get() returns None, so an empty embed table is correct for
        check. Build-time bundling of UI assets belongs to `prod_build`,
        which runs vite in its own worktree.

        Test compile and run are split into two steps so each gets its own
        CHECK_TIMEOUT budget. Cold test-binary compiles on this codebase can
        approach 300s on their own, and when bundled with ~50s of test runtime
        the combined step exceeds the timeout even though nothing is wrong.

        The final `codegen-stale` step guards against Rust-type edits landing
        without a regenerated `ui/src/generated/` directory (task 02677).
        `cargo test` runs ts-rs' per-type `export_bindings_*` tests which
        overwrite the generated .ts files; if those differ from what's
        committed to git, the developer forgot to regenerate.
        """
        run_step("cargo clippy", ["cargo", "clippy", "--", "-D", "warnings"])
        if sys.platform == "darwin":
            # macOS prod deploy uses native target (launchd_prod_deploy → prod_build target=None),
            # so the musl smoke check is opt-in: skip cleanly if the cross toolchain isn't installed.
            # See task 60001 for installing musl-cross-make on this machine.
            if shutil.which("x86_64-linux-musl-gcc"):
                run_step("cargo check musl", [
                    "cargo", "check", "--target", "x86_64-unknown-linux-musl",
                ])
            else:
                print("  i  cargo check musl: skipped (x86_64-linux-musl-gcc not on PATH; see task 60001)")
        # Linux hosts: the prior fallback was a plain `cargo check`, which is
        # strictly redundant with `cargo clippy` above (clippy implies check).
        # Drop it — musl validation belongs on the CI/macOS path that has the
        # cross toolchain installed.
        has_nextest = subprocess.run(
            ["cargo", "nextest", "--version"],
            capture_output=True,
        ).returncode == 0
        # nextest defaults to available_parallelism (= num_cpus). On low-RAM
        # boxes, num_cpus parallel test threads can swap and stall sensitive
        # tests (e.g. browser tests where Chrome's CDP WebSocket handshake
        # times out if Chrome can't get CPU+RAM during launch). Cap the
        # thread count by ~1.5 GiB headroom per thread so resource-starved
        # machines back off, while leaving fast machines effectively
        # unchanged.
        cpus = os.cpu_count() or 4
        try:
            with open("/proc/meminfo") as f:
                mem_gib = next(
                    int(l.split()[1]) for l in f if l.startswith("MemTotal:")
                ) / (1024 * 1024)
            mem_cap = max(1, int(mem_gib // 1.5))
        except (OSError, StopIteration):
            mem_cap = cpus
        test_threads = max(2, min(cpus - 1, mem_cap))
        if test_threads < cpus:
            print(f"  i  cargo test: capping to {test_threads} threads "
                  f"(cpus={cpus}, mem_cap={mem_cap})")
        if has_nextest:
            compile_cmd = ["cargo", "nextest", "run", "--no-run"]
            test_cmd = ["cargo", "nextest", "run",
                        "--test-threads", str(test_threads)]
        else:
            compile_cmd = ["cargo", "test", "--no-run"]
            test_cmd = ["cargo", "test", "--",
                        "--test-threads", str(test_threads)]
        run_step("cargo test compile", compile_cmd)
        run_step("cargo test", test_cmd)
        # Codegen staleness guard. `cargo test` above re-runs the ts-rs
        # `export_bindings_*` tests, which overwrite files in
        # `ui/src/generated/`. A non-empty porcelain status under that path
        # — modified or untracked — means the developer's Rust types and
        # the committed TS don't line up.
        run_step("codegen-stale", ["bash", "-c", (
            # Fail if `git status --porcelain -- ui/src/generated/` has
            # any output at all (covers modified *and* untracked).
            'out=$(git status --porcelain -- ui/src/generated/); '
            'if [ -n "$out" ]; then '
            '  echo "ui/src/generated/ has uncommitted changes:"; '
            '  echo "$out"; '
            '  echo ""; '
            '  echo "Run \'./dev.py codegen\' and commit the result."; '
            '  exit 1; '
            'fi'
        )])

    def lane_ui_lint():
        """UI lint lane: eslint (TS/TSX) → stylelint (CSS).

        Both run on the same source tree and need no build artifact, so
        bundling them into one thread avoids spending a parallel slot on
        a sub-second stylelint pass. Each emits its own result entry.
        """
        run_step("eslint", ["pnpm", "run", "lint"], UI_DIR)
        run_step("stylelint", ["pnpm", "run", "lint:css"], UI_DIR)

    def lane_fast():
        """Fast lane: cargo fmt then task validation."""
        run_step("cargo fmt", ["cargo", "fmt", "--check"])
        # Task validation runs in-process (taskmd is a Python dep, not a
        # subprocess) so it can't go through run_step. detail carries the
        # error list into the results tuple so a failure is readable in
        # the end-of-run summary.
        t0 = time.monotonic()
        ok, detail = cmd_tasks_validate(quiet=True)
        elapsed = time.monotonic() - t0
        with results_lock:
            sym = "\u2713" if ok else "\u2717"
            results.append(("task validation", 0 if ok else 1, elapsed, detail))
            print(f"  {sym} {'task validation':<18s} ({elapsed:.1f}s)")

    def lane_e2e():
        """E2E API-boundary tests driven through a real running binary.

        Spawns phoenix-ide on an ephemeral port with PHOENIX_ENABLE_MOCK_MODEL=1
        and an isolated temp DB, then runs a battery of scripted conversations
        through the same HTTP/SSE surface phoenix-client.py uses.

        The cargo bin build inside this lane shares the workspace target dir
        with `lane_rust`'s cargo invocations — cargo's target lock serializes
        them, so the bin link is cheap once clippy/test compile have populated
        target/. Designed to fit in lane_rust's wall-clock shadow.
        """
        run_step("e2e", ["uv", "run", "tests/e2e/run.py"])

    def check_package_lock_clean():
        """Tripwire: fail if `ui/pnpm-lock.yaml` has uncommitted changes.

        `./dev.py prod deploy` builds in a fresh worktree and runs
        `pnpm install --frozen-lockfile`, which fails on any lockfile drift.
        A locally-modified lock left uncommitted would only surface at deploy
        time — catch it during `./dev.py check` instead.
        """
        run_step("pkglock-clean", ["bash", "-c", (
            'out=$(git status --porcelain -- ui/pnpm-lock.yaml); '
            'if [ -n "$out" ]; then '
            '  echo "ui/pnpm-lock.yaml has uncommitted changes:"; '
            '  echo "$out"; '
            '  echo ""; '
            '  echo "Commit these before deploying, or \'pnpm install --frozen-lockfile\' in the build worktree will fail."; '
            '  exit 1; '
            'fi'
        )])

    def check_ast_grep():
        """Run structural lint rules via ast-grep (one result entry per rule file)."""
        import shutil
        if not shutil.which("ast-grep"):
            with results_lock:
                results.append(("ast-grep", 0, 0.0, ""))
                print(f"  - {'ast-grep':<18s} (skipped — not installed)")
            return
        rules_dir = ROOT / "ast-grep-rules"
        if not rules_dir.exists():
            return
        rule_files = sorted(rules_dir.glob("*.yml"))
        if not rule_files:
            return
        for rule_file in rule_files:
            run_step(f"ast-grep:{rule_file.stem[:14]}", [
                "ast-grep", "scan", "--rule", str(rule_file), "ui/src/",
            ])

    def check_allium():
        """Validate every specs/<name>/<name>.allium parses under v3 grammar.
        `allium check` always exits 1 (even on clean files), so we parse the
        JSON-stream output and aggregate error-severity diagnostics."""
        import shutil, json
        if not shutil.which("allium"):
            with results_lock:
                results.append(("allium specs", 0, 0.0, ""))
                print(f"  - {'allium specs':<18s} (skipped — install via 'cargo install allium-cli')")
            return
        spec_files = sorted((ROOT / "specs").glob("*/*.allium"))
        if not spec_files:
            with results_lock:
                results.append(("allium specs", 0, 0.0, ""))
                print(f"  - {'allium specs':<18s} (skipped — no .allium files)")
            return
        t0 = time.monotonic()
        try:
            proc = subprocess.run(
                ["allium", "check", *[str(p) for p in spec_files]],
                capture_output=True, text=True, timeout=60,
            )
        except subprocess.TimeoutExpired:
            elapsed = time.monotonic() - t0
            with results_lock:
                results.append(("allium specs", 1, elapsed, "allium check timed out after 60s"))
                print(f"  ✗ {'allium specs':<18s} ({elapsed:.1f}s)")
            return
        # Parse the concatenated JSON-doc stream that allium-cli emits
        # (one {...} per file passed). Use raw_decode to walk the stream
        # without depending on whitespace conventions.
        decoder = json.JSONDecoder()
        text = proc.stdout
        idx = 0
        failures = []
        docs_parsed = 0
        decode_error = None
        while idx < len(text):
            while idx < len(text) and text[idx].isspace():
                idx += 1
            if idx >= len(text):
                break
            try:
                doc, end = decoder.raw_decode(text, idx)
            except json.JSONDecodeError as e:
                decode_error = str(e)
                break
            idx = end
            docs_parsed += 1
            errs = [d for d in doc.get("diagnostics", []) if d.get("severity") == "error"]
            if errs:
                failures.append((doc.get("diagnostics", [{}])[0].get("location", {}).get("file") or "?", errs))
        elapsed = time.monotonic() - t0

        # Hard-fail on any structural problem with the gate itself, even if
        # earlier files already reported errors. A truncated/partial stream
        # could otherwise hide diagnostics from later files; an empty
        # stdout (allium changed output format, crashed, wrote only to
        # stderr) would silently pass; a parsed-doc count below the file
        # count means we lost some specs to a parse failure mid-stream.
        # Each of these makes the gate unreliable, so report them
        # explicitly rather than masking them behind whatever errors we
        # did see.
        gate_problems = []
        if decode_error:
            gate_problems.append(f"JSON-stream decode error after {docs_parsed} doc(s): {decode_error}")
        if docs_parsed == 0:
            gate_problems.append(
                f"allium check produced no parseable diagnostic docs "
                f"(passed {len(spec_files)} files; stdout {len(text)} bytes)"
            )
        elif docs_parsed != len(spec_files):
            gate_problems.append(
                f"allium check returned {docs_parsed} diagnostic doc(s) but "
                f"{len(spec_files)} spec(s) were passed -- some specs were not validated"
            )

        if gate_problems or failures:
            lines = []
            if gate_problems:
                lines.append("allium check gate failure:")
                for p in gate_problems:
                    lines.append(f"  {p}")
                if proc.stderr.strip():
                    lines.append(f"stderr (first 500 chars):\n{proc.stderr[:500]}")
                if text.strip() and not failures:
                    lines.append(f"stdout (first 500 chars):\n{text[:500]}")
                if failures:
                    lines.append("")
            for path, errs in failures:
                rel = path
                try:
                    rel = str(Path(path).relative_to(ROOT))
                except (ValueError, TypeError):
                    pass
                lines.append(f"{rel}: {len(errs)} error(s)")
                for e in errs[:5]:
                    loc = e.get("location", {}) or {}
                    msg = (e.get("message") or "?")
                    if len(msg) > 140:
                        msg = msg[:140] + "…"
                    lines.append(f"  L{loc.get('line', '?')}: {msg}")
                if len(errs) > 5:
                    lines.append(f"  … and {len(errs) - 5} more")
            out = "\n".join(lines)
            with results_lock:
                results.append(("allium specs", 1, elapsed, out))
                print(f"  ✗ {'allium specs':<18s} ({elapsed:.1f}s)")
        else:
            with results_lock:
                results.append(("allium specs", 0, elapsed, ""))
                print(f"  ✓ {'allium specs':<18s} ({elapsed:.1f}s)")

    def check_spec_anchors():
        """REQ-* anchor cross-validator. Fails on orphan code anchors —
        REQ-IDs referenced in source code but not canonically declared
        in any executive.md status row or requirements.md heading.
        Same logic as `./dev.py audit-specs`; the verbose /
        unanchored-half is on the standalone subcommand only (too
        noisy as a build gate)."""
        t0 = time.monotonic()
        try:
            spec_decls = _scan_for_canonical_req_decls(ROOT / "specs")
            code_anchors = {}
            for code_root_rel in ("crates", "ui/src"):
                code_root = ROOT / code_root_rel
                if not code_root.is_dir():
                    continue
                chunk = _scan_for_req_anchors(
                    code_root,
                    extensions=_CODE_EXTENSIONS,
                    skip_dirs=_CODE_SKIP_DIRS,
                )
                for req, locs in chunk.items():
                    code_anchors.setdefault(req, []).extend(locs)
        except Exception as e:
            elapsed = time.monotonic() - t0
            with results_lock:
                results.append(("spec anchors", 1, elapsed, f"scan failed: {e}"))
                print(f"  ✗ {'spec anchors':<18s} ({elapsed:.1f}s)")
            return

        elapsed = time.monotonic() - t0
        orphans = sorted(set(code_anchors.keys()) - set(spec_decls.keys()))
        if orphans:
            lines = [
                f"{len(orphans)} REQ-ID(s) referenced in code but not declared in any spec.",
                "These are typos, renames, or deletions where the code anchor wasn't updated.",
                "",
            ]
            for req in orphans:
                locs = code_anchors[req]
                lines.append(f"{req}  ({len(locs)} occurrence{'s' if len(locs) != 1 else ''})")
                for path, line, snippet in locs[:3]:
                    lines.append(f"  {path}:{line}  {snippet}")
                if len(locs) > 3:
                    lines.append(f"  ... and {len(locs) - 3} more")
            out = "\n".join(lines)
            with results_lock:
                results.append(("spec anchors", 1, elapsed, out))
                print(f"  ✗ {'spec anchors':<18s} ({elapsed:.1f}s)")
        else:
            with results_lock:
                results.append(("spec anchors", 0, elapsed, ""))
                print(f"  ✓ {'spec anchors':<18s} ({elapsed:.1f}s)")

    # Bootstrap UI deps so eslint / tsc / vitest can run on a fresh checkout.
    ensure_ui_deps()

    # Enable sccache (if installed) as the rustc wrapper so deps' object files
    # are shared across worktrees / `cargo clean` cycles. Honored by every
    # cargo invocation below because the env is inherited by run_step's
    # subprocesses. Skip cleanly if sccache isn't on PATH or the user has
    # explicitly set RUSTC_WRAPPER already.
    if shutil.which("sccache") and "RUSTC_WRAPPER" not in os.environ:
        os.environ["RUSTC_WRAPPER"] = "sccache"
        # Default cache dir + 20G cap. Devs can override via SCCACHE_DIR /
        # SCCACHE_CACHE_SIZE before invoking dev.py.
        os.environ.setdefault("SCCACHE_CACHE_SIZE", "20G")

    # Classify the environment up front so the Rust suite skips the
    # classes of tests that would otherwise produce env-noise failures.
    #
    # Contract for `./dev.py check`:
    #   - Red == broken code. Never "your network is broken" or
    #     "you don't have Chrome installed".
    #   - The internal signal env vars (PHOENIX_CHROME_EXECUTABLE,
    #     PHOENIX_SKIP_BROWSER_TESTS, PHOENIX_SKIP_NETWORK_TESTS,
    #     GIT_CONFIG_*) are MECHANISM — users never set them by hand.
    #   - Probes print iff classification CHANGES test behavior:
    #     auto-skipping a class, overriding a config, etc. Happy paths
    #     (Chromium found, fetcher reachable, signing works, deps
    #     present) stay silent so a normal-env run is clean.
    _classify_browser_env()
    _classify_network_env()

    # Probe for working commit signing. Some envs configure a custom
    # `gpg.ssh.program` (e.g. cloud sandboxes intercepting commits) that
    # rejects unrecognised callers, breaking any test that runs `git commit`.
    # If a probe commit fails, override `commit.gpgsign=false` for child
    # processes via GIT_CONFIG_COUNT/KEY/VALUE — affects subprocesses only,
    # not the developer's actual git config.
    #
    # Per the print-only-on-behavior-change rule above, the success
    # branch is silent and only the override branch prints.
    import tempfile as _tempfile
    try:
        with _tempfile.TemporaryDirectory() as _td:
            subprocess.run(["git", "init", "--quiet"], cwd=_td, check=True,
                           capture_output=True, timeout=5)
            subprocess.run(
                ["git", "-c", "user.email=probe@test", "-c", "user.name=probe",
                 "commit", "--allow-empty", "-m", "probe"],
                cwd=_td, check=True, capture_output=True, timeout=10,
            )
        _signing_ok = True
    except Exception:
        _signing_ok = False
    if not _signing_ok:
        os.environ["GIT_CONFIG_COUNT"] = "1"
        os.environ["GIT_CONFIG_KEY_0"] = "commit.gpgsign"
        os.environ["GIT_CONFIG_VALUE_0"] = "false"
        print("  i  commit signing probe failed — disabling commit.gpgsign for tests")

    print("Running checks in parallel...\n")

    # Threads carry the lane name so a hung-thread report names the lane.
    threads = [
        threading.Thread(target=lane_rust, name="rust"),
        # Use the `typecheck` script so contributors running `pnpm typecheck`
        # locally exercise the same `tsc -b --noEmit` invocation this lane
        # uses. Project references + exactOptionalPropertyTypes only fire
        # under `-b`; bare `pnpm exec tsc --noEmit` silently misses them.
        threading.Thread(target=run_step, args=("tsc typecheck", ["pnpm", "run", "typecheck"], UI_DIR), name="tsc"),
        threading.Thread(target=lane_ui_lint, name="ui-lint"),
        threading.Thread(target=run_step, args=("vitest", ["pnpm", "exec", "vitest", "run"], UI_DIR), name="vitest"),
        threading.Thread(target=lane_fast, name="fast"),
        threading.Thread(target=check_ast_grep, name="ast-grep"),
        threading.Thread(target=check_allium, name="allium"),
        threading.Thread(target=check_spec_anchors, name="spec-anchors"),
        threading.Thread(target=check_package_lock_clean, name="pkglock"),
        threading.Thread(target=lane_e2e, name="e2e"),
    ]
    # daemon=True so a hung lane does not block interpreter shutdown after
    # we report it as failed and call sys.exit(1). Non-daemon threads cause
    # Python to wait at exit, defeating the hung-lane detection below.
    # run_step's subprocess.run already enforces CHECK_TIMEOUT per step via
    # SIGKILL on the child; daemon=True covers the case where a lane is
    # wedged in Python (not in a subprocess) past LANE_JOIN_TIMEOUT.
    for t in threads:
        t.daemon = True
        t.start()
    # Per-thread join budget must cover the longest *lane*, not a single step.
    # lane_rust runs ~6 sequential steps each with their own CHECK_TIMEOUT,
    # so we cap join at 6×CHECK_TIMEOUT + 30s. After joining we still verify
    # the thread actually finished — Thread.join() returning after a timeout
    # does not imply the thread is done, so a still-alive lane is recorded
    # as an explicit failure rather than silently dropped from results.
    LANE_JOIN_TIMEOUT = (CHECK_TIMEOUT * 6) + 30
    for t in threads:
        t.join(timeout=LANE_JOIN_TIMEOUT)
    stuck = [t for t in threads if t.is_alive()]
    for t in stuck:
        with results_lock:
            label = f"hung: {t.name}"
            results.append((
                label, 1, LANE_JOIN_TIMEOUT,
                f"lane did not finish within {LANE_JOIN_TIMEOUT}s — "
                "a subprocess may be orphaned; investigate before retrying",
            ))
            print(f"  ✗ {label:<18s} ({LANE_JOIN_TIMEOUT}s)")

    total_elapsed = time.monotonic() - t_start
    failures = [(n, out) for n, rc, _, out in results if rc != 0]

    if failures:
        print()
        for name, output in failures:
            print(f"\u2500\u2500 {name} {'─' * (50 - len(name))}")
            if output:
                print(output)
            print()
        print(f"\u2717 {len(failures)} of {len(results)} checks failed ({total_elapsed:.1f}s)")
        sys.exit(1)
    else:
        print(f"\n\u2713 All {len(results)} checks passed ({total_elapsed:.1f}s)")


# =============================================================================
# Task Validation
# =============================================================================
#
# Backed by taskmd >= 1.0. In 1.0 the filename is the sole source of truth
# for task metadata (id, priority, status, slug); bodies are free-form
# markdown with no frontmatter. validate() therefore checks filename
# pattern and duplicate IDs only, and fix() handles legacy-ID migration,
# duplicate-ID renumbering, and (under explicit opt-in) stripping pre-1.0
# YAML frontmatter.
#
# API surface used:
#   - taskmd.validate(tasks_dir) -> ValidationResult  (.ok, .errors, .file_count)
#   - taskmd.fix(tasks_dir, migrate=False) -> FixResult  (renames, renumbered,
#     migrated, frontmatter_pending, errors, summary())
#   - taskmd.VALID_STATUSES, taskmd.VALID_PRIORITIES  (frozensets)
#
# fix() takes migrate={None,True,False}. We pass migrate=False here because
# the repo migrated off frontmatter in commit b90a846; if frontmatter
# reappears we want fix to treat the file's ID/dup state, not silently
# rewrite the body. To strip newly reintroduced frontmatter, run
# `taskmd fix --migrate` from the CLI directly (it's destructive — commit
# first).


def cmd_codegen() -> bool:
    """Regenerate `ui/src/generated/` from the Rust source of truth.

    Delegates to ts-rs' per-type `export_bindings_*` tests (emitted by
    `#[derive(ts_rs::TS)]`). Those tests run as part of `cargo test` too,
    but this subcommand is the fast-path for iterating on a Rust type:

        ./dev.py codegen     # regenerate only
        ./dev.py check       # full check including codegen-stale guard

    Returns True on success, False on any cargo failure.
    """
    # Run only the export tests so this is fast even on a cold target.
    # Test filter `export_bindings` matches every ts-rs-emitted test name.
    proc = subprocess.run(
        ["cargo", "test", "--quiet", "export_bindings"],
        cwd=ROOT,
    )
    if proc.returncode != 0:
        print("✗ codegen tests failed", file=sys.stderr)
        return False
    print("✓ regenerated ui/src/generated/")
    # Best-effort summary of what changed.
    diff = subprocess.run(
        ["git", "diff", "--stat", "--", "ui/src/generated/"],
        cwd=ROOT, capture_output=True, text=True,
    )
    if diff.stdout.strip():
        print(diff.stdout)
    else:
        print("  (no changes)")
    return True


def cmd_tasks_validate(quiet: bool = False) -> tuple[bool, str]:
    """Validate all task files using taskmd.

    Checks filename pattern conformance and duplicate IDs. Returns
    (ok, detail): detail is the formatted error block, empty when ok, so a
    quiet caller can still surface it (e.g. `./dev.py check`'s summary).
    """
    import taskmd
    tasks_dir = ROOT / "tasks"
    result = taskmd.validate(tasks_dir)

    if result.ok:
        if not quiet:
            print(f"✓ {result.file_count} task files validated")
        return True, ""

    # taskmd version in the header: a floating dependency that resolved to
    # a newer, stricter taskmd is a common cause of a fail-CI/pass-local split.
    version = getattr(taskmd, "__version__", "unknown")
    lines = [f"✗ {len(result.errors)} task validation error(s) (taskmd {version}):"]
    lines += [f"  - {err}" for err in result.errors]
    lines.append("")
    lines.append("Run './dev.py tasks fix' to auto-fix (legacy IDs, duplicate IDs).")
    detail = "\n".join(lines)
    if not quiet:
        print(detail)
    return False, detail


# REQ-* anchor regex. Matches `REQ-{PREFIX}-{NUM}` where PREFIX is one
# or more uppercase-letter/digit/hyphen segments (e.g. REQ-BED-001,
# REQ-TASKS-UI-007, REQ-CONV-018). Word boundaries match between
# alphanumeric and non-alphanumeric chars (`-`, `.`, `/`), so
# `REQ-FOO-001-style`, `REQ-FOO-001.md`, and `…/REQ-FOO-001` all
# yield the trailing-digit-bounded match `REQ-FOO-001` — which is
# the right behaviour: those still mention the REQ.
_REQ_ANCHOR_RE = re.compile(r'\bREQ-[A-Z][A-Z0-9-]*-\d+\b')

# Canonical REQ-declaration patterns. The audit's gate has two halves:
#   - SOURCE CODE (extensions below) is scanned with the broad
#     _REQ_ANCHOR_RE — every comment mention counts as an anchor.
#   - SPECIFICATIONS are scanned with these tighter patterns so cross-
#     references in design.md / .allium prose don't count as
#     declarations. Only the canonical declaration sites — executive
#     status table rows and requirements.md `### REQ-...` headings —
#     register a REQ-ID as "owned by" some spec. Without this
#     tightening, a code anchor pointing at a non-existent REQ would
#     pass the orphan check whenever any spec happens to mention that
#     ID in prose. (Catch from Copilot review on PR #63.)
_REQ_DECL_TABLE_RE = re.compile(r'^\|\s*\*\*(REQ-[A-Z][A-Z0-9-]*-\d+)[:.]')
_REQ_DECL_HEADING_RE = re.compile(r'^###\s+(REQ-[A-Z][A-Z0-9-]*-\d+)(?::|\s|$)')

# Code file extensions to scan for REQ-* anchors.
_CODE_EXTENSIONS = {".rs", ".tsx", ".ts", ".css", ".html", ".js", ".py"}

# Directory names to prune during the walk. These are matched against
# individual `os.walk` dirnames (single path components), not full
# paths — multi-component entries like `ui/src/generated` would never
# match. We rely on `generated` alone catching `ui/src/generated/`,
# `target/.../generated/`, etc.
_CODE_SKIP_DIRS = {
    "node_modules", "target", ".git", "dist", "build", ".vite",
    "generated",
}


def _scan_for_req_anchors(root: Path, *, extensions: set[str], skip_dirs: set[str]):
    """Walk `root` for any REQ-* mention. Used for source-code anchor
    scanning, where every comment mention is a valid anchor.

    Returns: { req_id: [(rel_path, line_number, snippet), ...] }.

    Uses os.walk so we can prune skipped directories before traversing
    into them — `Path.rglob` would still descend into node_modules /
    target before our filter runs, which on a typical dev machine
    makes this lane visibly slow.
    """
    from collections import defaultdict
    found: dict[str, list[tuple[str, int, str]]] = defaultdict(list)
    for dirpath, dirnames, filenames in os.walk(root):
        # Prune in-place so os.walk skips matching subtrees entirely.
        dirnames[:] = [d for d in dirnames if d not in skip_dirs and not d.startswith(".")]
        for name in filenames:
            path = Path(dirpath) / name
            if path.suffix not in extensions:
                continue
            try:
                text = path.read_text()
            except (UnicodeDecodeError, PermissionError, OSError):
                # Best-effort scanner — transient FS issues, weird
                # encodings, missing-after-listing races, all fall
                # through silently.
                continue
            for line_num, line in enumerate(text.splitlines(), 1):
                for m in _REQ_ANCHOR_RE.finditer(line):
                    rel = str(path.relative_to(ROOT))
                    snippet = line.strip()
                    if len(snippet) > 100:
                        snippet = snippet[:97] + "..."
                    found[m.group()].append((rel, line_num, snippet))
    return found


def _scan_for_canonical_req_decls(specs_root: Path):
    """Walk `specs_root` and return canonically-declared REQ-IDs.

    Canonical declaration sites:
      - Executive status table rows: `| **REQ-FOO-NNN:** ... |`
      - Requirements headings:        `### REQ-FOO-NNN: ...`

    Cross-references in design.md prose, .allium @guidance blocks,
    and parenthetical mentions are NOT declarations — they're
    references, and a REQ that only appears as a reference is not
    really declared anywhere. This is the canonical-only set the
    orphan check uses to validate code anchors against.

    Returns: { req_id: [(rel_path, line_number), ...] } where each
    entry is a canonical declaration site.
    """
    from collections import defaultdict
    found: dict[str, list[tuple[str, int]]] = defaultdict(list)
    for dirpath, dirnames, filenames in os.walk(specs_root):
        dirnames[:] = [d for d in dirnames if not d.startswith(".")]
        for name in filenames:
            # Only executive.md and requirements.md carry canonical
            # declarations. design.md and .allium files are
            # cross-reference-only.
            if name not in ("executive.md", "requirements.md"):
                continue
            path = Path(dirpath) / name
            try:
                text = path.read_text()
            except (UnicodeDecodeError, PermissionError, OSError):
                continue
            patterns = (_REQ_DECL_TABLE_RE, _REQ_DECL_HEADING_RE)
            for line_num, line in enumerate(text.splitlines(), 1):
                for pat in patterns:
                    m = pat.match(line)
                    if m:
                        rel = str(path.relative_to(ROOT))
                        found[m.group(1)].append((rel, line_num))
                        break
    return found


def cmd_audit_specs(verbose: bool = False) -> bool:
    """Cross-validate REQ-* anchors between specs/ and source code.

    Reports:
      - Orphan code anchors: REQ-* IDs referenced in source code that
        are not canonically declared in any executive.md status row or
        requirements.md `### REQ-...` heading. Cross-references in
        prose don't count as declarations — see
        `_scan_for_canonical_req_decls` for the rule. These are
        typos, renames, or deletions where the code anchor wasn't
        updated. ALWAYS a bug.
      - Optional (verbose): unanchored REQ-IDs canonically declared
        in specs/ that have no source-code reference. High-noise
        signal — many REQs legitimately have no explicit anchors
        (design intent, behavioural invariants, future targets) —
        but useful for spotting "✅ Complete" claims without code
        coverage.

    Returns True if no orphan anchors found.
    """
    print("Scanning specs/ and code for REQ-* references...")

    spec_decls = _scan_for_canonical_req_decls(ROOT / "specs")
    code_anchors = {}
    for code_root_rel in ("crates", "ui/src"):
        code_root = ROOT / code_root_rel
        if not code_root.is_dir():
            continue
        chunk = _scan_for_req_anchors(
            code_root,
            extensions=_CODE_EXTENSIONS,
            skip_dirs=_CODE_SKIP_DIRS,
        )
        for req, locs in chunk.items():
            code_anchors.setdefault(req, []).extend(locs)

    declared = set(spec_decls.keys())
    anchored = set(code_anchors.keys())

    print()
    print(f"  canonically declared in specs/: {len(declared)} unique REQ-IDs")
    print(f"  referenced in code:             {len(anchored)} unique REQ-IDs")
    print()

    orphan_anchors = sorted(anchored - declared)
    unanchored_reqs = sorted(declared - anchored)

    ok = True

    if orphan_anchors:
        ok = False
        print(f"✗ Orphan code anchors ({len(orphan_anchors)} REQ-IDs referenced in code "
              f"but not canonically declared in any spec):")
        print()
        for req in orphan_anchors:
            locs = code_anchors[req]
            print(f"  {req}  ({len(locs)} occurrence{'s' if len(locs) != 1 else ''})")
            for path, line, snippet in locs[:3]:
                print(f"    {path}:{line}  {snippet}")
            if len(locs) > 3:
                print(f"    ... and {len(locs) - 3} more")
        print()
    else:
        print("✓ All code REQ-* anchors resolve to a canonical spec declaration")
        print()

    if verbose:
        if unanchored_reqs:
            print(f"ℹ Unanchored REQ-IDs ({len(unanchored_reqs)} canonically declared "
                  f"in specs/ with no source-code reference):")
            print()
            print("  (Note: many REQs legitimately have no source-code anchor — design "
                  "intent, behavioural invariants, future targets. Filter for ✅-status "
                  "REQs to find real drift.)")
            print()
            for req in unanchored_reqs:
                locs = spec_decls[req]
                primary = locs[0]
                print(f"  {req}  →  {primary[0]}:{primary[1]}")
            print()

    return ok



def cmd_tasks_fix() -> bool:
    """Auto-fix task files using taskmd: migrate legacy IDs and renumber duplicates.

    Frontmatter stripping is intentionally NOT performed by this command —
    the repo migrated off frontmatter once already. To strip frontmatter
    that has been reintroduced, run `taskmd fix --migrate` from the CLI
    directly after committing.

    Returns True if all files are now correct, False on errors.
    """
    import taskmd
    tasks_dir = ROOT / "tasks"
    result = taskmd.fix(tasks_dir, migrate=False)

    if not result.ok:
        print(f"\n✗ {len(result.errors)} error(s):")
        for err in result.errors:
            print(f"  - {err}")
        return False

    for old, new in result.renames:
        print(f"  {old} -> {new}")
    for old_id, new_id, old_name, new_name in result.renumbered:
        print(f"  renumbered {old_id} -> {new_id}: {old_name} -> {new_name}")

    summary = result.summary()
    print(f"✓ {summary}")
    return True


def cmd_taskmd(taskmd_args: list[str]) -> None:
    """Passthrough to the `taskmd` CLI.

    `taskmd` ships as a dependency of this script's uv env (see the inline
    metadata up top), so the console script is on PATH whenever dev.py runs —
    even though it isn't installed globally. This subcommand exposes it so the
    AGENTS.md happy path (`taskmd new --slug … --priority …`, body on stdin)
    works from any checkout without a separate install step. Args are forwarded
    verbatim; stdin/stdout/stderr are inherited; run from the repo root so
    taskmd auto-detects `tasks/` via its `_TEMPLATE.md` marker.
    """
    exe = shutil.which("taskmd")
    if exe is None:
        print(
            "ERROR: `taskmd` is not on PATH. It's a dependency of dev.py's uv env, "
            "so this shouldn't happen — try `uv sync` (or report it).",
            file=sys.stderr,
        )
        sys.exit(1)
    result = subprocess.run([exe, *taskmd_args], cwd=ROOT)
    sys.exit(result.returncode)


# =============================================================================
# Production Commands
# =============================================================================




def detect_prod_env() -> str:
    """Detect production environment: 'launchd', 'native', or 'daemon'.

    Returns:
        'launchd': macOS - native launchd deployment (user agent)
        'native': Linux with systemd - full production deployment
        'daemon': Fallback - background daemon in ~/.phoenix-ide/
    """
    if sys.platform == "darwin":
        return "launchd"

    elif sys.platform == "linux":
        # Linux: systemd preferred, daemon fallback
        if check_systemd_available():
            return "native"
        return "daemon"

    else:
        # Other platforms: daemon mode only
        return "daemon"


# Production build worktree location
PROD_BUILD_WORKTREE = ROOT.parent / ".phoenix-ide-build"


def check_systemd_available() -> bool:
    """Check if systemd is available as the init system."""
    try:
        # Check if PID 1 is systemd
        result = subprocess.run(
            ["ps", "-p", "1", "-o", "comm="],
            capture_output=True, text=True, timeout=5
        )
        return result.returncode == 0 and "systemd" in result.stdout.strip()
    except Exception:
        return False


def prod_build(version: str | None = None, strip: bool = True, target: str | None = "x86_64-unknown-linux-musl") -> Path:
    """Build a production binary from a git tag or HEAD.

    Uses a separate git worktree to avoid disturbing the main working directory.
    Returns path to the built binary.

    Args:
        version: Git tag or None for HEAD
        strip: Whether to strip debug symbols (default True, False for debugging)
        target: Cargo build target, or None for native host architecture
    """
    # Determine what to build
    if version:
        # Check if tag exists
        result = subprocess.run(
            ["git", "rev-parse", f"refs/tags/{version}"],
            cwd=ROOT, capture_output=True
        )
        if result.returncode != 0:
            print(f"Tag '{version}' not found", file=sys.stderr)
            sys.exit(1)
        ref = version
        print(f"Building from tag: {version}")
    else:
        # Use current HEAD commit
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT, capture_output=True, text=True
        )
        commit = result.stdout.strip()
        version = f"dev-{commit[:8]}"
        ref = commit
        # Warn if there are uncommitted changes — they won't be included in the build.
        dirty = subprocess.run(
            ["git", "status", "--porcelain"],
            cwd=ROOT, capture_output=True, text=True
        ).stdout.strip()
        if dirty:
            print(f"⚠ Warning: uncommitted changes will NOT be included in the build:")
            for line in dirty.splitlines()[:10]:
                print(f"    {line}")
            if len(dirty.splitlines()) > 10:
                print(f"    ... and {len(dirty.splitlines()) - 10} more")
            print()
        print(f"Building from HEAD: {version}")
    
    # Set up or update the build worktree
    worktree = PROD_BUILD_WORKTREE
    
    if worktree.exists():
        print(f"Updating build worktree to {ref}...")
        subprocess.run(["git", "checkout", "--force", ref], cwd=worktree, check=True, capture_output=True)
    else:
        # Create new worktree
        print(f"Creating build worktree at {worktree}...")
        subprocess.run(
            ["git", "worktree", "add", "--detach", str(worktree), ref],
            cwd=ROOT, check=True
        )
    
    ui_dir = worktree / "ui"

    # Build UI. Corepack-prepare pnpm before any pnpm invocation in the
    # worktree (the helper reads the pin from the main checkout's package.json,
    # but corepack state is global so the worktree picks it up too).
    ensure_corepack_pnpm()
    pnpm_env = node_env()
    print("Installing UI dependencies...")
    result = subprocess.run(
        ["pnpm", "install", "--frozen-lockfile"],
        cwd=ui_dir, capture_output=True, text=True, env=pnpm_env,
    )
    if result.returncode != 0:
        print(result.stdout, end="")
        print(result.stderr, file=sys.stderr, end="")
        raise SystemExit(f"pnpm install --frozen-lockfile failed (exit {result.returncode})")

    print("Building UI...")
    subprocess.run(["pnpm", "run", "build"], cwd=ui_dir, check=True, env=pnpm_env)
    
    # Build Rust
    build_env = os.environ.copy()
    needs_cross = target and sys.platform != "linux"
    if needs_cross:
        raise SystemExit(f"Cross-compilation not supported on {sys.platform}; use CI for release builds.")
    cargo_cmd = ["cargo", "build", "--release"]
    if target:
        print(f"Building Rust ({target}, release)...")
        cargo_cmd += ["--target", target]
        binary = worktree / "target" / target / "release" / "phoenix_ide"
    else:
        print("Building Rust (native, release)...")
        binary = worktree / "target" / "release" / "phoenix_ide"
    subprocess.run(cargo_cmd, cwd=worktree, check=True, env=build_env)

    # Strip the binary (unless debugging)
    if strip:
        print("Stripping binary...")
        subprocess.run(["strip", str(binary)], check=True)
    else:
        print("Keeping debug symbols (unstripped)...")

    size_mb = binary.stat().st_size / (1024 * 1024)
    print(f"Built: {binary} ({size_mb:.1f} MB)")

    return binary


# =============================================================================
# Systemd Unit Generation
# =============================================================================

@dataclasses.dataclass
class SystemdConfig:
    """Configuration for systemd unit generation."""
    user: str
    db_path: str
    install_dir: str
    port: int
    llm_gateway: str | None = None
    # When set, injects Environment=HOME=<path> so the service user can find
    # ~/.claude/.credentials.json for per-request OAuth token reads.
    home_dir: str | None = None
    # When set, appends `EnvironmentFile=<path>`. Values in this file override
    # the inline `Environment=` lines (systemd processes assignments in order).
    env_file_path: str | None = None


def detect_service_user() -> str:
    """Determine which user to run the systemd service as.

    Single-operator homelab: run as the deploying user. This makes `~/.codex`,
    `~/.claude`, git config, and ssh keys readable to the service without
    per-file ACL gymnastics.

    If invoked under sudo, prefer the real (`SUDO_USER`) user over root.
    """
    import pwd
    uid = os.getuid()
    if uid == 0:
        sudo_user = os.environ.get("SUDO_USER")
        if not sudo_user:
            print(
                "ERROR: deploy is running as root with no SUDO_USER set. "
                "Re-run as your normal user; the script uses sudo internally.",
                file=sys.stderr,
            )
            sys.exit(1)
        return pwd.getpwnam(sudo_user).pw_name
    return pwd.getpwuid(uid).pw_name


# Configs for each deployment target
NATIVE_SYSTEMD_CONFIG = SystemdConfig(
    user="exedev",  # placeholder; overridden at deploy time by detect_service_user()
    db_path=str(PROD_DB_PATH),
    install_dir=str(PROD_INSTALL_DIR),
    port=PROD_PORT,
    llm_gateway=None,  # Set at deploy time via get_llm_gateway()
)



def generate_systemd_socket(config: SystemdConfig) -> str:
    """Generate systemd socket unit file content.
    
    The socket unit owns the listening socket and keeps it open during
    service restarts, enabling zero-downtime upgrades.
    """
    return f"""[Unit]
Description=Phoenix IDE Socket
Documentation=https://github.com/phoenix-ide/phoenix-ide

[Socket]
# Production port - socket stays open during service restarts
ListenStream={config.port}
# Disable Nagle's algorithm for lower latency (SSE, interactive)
NoDelay=true
# Allow connections to queue during restart
Backlog=128

[Install]
WantedBy=sockets.target
"""


def _install_prod_env_file(env: dict[str, str], service_user: str) -> str | None:
    """Install /etc/phoenix-ide/phoenix.env from a parsed env dict.

    Mode 0640 root:<service_user> -- readable by the service unit, not world-readable
    (matters once the file holds API keys). Returns the installed path, or None if
    `env` was empty (in which case any stale file is removed).
    """
    if not env:
        # Best-effort cleanup of stale config from an earlier deploy.
        subprocess.run(["sudo", "rm", "-f", str(PROD_ENV_FILE)], check=False)
        return None

    # Re-escape embedded newlines (e.g. LLM_CUSTOM_HEADERS) so each line is a
    # single KEY=value pair. The Rust loader unescapes `\n` itself.
    escaped_newline = "\\n"
    lines = [f"{k}={v.replace(chr(10), escaped_newline)}" for k, v in env.items()]
    content = "\n".join(lines) + "\n"

    subprocess.run(["sudo", "mkdir", "-p", str(PROD_ENV_FILE.parent)], check=True)
    proc = subprocess.run(
        ["sudo", "tee", str(PROD_ENV_FILE)],
        input=content.encode(),
        capture_output=True,
    )
    if proc.returncode != 0:
        print(f"Failed to write prod env file: {proc.stderr.decode()}", file=sys.stderr)
        sys.exit(1)
    subprocess.run(["sudo", "chown", f"root:{service_user}", str(PROD_ENV_FILE)], check=True)
    subprocess.run(["sudo", "chmod", "0640", str(PROD_ENV_FILE)], check=True)
    return str(PROD_ENV_FILE)


def generate_systemd_service(config: SystemdConfig, version: str) -> str:
    """Generate systemd service unit file content.

    This unit requires the socket unit (phoenix-ide.socket) which provides
    the listening socket via systemd socket activation.
    """
    env_lines = [
        f"Environment=PHOENIX_DB_PATH={config.db_path}",
        f"Environment=PHOENIX_VERSION={version}",
    ]

    if config.llm_gateway:
        # Native mode: use LLM_GATEWAY directly
        env_lines.append(f"Environment=LLM_GATEWAY={config.llm_gateway}")

    if config.home_dir:
        # Allow the service user to find ~/.claude/.credentials.json for OAuth auth.
        # System users have no real home, so we point HOME at the deploying user's home.
        env_lines.append(f"Environment=HOME={config.home_dir}")

    if config.env_file_path:
        env_lines.append(f"EnvironmentFile={config.env_file_path}")

    env_section = "\n".join(env_lines)

    return f"""[Unit]
Description=Phoenix IDE
Documentation=https://github.com/phoenix-ide/phoenix-ide
# Socket must be ready before service starts
Requires=phoenix-ide.socket
After=network.target phoenix-ide.socket

[Service]
Type=simple
User={config.user}
{env_section}
ExecStart={config.install_dir}/phoenix-ide
# SIGHUP triggers graceful shutdown; systemd restarts with same socket.
# `+` prefix runs ExecReload as root, ignoring User= -- otherwise a deploy
# that crosses a User= boundary leaves scottopell trying to signal a
# phoenix-dev process and silently fails with EPERM.
ExecReload=+/bin/kill -HUP $MAINPID
# Restart always (including after SIGHUP which exits 0)
Restart=always
RestartSec=1
# Give connections time to drain during graceful shutdown
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
"""


def native_prod_deploy(version: str | None = None):
    """Build and deploy to production (native Linux)."""
    # Check if systemd is available
    if not check_systemd_available():
        print("ERROR: systemd is not available on this system.", file=sys.stderr)
        print("Production deployment requires systemd for service management.", file=sys.stderr)
        print("", file=sys.stderr)
        print("This system is running in a container or non-systemd environment.", file=sys.stderr)
        print("Options:", file=sys.stderr)
        print("  - Use './dev.py up' for development mode instead", file=sys.stderr)
        print("  - This system does not have systemd available", file=sys.stderr)
        sys.exit(1)

    # Build
    binary = prod_build(version)
    
    # Determine version string for display
    if version is None:
        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=ROOT, capture_output=True, text=True
        )
        version = f"dev-{result.stdout.strip()}"
    
    # Create install directory (service keeps running - we'll reload after copy)
    print(f"Installing to {PROD_INSTALL_DIR}...")
    subprocess.run(["sudo", "mkdir", "-p", str(PROD_INSTALL_DIR)], check=True)
    
    # Copy binary (remove first to handle "text file busy" when process is running)
    dest = PROD_INSTALL_DIR / "phoenix-ide"
    subprocess.run(["sudo", "rm", "-f", str(dest)], check=True)
    subprocess.run(["sudo", "cp", str(binary), str(dest)], check=True)
    subprocess.run(["sudo", "chmod", "+x", str(dest)], check=True)
    
    # Detect service user first so we can set up the DB directory correctly
    service_user = detect_service_user()

    # For native systemd deployments the service runs as a dedicated system user,
    # so the DB must live somewhere that user owns — /var/lib/phoenix-ide/ is the
    # standard Linux convention.  (~/.phoenix-ide is only used for dev/daemon mode.)
    native_db_dir = Path("/var/lib/phoenix-ide")
    native_db_path = native_db_dir / "prod.db"
    subprocess.run(["sudo", "mkdir", "-p", str(native_db_dir)], check=True)
    # `-R` so an existing prod.db (and its sqlite -shm/-wal sidecars) created
    # under a previous service_user are migrated to the current one.
    subprocess.run(["sudo", "chown", "-R", f"{service_user}:{service_user}", str(native_db_dir)], check=True)

    # Load .phoenix-ide.env overrides (LLM_API_KEY_HELPER, OPENAI_USE_CODEX_AUTH, etc.)
    env_overrides: dict[str, str] = {}
    env_file_loaded = _load_env_file(env_overrides)
    if env_file_loaded:
        print(f"  Loaded env from {env_file_loaded}")

    # Auto-detect gateway only if the env file didn't already provide LLM config (mirrors launchd).
    gateway = None if _env_provides_llm_config(env_overrides) else get_llm_gateway()

    env_file_path = _install_prod_env_file(env_overrides, service_user)
    if env_file_path:
        print(f"  Installed prod env file: {env_file_path} (0640 root:{service_user})")

    # Configure for native deployment.
    # OAuth token auth: the binary reads ~/.claude/.credentials.json per request.
    # Requires: chmod g+r ~/.claude/.credentials.json + service user in owner's group.
    # See skills/phoenix-deployment/SYSTEMD.md for setup instructions.
    config = dataclasses.replace(
        NATIVE_SYSTEMD_CONFIG,
        user=service_user,
        db_path=str(native_db_path),
        llm_gateway=gateway,
        home_dir=str(Path.home()),
        env_file_path=env_file_path,
    )

    # Install systemd socket unit (for socket activation)
    print("Installing systemd socket unit...")
    socket_content = generate_systemd_socket(config)
    socket_file = Path(f"/etc/systemd/system/{PROD_SERVICE_NAME}.socket")
    
    proc = subprocess.run(
        ["sudo", "tee", str(socket_file)],
        input=socket_content.encode(),
        capture_output=True
    )
    if proc.returncode != 0:
        print(f"Failed to write socket unit: {proc.stderr.decode()}", file=sys.stderr)
        sys.exit(1)

    # Install systemd service unit
    print("Installing systemd service unit...")
    unit_content = generate_systemd_service(config, version)
    unit_file = Path(f"/etc/systemd/system/{PROD_SERVICE_NAME}.service")

    proc = subprocess.run(
        ["sudo", "tee", str(unit_file)],
        input=unit_content.encode(),
        capture_output=True
    )
    if proc.returncode != 0:
        print(f"Failed to write service unit: {proc.stderr.decode()}", file=sys.stderr)
        sys.exit(1)
    
    # Reload systemd
    subprocess.run(["sudo", "systemctl", "daemon-reload"], check=True)
    
    # Enable both socket and service
    subprocess.run(["sudo", "systemctl", "enable", f"{PROD_SERVICE_NAME}.socket"], check=True)
    subprocess.run(["sudo", "systemctl", "enable", PROD_SERVICE_NAME], check=True)
    
    # Check current state
    socket_active = subprocess.run(
        ["systemctl", "is-active", f"{PROD_SERVICE_NAME}.socket"],
        capture_output=True, text=True
    ).stdout.strip() == "active"
    
    service_active = subprocess.run(
        ["systemctl", "is-active", PROD_SERVICE_NAME],
        capture_output=True, text=True
    ).stdout.strip() == "active"
    
    if service_active:
        # Service running - send SIGHUP for hot reload
        # With socket activation, this triggers graceful shutdown -> systemd restart
        print("Sending reload signal (SIGHUP) for zero-downtime upgrade...")
        # Capture MainPID before reload so we can verify the process actually
        # restarted. is-active alone isn't enough -- if ExecReload fails
        # (e.g. EPERM signaling across a User= change), the OLD process keeps
        # serving and the unit still reports active. The /version endpoint
        # returns the cargo package version, so it can't distinguish either.
        old_pid = subprocess.run(
            ["systemctl", "show", PROD_SERVICE_NAME, "-p", "MainPID", "--value"],
            capture_output=True, text=True,
        ).stdout.strip()
        subprocess.run(["sudo", "systemctl", "reload", PROD_SERVICE_NAME], check=True)

        # Poll for new PID. SIGHUP graceful exit + Restart=always cycles the
        # process; the new MainPID should appear within a few seconds.
        new_pid = old_pid
        for _ in range(15):
            time.sleep(1)
            new_pid = subprocess.run(
                ["systemctl", "show", PROD_SERVICE_NAME, "-p", "MainPID", "--value"],
                capture_output=True, text=True,
            ).stdout.strip()
            if new_pid not in ("0", "", old_pid):
                break

        if new_pid in ("0", "", old_pid):
            print(
                f"\n✗ Reload did not replace the running process "
                f"(MainPID still {old_pid or '<none>'}).",
                file=sys.stderr,
            )
            print(
                f"  Inspect: sudo journalctl -u {PROD_SERVICE_NAME} -n 50 --no-pager",
                file=sys.stderr,
            )
            sys.exit(1)

        # Verify it came back up
        result = subprocess.run(
            ["systemctl", "is-active", PROD_SERVICE_NAME],
            capture_output=True, text=True
        )
        if result.stdout.strip() == "active":
            write_deployed_sha()
            print(f"\n✓ Deployed {version} to production (zero-downtime upgrade)")
            print(f"  Service: {PROD_SERVICE_NAME}")
            print(f"  Port: {PROD_PORT}")
            print(f"  Socket: {PROD_SERVICE_NAME}.socket (keeps connections alive)")
            print(f"  Database: {config.db_path}")
            print(f"  URL: {_prod_display_url()}")
        else:
            print(f"\n⚠ Service restarting... check status with: systemctl status {PROD_SERVICE_NAME}")
    else:
        # Service not running - start socket first, then service
        print("Starting socket and service...")

        # Stop any existing (non-socket-activated) service first
        subprocess.run(["sudo", "systemctl", "stop", PROD_SERVICE_NAME], capture_output=True)

        # Start the socket (service will be started on first connection or explicitly)
        subprocess.run(["sudo", "systemctl", "start", f"{PROD_SERVICE_NAME}.socket"], check=True)
        subprocess.run(["sudo", "systemctl", "start", PROD_SERVICE_NAME], check=True)
        time.sleep(1)

        # Verify it started
        result = subprocess.run(
            ["systemctl", "is-active", PROD_SERVICE_NAME],
            capture_output=True, text=True
        )
        if result.stdout.strip() == "active":
            write_deployed_sha()
            print(f"\n✓ Deployed {version} to production")
            print(f"  Service: {PROD_SERVICE_NAME}")
            print(f"  Port: {PROD_PORT}")
            print(f"  Socket: {PROD_SERVICE_NAME}.socket (zero-downtime upgrades enabled)")
            print(f"  Database: {config.db_path}")
            print(f"  URL: {_prod_display_url()}")
        else:
            print(f"\n✗ Service failed to start", file=sys.stderr)
            subprocess.run(["sudo", "journalctl", "-u", PROD_SERVICE_NAME, "-n", "20", "--no-pager"])
            sys.exit(1)


def _load_env_file(env: dict[str, str], filename: str = ".phoenix-ide.env") -> str | None:
    """Load an env file from project root into env dict. Returns path if loaded.

    Simple KEY=VALUE format, one per line. Lines starting with # are comments.
    Literal \\n in values is unescaped to real newlines (for LLM_CUSTOM_HEADERS).
    A KEY with an empty value (`KEY=`) is loaded as an empty string and overrides
    any prior setting -- this is how dev overrides clear a value (e.g., disabling
    PHOENIX_PASSWORD by setting it to empty in the dev file).
    """
    env_file = ROOT / filename
    if not env_file.exists():
        return None
    with open(env_file) as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            key, sep, value = line.partition("=")
            if key and sep:
                env[key.strip()] = value.strip().replace("\\n", "\n")
    return str(env_file)


def _env_provides_llm_config(env: dict[str, str]) -> bool:
    """True if `env` already specifies how to reach an LLM, so the deploy paths
    should not auto-detect and inject a local gateway. Counts a credential
    helper, an explicit gateway, or a direct provider API key."""
    return any(
        env.get(k)
        for k in ("LLM_API_KEY_HELPER", "LLM_GATEWAY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY")
    )


def _detect_codex_auth(env: dict[str, str]) -> str | None:
    """Return a human-readable summary if the server will find a ChatGPT/Codex
    bridge auth file at startup, else None. Mirrors
    `codex_credential::resolve_active_auth_path` in the Rust binary: prefer
    Phoenix's own `~/.phoenix-ide/codex-auth.json` (written by the in-app
    `/codex/login` flow), then Codex CLI's `~/.codex/auth.json` when piggyback
    mode (`OPENAI_USE_CODEX_AUTH=1`) is enabled."""
    phoenix_auth = Path.home() / ".phoenix-ide" / "codex-auth.json"
    if phoenix_auth.exists():
        return f"chatgpt bridge ({phoenix_auth})"
    piggyback_raw = env.get("OPENAI_USE_CODEX_AUTH") or os.environ.get("OPENAI_USE_CODEX_AUTH", "")
    if piggyback_raw.lower() in ("1", "true", "yes", "on"):
        codex_home = env.get("CODEX_HOME") or os.environ.get("CODEX_HOME")
        codex_auth = Path(codex_home) / "auth.json" if codex_home else Path.home() / ".codex" / "auth.json"
        if codex_auth.exists():
            return f"chatgpt bridge ({codex_auth}, OPENAI_USE_CODEX_AUTH=1)"
    return None


def _llm_mode_summary(env: dict[str, str], auto_gateway: str | None) -> str:
    """Human-readable description of how the deployed server will reach an LLM,
    for the post-deploy summary line. `auto_gateway` is the gateway the deploy
    auto-detected (None when `env` already provided LLM config)."""
    if env.get("LLM_API_KEY_HELPER"):
        return "api_key_helper (from .phoenix-ide.env)"
    if env.get("LLM_GATEWAY"):
        return f"gateway ({env['LLM_GATEWAY']}, from .phoenix-ide.env)"
    keys = [k for k in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY") if env.get(k)]
    if keys:
        return f"{' + '.join(keys)} (from .phoenix-ide.env)"
    if auto_gateway:
        return f"gateway ({auto_gateway}, auto-detected)"
    codex = _detect_codex_auth(env)
    if codex:
        return codex
    return "none detected — server has no LLM configured"


def _configure_llm_env(env: dict[str, str]) -> str:
    """Configure LLM environment variables. Returns a human-readable mode string.

    Priority:
    1. .phoenix-ide.env overrides (LLM_API_KEY_HELPER, ANTHROPIC_API_KEY, etc.)
    2. Auto-detected exe.dev gateway (LLM_GATEWAY)
    3. ANTHROPIC_API_KEY from shell environment
    """
    # If env file provided LLM config, respect it — skip auto-detection
    if env.get("LLM_API_KEY_HELPER"):
        helper = env["LLM_API_KEY_HELPER"]
        return f"api_key_helper ({helper})"
    if env.get("LLM_GATEWAY"):
        return f"gateway ({env['LLM_GATEWAY']})"
    if env.get("ANTHROPIC_API_KEY"):
        return "direct API key (ANTHROPIC_API_KEY)"

    # Auto-detect exe.dev gateway
    gateway = get_llm_gateway()
    if gateway:
        env["LLM_GATEWAY"] = gateway
        return f"gateway ({gateway}) [auto-detected]"

    # Last resort: check shell env for API key
    api_key = os.environ.get("ANTHROPIC_API_KEY")
    if api_key:
        env["ANTHROPIC_API_KEY"] = api_key
        return "direct API key (ANTHROPIC_API_KEY)"

    print("ERROR: No LLM configuration found.", file=sys.stderr)
    print("  Options:", file=sys.stderr)
    print("    1. Create .phoenix-ide.env with LLM_API_KEY_HELPER or ANTHROPIC_API_KEY", file=sys.stderr)
    print("    2. Set ANTHROPIC_API_KEY in your environment", file=sys.stderr)
    print("    3. Run on a host with an exe.dev gateway", file=sys.stderr)
    sys.exit(1)


def _repo_env() -> dict[str, str]:
    env: dict[str, str] = {}
    _load_env_file(env)
    return env


def _prod_display_url(env: dict[str, str] | None = None) -> str:
    env = env or _repo_env()
    if url := env.get("PHOENIX_PUBLIC_URL"):
        return url
    scheme = "https" if tls_enabled_from_env(env) else "http"
    return f"{scheme}://localhost:{PROD_PORT}"


def _prod_local_health_url(env: dict[str, str] | None = None) -> str:
    env = env or _repo_env()
    scheme = "https" if tls_enabled_from_env(env) else "http"
    return f"{scheme}://localhost:{PROD_PORT}/version"


def _open_prod_health(env: dict[str, str] | None = None, timeout: float = 5.0):
    import ssl
    import urllib.request

    env = env or _repo_env()
    context = ssl._create_unverified_context() if tls_enabled_from_env(env) else None
    return urllib.request.urlopen(_prod_local_health_url(env), timeout=timeout, context=context)


def prod_daemon_deploy():
    """Deploy as background daemon in ~/.phoenix-ide/ (no systemd).

    Used when systemd is not available (containers, non-systemd Linux).
    Daemonizes the process and returns to shell immediately.
    """
    # Build binary (keep debug symbols for debugging)
    binary = prod_build(version=None, strip=False)

    # Set up environment
    env = os.environ.copy()
    env["PHOENIX_PORT"] = str(PROD_PORT)  # Use prod port (8031)

    prod_dir = Path.home() / ".phoenix-ide"
    prod_dir.mkdir(parents=True, exist_ok=True)

    prod_db_path = prod_dir / "prod.db"
    prod_log_path = prod_dir / "prod.log"
    prod_pid_path = prod_dir / "prod.pid"

    env["PHOENIX_DB_PATH"] = str(prod_db_path)

    # Load .phoenix-ide.env (overrides auto-detection)
    env_file = _load_env_file(env)
    if env_file:
        print(f"  Loaded env from {env_file}")
    else:
        print(f"  No .phoenix-ide.env found (using auto-detection)")

    # Configure LLM auth
    llm_mode = _configure_llm_env(env)
    print(f"  LLM mode: {llm_mode}")

    # Stop existing daemon if running
    if prod_pid_path.exists():
        try:
            with open(prod_pid_path) as f:
                old_pid = int(f.read().strip())
            os.kill(old_pid, 15)  # SIGTERM
            time.sleep(1)
        except (ProcessLookupError, ValueError):
            pass  # Process already dead or invalid PID
        prod_pid_path.unlink(missing_ok=True)

    # Start daemonized process
    with open(prod_log_path, "w") as log:
        proc = subprocess.Popen(
            [str(binary)],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True  # Daemonize: detach from terminal
        )

    # Save PID
    with open(prod_pid_path, "w") as f:
        f.write(str(proc.pid))

    # Verify startup
    time.sleep(2)
    if proc.poll() is not None:
        print("ERROR: Server failed to start. Check logs:", file=sys.stderr)
        print(f"  {prod_log_path}", file=sys.stderr)
        sys.exit(1)

    # Health check
    try:
        import urllib.request
        with _open_prod_health(env, timeout=5) as resp:
            version_text = resp.read().decode().strip()
            version_info = {"version": version_text}
    except Exception as e:
        print(f"WARNING: Server started but health check failed: {e}", file=sys.stderr)
        version_info = {"version": "unknown"}

    write_deployed_sha()
    print(f"\n✓ Deployed daemon to production")
    print(f"  Version: {version_info.get('version', 'unknown')}")
    print(f"  Port: {PROD_PORT}")
    print(f"  Database: {prod_db_path}")
    print(f"  Logs: {prod_log_path}")
    print(f"  PID: {proc.pid} (saved to {prod_pid_path})")
    print(f"  LLM Mode: {llm_mode}")
    print(f"  URL: {_prod_display_url(env)}")
    print()
    print("Use './dev.py prod status' to check status")
    print("Use './dev.py prod stop' to stop the server")


def prod_daemon_status():
    """Show daemon deployment status."""
    prod_dir = Path.home() / ".phoenix-ide"
    prod_pid_path = prod_dir / "prod.pid"
    prod_log_path = prod_dir / "prod.log"

    if not prod_pid_path.exists():
        print("Status: Not running (no PID file)")
        return

    try:
        with open(prod_pid_path) as f:
            pid = int(f.read().strip())

        # Check if process exists
        os.kill(pid, 0)  # Signal 0 = check existence
        print(f"Status: Running (PID {pid})")

        # Health check
        try:
            import urllib.request
            _open_prod_health(timeout=2).close()
            print(f"  Health: OK")
        except Exception as e:
            print(f"  Health: Unreachable ({type(e).__name__}: {e})")
        print(f"  Port: {PROD_PORT}")
        print(f"  URL: {_prod_display_url()}")

        if sha := read_deployed_sha():
            print(f"  Commit: {sha}")
        print(f"  Logs: {prod_log_path}")

    except ProcessLookupError:
        print(f"Status: Dead (PID {pid} not found)")
        print("Run './dev.py prod deploy' to restart")
    except (ValueError, FileNotFoundError):
        print("Status: Unknown (invalid PID file)")


def prod_daemon_stop():
    """Stop daemon deployment."""
    prod_dir = Path.home() / ".phoenix-ide"
    prod_pid_path = prod_dir / "prod.pid"

    if not prod_pid_path.exists():
        print("No daemon running (no PID file)")
        return

    try:
        with open(prod_pid_path) as f:
            pid = int(f.read().strip())

        print(f"Stopping daemon (PID {pid})...")
        os.kill(pid, 15)  # SIGTERM

        # Wait for graceful shutdown
        for _ in range(10):
            time.sleep(0.5)
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
        else:
            print("Graceful shutdown timed out, forcing...")
            os.kill(pid, 9)  # SIGKILL

        prod_pid_path.unlink(missing_ok=True)
        print("✓ Stopped")

    except ProcessLookupError:
        print(f"Process {pid} not found (already stopped)")
        prod_pid_path.unlink(missing_ok=True)
    except (ValueError, FileNotFoundError):
        print("Invalid or missing PID file")


def get_systemd_override_dir() -> Path:
    """Get the systemd drop-in override directory for phoenix-ide."""
    return Path(f"/etc/systemd/system/{PROD_SERVICE_NAME}.service.d")


def list_systemd_overrides() -> list[tuple[str, str]]:
    """List all systemd drop-in overrides. Returns [(filename, content), ...]."""
    override_dir = get_systemd_override_dir()
    if not override_dir.exists():
        return []
    
    overrides = []
    for conf in sorted(override_dir.glob("*.conf")):
        try:
            content = conf.read_text().strip()
            overrides.append((conf.name, content))
        except Exception:
            overrides.append((conf.name, "<unreadable>"))
    return overrides


def native_prod_override_set(name: str, value: str):
    """Set a systemd environment override."""
    override_dir = get_systemd_override_dir()
    conf_file = override_dir / f"{name}.conf"
    content = f"[Service]\nEnvironment={name}={value}\n"
    
    subprocess.run(["sudo", "mkdir", "-p", str(override_dir)], check=True)
    
    # Remove any existing conf files that set the same variable
    # (prevents conflicts from differently-named files)
    if override_dir.exists():
        for existing in override_dir.glob("*.conf"):
            if existing.name == f"{name}.conf":
                continue  # Will be overwritten anyway
            try:
                existing_content = existing.read_text()
                if f"Environment={name}=" in existing_content:
                    subprocess.run(["sudo", "rm", str(existing)], check=True)
                    print(f"  Removed conflicting override: {existing.name}")
            except Exception:
                pass
    
    # Write via sudo tee
    proc = subprocess.run(
        ["sudo", "tee", str(conf_file)],
        input=content.encode(),
        capture_output=True
    )
    if proc.returncode != 0:
        print(f"ERROR: Failed to write {conf_file}", file=sys.stderr)
        sys.exit(1)
    
    subprocess.run(["sudo", "systemctl", "daemon-reload"], check=True)
    subprocess.run(["sudo", "systemctl", "restart", PROD_SERVICE_NAME], check=True)
    print(f"✓ Set {name}={value}")
    print(f"  Service restarted")


def native_prod_override_unset(name: str):
    """Remove a systemd environment override."""
    override_dir = get_systemd_override_dir()
    conf_file = override_dir / f"{name}.conf"
    
    if not conf_file.exists():
        print(f"No override '{name}' found")
        return
    
    subprocess.run(["sudo", "rm", str(conf_file)], check=True)
    subprocess.run(["sudo", "systemctl", "daemon-reload"], check=True)
    subprocess.run(["sudo", "systemctl", "restart", PROD_SERVICE_NAME], check=True)
    print(f"✓ Removed {name} override")
    print(f"  Service restarted")


def native_prod_status():
    """Show production service status (native Linux)."""
    # Check if service exists
    result = subprocess.run(
        ["systemctl", "is-active", PROD_SERVICE_NAME],
        capture_output=True, text=True
    )
    status = result.stdout.strip()
    
    if status == "active":
        print(f"Production: running")
        print(f"  Port: {PROD_PORT}")
        print(f"  URL: {_prod_display_url()}")
        print(f"  Database: {PROD_DB_PATH}")

        # Health check
        try:
            _open_prod_health(timeout=2).close()
            print(f"  Health: OK")
        except Exception:
            print(f"  Health: not responding")

        if sha := read_deployed_sha():
            print(f"  Commit: {sha}")
    else:
        print(f"Production: {status}")
    
    # Show OAuth token status from credentials file (read directly by the binary).
    creds_path = Path.home() / ".claude" / ".credentials.json"
    if creds_path.exists():
        try:
            import datetime
            creds = json.loads(creds_path.read_text())
            expires_at = creds["claudeAiOauth"]["expiresAt"]
            expires_dt = datetime.datetime.fromtimestamp(
                int(expires_at) / 1000, tz=datetime.timezone.utc
            )
            now = datetime.datetime.now(tz=datetime.timezone.utc)
            if expires_dt < now:
                expiry_str = f"EXPIRED (was {expires_dt.strftime('%Y-%m-%d %H:%M UTC')})"
                print(f"  ⚠ OAuth token expired — run `claude login` to refresh")
            else:
                delta = expires_dt - now
                hours = int(delta.total_seconds() // 3600)
                mins = int((delta.total_seconds() % 3600) // 60)
                expiry_str = f"{expires_dt.strftime('%Y-%m-%d %H:%M UTC')} (in {hours}h{mins}m)"
            print(f"  OAuth token: {expiry_str}")
        except Exception:
            pass


def native_prod_stop():
    """Stop production service (native Linux)."""
    subprocess.run(["sudo", "systemctl", "stop", PROD_SERVICE_NAME])
    print(f"Stopped {PROD_SERVICE_NAME}")


# =============================================================================
# launchd (native macOS) deployment
# =============================================================================


# Tools we probe for visibility from the launchd PATH. Listed because each
# is something a Phoenix bash-tool invocation has plausibly needed; MISSING
# here means an agent will hit "command not found" at runtime.
_LAUNCHD_PATH_PROBE_TOOLS = (
    "git", "gh", "uv", "node", "pnpm", "cargo", "rustc",
    "python3", "rg", "jq", "taskmd",
)


def capture_login_shell_path() -> tuple[str, str]:
    """Capture the user's login-shell PATH for injection into the launchd plist.

    Returns (path_string, source_description). Falls back to the ambient PATH
    if the login shell can't be invoked, exits non-zero, or returns empty
    output — every fallback path emits a stderr WARNING so the deploy log
    matches what actually got injected.
    """
    shell = os.environ.get("SHELL", "/bin/zsh")
    cmd = [shell, "-lc", 'printf %s "$PATH"']
    cmd_display = f"`{shell} -lc 'printf %s \"$PATH\"'`"
    reason: str | None = None
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        if result.returncode != 0:
            reason = f"exit={result.returncode} stderr={result.stderr.strip()!r}"
        elif not result.stdout.strip():
            reason = "empty stdout"
        else:
            return result.stdout.strip(), cmd_display
    except (subprocess.SubprocessError, OSError) as e:
        reason = str(e)
    print(
        f"  WARNING: login-shell PATH capture via {cmd_display} failed ({reason}); "
        "using ambient PATH",
        file=sys.stderr,
    )
    return os.environ.get("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"), "ambient $PATH (fallback)"


def print_launchd_path_report(path_str: str, source: str) -> None:
    """Print one-line PATH summary; show full dirs + per-tool paths only when
    a probe tool is MISSING (the case worth eyeballing the deploy log for)."""
    import shutil as _shutil
    dirs = [d for d in path_str.split(":") if d]
    resolved = {t: _shutil.which(t, path=path_str) for t in _LAUNCHD_PATH_PROBE_TOOLS}
    missing = [t for t, p in resolved.items() if not p]
    print(f"  PATH for launchd plist: {len(dirs)} dirs via {source}")
    if not missing:
        print(f"  Tools resolved: {len(_LAUNCHD_PATH_PROBE_TOOLS)}/{len(_LAUNCHD_PATH_PROBE_TOOLS)} ({', '.join(_LAUNCHD_PATH_PROBE_TOOLS)})")
        return
    print(f"  Tools MISSING from PATH: {', '.join(missing)}")
    print("  PATH dirs:")
    for d in dirs:
        print(f"    {d}")
    width = max(len(t) for t in _LAUNCHD_PATH_PROBE_TOOLS)
    print("  Tool resolution:")
    for tool, path in resolved.items():
        print(f"    {tool:<{width}} -> {path if path else 'MISSING'}")


def generate_launchd_plist(
    version: str,
    llm_gateway: str | None,
    extra_env: dict[str, str] | None = None,
    path_override: str | None = None,
) -> str:
    """Generate a launchd plist for the Phoenix IDE server."""
    path_str = path_override if path_override is not None else capture_login_shell_path()[0]
    env_vars = {
        "PATH": path_str,
        "PHOENIX_DB_PATH": str(PROD_DB_PATH),
        "PHOENIX_PORT": str(PROD_PORT),
        "PHOENIX_VERSION": version,
    }
    if llm_gateway:
        env_vars["LLM_GATEWAY"] = llm_gateway
    # Merge .phoenix-ide.env overrides (LLM_API_KEY_HELPER, base URLs, etc.)
    if extra_env:
        env_vars.update(extra_env)

    # XML-escape both keys and values: PATH (and arbitrary env values from
    # .phoenix-ide.env) can contain `&`, `<`, `>` which would otherwise
    # produce an invalid plist.
    from xml.sax.saxutils import escape as _xml_escape
    env_xml = "\n".join(
        f"      <key>{_xml_escape(k)}</key>\n      <string>{_xml_escape(v)}</string>"
        for k, v in env_vars.items()
    )

    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>

  <key>ProgramArguments</key>
  <array>
    <string>{LAUNCHD_INSTALL_DIR / "phoenix-ide"}</string>
  </array>

  <key>EnvironmentVariables</key>
  <dict>
{env_xml}
  </dict>

  <key>RunAtLoad</key>
  <true/>

  <key>KeepAlive</key>
  <true/>

  <key>ProcessType</key>
  <string>Interactive</string>

  <key>StandardOutPath</key>
  <string>{LAUNCHD_LOG_PATH}</string>

  <key>StandardErrorPath</key>
  <string>{LAUNCHD_LOG_PATH}</string>
</dict>
</plist>
"""


def _ensure_newsyslog_config():
    """Install /etc/newsyslog.d/<label>.conf for prod.log rotation.

    Uses copy-truncate (`c` flag) so launchd's open stdout/stderr fd stays
    valid across rotation. Daily at midnight, 14 generations, bzip2.
    Idempotent: skips sudo if installed file already matches desired content.
    """
    import pwd
    user = pwd.getpwuid(os.getuid()).pw_name
    group = "staff"
    desired = (
        f"# Installed by ./dev.py prod deploy — log rotation for phoenix-ide.\n"
        f"# logfilename                              [owner:group]    mode count size when  flags\n"
        f"{LAUNCHD_LOG_PATH}    {user}:{group}    644  14    *    @T00  Jc\n"
    )
    try:
        existing = NEWSYSLOG_CONF_PATH.read_text()
        if existing == desired:
            return  # Already installed, no sudo needed
    except (FileNotFoundError, PermissionError):
        pass

    print(f"  Installing {NEWSYSLOG_CONF_PATH} (sudo required)…")
    import tempfile
    with tempfile.NamedTemporaryFile("w", delete=False, suffix=".conf") as tmp:
        tmp.write(desired)
        tmp_path = tmp.name
    try:
        result = subprocess.run(
            ["sudo", "-n", "install", "-m", "644", "-o", "root", "-g", "wheel",
             tmp_path, str(NEWSYSLOG_CONF_PATH)],
            capture_output=True, text=True,
        )
        if result.returncode == 0:
            print(f"  ✓ Log rotation installed: daily @T00, 14 generations, bzip2, copy-truncate")
        else:
            # Non-fatal: deploy proceeds without rotation. Print one-shot install command.
            print(f"  WARN: could not install rotation config (sudo unavailable in this shell).", file=sys.stderr)
            print(f"  To enable: sudo install -m 644 -o root -g wheel {tmp_path!s} {NEWSYSLOG_CONF_PATH}", file=sys.stderr)
            print(f"  Or rerun `./dev.py prod deploy` from an interactive terminal.", file=sys.stderr)
            return  # Skip cleanup so the printed path stays valid for the user
    except FileNotFoundError:
        print(f"  WARN: sudo not found; rotation config not installed.", file=sys.stderr)
    Path(tmp_path).unlink(missing_ok=True)


def _launchd_stop_if_loaded():
    """Stop and unload the launchd service if it is currently loaded."""
    uid = os.getuid()
    domain_target = f"gui/{uid}/{LAUNCHD_LABEL}"
    result = subprocess.run(
        ["launchctl", "print", domain_target],
        capture_output=True, text=True,
    )
    # launchctl print returns 0 even when service doesn't exist — check output
    if "Could not find service" in result.stderr or "Could not find service" in result.stdout:
        return  # Not loaded, nothing to do
    # Service is loaded — bootout stops and unloads it
    subprocess.run(
        ["launchctl", "bootout", f"gui/{uid}", str(LAUNCHD_PLIST_PATH)],
        capture_output=True,  # Suppress output; may warn if already stopping
    )
    # Brief wait for process to exit
    time.sleep(1)


def launchd_prod_deploy(version: str | None = None):
    """Build and deploy to production via launchd (native macOS)."""
    # Build native macOS binary
    binary = prod_build(version, target=None)

    # Determine version string
    if version is None:
        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=ROOT, capture_output=True, text=True,
        )
        version = f"dev-{result.stdout.strip()}"

    # Install log rotation config (idempotent; sudo only if missing/stale)
    _ensure_newsyslog_config()

    # Stop existing service
    _launchd_stop_if_loaded()

    # Install binary
    LAUNCHD_INSTALL_DIR.mkdir(parents=True, exist_ok=True)
    dest = LAUNCHD_INSTALL_DIR / "phoenix-ide"
    # Remove first to avoid "text file busy" if somehow still running
    dest.unlink(missing_ok=True)
    import shutil
    shutil.copy2(str(binary), str(dest))
    dest.chmod(0o755)

    # Ad-hoc codesign with a stable identifier so macOS remembers FDA grants
    # across redeploys (the linker's default signature changes every build)
    subprocess.run(
        ["codesign", "--force", "--sign", "-", "--identifier", LAUNCHD_LABEL, str(dest)],
        check=True,
    )

    # Load .phoenix-ide.env and detect LLM gateway
    env_overrides: dict[str, str] = {}
    env_file = _load_env_file(env_overrides)
    if env_file:
        print(f"  Loaded env from {env_file}")

    # Auto-detect gateway only if the env file didn't already provide LLM config
    gateway = None if _env_provides_llm_config(env_overrides) else get_llm_gateway()

    # Capture login-shell PATH so the launchd service sees user-installed
    # tools (uv, gh, node, …). launchd's default PATH is just
    # /usr/bin:/bin:/usr/sbin:/sbin and bash -c is non-interactive, so without
    # this the bash tool can't find anything in Homebrew/MacPorts/Volta/cargo.
    path_str, path_source = capture_login_shell_path()
    print_launchd_path_report(path_str, path_source)

    # Generate and write plist
    plist_content = generate_launchd_plist(version, gateway, env_overrides, path_override=path_str)
    LAUNCHD_PLIST_PATH.parent.mkdir(parents=True, exist_ok=True)
    LAUNCHD_PLIST_PATH.write_text(plist_content)

    # Bootstrap (load + start) the service
    uid = os.getuid()
    result = subprocess.run(
        ["launchctl", "bootstrap", f"gui/{uid}", str(LAUNCHD_PLIST_PATH)],
        capture_output=True, text=True,
    )
    if result.returncode != 0 and "already bootstrapped" not in result.stderr:
        print(f"ERROR: launchctl bootstrap failed: {result.stderr}", file=sys.stderr)
        sys.exit(1)

    # Health check with retry (server may take a few seconds to bind the port)
    health_version = None
    for attempt in range(5):
        time.sleep(2)
        try:
            with _open_prod_health(env_overrides, timeout=5) as resp:
                health_version = resp.read().decode().strip()
            break
        except Exception:
            if attempt < 4:
                continue
            print("WARNING: Server started but health check failed after 10s", file=sys.stderr)

    write_deployed_sha()
    llm_mode = _llm_mode_summary(env_overrides, gateway)
    print(f"\n✓ Deployed {version} to production (launchd)")
    if health_version:
        print(f"  Version: {health_version}")
    print(f"  Database: {PROD_DB_PATH}")
    print(f"  Logs: {LAUNCHD_LOG_PATH}")
    print(f"  LLM: {llm_mode}")
    print(f"  URL: {_prod_display_url(env_overrides)}")


def launchd_prod_status():
    """Show launchd service status."""
    uid = os.getuid()
    domain_target = f"gui/{uid}/{LAUNCHD_LABEL}"
    result = subprocess.run(
        ["launchctl", "print", domain_target],
        capture_output=True, text=True,
    )
    if "Could not find service" in result.stderr or "Could not find service" in result.stdout:
        print("Production: not loaded")
        print(f"  Run './dev.py prod deploy' to start")
        return

    # Parse state and pid from launchctl print output
    state = "unknown"
    pid = None
    for line in result.stdout.splitlines():
        line = line.strip()
        if line.startswith("state = "):
            state = line.split("= ", 1)[1]
        elif line.startswith("pid = "):
            try:
                pid = int(line.split("= ", 1)[1])
            except ValueError:
                pass

    print(f"Production: {state}" + (f" (PID {pid})" if pid else ""))

    # Health check
    try:
        _open_prod_health(timeout=2).close()
        print(f"  Health: OK")
    except Exception:
        print(f"  Health: not responding")

    if sha := read_deployed_sha():
        print(f"  Commit: {sha}")
    print(f"  Port: {PROD_PORT}")
    print(f"  Database: {PROD_DB_PATH}")
    print(f"  Logs: {LAUNCHD_LOG_PATH}")
    print(f"  URL: {_prod_display_url()}")


def launchd_prod_stop():
    """Stop the launchd service."""
    _launchd_stop_if_loaded()
    print(f"Stopped {LAUNCHD_LABEL}")


def launchd_prod_override_set(name: str, value: str):
    """Set an environment variable in the launchd plist and reload."""
    import plistlib

    if not LAUNCHD_PLIST_PATH.exists():
        print("ERROR: No plist found. Run './dev.py prod deploy' first.", file=sys.stderr)
        sys.exit(1)

    with open(LAUNCHD_PLIST_PATH, "rb") as f:
        plist = plistlib.load(f)

    if "EnvironmentVariables" not in plist:
        plist["EnvironmentVariables"] = {}
    plist["EnvironmentVariables"][name] = value

    with open(LAUNCHD_PLIST_PATH, "wb") as f:
        plistlib.dump(plist, f, fmt=plistlib.FMT_XML)

    # Reload service
    _launchd_stop_if_loaded()
    uid = os.getuid()
    subprocess.run(
        ["launchctl", "bootstrap", f"gui/{uid}", str(LAUNCHD_PLIST_PATH)],
        capture_output=True,
    )
    print(f"✓ Set {name}={value}")
    print(f"  Service reloaded")


def launchd_prod_override_unset(name: str):
    """Remove an environment variable from the launchd plist and reload."""
    import plistlib

    if not LAUNCHD_PLIST_PATH.exists():
        print("ERROR: No plist found. Run './dev.py prod deploy' first.", file=sys.stderr)
        sys.exit(1)

    with open(LAUNCHD_PLIST_PATH, "rb") as f:
        plist = plistlib.load(f)

    env_vars = plist.get("EnvironmentVariables", {})
    if name not in env_vars:
        print(f"No override '{name}' found in plist")
        return

    del env_vars[name]

    with open(LAUNCHD_PLIST_PATH, "wb") as f:
        plistlib.dump(plist, f, fmt=plistlib.FMT_XML)

    # Reload service
    _launchd_stop_if_loaded()
    uid = os.getuid()
    subprocess.run(
        ["launchctl", "bootstrap", f"gui/{uid}", str(LAUNCHD_PLIST_PATH)],
        capture_output=True,
    )
    print(f"✓ Removed {name} override")
    print(f"  Service reloaded")


def cmd_prod_build(version: str | None = None):
    """Build production binary from git tag."""
    if sys.platform == "darwin":
        prod_build(version, target=None)
    elif sys.platform == "linux":
        prod_build(version)
    else:
        print(f"Unsupported platform: {sys.platform}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_deploy(version: str | None = None):
    """Build and deploy to production (auto-detects environment)."""
    print("Running pre-deploy checks...\n")
    cmd_check()
    print()

    env = detect_prod_env()

    if env == "launchd":
        launchd_prod_deploy(version)

    elif env == "native":
        native_prod_deploy(version)

    elif env == "daemon":
        print("Detected: No systemd (daemon mode)")
        print("    Running production build as background daemon")
        print()
        prod_daemon_deploy()

    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_status():
    """Show production status (auto-detects environment)."""
    env = detect_prod_env()

    if env == "launchd":
        launchd_prod_status()
    elif env == "native":
        native_prod_status()
    elif env == "daemon":
        prod_daemon_status()
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_stop():
    """Stop production service (auto-detects environment)."""
    env = detect_prod_env()

    if env == "launchd":
        launchd_prod_stop()
    elif env == "native":
        native_prod_stop()
    elif env == "daemon":
        prod_daemon_stop()
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_override_set(name: str, value: str):
    """Set an environment override for the production service."""
    env = detect_prod_env()

    if env == "launchd":
        launchd_prod_override_set(name, value)
    elif env == "native":
        native_prod_override_set(name, value)
    elif env == "daemon":
        print("ERROR: Overrides not supported for daemon mode", file=sys.stderr)
        print("Stop the daemon and restart with environment variables set.", file=sys.stderr)
        sys.exit(1)
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_override_unset(name: str):
    """Remove an environment override from the production service."""
    env = detect_prod_env()

    if env == "launchd":
        launchd_prod_override_unset(name)
    elif env == "native":
        native_prod_override_unset(name)
    elif env == "daemon":
        print("ERROR: Overrides not supported for daemon mode", file=sys.stderr)
        sys.exit(1)
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


# =============================================================================
# Main
# =============================================================================

def main():
    # `taskmd` is a verbatim passthrough to the bundled taskmd CLI — intercept
    # it before argparse so flags like `--version` / `--help` reach taskmd
    # rather than dev.py's own parser (argparse.REMAINDER doesn't handle a
    # leading optional cleanly).
    if len(sys.argv) >= 2 and sys.argv[1] == "taskmd":
        cmd_taskmd(sys.argv[2:])
        return

    parser = argparse.ArgumentParser(prog="dev.py", description="Phoenix development tasks")
    sub = parser.add_subparsers(dest="command", required=True)

    # up
    up_parser = sub.add_parser("up", help="Build and start servers")
    up_parser.add_argument("--port", type=int, default=None, help="Phoenix port (default: auto from worktree hash)")
    up_parser.add_argument("--vite-port", type=int, default=None, help="Vite port (default: auto from worktree hash)")
    up_parser.add_argument("--no-seed", action="store_true", default=False, help="Skip auto-seeding the dev DB on startup")
    up_parser.add_argument("--https", dest="https", action="store_true", default=False, help="Serve Phoenix over auto-managed HTTPS")
    up_parser.add_argument("--tls", dest="https", action="store_true", help=argparse.SUPPRESS)

    # down
    sub.add_parser("down", help="Stop all servers")

    # restart
    restart_parser = sub.add_parser("restart", help="Rebuild Rust and restart Phoenix")
    restart_parser.add_argument("--port", type=int, default=None, help="Phoenix port (default: auto from worktree hash)")
    restart_parser.add_argument("--https", dest="https", action="store_true", default=False, help="Serve Phoenix over auto-managed HTTPS")
    restart_parser.add_argument("--tls", dest="https", action="store_true", help=argparse.SUPPRESS)

    # status
    sub.add_parser("status", help="Check what's running")

    # check
    sub.add_parser("check", help="Run lint, fmt check, and tests")

    # audit-specs (manual / verbose run; the orphan-anchor half also
    # runs as a `spec anchors` lane inside `./dev.py check`)
    audit_parser = sub.add_parser(
        "audit-specs",
        help="Cross-validate REQ-* anchors between specs/ and source code",
    )
    audit_parser.add_argument(
        "--verbose", "-v", action="store_true", default=False,
        help="Also list REQ-IDs declared in specs/ without a source-code anchor",
    )

    # codegen
    sub.add_parser("codegen", help="Regenerate ui/src/generated/ from Rust types (task 02677)")

    # seed (offline)
    sub.add_parser("seed", help="Populate dev DB with representative conversations (offline; refuses if Phoenix is running)")

    # tls
    tls_parser = sub.add_parser("tls", help="Manage Phoenix HTTPS certificates")
    tls_sub = tls_parser.add_subparsers(dest="tls_command", required=True)
    tls_ca = tls_sub.add_parser("ca", help="Create or show the Phoenix private CA")
    tls_ca.add_argument("--dir", type=Path, default=TLS_CA_DIR, help=f"CA directory (default: {TLS_CA_DIR})")
    tls_issue = tls_sub.add_parser("issue", help="Issue a per-host TLS bundle")
    tls_issue.add_argument("host", help="Primary DNS name for the Phoenix host")
    tls_issue.add_argument("--host", dest="extra_hosts", action="append", default=[], help="Additional DNS/IP SAN; repeatable")
    tls_issue.add_argument("--ca-dir", type=Path, default=TLS_CA_DIR, help=f"CA directory (default: {TLS_CA_DIR})")
    tls_issue.add_argument("--out-dir", type=Path, default=TLS_BUNDLE_DIR, help=f"Bundle output directory (default: {TLS_BUNDLE_DIR})")
    tls_issue.add_argument("--port", type=int, default=PROD_PORT, help=f"Public Phoenix port (default: {PROD_PORT})")
    tls_install = tls_sub.add_parser("install", help="Install a TLS bundle into this host's repo/env")
    tls_install.add_argument("bundle", type=Path, help="Bundle created by ./dev.py tls issue")
    tls_install.add_argument("--install-dir", type=Path, default=TLS_INSTALL_DIR, help=f"Certificate install directory (default: {TLS_INSTALL_DIR})")
    tls_install.add_argument("--env-file", type=Path, default=ROOT / ".phoenix-ide.env", help="Env file to update (default: repo .phoenix-ide.env)")

    # prod
    prod_parser = sub.add_parser("prod", help="Production deployment")
    prod_sub = prod_parser.add_subparsers(dest="prod_command", required=True)
    build_parser = prod_sub.add_parser("build", help="Build production binary from git tag")
    build_parser.add_argument("version", nargs="?", help="Git tag (default: HEAD)")
    deploy_parser = prod_sub.add_parser("deploy", help="Build and deploy to production")
    deploy_parser.add_argument("version", nargs="?", help="Git tag (default: HEAD)")
    prod_sub.add_parser("status", help="Show production status")
    prod_sub.add_parser("stop", help="Stop production service")
    # Override management
    override_set_parser = prod_sub.add_parser("set", help="Set environment override")
    override_set_parser.add_argument("name", help="Environment variable name (e.g., RUST_LOG)")
    override_set_parser.add_argument("value", help="Environment variable value (e.g., debug)")
    override_unset_parser = prod_sub.add_parser("unset", help="Remove environment override")
    override_unset_parser.add_argument("name", help="Environment variable name to remove")

    # tasks
    tasks_parser = sub.add_parser("tasks", help="Task management")
    tasks_sub = tasks_parser.add_subparsers(dest="tasks_command", required=True)
    tasks_sub.add_parser("validate", help="Validate task filenames and check for duplicate IDs")
    tasks_sub.add_parser("fix", help="Migrate legacy IDs and renumber duplicate task IDs")

    # taskmd — passthrough to the bundled taskmd CLI (intercepted above before
    # argparse runs; this entry exists only so it shows up in `./dev.py --help`)
    sub.add_parser(
        "taskmd",
        help="Run the bundled taskmd CLI; all args forwarded (e.g. ./dev.py taskmd new --slug fix-x --priority p1)",
        add_help=False,
    )

    args = parser.parse_args()

    if args.command == "up":
        cmd_up(
            phoenix_port=args.port,
            vite_port=args.vite_port,
            no_seed=args.no_seed,
            tls=args.https,
        )
    elif args.command == "down":
        cmd_down()
    elif args.command == "restart":
        cmd_restart(phoenix_port=args.port, tls=args.https)
    elif args.command == "status":
        cmd_status()
    elif args.command == "check":
        cmd_check()
    elif args.command == "codegen":
        if not cmd_codegen():
            sys.exit(1)
    elif args.command == "seed":
        cmd_seed()
    elif args.command == "tls":
        if args.tls_command == "ca":
            cmd_tls_ca(args.dir)
        elif args.tls_command == "issue":
            cmd_tls_issue(
                args.host,
                extra_hosts=args.extra_hosts,
                ca_dir=args.ca_dir,
                out_dir=args.out_dir,
                port=args.port,
            )
        elif args.tls_command == "install":
            cmd_tls_install(args.bundle, install_dir=args.install_dir, env_file=args.env_file)
    elif args.command == "prod":
        if args.prod_command == "build":
            cmd_prod_build(args.version)
        elif args.prod_command == "deploy":
            cmd_prod_deploy(args.version)
        elif args.prod_command == "status":
            cmd_prod_status()
        elif args.prod_command == "stop":
            cmd_prod_stop()
        elif args.prod_command == "set":
            cmd_prod_override_set(args.name, args.value)
        elif args.prod_command == "unset":
            cmd_prod_override_unset(args.name)
    elif args.command == "tasks":
        if args.tasks_command == "validate":
            ok, _ = cmd_tasks_validate()
            if not ok:
                sys.exit(1)
        elif args.tasks_command == "fix":
            if not cmd_tasks_fix():
                sys.exit(1)
    elif args.command == "audit-specs":
        if not cmd_audit_specs(verbose=args.verbose):
            sys.exit(1)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as e:
        cmd = " ".join(str(a) for a in e.cmd)
        print(f"ERROR: command failed (exit {e.returncode}): {cmd}", file=sys.stderr)
        if e.stderr:
            stderr = e.stderr if isinstance(e.stderr, str) else e.stderr.decode(errors="replace")
            print(stderr, file=sys.stderr, end="")
        sys.exit(e.returncode)
