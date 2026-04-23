#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "taskmd",
# ]
# ///
"""Development tasks for phoenix-ide."""

import argparse
import dataclasses
import fcntl
import hashlib
import json
import os
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
LOG_FILE = ROOT / "phoenix.log"


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

# Production paths
PROD_SERVICE_NAME = "phoenix-ide"
PROD_INSTALL_DIR = Path("/opt/phoenix-ide")
PROD_DB_PATH = Path.home() / ".phoenix-ide" / "prod.db"
PROD_PORT = 8031

# Lima VM configuration (dev environment only — create/shell/destroy)
LIMA_VM_NAME = "phoenix-ide"
LIMA_YAML = ROOT / "lima" / "phoenix-ide.yaml"

# launchd (native macOS) configuration
LAUNCHD_LABEL = "com.phoenix-ide.server"
LAUNCHD_PLIST_PATH = Path.home() / "Library" / "LaunchAgents" / f"{LAUNCHD_LABEL}.plist"
LAUNCHD_INSTALL_DIR = Path.home() / ".phoenix-ide"
LAUNCHD_LOG_PATH = Path.home() / ".phoenix-ide" / "prod.log"
PROD_SHA_PATH = Path.home() / ".phoenix-ide" / "deployed.sha"

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
        # Release database lock if stopping Phoenix
        if name == "Phoenix" and _db_lock is not None:
            _db_lock.release()
            _db_lock = None
    return True


def ensure_ui_deps():
    """Ensure UI dependencies are installed."""
    if not (UI_DIR / "node_modules").exists():
        print("Installing UI dependencies...")
        subprocess.run(["npm", "install"], cwd=UI_DIR, check=True, env=node_env())


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


def start_phoenix(port: int, release: bool = True):
    """Start the Phoenix server."""
    global _db_lock
    
    if get_pid(PHOENIX_PID_FILE):
        print("Phoenix server already running")
        return

    binary = ROOT / "target" / ("release" if release else "debug") / "phoenix_ide"
    if not binary.exists():
        print(f"Binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    # Acquire database lock
    db_path = get_db_path()
    _db_lock = DatabaseLock()
    if not _db_lock.acquire():
        print(f"ERROR: Database is locked by another process.", file=sys.stderr)
        print(f"  Lock file: {get_lock_path()}", file=sys.stderr)
        print(f"  Run './dev.py down' in the other instance first.", file=sys.stderr)
        sys.exit(1)

    env = os.environ.copy()
    # Load .phoenix-ide.env overrides (LLM_API_KEY_HELPER, base URLs, etc.)
    env_file = _load_env_file(env)
    if env_file:
        print(f"  Loaded env from {env_file}")
    # Auto-detect gateway only if .phoenix-ide.env didn't provide LLM config
    if not env.get("LLM_API_KEY_HELPER") and not env.get("LLM_GATEWAY"):
        if gateway := get_llm_gateway():
            env["LLM_GATEWAY"] = gateway
    env["PHOENIX_PORT"] = str(port)
    env["PHOENIX_DB_PATH"] = str(db_path)
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


def start_vite(port: int, phoenix_port: int):
    """Start the Vite dev server."""
    if get_pid(VITE_PID_FILE):
        print("Vite dev server already running")
        return

    ensure_ui_deps()

    env = os.environ.copy()
    # Pass Phoenix port to Vite for proxy configuration
    env["VITE_API_PORT"] = str(phoenix_port)
    
    # Start Vite in background (bind to 0.0.0.0 for external access)
    proc = subprocess.Popen(
        ["npm", "run", "dev", "--", "--port", str(port), "--host", "0.0.0.0"],
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
    print(f"  Proxying /api to Phoenix on port {phoenix_port}")


# =============================================================================
# Commands
# =============================================================================

def cmd_up(phoenix_port: int | None = None, vite_port: int | None = None):
    """Build and start Phoenix + Vite dev servers."""
    default_phoenix, default_vite = get_default_ports()
    phoenix_port = phoenix_port or default_phoenix
    vite_port = vite_port or default_vite
    
    print(f"Worktree: {ROOT}")
    print(f"  Hash: {get_worktree_hash()}, Port offset: +{get_port_offset()}")
    print()
    
    build_rust(release=True)
    start_phoenix(port=phoenix_port)
    start_vite(port=vite_port, phoenix_port=phoenix_port)
    print()
    print(f"Ready! UI: http://localhost:{vite_port}")
    print(f"        API: http://localhost:{phoenix_port}")
    print(f"        Log: {LOG_FILE}")


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


def cmd_restart(phoenix_port: int | None = None):
    """Rebuild Rust and restart Phoenix (Vite stays for hot reload)."""
    default_phoenix, default_vite = get_default_ports()
    phoenix_port = phoenix_port or default_phoenix

    build_rust(release=True)
    stop_process(PHOENIX_PID_FILE, "Phoenix")
    time.sleep(0.5)
    start_phoenix(port=phoenix_port)
    vite_pid = get_pid(VITE_PID_FILE)
    if vite_pid:
        print(f"Phoenix restarted. Vite still running for UI hot reload.")
        print(f"  UI:  http://localhost:{default_vite}")
        print(f"  API: http://localhost:{phoenix_port}")
    else:
        print(f"Phoenix restarted. Vite not running (start with ./dev.py up).")
        print(f"  API: http://localhost:{phoenix_port}")


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
    else:
        print("Phoenix: stopped")

    if vite_pid:
        print(f"Vite:    running (PID {vite_pid})")
    else:
        print("Vite:    stopped")

    if phoenix_pid:
        try:
            import urllib.request
            with urllib.request.urlopen(f"http://localhost:{default_phoenix}/api/models", timeout=2) as resp:
                data = json.loads(resp.read())
                print(f"Models:  {', '.join(data.get('models', []))}")
        except Exception:
            pass


def cmd_check():
    """Run lint, format check, tests, and task validation in parallel."""
    results = []  # (name, returncode, elapsed, output)
    results_lock = threading.Lock()
    t_start = time.monotonic()

    CHECK_TIMEOUT = 300  # 5 minutes per step -- kill and fail if exceeded

    def run_step(name, cmd, cwd=ROOT):
        t0 = time.monotonic()
        try:
            # UI steps need the correct Node.js version on PATH
            env = node_env() if Path(cwd) == UI_DIR else None
            proc = subprocess.run(
                cmd, cwd=cwd, capture_output=True, text=True,
                timeout=CHECK_TIMEOUT, env=env,
            )
            elapsed = time.monotonic() - t0
            output = (proc.stdout + proc.stderr).strip()
            rc = proc.returncode
        except subprocess.TimeoutExpired:
            elapsed = time.monotonic() - t0
            output = f"TIMEOUT after {CHECK_TIMEOUT}s"
            rc = 1
        with results_lock:
            ok = "\u2713" if rc == 0 else "\u2717"
            results.append((name, rc, elapsed, output))
            print(f"  {ok} {name:<18s} ({elapsed:.1f}s)")

    def lane_rust():
        """Rust lane: clippy → musl smoke check → test compile → test run → codegen staleness check.

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
            run_step("cargo check musl", [
                "cargo", "check", "--target", "x86_64-unknown-linux-musl",
            ])
        else:
            run_step("cargo check musl", ["cargo", "check"])
        has_nextest = subprocess.run(
            ["cargo", "nextest", "--version"],
            capture_output=True,
        ).returncode == 0
        if has_nextest:
            compile_cmd = ["cargo", "nextest", "run", "--no-run"]
            test_cmd = ["cargo", "nextest", "run"]
        else:
            compile_cmd = ["cargo", "test", "--no-run"]
            test_cmd = ["cargo", "test"]
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

    def lane_fast():
        """Fast lane: cargo fmt then task validation."""
        run_step("cargo fmt", ["cargo", "fmt", "--check"])
        # Task validation (Python, not a subprocess)
        t0 = time.monotonic()
        ok = cmd_tasks_validate(quiet=True)
        elapsed = time.monotonic() - t0
        with results_lock:
            sym = "\u2713" if ok else "\u2717"
            results.append(("task validation", 0 if ok else 1, elapsed, ""))
            print(f"  {sym} {'task validation':<18s} ({elapsed:.1f}s)")

    def check_package_lock_clean():
        """Tripwire: fail if `ui/package-lock.json` has uncommitted changes.

        `./dev.py prod deploy` builds in a fresh worktree and runs `npm ci`,
        which is strict about lockfile sync. Local-only additions (e.g. a
        native dep that adds transitive packages on the current platform)
        can leave the developer's lock drifted from HEAD without `./dev.py
        check` noticing, because vitest / eslint / tsc all tolerate the
        drift. `npm ci` in the build worktree then fails the deploy.
        """
        run_step("pkglock-clean", ["bash", "-c", (
            'out=$(git status --porcelain -- ui/package-lock.json); '
            'if [ -n "$out" ]; then '
            '  echo "ui/package-lock.json has uncommitted changes:"; '
            '  echo "$out"; '
            '  echo ""; '
            '  echo "Commit these before deploying, or \'npm ci\' in the build worktree will fail."; '
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

    print("Running checks in parallel...\n")

    threads = [
        threading.Thread(target=lane_rust),
        threading.Thread(target=run_step, args=("tsc typecheck", ["npx", "tsc", "-b", "--noEmit"], UI_DIR)),
        threading.Thread(target=run_step, args=("eslint", ["npm", "run", "lint"], UI_DIR)),
        threading.Thread(target=run_step, args=("vitest", ["npx", "vitest", "run"], UI_DIR)),
        threading.Thread(target=lane_fast),
        threading.Thread(target=check_ast_grep),
        threading.Thread(target=check_package_lock_clean),
    ]
    for t in threads:
        t.start()
    for t in threads:
        # Timeout on join so Ctrl+C is responsive (subprocess.run timeout
        # handles the actual deadline; this just prevents infinite wait
        # if a thread somehow survives)
        t.join(timeout=CHECK_TIMEOUT + 30)

    total_elapsed = time.monotonic() - t_start
    failures = [(n, out) for n, rc, _, out in results if rc != 0]

    if failures:
        print()
        for name, output in failures:
            print(f"\u2500\u2500 {name} {'\u2500' * (50 - len(name))}")
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
# Implementation report (taskmd integration):
#
# API surface used:
#   - taskmd.validate(tasks_dir) -> ValidationResult  (.ok, .errors, .file_count)
#   - taskmd.fix(tasks_dir) -> FixResult  (.ok, .errors, .patched, .renamed)
#   - taskmd.VALID_STATUSES, taskmd.VALID_PRIORITIES  (frozensets)
#
# API gaps:
#   - No way to pass a custom filename regex to validate/fix. Phoenix migrated
#     from 3-digit single-dash to 4-digit double-dash format to match taskmd's
#     built-in pattern exactly, so no gap remains post-migration.
#
# API friction:
#   - taskmd.validate() prints nothing and returns structured data; dev.py's
#     cmd_tasks_validate() has a quiet=True codepath used by cmd_check that
#     suppresses output. The mapping is clean: always call taskmd.validate(),
#     then conditionally print based on quiet.
#   - cmd_tasks_fix() previously printed per-file rename lines inline. taskmd's
#     FixResult only gives aggregate counts (patched, renamed), not per-file
#     detail. The old behavior printed "  old.md -> new.md" for each rename;
#     that granularity is lost. The aggregate summary is sufficient.
#   - taskmd.VALID_STATUSES excludes "pending" (Phoenix used it). Migration
#     converted all "pending" files to "ready" in both filename and frontmatter.
#
# Suggestions for taskmd:
#   - Add FixResult.renames: list[tuple[str, str]] so callers can display
#     per-file rename detail without re-implementing scan logic.
#   - Consider exposing ValidationResult.file_count as the count of files
#     examined (currently it is, but document it clearly — it counts all task
#     files seen, not just those with errors).
#   - A FixResult.summary() -> str convenience method returning the canonical
#     "patched N file(s), renamed M file(s)" string would reduce boilerplate.


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


def cmd_tasks_validate(quiet: bool = False) -> bool:
    """Validate all task files using taskmd.

    Returns True if all tasks pass, False otherwise.
    """
    import taskmd
    tasks_dir = ROOT / "tasks"
    result = taskmd.validate(tasks_dir)

    if not result.ok:
        if not quiet:
            print(f"✗ {len(result.errors)} task validation error(s):")
            for err in result.errors:
                print(f"  - {err}")
            print("\nRun './dev.py tasks fix' to auto-fix (injects missing 'created', renames files).")
        return False

    if not quiet:
        print(f"✓ {result.file_count} task files validated")
    return True


def cmd_tasks_fix() -> bool:
    """Auto-fix task files using taskmd: inject missing 'created' and rename to match frontmatter.

    Returns True if all files are now correct, False on errors.
    """
    import taskmd
    tasks_dir = ROOT / "tasks"
    result = taskmd.fix(tasks_dir)

    if not result.ok:
        print(f"\n✗ {len(result.errors)} error(s):")
        for err in result.errors:
            print(f"  - {err}")
        return False

    if result.patched or result.renamed:
        parts = []
        if result.patched:
            parts.append(f"patched {result.patched} file(s)")
        if result.renamed:
            parts.append(f"renamed {result.renamed} file(s)")
        print(f"\n✓ {', '.join(parts).capitalize()}")
    else:
        print("✓ All files already correctly named")
    return True



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
    
    # Build UI
    npm_env = node_env()
    print("Installing UI dependencies...")
    result = subprocess.run(["npm", "ci"], cwd=ui_dir, capture_output=True, text=True, env=npm_env)
    if result.returncode != 0:
        print(result.stdout, end="")
        print(result.stderr, file=sys.stderr, end="")
        raise SystemExit(f"npm ci failed (exit {result.returncode})")

    print("Building UI...")
    subprocess.run(["npm", "run", "build"], cwd=ui_dir, check=True, env=npm_env)
    
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


def detect_service_user() -> str:
    """Detect which service user to run phoenix-ide as.

    Checks for supported users in priority order. Fails with a clear message
    if none exist so the operator knows what to create.
    """
    import pwd
    for candidate in ("phoenix-dev", "exedev"):
        try:
            pwd.getpwnam(candidate)
            return candidate
        except KeyError:
            continue
    print(
        "ERROR: No supported service user found. "
        "Create one of: phoenix-dev, exedev\n"
        "  e.g.: sudo useradd --system --no-create-home phoenix-dev",
        file=sys.stderr,
    )
    sys.exit(1)


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
# SIGHUP triggers graceful shutdown; systemd restarts with same socket
ExecReload=/bin/kill -HUP $MAINPID
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
    subprocess.run(["sudo", "chown", f"{service_user}:{service_user}", str(native_db_dir)], check=True)

    # Configure for native deployment.
    # OAuth token auth: the binary reads ~/.claude/.credentials.json per request.
    # Requires: chmod g+r ~/.claude/.credentials.json + service user in owner's group.
    # See skills/phoenix-deployment/SYSTEMD.md for setup instructions.
    config = dataclasses.replace(
        NATIVE_SYSTEMD_CONFIG,
        user=service_user,
        db_path=str(native_db_path),
        llm_gateway=get_llm_gateway(),
        home_dir=str(Path.home()),
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
        subprocess.run(["sudo", "systemctl", "reload", PROD_SERVICE_NAME], check=True)
        
        # Wait briefly for restart
        time.sleep(2)
        
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
            print(f"  URL: http://localhost:{PROD_PORT}")
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
            print(f"  URL: http://localhost:{PROD_PORT}")
        else:
            print(f"\n✗ Service failed to start", file=sys.stderr)
            subprocess.run(["sudo", "journalctl", "-u", PROD_SERVICE_NAME, "-n", "20", "--no-pager"])
            sys.exit(1)


def _load_env_file(env: dict[str, str]) -> str | None:
    """Load .phoenix-ide.env from project root into env dict. Returns path if loaded.

    Simple KEY=VALUE format, one per line. Lines starting with # are comments.
    Literal \\n in values is unescaped to real newlines (for LLM_CUSTOM_HEADERS).
    """
    env_file = ROOT / ".phoenix-ide.env"
    if not env_file.exists():
        return None
    with open(env_file) as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            key, _, value = line.partition("=")
            if key and value:
                env[key.strip()] = value.strip().replace("\\n", "\n")
    return str(env_file)


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

    prod_db_path = prod_dir / "prod.db"  # Consistent with native/lima
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
        with urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=5) as resp:
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
            urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=2).close()
            print(f"  Health: OK")
        except Exception as e:
            print(f"  Health: Unreachable ({type(e).__name__}: {e})")
        print(f"  Port: {PROD_PORT}")
        print(f"  URL: http://localhost:{PROD_PORT}")

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
        print(f"  URL: http://localhost:{PROD_PORT}")
        print(f"  Database: {PROD_DB_PATH}")

        # Health check
        try:
            import urllib.request
            urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=2).close()
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


def generate_launchd_plist(version: str, llm_gateway: str | None, extra_env: dict[str, str] | None = None) -> str:
    """Generate a launchd plist for the Phoenix IDE server."""
    env_vars = {
        "PHOENIX_DB_PATH": str(PROD_DB_PATH),
        "PHOENIX_PORT": str(PROD_PORT),
        "PHOENIX_VERSION": version,
    }
    if llm_gateway:
        env_vars["LLM_GATEWAY"] = llm_gateway
    # Merge .phoenix-ide.env overrides (LLM_API_KEY_HELPER, base URLs, etc.)
    if extra_env:
        env_vars.update(extra_env)

    env_xml = "\n".join(
        f"      <key>{k}</key>\n      <string>{v}</string>"
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

    # Auto-detect gateway only if env file didn't provide LLM config
    gateway = None
    if not env_overrides.get("LLM_API_KEY_HELPER") and not env_overrides.get("LLM_GATEWAY"):
        gateway = get_llm_gateway()

    # Generate and write plist
    plist_content = generate_launchd_plist(version, gateway, env_overrides)
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
    import urllib.request
    health_version = None
    for attempt in range(5):
        time.sleep(2)
        try:
            with urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=5) as resp:
                health_version = resp.read().decode().strip()
            break
        except Exception:
            if attempt < 4:
                continue
            print("WARNING: Server started but health check failed after 10s", file=sys.stderr)

    write_deployed_sha()
    if env_overrides.get("LLM_API_KEY_HELPER"):
        llm_mode = "api_key_helper (from .phoenix-ide.env)"
    elif gateway:
        llm_mode = f"gateway ({gateway})"
    else:
        llm_mode = "no gateway detected"
    print(f"\n✓ Deployed {version} to production (launchd)")
    if health_version:
        print(f"  Version: {health_version}")
    print(f"  Database: {PROD_DB_PATH}")
    print(f"  Logs: {LAUNCHD_LOG_PATH}")
    print(f"  LLM: {llm_mode}")
    print(f"  URL: http://localhost:{PROD_PORT}")


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
        import urllib.request
        urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=2).close()
        print(f"  Health: OK")
    except Exception:
        print(f"  Health: not responding")

    if sha := read_deployed_sha():
        print(f"  Commit: {sha}")
    print(f"  Port: {PROD_PORT}")
    print(f"  Database: {PROD_DB_PATH}")
    print(f"  Logs: {LAUNCHD_LOG_PATH}")
    print(f"  URL: http://localhost:{PROD_PORT}")


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
# Lima VM Commands
# =============================================================================

def lima_shell_quiet(cmd: str, check=True) -> subprocess.CompletedProcess:
    """Run a command inside the Lima VM, capturing output."""
    return subprocess.run(
        ["limactl", "shell", "--workdir", "/", LIMA_VM_NAME, "--", "bash", "-lc", cmd],
        check=check, capture_output=True, text=True,
    )


def lima_is_running() -> bool:
    """Check if the Lima VM is running."""
    result = subprocess.run(
        ["limactl", "list", "--json"],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        return False
    for line in result.stdout.strip().splitlines():
        try:
            vm = json.loads(line)
        except json.JSONDecodeError:
            continue
        if vm.get("name") == LIMA_VM_NAME and vm.get("status") == "Running":
            return True
    return False


def lima_vm_exists() -> bool:
    """Check if the Lima VM exists (any status)."""
    result = subprocess.run(
        ["limactl", "list", "--json"],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        return False
    for line in result.stdout.strip().splitlines():
        try:
            vm = json.loads(line)
        except json.JSONDecodeError:
            continue
        if vm.get("name") == LIMA_VM_NAME:
            return True
    return False


def lima_ensure_running():
    """Start VM if stopped, error if not created."""
    if lima_is_running():
        return
    if not lima_vm_exists():
        print("Lima VM does not exist. Run './dev.py lima create' first.", file=sys.stderr)
        sys.exit(1)
    print("Starting Lima VM...")
    subprocess.run(["limactl", "start", LIMA_VM_NAME], check=True)


def cmd_lima_create():
    """Create and provision the Lima VM."""
    # Check limactl exists
    result = subprocess.run(["limactl", "--version"], capture_output=True)
    if result.returncode != 0:
        print("limactl not found. Install Lima: brew install lima", file=sys.stderr)
        sys.exit(1)

    if lima_vm_exists():
        print(f"VM '{LIMA_VM_NAME}' already exists.")
        if not lima_is_running():
            print("Starting VM...")
            subprocess.run(["limactl", "start", LIMA_VM_NAME], check=True)
        print("VM is running.")
        return

    print(f"Creating Lima VM '{LIMA_VM_NAME}' from {LIMA_YAML}...")
    subprocess.run(
        ["limactl", "create", f"--name={LIMA_VM_NAME}", str(LIMA_YAML)],
        check=True,
    )

    print("Starting VM...")
    subprocess.run(["limactl", "start", LIMA_VM_NAME], check=True)

    # Verify provisioning
    print("\nVerifying provisioning...")
    result = lima_shell_quiet("rustc --version", check=False)
    if result.returncode == 0:
        print(f"  Rust: {result.stdout.strip()}")
    else:
        print("  WARNING: Rust not found. Provisioning may have failed.", file=sys.stderr)

    result = lima_shell_quiet("uname -r", check=False)
    if result.returncode == 0:
        print(f"  Kernel: {result.stdout.strip()}")

    result = lima_shell_quiet("node --version", check=False)
    if result.returncode == 0:
        print(f"  Node: {result.stdout.strip()}")

    print(f"\nVM '{LIMA_VM_NAME}' is ready.")


def cmd_lima_shell():
    """Open an interactive shell in the Lima VM."""
    lima_ensure_running()
    os.execvp("limactl", [
        "limactl", "shell", "--workdir", "/", LIMA_VM_NAME,
    ])


def cmd_lima_destroy():
    """Delete the Lima VM."""
    if not lima_vm_exists():
        print(f"VM '{LIMA_VM_NAME}' does not exist.")
        return

    print(f"Deleting VM '{LIMA_VM_NAME}'...")
    subprocess.run(["limactl", "delete", LIMA_VM_NAME, "--force"], check=True)
    print("VM deleted.")


# =============================================================================
# Main
# =============================================================================

def main():
    parser = argparse.ArgumentParser(prog="dev.py", description="Phoenix development tasks")
    sub = parser.add_subparsers(dest="command", required=True)

    # up
    up_parser = sub.add_parser("up", help="Build and start servers")
    up_parser.add_argument("--port", type=int, default=None, help="Phoenix port (default: auto from worktree hash)")
    up_parser.add_argument("--vite-port", type=int, default=None, help="Vite port (default: auto from worktree hash)")

    # down
    sub.add_parser("down", help="Stop all servers")

    # restart
    restart_parser = sub.add_parser("restart", help="Rebuild Rust and restart Phoenix")
    restart_parser.add_argument("--port", type=int, default=None, help="Phoenix port (default: auto from worktree hash)")

    # status
    sub.add_parser("status", help="Check what's running")

    # check
    sub.add_parser("check", help="Run lint, fmt check, and tests")

    # codegen
    sub.add_parser("codegen", help="Regenerate ui/src/generated/ from Rust types (task 02677)")

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

    # lima
    lima_parser = sub.add_parser("lima", help="Lima VM management")
    lima_sub = lima_parser.add_subparsers(dest="lima_command", required=True)
    lima_sub.add_parser("create", help="Create and provision Lima VM")
    lima_sub.add_parser("shell", help="Open shell in Lima VM")
    lima_sub.add_parser("destroy", help="Delete Lima VM")

    # tasks
    tasks_parser = sub.add_parser("tasks", help="Task management")
    tasks_sub = tasks_parser.add_subparsers(dest="tasks_command", required=True)
    tasks_sub.add_parser("validate", help="Validate task file naming and frontmatter")
    tasks_sub.add_parser("fix", help="Auto-rename task files to match frontmatter")

    args = parser.parse_args()

    if args.command == "up":
        cmd_up(phoenix_port=args.port, vite_port=args.vite_port)
    elif args.command == "down":
        cmd_down()
    elif args.command == "restart":
        cmd_restart(phoenix_port=args.port)
    elif args.command == "status":
        cmd_status()
    elif args.command == "check":
        cmd_check()
    elif args.command == "codegen":
        if not cmd_codegen():
            sys.exit(1)
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
    elif args.command == "lima":
        if args.lima_command == "create":
            cmd_lima_create()
        elif args.lima_command == "shell":
            cmd_lima_shell()
        elif args.lima_command == "destroy":
            cmd_lima_destroy()
    elif args.command == "tasks":
        if args.tasks_command == "validate":
            if not cmd_tasks_validate():
                sys.exit(1)
        elif args.tasks_command == "fix":
            if not cmd_tasks_fix():
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
