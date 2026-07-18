#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "taskmd>=1.0,<2",
# ]
# ///
"""Development tasks for phoenix-ide."""

import argparse
import contextlib
import dataclasses
import datetime
import enum
import fcntl
import hashlib
import ipaddress
import json
import os
import plistlib
import re
import shutil
import signal
import socket
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
PHOENIX_PORT_FILE = ROOT / ".phoenix.port"
VITE_PORT_FILE = ROOT / ".vite.port"
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
SYSTEMD_DATA_DIR = Path("/var/lib/phoenix-ide")
SYSTEMD_DB_PATH = SYSTEMD_DATA_DIR / "prod.db"
SYSTEMD_DEPLOY_DIR = Path("/var/lib/phoenix-ide-deploy")
SYSTEMD_TRANSACTION_ROOT = SYSTEMD_DEPLOY_DIR / "transactions"
SYSTEMD_STATUS_PATH = SYSTEMD_DEPLOY_DIR / "status.json"
SYSTEMD_ACTIVE_PATH = SYSTEMD_DEPLOY_DIR / "active"
SYSTEMD_ACTIVATION_LOCK_PATH = SYSTEMD_DEPLOY_DIR / "activation.lock"
SYSTEMD_CLAIM_LOCK_PATH = SYSTEMD_DEPLOY_DIR / "claim.lock"
SYSTEMD_DEPLOYED_SHA_PATH = SYSTEMD_DATA_DIR / "deployed.sha"
SYSTEMD_SERVICE_PATH = Path(f"/etc/systemd/system/{PROD_SERVICE_NAME}.service")
SYSTEMD_SOCKET_PATH = Path(f"/etc/systemd/system/{PROD_SERVICE_NAME}.socket")
SYSTEMD_HELPER_SOURCE = ROOT / "scripts/systemd_deploy_helper.py"
SYSTEMD_HANDOFF_PROTOCOL_VERSION = 1

# launchd (native macOS) configuration
LAUNCHD_LABEL = "com.phoenix-ide.server"
LAUNCHD_PLIST_PATH = Path.home() / "Library" / "LaunchAgents" / f"{LAUNCHD_LABEL}.plist"
LAUNCHD_INSTALL_DIR = Path.home() / ".phoenix-ide"
LAUNCHD_LOG_PATH = Path.home() / ".phoenix-ide" / "prod.log"
PROD_SHA_PATH = Path.home() / ".phoenix-ide" / "deployed.sha"
LAUNCHD_DEPLOY_DIR = Path.home() / ".phoenix-ide" / "deploy"
LAUNCHD_DEPLOY_STATUS_PATH = LAUNCHD_DEPLOY_DIR / "status.json"
LAUNCHD_DEPLOY_LOCK_PATH = LAUNCHD_DEPLOY_DIR / "activate.lock"
LAUNCHD_DEPLOY_CLAIM_LOCK_PATH = LAUNCHD_DEPLOY_DIR / "claim.lock"
LAUNCHD_DEPLOY_ACTIVE_PATH = LAUNCHD_DEPLOY_DIR / "active"
LAUNCHD_DEPLOY_HELPER_PREFIX = "com.phoenix-ide.deploy"
LAUNCHD_HANDOFF_PROTOCOL_VERSION = 1
NEWSYSLOG_CONF_PATH = Path("/etc/newsyslog.d") / f"{LAUNCHD_LABEL}.conf"

# Dev ports are assigned deterministically from the worktree path hash. Keep
# Phoenix and Vite in disjoint blocks so each worktree has a stable pair while
# avoiding the birthday-problem collisions we saw with a 9-slot pool.
#
# Workspace port forwarding currently exposes 8000-8050 plus 8080/8443. Reserve
# 8031 for prod and use most of the remaining 8000-series ports for dev:
#   Phoenix API: 8000-8030  (31 slots)
#   Vite:        8032-8050  (19 slots)
# The two ports are derived from different hash slices so worktrees with the
# same Phoenix slot do not necessarily collide on Vite too.
BASE_PHOENIX_PORT = 8000
PHOENIX_PORT_RANGE = 31
BASE_VITE_PORT = 8032
VITE_PORT_RANGE = 19
DEV_PORT_MIN = 8000
DEV_PORT_MAX = 8050

# Database directory
DB_DIR = Path.home() / ".phoenix-ide"
# Dev-server registry. Lives in $HOME, NOT in the worktree, so the kill-handle
# for a spawned server survives the worktree being deleted out from under it
# (the in-worktree .pid files do not). `./dev.py reap` reads it to clean up
# servers orphaned when a worktree is removed without `./dev.py down` first.
DEV_REGISTRY_DIR = DB_DIR / "dev-registry"
TLS_CA_DIR = DB_DIR / "tls"
TLS_BUNDLE_DIR = DB_DIR / "tls-bundles"
TLS_INSTALL_DIR = DB_DIR / "tls"


def write_deployed_sha(sha: str | None = None):
    """Record an explicitly selected SHA, or local HEAD for non-launchd modes."""
    if sha is None:
        sha = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True
        ).stdout.strip()
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


def get_worktree_hash() -> str:
    """Get a short hash of the worktree path for unique identification."""
    return hashlib.md5(str(ROOT).encode()).hexdigest()[:8]


def get_port_offsets() -> tuple[int, int]:
    """Get deterministic Phoenix/Vite port offsets from different hash slices."""
    worktree_hash = get_worktree_hash()
    phoenix_offset = int(worktree_hash[:4], 16) % PHOENIX_PORT_RANGE
    vite_offset = int(worktree_hash[4:8], 16) % VITE_PORT_RANGE
    return phoenix_offset, vite_offset


def _port_is_available(port: int) -> bool:
    """Return true when a dev server can bind the port on every interface it uses.

    Probes both the wildcard and loopback address with no SO_REUSEADDR. Dev
    servers bind 127.0.0.1 (or 0.0.0.0 when the API is password-protected), but on
    BSD/macOS a wildcard bind *with* SO_REUSEADDR silently coexists with an
    existing loopback-specific bind — so the old REUSEADDR wildcard probe reported
    a loopback-squatted port (e.g. an orphaned Vite from another worktree) as
    free. The forward-walk in `_select_available_port` then never fired and the
    server died on EADDRINUSE. Binding each interface without REUSEADDR makes the
    probe fail iff a server already holds the port on either one.

    SO_REUSEADDR cannot be reinstated to soften the TIME_WAIT case below: an
    empirical check shows a loopback bind *with* REUSEADDR coexists with a live
    *wildcard* listener (it simply moves the coexistence false-positive from the
    loopback axis to the wildcard one), so a 0.0.0.0 squatter would again probe
    free. The accepted cost of no REUSEADDR: a port still in TIME_WAIT from a
    just-exited dev server probes unavailable, so a rapid down→up that re-runs
    selection forward-walks to the next deterministic port until TIME_WAIT drains
    (~30-60s). Self-healing, and the stored per-worktree port short-circuits the
    common restart path before selection runs.
    """
    for host in ("0.0.0.0", "127.0.0.1"):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            try:
                sock.bind((host, port))
            except OSError:
                return False
    return True


def _listening_pids(port: int) -> list[int]:
    """PIDs holding a LISTEN socket on the TCP port ([] on lsof failure)."""
    try:
        out = subprocess.run(
            ["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-t"],
            capture_output=True,
            text=True,
            timeout=5,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    return [int(x) for x in out.split() if x.strip().isdigit()]


def _is_descendant_of(pid: int, ancestor: int) -> bool:
    """True if `pid` is `ancestor` itself or anywhere below it in the process tree."""
    seen: set[int] = set()
    cur = pid
    while cur and cur > 1 and cur not in seen:
        if cur == ancestor:
            return True
        seen.add(cur)
        try:
            out = subprocess.run(
                ["ps", "-o", "ppid=", "-p", str(cur)],
                capture_output=True,
                text=True,
                timeout=5,
            ).stdout.strip()
        except (OSError, subprocess.SubprocessError):
            return False
        cur = int(out) if out.isdigit() else 0
    return cur == ancestor


def _wait_for_port(
    host: str, port: int, proc: "subprocess.Popen", timeout: float = 15.0
) -> bool:
    """Poll until *our* server bound host:port, or proc dies, or timeout.

    A `--strictPort` dev server that loses a port race exits on EADDRINUSE.
    Checking the spawned PID alone misses this when that PID is a wrapper (pnpm)
    whose child died but who lingers: the wrapper looks alive, so the launch is
    reported as success while an orphan keeps serving stale code.

    A bare connect is not enough either: if an orphan already squats the port,
    our child dies on EADDRINUSE while the wrapper lingers (`proc.poll()` is
    None) and the connect reaches the *squatter*, returning 0 — proving only that
    *a* server bound, not ours. So on a successful connect we additionally require
    a process listening on the port to be within `proc`'s tree (the wrapper's
    child server), which the squatter cannot satisfy.
    """
    probe_host = "127.0.0.1" if host == "0.0.0.0" else host
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            return False
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.settimeout(0.5)
            connected = sock.connect_ex((probe_host, port)) == 0
        if connected and any(
            _is_descendant_of(p, proc.pid) for p in _listening_pids(port)
        ):
            return True
        time.sleep(0.2)
    return False


def _select_available_port(base: int, range_size: int, preferred_offset: int, reserved: set[int]) -> tuple[int, int]:
    """Pick a deterministic available port, probing forward on collisions."""
    for step in range(range_size):
        offset = (preferred_offset + step) % range_size
        port = base + offset
        if port in reserved or port == PROD_PORT:
            continue
        if _port_is_available(port):
            return port, offset
    first = base
    last = base + range_size - 1
    print(f"ERROR: no available dev port in range {first}-{last}.", file=sys.stderr)
    sys.exit(1)


def get_port_offset() -> int:
    """Get the Phoenix port offset for legacy status output."""
    phoenix_offset, _ = get_port_offsets()
    return phoenix_offset


def get_default_ports() -> tuple[int, int]:
    """Get deterministic hash-based Phoenix and Vite ports for this worktree."""
    phoenix_offset, vite_offset = get_port_offsets()
    phoenix = BASE_PHOENIX_PORT + phoenix_offset
    vite = BASE_VITE_PORT + vite_offset
    for name, port in [("Phoenix", phoenix), ("Vite", vite)]:
        if port == PROD_PORT:
            print(f"ERROR: {name} port {port} collides with prod port {PROD_PORT}.", file=sys.stderr)
            print(f"  Worktree hash produced offsets Phoenix={phoenix_offset}, Vite={vite_offset}. Use --port to override.", file=sys.stderr)
            sys.exit(1)
        if not (DEV_PORT_MIN <= port <= DEV_PORT_MAX):
            print(f"ERROR: {name} port {port} outside allowed range {DEV_PORT_MIN}-{DEV_PORT_MAX}.", file=sys.stderr)
            print(f"  Worktree hash produced offsets Phoenix={phoenix_offset}, Vite={vite_offset}. Use --port to override.", file=sys.stderr)
            sys.exit(1)
    return (phoenix, vite)


def select_dev_ports(preferred_phoenix: int, preferred_vite: int) -> tuple[int, int]:
    """Select available dev ports, preferring the deterministic hash-based pair."""
    preferred_phoenix_offset = preferred_phoenix - BASE_PHOENIX_PORT
    preferred_vite_offset = preferred_vite - BASE_VITE_PORT
    phoenix, _ = _select_available_port(
        BASE_PHOENIX_PORT,
        PHOENIX_PORT_RANGE,
        preferred_phoenix_offset,
        reserved={PROD_PORT},
    )
    vite, _ = _select_available_port(
        BASE_VITE_PORT,
        VITE_PORT_RANGE,
        preferred_vite_offset,
        reserved={PROD_PORT, phoenix},
    )
    return phoenix, vite


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


def kill_process_group(pid: int) -> bool:
    """SIGTERM a process's whole group, escalating to SIGKILL if it lingers.

    Kills the group (not just the pid) to catch child workers — e.g. Vite's
    pnpm parent spawns node children that survive if only the parent is killed.
    Servers are started with start_new_session=True, so each is its own session
    leader (pgid == pid) and the group never spans unrelated processes.

    Returns True if the process is gone afterward, False if it survived both
    signals (e.g. EPERM) — callers must not report success on a False.
    """

    def _signal(sig: int) -> None:
        # Best-effort: a missing group/pid means it is already gone; a genuine
        # failure to kill is surfaced via the is_process_running check below,
        # not by raising, so the final return value is the single source of truth.
        try:
            os.killpg(os.getpgid(pid), sig)
        except (OSError, ProcessLookupError):
            try:
                os.kill(pid, sig)
            except OSError:
                pass

    _signal(signal.SIGTERM)
    for _ in range(10):
        if not is_process_running(pid):
            return True
        time.sleep(0.1)
    _signal(signal.SIGKILL)
    for _ in range(5):
        if not is_process_running(pid):
            return True
        time.sleep(0.1)
    return not is_process_running(pid)


def stop_process(pid_file: Path, name: str) -> bool:
    """Stop a process by PID file. Returns True if was running."""
    global _db_lock

    pid = get_pid(pid_file)
    if pid is None:
        return False
    try:
        if kill_process_group(pid):
            print(f"Stopped {name} (PID {pid})")
        else:
            print(f"Could not stop {name} (PID {pid}); still running")
    finally:
        unregister_dev_process(name.lower())
        if pid_file.exists():
            pid_file.unlink()
        if name == "Phoenix" and PHOENIX_PORT_FILE.exists():
            PHOENIX_PORT_FILE.unlink()
        if name == "Vite" and VITE_PORT_FILE.exists():
            VITE_PORT_FILE.unlink()
        if name == "Vite" and VITE_PROXY_FILE.exists():
            VITE_PROXY_FILE.unlink()
        # Release database lock if stopping Phoenix
        if name == "Phoenix" and _db_lock is not None:
            _db_lock.release()
            _db_lock = None
    return True


def _registry_path(kind: str) -> Path:
    return DEV_REGISTRY_DIR / f"{get_worktree_hash()}.{kind}.json"


def register_dev_process(kind: str, pid: int, port: int) -> None:
    """Record a spawned dev server outside the worktree so `reap` can find it
    even after the worktree directory (and its .pid files) are deleted.

    Best-effort: a registry failure must never block a server from starting.
    """
    try:
        DEV_REGISTRY_DIR.mkdir(parents=True, exist_ok=True)
        _registry_path(kind).write_text(
            json.dumps({"kind": kind, "pid": pid, "worktree": str(ROOT), "port": port})
        )
    except OSError:
        pass


def unregister_dev_process(kind: str) -> None:
    """Remove a dev server's registry record (clean shutdown via `down`)."""
    try:
        _registry_path(kind).unlink(missing_ok=True)
    except OSError:
        pass


def _process_cwd(pid: int) -> Path | None:
    """Return a live process's working directory via lsof, or None.

    lsof reports the recorded path even when the directory has been deleted,
    which is exactly what lets reap recognise an orphan of a removed worktree.
    Uses lsof rather than /proc because this is macOS-first tooling.
    """
    try:
        out = subprocess.run(
            ["lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn"],
            capture_output=True,
            text=True,
            timeout=5,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    for line in out.splitlines():
        if line.startswith("n"):
            return Path(line[1:])
    return None


def _looks_like_dev_server(command: str) -> bool:
    """True if a process command line is a Phoenix/Vite dev server we spawn.

    Matches the debug/release Phoenix binary (underscore name — the prod binary
    is `phoenix-ide` with a dash and won't match), the Vite worker, and the
    pnpm parent that launches it.
    """
    return (
        "/target/release/phoenix_ide" in command
        or "/target/debug/phoenix_ide" in command
        or "vite/bin/vite.js" in command
        or "pnpm run dev" in command
    )


def _list_dev_processes() -> list[tuple[int, str]]:
    """Enumerate (pid, command) for every live Phoenix/Vite dev process."""
    try:
        out = subprocess.run(
            ["ps", "-axww", "-o", "pid=,command="],
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    procs = []
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        pid_str, _, command = line.partition(" ")
        if not pid_str.isdigit() or not _looks_like_dev_server(command):
            continue
        procs.append((int(pid_str), command))
    return procs


def _path_hash(path: str) -> str:
    """The dev-DB/registry hash of a worktree path (matches get_worktree_hash)."""
    return hashlib.md5(str(path).encode()).hexdigest()[:8]


def _worktree_entries() -> list[tuple[str, bool]] | None:
    """`(path, prunable)` for every worktree git currently tracks.

    `prunable` is true when git has flagged the entry's directory as gone (it
    keeps emitting the path until `git worktree prune` runs). Returns None when
    the list cannot be obtained, so the caller skips pruning rather than guess.
    """
    try:
        out = subprocess.run(
            ["git", "worktree", "list", "--porcelain"],
            capture_output=True,
            text=True,
            timeout=10,
            cwd=ROOT,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    entries: list[tuple[str, bool]] = []
    path: str | None = None
    prunable = False
    for line in out.stdout.splitlines() + [""]:
        if line.startswith("worktree "):
            path = line[len("worktree "):]
            prunable = False
        elif line.startswith("prunable "):
            prunable = True
        elif line == "" and path is not None:
            entries.append((path, prunable))
            path = None
    return entries


def _git_worktree_bases() -> list[Path]:
    """Directories that hold per-worktree checkouts, under the main repo.

    Resolved from the shared git dir so it is correct from inside any worktree,
    not just the main checkout. dev.py spawns dev servers in these trees but
    never creates or removes the worktrees themselves — the Claude and Phoenix
    harnesses do — which is why reclaiming a stale one runs under a strict
    predicate (see `reclaim_deregistered_worktrees`).
    """
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            cwd=ROOT,
        )
    except (OSError, subprocess.SubprocessError):
        return []
    if out.returncode != 0 or not out.stdout.strip():
        return []
    main_root = Path(out.stdout.strip()).parent
    return [main_root / ".claude" / "worktrees", main_root / ".phoenix" / "worktrees"]


def _dir_size_bytes(path: Path) -> int:
    """Best-effort recursive size of a directory in bytes (0 on failure)."""
    try:
        out = subprocess.run(
            ["du", "-sk", str(path)], capture_output=True, text=True, timeout=60
        )
        if out.returncode == 0:
            return int(out.stdout.split()[0]) * 1024
    except (OSError, subprocess.SubprocessError, ValueError, IndexError):
        pass
    return 0


def _registry_snapshot() -> list[tuple[str, int]]:
    """`(worktree_path, pid)` for every dev-server registry record, read before
    `reap_orphans` mutates the registry so DB pruning can still resolve hashes
    back to their owning paths."""
    snapshot: list[tuple[str, int]] = []
    if not DEV_REGISTRY_DIR.exists():
        return snapshot
    for rec_file in DEV_REGISTRY_DIR.glob("*.json"):
        try:
            rec = json.loads(rec_file.read_text())
            snapshot.append((str(rec["worktree"]), int(rec["pid"])))
        except (OSError, KeyError, ValueError, json.JSONDecodeError):
            continue
    return snapshot


def prune_orphan_dbs(
    registry: list[tuple[str, int]], verbose: bool = True, dry_run: bool = False
) -> int:
    """Delete per-worktree dev databases whose owning worktree is gone.

    A dev DB is `phoenix-<hash>.db` (plus `-wal`/`-shm`/`.lock` sidecars), where
    `<hash>` is the worktree-path hash. The `~/.phoenix-ide` namespace is shared
    across every Phoenix checkout on the machine, and the hash is not invertible,
    so a DB is pruned only with *positive evidence* its owning path is gone —
    never merely because this checkout's `git worktree list` doesn't mention it
    (that would delete another live checkout's database).

    Evidence that a hash is gone: a `git worktree` entry whose path is missing or
    `prunable`, or a registry record whose `worktree` path no longer exists. A
    hash is protected when any source maps it to an existing path or a live
    process; protection wins over gone-evidence. A DB whose hash matches no known
    path is left untouched. Runs after the process reap so a just-reaped server's
    DB is collected in the same pass.
    """
    entries = _worktree_entries()
    if entries is None:
        if verbose:
            print("  Skipping DB prune: could not list worktrees.")
        return 0

    gone: set[str] = set()
    keep: set[str] = set()
    for path, prunable in entries:
        (gone if (prunable or not Path(path).exists()) else keep).add(_path_hash(path))
    for path, pid in registry:
        h = _path_hash(path)
        if is_process_running(pid):
            keep.add(h)  # live server — protect regardless of path state
        elif not Path(path).exists():
            gone.add(h)
        else:
            keep.add(h)

    prunable_hashes = gone - keep
    verb = "Would prune" if dry_run else "Pruned"
    pruned = 0
    freed = 0
    db_re = re.compile(r"^phoenix-([0-9a-f]{8})\.db$")
    for db in sorted(DB_DIR.glob("phoenix-*.db")):
        m = db_re.match(db.name)
        if not m or m.group(1) not in prunable_hashes:
            continue
        h = m.group(1)
        _files, size = _unlink_db_sidecars(h, dry_run=dry_run)
        pruned += 1
        freed += size
        if verbose:
            print(f"  {verb} orphaned dev DB phoenix-{h} ({size / 1e6:.1f} MB)")
    if pruned and not verbose:
        print(f"{verb} {pruned} orphaned dev database(s), {freed / 1e6:.1f} MB.")
    return pruned


def _unlink_db_sidecars(h: str, dry_run: bool = False) -> tuple[int, int]:
    """Delete the `phoenix-<h>.*` dev DB and its sidecars. Returns (files, bytes).

    Single owner of dev-DB sidecar globbing/sizing so `prune_orphan_dbs` and
    `_reclaim_worktree_db` cannot drift on which suffixes count as a sidecar.
    """
    sidecars = sorted(DB_DIR.glob(f"phoenix-{h}.*"))
    size = sum(f.stat().st_size for f in sidecars if f.exists())
    if not dry_run:
        for f in sidecars:
            f.unlink(missing_ok=True)
    return (len(sidecars), size)


def _reclaim_worktree_db(worktree_path: Path, verbose: bool = True) -> None:
    """Delete the dev DB + sidecars keyed to a removed worktree's resolved path.

    The hash is `get_worktree_hash` of the worktree's own resolved path, so it is
    unique to that path — no live checkout shares it. `prune_orphan_dbs` cannot
    collect it: a deregistered worktree has neither a `git worktree list` entry
    nor (usually) a registry record, so its hash never enters the gone-set.
    """
    h = _path_hash(str(worktree_path))
    files, _bytes = _unlink_db_sidecars(h)
    if verbose and files:
        print(f"    + reclaimed dev DB phoenix-{h} ({files} files)")


def reclaim_deregistered_worktrees(
    verbose: bool = True, dry_run: bool = False
) -> tuple[int, int]:
    """Remove worktree directories git no longer tracks. Returns (count, bytes).

    A husk is left when a worktree is removed without first stopping its dev
    server: the running server holds the directory as its cwd, so the harness's
    final rmdir fails and an untracked shell survives — often just `<name>/ui`,
    sometimes a full tree with an intact multi-GB `target/`. reap is the only
    janitor that can collect it, because git has already forgotten the path and
    `git worktree prune` no longer emits it.

    dev.py neither creates nor removes these worktrees, so deletion runs under a
    strict predicate — a directory is reclaimed only when all hold:
      1. it is a direct child of `<main>/.claude/worktrees` or `<main>/.phoenix/worktrees`,
      2. its resolved path is absent from `git worktree list`,
      3. no live dev server is rooted in it.
    `git worktree add` registers a worktree in the same step that creates its
    directory, so an untracked directory here is always a removal husk, never a
    create in flight — the predicate cannot race worktree creation.
    """
    entries = _worktree_entries()
    if entries is None:
        if verbose:
            print("  Skipping worktree reclaim: could not list worktrees.")
        return (0, 0)
    tracked = {str(Path(p).resolve()) for p, _ in entries}

    # Predicate 3: a live dev server rooted in a directory protects it. Scoped to
    # the Phoenix/Vite processes reap owns, not every process on the machine.
    # Resolve every cwd so symlinks in the chain (a symlinked worktrees base,
    # /var→/private/var) can't make the containment test miss and delete a live
    # tree. If any live server's cwd can't be read (lsof timeout/error), fail
    # safe: skip reclamation entirely rather than risk deleting an unprotected
    # but live worktree.
    rooted: list[Path] = []
    for pid, _cmd in _list_dev_processes():
        cwd = _process_cwd(pid)
        if cwd is None:
            if verbose:
                print("  Skipping worktree reclaim: a live dev server's cwd "
                      "could not be read; failing safe.")
            return (0, 0)
        rooted.append(cwd.resolve())

    count = 0
    freed = 0
    for base in _git_worktree_bases():
        if not base.is_dir():
            continue
        for child in sorted(base.iterdir()):
            # Skip symlinks: rmtree would follow into and delete the target tree.
            if child.is_symlink() or not child.is_dir():
                continue
            rp = child.resolve()
            # Never delete the tree this interpreter runs from. ROOT is resolved
            # at import; `rp in ROOT.parents` covers `up`/`reap` invoked from
            # inside a husk worktree (ROOT below rp), which predicate 3 misses
            # because the controlling python is not a spawned dev server.
            if rp == ROOT or rp in ROOT.parents:
                continue
            if str(rp) in tracked:
                continue  # predicate 2: still a tracked worktree
            if any(c == rp or rp in c.parents for c in rooted):
                if verbose:
                    print(f"  Skipping {child.name}: a dev server is still rooted there")
                continue  # predicate 3
            if dry_run:
                count += 1
                if verbose:
                    print(f"  Would reclaim deregistered worktree {child.name}")
                continue
            # du only on the real path, right before removal — never on dry-run,
            # where it would block `up`/`reap` for seconds over multi-GB husks
            # purely for a log line.
            size = _dir_size_bytes(rp)
            try:
                shutil.rmtree(rp)
            except OSError as e:
                if verbose:
                    print(f"  Failed to remove {child}: {e}")
                continue
            _reclaim_worktree_db(rp, verbose=verbose)
            count += 1
            freed += size
            if verbose:
                print(f"  Reclaimed deregistered worktree {child.name} ({size / 1e9:.2f} GB)")
    return (count, freed)


def reap_orphans(
    verbose: bool = True, dry_run: bool = False, reclaim_dirs: bool = False
) -> int:
    """Kill dev servers orphaned when their worktree was deleted without `down`.

    A process is reaped only when its live cwd resolves to a directory that no
    longer exists — a condition a recycled PID cannot satisfy, so the kill is
    safe. Two sources are checked: the registry (authoritative, survives the
    worktree's deletion) and a heuristic process scan (catches orphans spawned
    before the registry existed). Servers whose worktree still exists, including
    the current one, are never touched. With dry_run, reports what would be
    killed and mutates nothing (no kills, no registry-record removal).

    `reclaim_dirs` additionally deletes deregistered worktree *directories*. It is
    opt-in (explicit `./dev.py reap` only) and off for the auto-reap on `up`:
    killing a stray process is cheap and reversible, but `rmtree` is neither, so
    the startup path must never trigger it.
    """
    verb = "Would reap" if dry_run else "Reaped"
    reaped = 0
    handled: set[int] = set()

    # Snapshot the registry before the loop below unlinks records: DB pruning
    # needs the hash->path mapping these records carry, including for worktrees
    # this pass is about to reap.
    registry = _registry_snapshot()

    if DEV_REGISTRY_DIR.exists():
        for rec_file in sorted(DEV_REGISTRY_DIR.glob("*.json")):
            try:
                rec = json.loads(rec_file.read_text())
                pid = int(rec["pid"])
                worktree = str(rec["worktree"])
            except (OSError, KeyError, ValueError, json.JSONDecodeError):
                if not dry_run:
                    rec_file.unlink(missing_ok=True)
                continue
            if Path(worktree).exists():
                # Live worktree: not an orphan. Drop the record only if dead.
                if not is_process_running(pid) and not dry_run:
                    rec_file.unlink(missing_ok=True)
                continue
            # Worktree gone. Kill only if the pid is alive and still rooted in
            # that deleted tree (guards against PID reuse).
            kept = False
            if is_process_running(pid):
                cwd = _process_cwd(pid)
                if cwd is not None and not cwd.exists() and str(cwd).startswith(worktree):
                    handled.add(pid)
                    if dry_run or kill_process_group(pid):
                        reaped += 1
                        if verbose:
                            print(f"  {verb} {rec.get('kind', '?')} (PID {pid}, port {rec.get('port', '?')}) — {worktree}")
                    else:
                        kept = True  # kill failed; keep the record so a later reap retries
                        if verbose:
                            print(f"  Failed to kill PID {pid} ({worktree}); keeping record to retry")
                elif verbose:
                    print(f"  Skipping registry PID {pid}: no longer rooted in deleted {worktree} (PID reused?)")
            if not dry_run and not kept:
                rec_file.unlink(missing_ok=True)

    for pid, command in _list_dev_processes():
        if pid in handled:
            continue
        cwd = _process_cwd(pid)
        if cwd is None or cwd.exists() or "/worktrees/" not in str(cwd):
            continue
        if not dry_run and not kill_process_group(pid):
            if verbose:
                print(f"  Failed to kill orphan (PID {pid}) — cwd {cwd}")
            continue
        reaped += 1
        if verbose:
            print(f"  {verb} orphan (PID {pid}) — cwd {cwd}")

    # Remove worktree directories git has forgotten (the husk a worktree leaves
    # when it is deleted without `down` first). Opt-in (explicit `reap` only) and
    # after the process reap so a server just killed above no longer roots its dir.
    reclaimed, freed = (
        reclaim_deregistered_worktrees(verbose=verbose, dry_run=dry_run)
        if reclaim_dirs
        else (0, 0)
    )

    # Reclaim DB files of deleted worktrees. Runs after the process reap so a
    # server just killed above has its DB collected in the same pass.
    pruned = prune_orphan_dbs(registry, verbose=verbose, dry_run=dry_run)

    if reclaimed and not verbose:
        verb = "Would reclaim" if dry_run else "Reclaimed"
        print(f"{verb} {reclaimed} stale worktree dir(s), {freed / 1e9:.2f} GB.")
    if verbose and reaped == 0 and pruned == 0 and reclaimed == 0:
        print("No orphaned dev servers, databases, or stale worktrees found.")
    return reaped


def cmd_reap(dry_run: bool = False) -> None:
    """Kill dev servers orphaned by deleted worktrees and reclaim their dirs."""
    n = reap_orphans(verbose=True, dry_run=dry_run, reclaim_dirs=True)
    if n and not dry_run:
        print(f"Reaped {n} orphaned dev process group(s).")


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


def _resolved_phoenix_env() -> dict[str, str]:
    """Phoenix env with the same .env overrides start_phoenix applies, for
    resolving TLS paths without starting the server."""
    env = os.environ.copy()
    _load_env_file(env)
    _load_env_file(env, ".phoenix-ide.dev.env")
    return env


def phoenix_tls_cert_paths(env: dict[str, str] | None = None) -> tuple[Path, Path]:
    """Cert/key files Phoenix's TLS uses, so Vite can serve the same identity.
    Mirrors crates/phoenix-ide/src/tls.rs: explicit manual paths win; otherwise
    the auto-managed leaf under the TLS dir (PHOENIX_TLS_DIR, else <db dir>/tls)."""
    env = env or _resolved_phoenix_env()
    cert = env.get("PHOENIX_TLS_CERT_PATH")
    key = env.get("PHOENIX_TLS_KEY_PATH")
    if cert and key:
        return Path(cert), Path(key)
    tls_dir_env = env.get("PHOENIX_TLS_DIR")
    base = Path(tls_dir_env) if tls_dir_env else get_db_path().parent / "tls"
    return base / "phoenix-local-server.pem", base / "phoenix-local-server-key.pem"


def phoenix_tls_ca_path(env: dict[str, str] | None = None) -> Path | None:
    """Path to the auto-managed local CA the browser must trust, or None when
    TLS is configured with manual cert paths (caller-managed trust)."""
    env = env or _resolved_phoenix_env()
    if env.get("PHOENIX_TLS_CERT_PATH") and env.get("PHOENIX_TLS_KEY_PATH"):
        return None
    tls_dir_env = env.get("PHOENIX_TLS_DIR")
    base = Path(tls_dir_env) if tls_dir_env else get_db_path().parent / "tls"
    return base / "phoenix-local-ca.pem"


def _wait_for_files(paths: list[Path], timeout: float = 8.0) -> bool:
    """Poll until every path exists or the timeout elapses. Phoenix issues its
    auto cert at startup, so Vite (started right after) must wait for it."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        if all(p.exists() for p in paths):
            return True
        time.sleep(0.1)
    return all(p.exists() for p in paths)


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
    env["PHOENIX_PORT"] = str(port)
    env["PHOENIX_DB_PATH"] = str(db_path)
    # Bind loopback so the dev server is never network-reachable: on a remote or
    # shared dev host (or a port-forwarded sandbox) a 0.0.0.0 bind would expose an
    # unauthenticated agent that runs arbitrary commands. A loopback bind also
    # satisfies the binary's fail-closed guard with no password and no insecure-bind
    # override. Vite proxies to localhost, so a loopback backend is fine — and the
    # Vite dev server itself is bound loopback in passwordless mode (see
    # `_dev_bind_host`) so its proxy can't re-expose this backend to the network.
    env["PHOENIX_BIND_ADDR"] = "127.0.0.1"
    # Unified logging: the binary owns the log file (PHOENIX_LOG_FILE) and writes
    # JSON there directly. stdout is off so the only writer to the file is the
    # binary's appender; the Popen redirect below only captures pre-logger /
    # panic output as a safety net. Same PHOENIX_LOG_FILE mechanism the prod
    # launchers use.
    env["PHOENIX_LOG_FILE"] = str(LOG_FILE)
    env["PHOENIX_LOG_STDOUT"] = "false"
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

    # Fresh log per restart, then append: the binary's appender and this
    # redirect both open the file with O_APPEND, so their writes interleave
    # safely. Truncating here (not "w" on the Popen handle) gives the
    # fresh-per-restart behavior without a non-appending second writer.
    LOG_FILE.write_text("")
    with open(LOG_FILE, "a") as log:
        proc = subprocess.Popen(
            [str(binary)],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        PHOENIX_PID_FILE.write_text(str(proc.pid))
    PHOENIX_PORT_FILE.write_text(str(port) + "\n")
    register_dev_process("phoenix", proc.pid, port)

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


def _dev_bind_host() -> str:
    """Network host for the Vite dev server.

    Loopback by default. The Phoenix backend already binds loopback, but Vite
    proxies `/api` and `/preview` to it, so a `0.0.0.0` Vite would re-expose the
    unauthenticated dev API to anyone who can reach the Vite port (a shared or
    port-forwarded host) — defeating the backend's loopback bind. Bind Vite to
    `0.0.0.0` only when the API is actually protected (`PHOENIX_PASSWORD` set) or
    the operator explicitly opts into the insecure bind
    (`PHOENIX_ALLOW_INSECURE_BIND=1`), mirroring the binary's own guard.
    """
    env = os.environ.copy()
    _load_env_file(env)
    _load_env_file(env, ".phoenix-ide.dev.env")
    if env.get("PHOENIX_PASSWORD") or env.get("PHOENIX_ALLOW_INSECURE_BIND") == "1":
        return "0.0.0.0"
    return "127.0.0.1"


def start_vite(port: int, phoenix_port: int, phoenix_tls: bool = False) -> bool:
    """Start the Vite dev server. Returns whether Vite serves over HTTPS (h2)."""
    scheme = "https" if phoenix_tls else "http"
    vite_host = _dev_bind_host()
    # The restart key includes the bind host so a later auth / insecure-bind env
    # change (which flips `_dev_bind_host` between 127.0.0.1 and 0.0.0.0) actually
    # restarts Vite instead of being masked by "already running". The proxy target
    # is 127.0.0.1 to match the backend's IPv4 loopback bind.
    desired_proxy = f"{scheme}://127.0.0.1:{phoenix_port}|host={vite_host}"
    cert_paths = phoenix_tls_cert_paths() if phoenix_tls else None
    if get_pid(VITE_PID_FILE):
        current_proxy = VITE_PROXY_FILE.read_text().strip() if VITE_PROXY_FILE.exists() else ""
        if current_proxy == desired_proxy:
            print("Vite dev server already running")
            return bool(cert_paths and all(p.exists() for p in cert_paths))
        print("Restarting Vite dev server for API proxy/host change")
        stop_process(VITE_PID_FILE, "Vite")

    ensure_ui_deps()

    env = node_env()
    # Pass Phoenix port to Vite for proxy configuration
    env["VITE_API_PORT"] = str(phoenix_port)
    vite_tls = False
    if phoenix_tls:
        env["VITE_API_SCHEME"] = "https"
        env.setdefault("VITE_API_PROXY_SECURE", "false")
        # Serve Vite itself over the same cert so the browser↔Vite hop is HTTP/2
        # (Vite 8 → http2 whenever server.https is set). Phoenix issues the cert
        # at startup, so wait for it; fall back to HTTP/1.1 if it never appears.
        cert, key = cert_paths
        if _wait_for_files([cert, key]):
            env["VITE_HTTPS_CERT"] = str(cert)
            env["VITE_HTTPS_KEY"] = str(key)
            vite_tls = True
        else:
            print(f"  ⚠ Vite HTTPS disabled: Phoenix TLS cert not found at {cert}")

    # Start Vite in background. `vite_host` (loopback unless the API is password-
    # protected or the insecure bind is opted into, see `_dev_bind_host`) was
    # resolved above and folded into the restart key — a `0.0.0.0` Vite proxies
    # the unauthenticated backend to the network.
    # pnpm passes args after the script name directly — no `--` separator.
    proc = subprocess.Popen(
        ["pnpm", "run", "dev", "--port", str(port), "--strictPort", "--host", vite_host],
        cwd=UI_DIR,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    VITE_PID_FILE.write_text(str(proc.pid))
    VITE_PORT_FILE.write_text(str(port) + "\n")
    register_dev_process("vite", proc.pid, port)

    # `proc` is the pnpm wrapper, not Vite — confirm the port actually bound
    # rather than trusting the wrapper PID, which lingers when its child Vite
    # dies on a --strictPort EADDRINUSE.
    if not _wait_for_port(vite_host, port, proc):
        print(
            f"Vite failed to bind port {port} — already in use or crashed on start.",
            file=sys.stderr,
        )
        print(
            "  An orphaned dev server (e.g. from a deleted worktree) may hold it; "
            "try './dev.py reap' or './dev.py up --vite-port <free>'.",
            file=sys.stderr,
        )
        # Don't leave the orphan this check exists to detect: the wrapper may
        # still be alive (its child died on EADDRINUSE), so kill the group and
        # clear every record we wrote above before bailing.
        kill_process_group(proc.pid)
        unregister_dev_process("vite")
        VITE_PID_FILE.unlink(missing_ok=True)
        VITE_PORT_FILE.unlink(missing_ok=True)
        sys.exit(1)

    print(f"Started Vite dev server (PID {proc.pid}, port {port})")
    VITE_PROXY_FILE.write_text(desired_proxy + "\n")
    print(f"  Proxying /api to Phoenix at {desired_proxy}")
    if vite_tls:
        print("  Serving UI over HTTPS (HTTP/2)")
    return vite_tls


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
    # Self-heal: reclaim ports/PIDs from servers orphaned when a worktree was
    # deleted without `./dev.py down`. Runs before port selection so freed
    # ports are available to probe. Quiet unless something was actually reaped.
    if reap_orphans(verbose=False):
        print("Reaped orphaned dev servers from deleted worktrees.")
    default_phoenix, default_vite = get_default_ports()
    preferred_phoenix = phoenix_port or default_phoenix
    preferred_vite = vite_port or default_vite
    phoenix_pid = get_pid(PHOENIX_PID_FILE)
    vite_pid = get_pid(VITE_PID_FILE)
    phoenix_port = int(PHOENIX_PORT_FILE.read_text().strip()) if phoenix_pid and PHOENIX_PORT_FILE.exists() else preferred_phoenix
    vite_port = int(VITE_PORT_FILE.read_text().strip()) if vite_pid and VITE_PORT_FILE.exists() else preferred_vite
    if not phoenix_pid and not vite_pid and phoenix_port == preferred_phoenix and vite_port == preferred_vite:
        phoenix_port, vite_port = select_dev_ports(preferred_phoenix, preferred_vite)
    elif not phoenix_pid and phoenix_port == preferred_phoenix:
        phoenix_port, _ = select_dev_ports(preferred_phoenix, vite_port)
    elif not vite_pid and vite_port == preferred_vite:
        _, vite_port = select_dev_ports(phoenix_port, preferred_vite)
    
    print(f"Worktree: {ROOT}")
    phoenix_offset, vite_offset = get_port_offsets()
    print(f"  Hash: {get_worktree_hash()}, Port offsets: Phoenix +{phoenix_offset}, Vite +{vite_offset}")
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
    vite_tls = start_vite(port=vite_port, phoenix_port=phoenix_port, phoenix_tls=phoenix_tls)
    api_scheme = "https" if phoenix_tls else "http"
    ui_scheme = "https" if vite_tls else "http"
    print()
    print(f"Ready! UI: {ui_scheme}://localhost:{vite_port}")
    print(f"        API: {api_scheme}://localhost:{phoenix_port}")
    print(f"        Log: {LOG_FILE}")
    if vite_tls:
        ca = phoenix_tls_ca_path()
        if ca:
            print(f"        HTTP/2 enabled. If the browser warns, trust the local CA once: {ca}")


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
#   - Branch mode diff-review fixture backed by a deterministic local git repo
#     under `.phoenix/seed-worktrees/diff-review-fixture`.
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
_SEED_DIFF_REVIEW_TEXT = "Review the seeded Branch-mode diff fixture"

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
    # conv_mode-touching migrations: the seed builds clean normalized cm_*
    # columns and drops the conv_mode blob itself, so these must be skipped on
    # the next startup (they reference the now-absent conv_mode column, and
    # their legacy-data cleanups have nothing to do on pristine seed rows).
    (1, "rewrite_standalone_to_direct"),
    (2, "backfill_empty_convmode_fields"),
    (7, "backfill_explore_worktree_path"),
    (21, "normalize_explore_taskmd_id_hint"),
    (28, "add_conv_mode_columns"),
    (29, "drop_conv_mode_blob"),
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
    # Normalized conv_mode columns (migration 028). The seed rows are inserted
    # with the legacy conv_mode blob (above), then projected into these columns
    # and the blob is DROPped at the end of seeding — matching the post-migration
    # production schema (the blob does not exist after migration 029).
    "ALTER TABLE conversations ADD COLUMN cm_kind TEXT",
    "ALTER TABLE conversations ADD COLUMN cm_branch_name TEXT",
    "ALTER TABLE conversations ADD COLUMN cm_worktree_path TEXT",
    "ALTER TABLE conversations ADD COLUMN cm_base_branch TEXT",
    "ALTER TABLE conversations ADD COLUMN cm_task_id TEXT",
    "ALTER TABLE conversations ADD COLUMN cm_task_title TEXT",
    "ALTER TABLE conversations ADD COLUMN cm_next_taskmd_id_hint TEXT",
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

    def _run_seed_git(cwd: Path, args: list[str]) -> None:
        env = os.environ.copy()
        env.update({
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_TERMINAL_PROMPT": "0",
        })
        result = subprocess.run(
            ["git", *args],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"git {' '.join(args)} failed in {cwd}:\n{result.stdout}{result.stderr}"
            )

    def _write_text(path: Path, text: str) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def _ensure_seed_diff_worktree() -> Path:
        """Create the deterministic Branch-mode diff fixture worktree."""
        fixture = ROOT / ".phoenix" / "seed-worktrees" / "diff-review-fixture"
        if fixture.exists():
            shutil.rmtree(fixture)
        fixture.mkdir(parents=True, exist_ok=True)

        _run_seed_git(fixture, ["init", "-q", "-b", "main"])
        for key, value in [
            ("user.email", "seed@example.invalid"),
            ("user.name", "Phoenix Seed"),
            ("commit.gpgsign", "false"),
            ("diff.mnemonicPrefix", "false"),
        ]:
            _run_seed_git(fixture, ["config", key, value])

        seed_file = fixture / "seed-diff-fixture.txt"
        _write_text(
            seed_file,
            "Phoenix seeded diff fixture\n"
            "baseline line retained for context\n"
            "line changed by branch commit\n",
        )
        _run_seed_git(fixture, ["add", "seed-diff-fixture.txt"])
        _run_seed_git(fixture, ["commit", "-q", "-m", "seed baseline"])

        _run_seed_git(fixture, ["checkout", "-q", "-b", "seed-diff-review"])
        _write_text(
            seed_file,
            "Phoenix seeded diff fixture\n"
            "baseline line retained for context\n"
            "line changed by branch commit (committed)\n",
        )
        _write_text(
            fixture / "committed-review-note.txt",
            "This file exists only on the seeded review branch.\n",
        )
        _run_seed_git(fixture, ["add", "seed-diff-fixture.txt", "committed-review-note.txt"])
        _run_seed_git(fixture, ["commit", "-q", "-m", "seed committed diff"])

        _write_text(
            seed_file,
            "Phoenix seeded diff fixture\n"
            "baseline line retained for context\n"
            "line changed by branch commit (committed)\n"
            "line changed in the working tree (uncommitted)\n",
        )
        _write_text(
            fixture / "uncommitted-review-note.txt",
            "This untracked file demonstrates the uncommitted diff section.\n",
        )
        return fixture

    def _ensure_diff_review_fixture(
        conn: sqlite3.Connection,
        *,
        project_id: str,
    ) -> bool:
        """Ensure a Branch-mode conversation with a deterministic committed and uncommitted diff exists."""
        worktree = _ensure_seed_diff_worktree()
        slug = "fixture-diff-review"
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
            conv_mode = conn.execute(
                "SELECT conv_mode FROM conversations WHERE id = ?",
                (conv_id,),
            ).fetchone()[0]
            try:
                parsed_mode = json.loads(conv_mode)
            except Exception:
                parsed_mode = {}
            if (
                archived == 0
                and message_count == 1
                and parsed_mode.get("mode") == "Branch"
                and parsed_mode.get("worktree_path") == str(worktree)
            ):
                return False
            conn.execute("DELETE FROM messages WHERE conversation_id = ?", (conv_id,))
            conn.execute("DELETE FROM conversations WHERE id = ?", (conv_id,))

        conv_id = str(_uuid.uuid4())
        state_json = json.dumps({"type": "idle"})
        mode_json = json.dumps({
            "mode": "Branch",
            "branch_name": "seed-diff-review",
            "worktree_path": str(worktree),
            "base_branch": "main",
        })
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
                "Fixture Diff Review",
                str(worktree),
                state_json,
                now,
                now,
                now,
                project_id,
                mode_json,
                "qa:diff-review",
            ),
        )
        msg_id = str(_uuid.uuid4())
        conn.execute(
            "INSERT INTO messages ("
            " message_id, conversation_id, sequence_id, message_type,"
            " content, created_at"
            ") VALUES (?, ?, 0, 'user', ?, ?)",
            (
                msg_id,
                conv_id,
                json.dumps({"text": _SEED_DIFF_REVIEW_TEXT, "images": []}),
                now,
            ),
        )
        return True

    def _ensure_grounding_panel_fixture_worktree() -> Path:
        fixture = ROOT / ".phoenix" / "seed-worktrees" / "grounding-panel-qa"
        # Reuse an already-built worktree: rebuilding on every seed wastes a full
        # git init + commits for no DB change, and clobbers any manual edits a
        # developer made while reviewing. A `.git` dir is the "already built"
        # signal; only a partial/corrupt leftover is torn down and rebuilt.
        if (fixture / ".git").is_dir():
            return fixture
        if fixture.exists():
            shutil.rmtree(fixture)
        fixture.mkdir(parents=True, exist_ok=True)

        _run_seed_git(fixture, ["init", "-q", "-b", "main"])
        for key, value in [
            ("user.email", "seed@example.invalid"),
            ("user.name", "Phoenix Seed"),
            ("commit.gpgsign", "false"),
        ]:
            _run_seed_git(fixture, ["config", key, value])

        _write_text(
            fixture / "README-with-a-very-long-name-that-still-needs-to-truncate.md",
            "# Grounding Panel QA Fixture\n\n"
            "This seeded repository exercises the real conversation grounding panel "
            "through Phoenix navigation instead of a mocked visual story.\n",
        )
        for path, content in [
            ("crates/phoenix-ide/src/main.rs", "fn main() { println!(\"grounding qa\"); }\n"),
            ("crates/phoenix-ide/src/runtime/very_deep_module_name_for_grounding_panel_review.rs", "pub fn fixture() {}\n"),
            ("ui/src/components/GroundingPanelQaIntegrationReviewSurface.tsx", "export function Fixture() { return null; }\n"),
            ("specs/grounding-panel/requirements.md", "# Grounding panel QA requirements\n"),
            ("docs/reference/architecture/very/deep/tree/with/long/names/notes.md", "# Deep tree\n"),
        ]:
            _write_text(fixture / path, content)

        task_bodies = {
            "22001-p2-in-progress--redesign-conversation-grounding-side-panel.md": "# Redesign conversation grounding side panel\n\nCurrent seeded task linked to the QA conversation.\n",
            "22002-p0-ready--fix-critical-mcp-auth-refresh-loop-with-long-slug.md": "# Fix critical MCP auth refresh loop with long slug\n",
            "22003-p1-blocked--blocked-on-github-token-scope-decision.md": "# Blocked on GitHub token scope decision\n",
            "22004-p3-brainstorming--explore-grounding-panel-information-architecture.md": "# Explore grounding panel information architecture\n",
            "22005-p4-done--archive-old-panel-screenshot-notes.md": "# Archive old panel screenshot notes\n",
            "22006-p2-wont-do--replace-panel-with-floating-modal.md": "# Replace panel with floating modal\n",
        }
        for filename, body in task_bodies.items():
            _write_text(fixture / "tasks" / filename, body)
        _write_text(fixture / "tasks" / ".taskmd", "")

        for base, name, description in [
            (".agents/skills", "phoenix-perf-hunt", "Profiles Phoenix React scenarios and coordinates focused performance attempts."),
            (".claude/skills", "very-long-project-specific-skill-name-for-truncation-review", "Project skill with deliberately long metadata for grounding-panel truncation review."),
            ("service/.agents/skills", "service-review", "Service-local skill discovered from an immediate child project."),
        ]:
            _write_text(
                fixture / base / name / "SKILL.md",
                f"# {name}\n\n{description}\n\n## Arguments\n\n`--scenario <name>`\n",
            )

        _run_seed_git(fixture, ["add", "."])
        _run_seed_git(fixture, ["commit", "-q", "-m", "seed grounding panel qa baseline"])
        _run_seed_git(fixture, ["checkout", "-q", "-b", "task-22001-redesign-conversation-grounding-side-panel"])
        _write_text(
            fixture / "ui/src/components/GroundingPanelQaIntegrationReviewSurface.tsx",
            "export function Fixture() { return <aside>Grounding QA branch change</aside>; }\n",
        )
        _run_seed_git(fixture, ["add", "."])
        _run_seed_git(fixture, ["commit", "-q", "-m", "seed grounding panel branch"])
        return fixture

    def _ensure_grounding_panel_qa_fixture(conn: sqlite3.Connection) -> bool:
        worktree = _ensure_grounding_panel_fixture_worktree()
        project_id = _find_or_create_project(conn, str(worktree))
        slug = "fixture-grounding-panel-qa"
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
            if archived == 0 and message_count >= 1:
                return False
            conn.execute("DELETE FROM messages WHERE conversation_id = ?", (conv_id,))
            conn.execute("DELETE FROM conversations WHERE id = ?", (conv_id,))

        conv_id = str(_uuid.uuid4())
        state_json = json.dumps({"type": "idle"})
        mode_json = json.dumps({
            "mode": "Work",
            "branch_name": "task-22001-redesign-conversation-grounding-side-panel",
            "worktree_path": str(worktree),
            "base_branch": "main",
            "task_id": "22001",
            "task_title": "Redesign conversation grounding side panel",
        })
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
                "Fixture Grounding Panel QA",
                str(worktree),
                state_json,
                now,
                now,
                now,
                project_id,
                mode_json,
                "qa:grounding-panel",
            ),
        )
        msg_id = str(_uuid.uuid4())
        conn.execute(
            "INSERT INTO messages ("
            " message_id, conversation_id, sequence_id, message_type,"
            " content, created_at"
            ") VALUES (?, ?, 0, 'user', ?, ?)",
            (
                msg_id,
                conv_id,
                json.dumps({
                    "text": "Review the seeded Grounding Panel QA integration conversation",
                    "images": [],
                }),
                now,
            ),
        )
        return True

    def _project_and_drop_conv_mode(conn: sqlite3.Connection) -> None:
        """Project the seeded conv_mode blob into the normalized cm_* columns
        (mirrors migration 028, including the empty-string -> NULL clean-up),
        then drop the blob so the seeded DB matches the post-migration-029
        production schema. The conv_mode migrations are pre-stamped, so the app
        will not re-run 028/029 against the dropped column.

        Runs in BOTH the full-seed and repair paths. Fixtures created in the
        repair branch carry a conv_mode blob too; without this projection their
        cm_* columns stay NULL and they hydrate as Explore instead of Work — so
        the very Work-mode grounding (task/branch/worktree) the fixture exists to
        demonstrate would never appear."""
        cols = {row[1] for row in conn.execute("PRAGMA table_info(conversations)")}
        if "conv_mode" not in cols:
            return
        conn.execute(
            "UPDATE conversations SET"
            " cm_kind = lower(json_extract(conv_mode, '$.mode')),"
            " cm_branch_name = NULLIF(json_extract(conv_mode, '$.branch_name'), ''),"
            " cm_worktree_path = NULLIF(json_extract(conv_mode, '$.worktree_path'), ''),"
            " cm_base_branch = NULLIF(json_extract(conv_mode, '$.base_branch'), ''),"
            " cm_task_id = NULLIF(json_extract(conv_mode, '$.task_id'), ''),"
            " cm_task_title = NULLIF(json_extract(conv_mode, '$.task_title'), ''),"
            " cm_next_taskmd_id_hint = NULLIF(json_extract(conv_mode, '$.next_taskmd_id_hint'), '')"
            " WHERE json_valid(conv_mode) AND json_extract(conv_mode, '$.mode') IS NOT NULL"
        )
        conn.execute("ALTER TABLE conversations DROP COLUMN conv_mode")

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
            created_diff_fixture = _ensure_diff_review_fixture(
                conn,
                project_id=project_id,
            )
            created_grounding_fixture = _ensure_grounding_panel_qa_fixture(conn)
            # Repaired fixtures carry a conv_mode blob; project it to cm_* (and
            # drop the blob) so they hydrate as Work, not Explore.
            _project_and_drop_conv_mode(conn)
            conn.commit()
            if not quiet_if_populated:
                count = _existing_active_count(conn)
                suffixes = []
                if created_fixture:
                    suffixes.append("repaired perf fixture")
                if created_heavy_fixture:
                    suffixes.append("repaired heavy fixture")
                if created_diff_fixture:
                    suffixes.append("repaired diff fixture")
                if created_grounding_fixture:
                    suffixes.append("repaired grounding panel fixture")
                suffix = f" + {', '.join(suffixes)}" if suffixes else ""
                print(f"✓ Dev DB already populated ({count} conversations) — skipping seed{suffix}.")
            return

        print("Seeding dev DB with representative conversations...")

        # [1/5] Direct standalones
        print("  [1/5] Direct standalones")
        for text in _SEED_DIRECT_STANDALONES:
            _insert_conv(
                conn,
                text=text,
                conv_mode=direct_mode,
                cwd=str(ROOT),
                project_id=project_id,
            )

        # [2/5] Direct chains (3-member + 2-member)
        print("  [2/5] Direct chains (3-member + 2-member)")
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

        # [3/5] Explore mode chain
        print("  [3/5] Explore mode chain (2-member)")
        eid1, eslug1 = _insert_conv(
            conn, text=_SEED_EXPLORE_TEXT, conv_mode=explore_mode,
            cwd=str(ROOT), project_id=project_id,
        )
        _exhaust_state(conn, eid1)
        _link_continuation(
            conn, parent_id=eid1, parent_slug=eslug1, chain_index=2,
            conv_mode=explore_mode, cwd=str(ROOT), project_id=project_id,
        )

        # [4/5] Branch diff fixture
        print("  [4/5] Branch diff fixture")
        _ensure_diff_review_fixture(
            conn,
            project_id=project_id,
        )

        # [5/5] Grounding panel QA integration fixture
        print("  [5/5] Grounding panel QA fixture")
        _ensure_grounding_panel_qa_fixture(conn)

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

        # Match the post-migration-029 production schema: project conv_mode into
        # the normalized cm_* columns and drop the blob.
        _project_and_drop_conv_mode(conn)

        conn.commit()


def cmd_qa_grounding_panel() -> None:
    """Capture grounding panel Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:grounding-panel"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_meta_viewer() -> None:
    """Capture MetaViewer edge-state Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:meta-viewer"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_conversation_panel() -> None:
    """Capture conversation side panel Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:conversation-panel"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_sidebar() -> None:
    """Capture sidebar Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:sidebar"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_task_approval() -> None:
    """Capture task approval Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:task-approval"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_mobile_conversation_list() -> None:
    """Capture mobile conversation list Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:mobile-conversation-list"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_coordinator() -> None:
    """Capture Coordinator Ladle screenshots across responsive viewports."""
    subprocess.run(
        ["pnpm", "qa:coordinator"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_mobile_multi_pr_conversation() -> None:
    """Capture the mobile multi-PR conversation fixture."""
    subprocess.run(
        ["pnpm", "qa:mobile-multi-pr-conversation"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_desktop_multi_pr_conversation() -> None:
    """Capture the desktop multi-PR conversation fixture."""
    subprocess.run(
        ["pnpm", "qa:desktop-multi-pr-conversation"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_new_conversation() -> None:
    """Capture the new-conversation page at desktop and mobile sizes."""
    subprocess.run(
        ["pnpm", "qa:new-conversation"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_message_list() -> None:
    """Capture message list Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:message-list"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_tool_results() -> None:
    """Capture comprehensive tool-result Ladle screenshots at desktop and mobile sizes."""
    subprocess.run(
        ["pnpm", "qa:tool-results"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_work_actions() -> None:
    """Capture Work Actions Ladle screenshots into ignored local artifacts."""
    subprocess.run(
        ["pnpm", "qa:work-actions"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


def cmd_qa_commission_review() -> None:
    """Capture commission review Ladle screenshots at desktop and mobile sizes."""
    subprocess.run(
        ["pnpm", "qa:commission-review"],
        cwd=ROOT / "ui",
        check=True,
        env=node_env(),
    )


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
    phoenix_pid = get_pid(PHOENIX_PID_FILE)
    vite_pid = get_pid(VITE_PID_FILE)
    preferred_phoenix = phoenix_port or default_phoenix
    phoenix_port = int(PHOENIX_PORT_FILE.read_text().strip()) if phoenix_pid and PHOENIX_PORT_FILE.exists() else preferred_phoenix
    vite_port = int(VITE_PORT_FILE.read_text().strip()) if vite_pid and VITE_PORT_FILE.exists() else default_vite
    if not phoenix_pid:
        phoenix_port, _ = select_dev_ports(phoenix_port, vite_port)
    vite_was_running = vite_pid is not None

    build_rust(release=True)
    stop_process(PHOENIX_PID_FILE, "Phoenix")
    time.sleep(0.5)
    phoenix_tls = start_phoenix(port=phoenix_port, tls=tls)
    api_scheme = "https" if phoenix_tls else "http"

    if vite_was_running:
        vite_tls = start_vite(port=vite_port, phoenix_port=phoenix_port, phoenix_tls=phoenix_tls)
        ui_scheme = "https" if vite_tls else "http"
        print(f"Phoenix restarted. Vite ready for UI hot reload.")
        print(f"  UI:  {ui_scheme}://localhost:{vite_port}")
        print(f"  API: {api_scheme}://localhost:{phoenix_port}")
    else:
        print(f"Phoenix restarted. Vite not running (start with ./dev.py up).")
        print(f"  API: {api_scheme}://localhost:{phoenix_port}")


def cmd_status():
    """Check what's running."""
    phoenix_pid = get_pid(PHOENIX_PID_FILE)
    vite_pid = get_pid(VITE_PID_FILE)
    default_phoenix, default_vite = get_default_ports()
    running_phoenix = int(PHOENIX_PORT_FILE.read_text().strip()) if phoenix_pid and PHOENIX_PORT_FILE.exists() else default_phoenix
    running_vite = int(VITE_PORT_FILE.read_text().strip()) if vite_pid and VITE_PORT_FILE.exists() else default_vite
    
    print(f"Worktree: {ROOT}")
    print(f"  Hash: {get_worktree_hash()}")
    print(f"  Default ports: Phoenix={default_phoenix}, Vite={default_vite}")
    if running_phoenix != default_phoenix or running_vite != default_vite:
        print(f"  Running ports: Phoenix={running_phoenix}, Vite={running_vite}")
    print(f"  Database: {get_db_path()}")
    print()

    if phoenix_pid:
        print(f"Phoenix: running (PID {phoenix_pid})")
        if scheme := _probe_phoenix_scheme(running_phoenix):
            print(f"  URL: {scheme}://localhost:{running_phoenix}")
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

            scheme = _probe_phoenix_scheme(running_phoenix) or "http"
            context = ssl._create_unverified_context() if scheme == "https" else None
            with urllib.request.urlopen(
                f"{scheme}://localhost:{running_phoenix}/api/models",
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
    2. Playwright's standard install dir at `/opt/pw-browsers/chromium-*/chrome-linux*/chrome`.
    3. Playwright's per-user cache at `~/.cache/ms-playwright/chromium-*/chrome-linux*/chrome`.
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
        (Path("/opt/pw-browsers"), "chromium-*/chrome-linux*/chrome"),
        (home / ".cache" / "ms-playwright", "chromium-*/chrome-linux*/chrome"),
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


# ===========================================================================
# Incremental check gating (Option A: path-based lane selection)
# ===========================================================================
# The lane registry: one row per check lane. Lane names match the thread
# names in cmd_check. `inputs` lists the path categories that, if touched,
# require the lane to run; None marks an always-on lane (sub-second and
# correctness-sensitive: rustfmt, task validation, lockfile tripwire).
# `desc` feeds the --pretty renderer and `check --lanes` validation errors.
_LANE_DEFS = [
    # (name, inputs, desc)
    ("rust", {"RUST"}, "codegen + cargo test (+ musl smoke on macOS)"),
    ("cargo-fmt", {"RUST"}, "cargo fmt --check"),
    ("clippy", {"RUST"}, "cargo clippy --all-targets (own target dir)"),
    ("tsc", {"UI"}, "tsc -b --noEmit project typecheck"),
    ("ui-lint", {"UI"}, "eslint + stylelint"),
    ("vitest", {"UI"}, "vitest run"),
    ("ast-grep", {"UI", "RUST", "ASTGREP"}, "structural lint rules over ui/src/ + crates/"),
    ("allium", {"SPECS"}, "allium spec validation"),
    ("spec-shape", {"SPECS"}, "spEARS v2 artifact-shape validation"),
    # spec-anchors cross-validates REQ-* anchors in code against specs/, so a
    # change to either side can orphan or satisfy an anchor.
    ("spec-anchors", {"SPECS", "RUST", "UI"}, "REQ-* anchor cross-validation"),
    ("e2e", {"RUST", "E2E"}, "API-boundary tests via a real binary"),
    ("task", None, "task filename validation"),
    ("pkglock", {"UI"}, "pnpm-lock drift tripwire"),
]
_LANE_INPUTS = {name: inputs for name, inputs, _ in _LANE_DEFS if inputs is not None}
_ALWAYS_ON_LANES = {name for name, inputs, _ in _LANE_DEFS if inputs is None}
_LANE_DESCS = {name: desc for name, _, desc in _LANE_DEFS}


_CI_LANE_GROUPS = {
    "rust": {"rust", "cargo-fmt"},
    "clippy": {"clippy"},
    "e2e": {"e2e"},
    "ui": {"tsc", "ui-lint", "vitest", "ast-grep", "allium", "spec-shape", "spec-anchors", "pkglock"},
    "fast": {"task"},
}
_LANE_TO_CI_GROUP = {
    lane: group
    for group, lanes in _CI_LANE_GROUPS.items()
    for lane in lanes
}


_LANE_ALIASES = {
    "fast": {"task"},
}


def _categorize_changed_paths(paths) -> set:
    """Map repo-relative changed paths to coarse lane-input categories.

    A path may contribute to several categories. `SELF` (dev.py itself)
    is special-cased by the caller into "run everything".
    """
    cats: set[str] = set()
    for p in paths:
        if p == "dev.py":
            cats.add("SELF")
        if p.startswith(".github/workflows/"):
            cats.add("SELF")
        if (
            p.startswith("crates/")
            or p in ("Cargo.toml", "Cargo.lock")
            or p.startswith(".cargo/")
            or p.startswith("rust-toolchain")
        ):
            cats.add("RUST")
        # Generated TS is produced by the Rust ts-rs export tests; a committed
        # change there must re-run the rust lane's codegen-stale guard.
        if p.startswith("ui/src/generated/"):
            cats.add("RUST")
        if p.startswith("ui/") and not p.startswith("ui/dist/"):
            cats.add("UI")
        if p.startswith("tasks/"):
            cats.add("TASKS")
        if p.startswith("specs/"):
            cats.add("SPECS")
        if p.startswith("tests/devpy/"):
            cats.add("SPECS")
        if p == "scripts/check_rust_test_timing.py":
            cats.update({"ASTGREP", "SPECS"})
        if p.startswith("ast-grep-rules/"):
            cats.add("ASTGREP")
        if p.startswith("tests/e2e/") or p == "phoenix-client.py":
            cats.add("E2E")
    return cats


# Single-slot memo for _changed_paths_vs_base: lane gating and rust
# crate-scoping both consume the diff, and the git state cannot change
# mid-check, so the ~8 git subprocesses behind it run once per invocation.
_changed_paths_memo: list = []


def _changed_paths_vs_base():
    """Return a set of repo-relative paths that differ from the integration
    base, or None if the base cannot be determined (caller runs everything).
    Memoized for the lifetime of the invocation.

    "Differ from base" = tracked changes between merge-base(HEAD, base) and the
    working tree, unioned with untracked-but-not-ignored files. The base ref is
    PHOENIX_CHECK_BASE if set, else the remote default branch (origin/HEAD),
    else origin/main, else main.
    """
    if not _changed_paths_memo:
        _changed_paths_memo.append(_compute_changed_paths_vs_base())
    return _changed_paths_memo[0]


def _resolve_check_merge_base():
    """Resolve the comparison commit used by incremental check planning."""
    def _git(args):
        return subprocess.run(
            ["git", *args], cwd=ROOT, capture_output=True, text=True,
        )

    base = os.environ.get("PHOENIX_CHECK_BASE")
    candidates = [base] if base else []
    head_ref = _git(["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
    if head_ref.returncode == 0 and head_ref.stdout.strip():
        candidates.append(head_ref.stdout.strip().removeprefix("refs/remotes/"))
    candidates += ["origin/main", "main", "origin/master", "master"]

    for candidate in candidates:
        if not candidate:
            continue
        merge_base = _git(["merge-base", "HEAD", candidate])
        if merge_base.returncode == 0 and merge_base.stdout.strip():
            return merge_base.stdout.strip()
    return None


def _compute_changed_paths_vs_base():
    def _git(args):
        return subprocess.run(
            ["git", *args], cwd=ROOT, capture_output=True, text=True,
        )

    merge_base = _resolve_check_merge_base()
    if merge_base is None:
        return None

    tracked = _git(["diff", "--name-only", merge_base])
    if tracked.returncode != 0:
        return None
    untracked = _git(["ls-files", "--others", "--exclude-standard"])
    if untracked.returncode != 0:
        return None

    paths = set()
    for chunk in (tracked.stdout, untracked.stdout):
        paths.update(line.strip() for line in chunk.splitlines() if line.strip())
    return paths


def _on_integration_base():
    """True when the checked-out branch IS the integration base branch
    (origin/HEAD's target, else main/master, else PHOENIX_CHECK_BASE).

    Incremental gating only makes sense on a feature branch measured against
    its base; on the base branch the merge-base diff is degenerate. A detached
    HEAD returns False — checking out a base commit without the branch ref is
    not "working on main", and we'd rather gate than over-run there.
    """
    def _git(args):
        return subprocess.run(
            ["git", *args], cwd=ROOT, capture_output=True, text=True,
        )

    cur = _git(["symbolic-ref", "--quiet", "--short", "HEAD"])
    if cur.returncode != 0 or not cur.stdout.strip():
        return False
    current = cur.stdout.strip()

    base_names = {"main", "master"}
    head_ref = _git(["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
    if head_ref.returncode == 0 and head_ref.stdout.strip():
        base_names.add(head_ref.stdout.strip().removeprefix("refs/remotes/origin/"))
    explicit = os.environ.get("PHOENIX_CHECK_BASE")
    if explicit:
        base_names.add(explicit.removeprefix("origin/"))
    return current in base_names


def _gate_lanes():
    """Decide which lanes to run for this check invocation.

    Returns (active: set[str], skipped: dict[str, str]) where skipped maps a
    lane key to a human reason. Always-on lanes are always in `active`.
    Honors PHOENIX_CHECK_ALL=1 (run everything, no gating).
    """
    all_lanes = set(_LANE_INPUTS) | _ALWAYS_ON_LANES
    if os.environ.get("PHOENIX_CHECK_ALL") == "1":
        return all_lanes, {}

    # CI must choose its gating mode explicitly. Auto-derived gating in CI
    # once skipped every lane on main pushes (merge-base(HEAD, origin/main)
    # == HEAD -> empty diff -> 1-second green check), so a CI run that
    # reaches here without an explicit base refuses loudly instead of
    # guessing. Full runs opt out via --all / PHOENIX_CHECK_ALL=1; gated
    # runs opt in via PHOENIX_CHECK_BASE.
    if os.environ.get("CI"):
        if not os.environ.get("PHOENIX_CHECK_BASE"):
            print(
                "✗ CI detected, but neither PHOENIX_CHECK_ALL=1 (full run) nor\n"
                "  PHOENIX_CHECK_BASE (explicit gating base, e.g. origin/main) is set.\n"
                "  Refusing to auto-derive an incremental-gating base in CI: on a\n"
                "  push to the base branch the derived diff is empty and every lane\n"
                "  would be silently skipped.",
                file=sys.stderr,
            )
            sys.exit(1)
        paths = _changed_paths_vs_base()
        if paths is None:
            print(
                f"✗ PHOENIX_CHECK_BASE={os.environ['PHOENIX_CHECK_BASE']} did not "
                "resolve to a merge base.\n"
                "  Fetch the ref in CI (e.g. checkout fetch-depth: 0) or run with "
                "PHOENIX_CHECK_ALL=1.",
                file=sys.stderr,
            )
            sys.exit(1)
    elif _on_integration_base():
        # HEAD is the integration base branch itself. Gating compares HEAD
        # against that base, so the diff is degenerate (empty, or reversed
        # once unpushed commits land) and lane selection is meaningless. CI
        # forbids this path entirely (the CI block above); locally we just
        # run every lane. Mirrors the SELF short-circuit below.
        return all_lanes, {}
    else:
        paths = _changed_paths_vs_base()
        if paths is None:
            # Base undeterminable locally — fail safe, run everything.
            return all_lanes, {}

    cats = _categorize_changed_paths(paths)
    if "SELF" in cats:
        # dev.py changed: the gating logic / lane definitions may differ.
        return all_lanes, {}

    active = set(_ALWAYS_ON_LANES)
    skipped = {}
    for lane, inputs in _LANE_INPUTS.items():
        hit = inputs & cats
        if hit:
            active.add(lane)
        else:
            need = "/".join(sorted(inputs))
            skipped[lane] = f"no {need} changes"
    return active, skipped


def _all_lanes() -> set[str]:
    return {name for name, _, _ in _LANE_DEFS}


def _resolve_check_lanes(gate: bool = True, lanes: str | None = None):
    """Return the active/skipped lane sets for a check invocation.

    This is the side-effect-free planner used by both `check` and the CI
    planning command. `lanes` has the same comma-separated filter semantics as
    `./dev.py check --lanes`.
    """
    if gate:
        active, skipped = _gate_lanes()
    else:
        active, skipped = _all_lanes(), {}

    if lanes is not None:
        requested_raw = {l.strip() for l in lanes.split(",") if l.strip()}
        known = _all_lanes()
        valid = known | set(_LANE_ALIASES)
        unknown = requested_raw - valid
        if unknown:
            print(f"✗ unknown lane(s): {', '.join(sorted(unknown))}\n"
                  f"  valid lanes: {', '.join(sorted(valid))}", file=sys.stderr)
            sys.exit(1)
        requested = set()
        for lane in requested_raw:
            requested |= _LANE_ALIASES.get(lane, {lane})
        active &= requested
        skipped = {k: v for k, v in skipped.items() if k in requested}
    return active, skipped


def _ci_groups_for_active_lanes(active: set[str]) -> dict[str, bool]:
    return {
        group: bool(active & lanes)
        for group, lanes in sorted(_CI_LANE_GROUPS.items())
    }


def _ci_lane_inventory_errors() -> list[str]:
    lane_names = _all_lanes()
    assigned = set(_LANE_TO_CI_GROUP)
    errors = []
    missing = lane_names - assigned
    extra = assigned - lane_names
    if missing:
        errors.append(f"lane(s) missing from _CI_LANE_GROUPS: {', '.join(sorted(missing))}")
    if extra:
        errors.append(f"unknown lane(s) assigned in _CI_LANE_GROUPS: {', '.join(sorted(extra))}")
    for lane in sorted(lane_names):
        groups = [g for g, lanes in _CI_LANE_GROUPS.items() if lane in lanes]
        if len(groups) > 1:
            errors.append(f"lane {lane!r} assigned to multiple CI groups: {', '.join(groups)}")
    return errors


def cmd_check_plan(gate: bool = True, lanes: str | None = None, fmt: str = "json") -> None:
    """Print the lane/group plan without running any check lanes."""
    errors = _ci_lane_inventory_errors()
    if errors:
        print("✗ invalid CI lane inventory:\n  - " + "\n  - ".join(errors), file=sys.stderr)
        sys.exit(1)

    active, skipped = _resolve_check_lanes(gate=gate, lanes=lanes)
    paths = None if (not gate or os.environ.get("PHOENIX_CHECK_ALL") == "1") else _changed_paths_vs_base()
    categories = None if paths is None else sorted(_categorize_changed_paths(paths))
    groups = _ci_groups_for_active_lanes(active)
    group_lanes = {
        group: sorted(active & lanes_in_group)
        for group, lanes_in_group in sorted(_CI_LANE_GROUPS.items())
    }
    payload = {
        "schema": "phoenix-check-plan/1",
        "active_lanes": sorted(active),
        "skipped_lanes": {k: skipped[k] for k in sorted(skipped)},
        "ci_groups": groups,
        "ci_group_lanes": group_lanes,
        "changed_paths": None if paths is None else sorted(paths),
        "categories": categories,
    }
    if fmt == "github-env":
        for group, enabled in groups.items():
            print(f"{group}={'true' if enabled else 'false'}")
        print("active_lanes=" + ",".join(payload["active_lanes"]))
    else:
        print(json.dumps(payload, sort_keys=True, separators=(",", ":")))


# Crate-level scoping for the rust lane (Stage 3 / per-crate gating).
#
# When the rust lane runs, we can often narrow `cargo clippy`/`cargo test` to
# just the changed crate(s) PLUS their reverse dependencies, instead of the
# whole workspace. The reverse-dependency closure is computed live from
# `cargo metadata` so it stays correct as crates are added/removed — no
# hand-maintained graph to drift.
#
# CONTRACT: this helper only ever NARROWS. Any uncertainty returns None, which
# the caller treats as "scope to the full workspace". A wrong narrowing would
# silently skip tests that should run (a correctness bug strictly worse than
# no gating), so every ambiguous case fails safe to full.

# Rust changes that are NOT confined to a single crate's source — these force a
# full-workspace rust lane because their blast radius isn't a crate subtree.
_RUST_NONCRATE_PREFIXES = (".cargo/", "rust-toolchain", "ui/src/generated/")
_RUST_NONCRATE_FILES = ("Cargo.toml", "Cargo.lock")


# Single-slot memo for `cargo metadata --no-deps`: rust crate-scoping and
# ts-rs export-crate discovery both read it, and the workspace manifest set
# cannot change mid-check.
_workspace_metadata_memo: list = []


def _workspace_metadata(env=None):
    """Parsed `cargo metadata --format-version 1 --no-deps` for the workspace.

    Ambient-environment calls are memoized for workspace checks. An explicit
    environment bypasses that memo so Cargo configuration such as
    `CARGO_TARGET_DIR` matches the command being prepared.
    """
    if env is None and _workspace_metadata_memo:
        return _workspace_metadata_memo[0]
    try:
        out = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=ROOT, capture_output=True, text=True, timeout=60, env=env,
        )
        meta = json.loads(out.stdout) if out.returncode == 0 else None
    except (subprocess.TimeoutExpired, OSError, json.JSONDecodeError):
        meta = None
    if env is None:
        _workspace_metadata_memo.append(meta)
    return meta


def _ts_export_crates():
    """Workspace crates that can emit ts-rs `export_bindings_*` tests — i.e.
    crates with a direct ts-rs dependency (`#[derive(ts_rs::TS)]` cannot
    appear without one, and ts-rs emits each export test into the crate that
    declares the type). Running the export tests of exactly these crates
    therefore produces the complete generated tree — the same scratch export
    a full-workspace codegen run would. Over-inclusion is safe (a ts-rs
    dependent with no `#[ts(export)]` types just adds compile scope).
    Returns None when metadata is unavailable (caller falls back to full
    workspace)."""
    meta = _workspace_metadata()
    if meta is None:
        return None
    return {
        p["name"] for p in meta["packages"]
        if any(d["name"] == "ts-rs" for d in p["dependencies"])
    }


def _workspace_rdeps_map():
    """Map each workspace crate -> set of crates that (transitively) depend on
    it, INCLUDING the crate itself. Derived from `cargo metadata`. Returns None
    on any failure (caller falls back to full workspace)."""
    meta = _workspace_metadata()
    if meta is None:
        return None
    ws = {p["name"] for p in meta["packages"]}
    # direct intra-workspace dependency edges: crate -> {deps in ws}
    deps = {
        p["name"]: {d["name"] for d in p["dependencies"] if d["name"] in ws}
        for p in meta["packages"]
    }
    # also map a crate's manifest directory (relative to ROOT) for path->crate
    manifest_dir = {}
    for p in meta["packages"]:
        try:
            rel = Path(p["manifest_path"]).parent.relative_to(ROOT)
            manifest_dir[str(rel)] = p["name"]
        except ValueError:
            pass
    # transitive reverse-deps: who must be re-checked when `target` changes
    rmap = {}
    for target in ws:
        closure = {target}
        changed = True
        while changed:
            changed = False
            for crate, crate_deps in deps.items():
                if crate not in closure and (closure & crate_deps):
                    closure.add(crate)
                    changed = True
        rmap[target] = closure
    return rmap, manifest_dir


def _crate_for_path(rel_path, manifest_dir):
    """Return the workspace crate owning a repo-relative path, or None if the
    path isn't inside a known crate's directory."""
    # Longest manifest-dir prefix wins (handles nested crates/ layouts).
    best = None
    for d, name in manifest_dir.items():
        if rel_path == d or rel_path.startswith(d + "/"):
            if best is None or len(d) > len(best[0]):
                best = (d, name)
    return best[1] if best else None


def _rust_scope_crates(paths):
    """Given the changed repo-relative paths, return the set of crate names the
    rust lane should scope to (changed crates + their reverse-dep closure), or
    None to mean 'no safe narrowing — run the full workspace'.

    Returns None (full) when:
      - a non-crate rust input changed (Cargo.lock, .cargo/, rust-toolchain,
        ui/src/generated/) — blast radius isn't a crate subtree;
      - any changed path under crates/ can't be mapped to a crate;
      - cargo metadata is unavailable.
    """
    rust_paths = [
        p for p in paths
        if p.startswith("crates/")
        or p in _RUST_NONCRATE_FILES
        or p.startswith(_RUST_NONCRATE_PREFIXES)
    ]
    # Non-crate-confined rust change -> can't narrow.
    for p in rust_paths:
        if p in _RUST_NONCRATE_FILES or p.startswith(_RUST_NONCRATE_PREFIXES):
            return None
    crate_paths = [p for p in rust_paths if p.startswith("crates/")]
    if not crate_paths:
        return None
    built = _workspace_rdeps_map()
    if built is None:
        return None
    rmap, manifest_dir = built
    scope = set()
    for p in crate_paths:
        crate = _crate_for_path(p, manifest_dir)
        if crate is None or crate not in rmap:
            # A crate path we can't attribute — fail safe to full.
            return None
        scope |= rmap[crate]
    return scope or None


class _PlainReporter:
    """Default check output: one line per event, printed as it happens.

    This is the exact pre-existing `./dev.py check` output format; --pretty
    swaps in _PrettyReporter, which renders the same event stream as a live
    lane table instead.
    """

    def __init__(self):
        self._lock = threading.Lock()

    def info(self, msg: str) -> None:
        with self._lock:
            print(f"  i  {msg}")

    def banner(self, msg: str) -> None:
        with self._lock:
            print(msg)

    def lane_start(self, lane: str) -> None:
        pass

    def lane_done(self, lane: str) -> None:
        pass

    def lane_skipped(self, lane: str, reason: str) -> None:
        with self._lock:
            print(f"  -  {lane:<18s} (skipped — {reason})")

    def step_start(self, lane: str, step: str) -> None:
        pass

    def step_skipped(self, lane: str, step: str, reason: str) -> None:
        with self._lock:
            print(f"  -  {step:<18s} (skipped — {reason})")

    def step_done(self, lane: str, step: str, rc: int, elapsed: float) -> None:
        with self._lock:
            sym = "✓" if rc == 0 else "✗"
            print(f"  {sym} {step:<18s} ({elapsed:.1f}s)")

    def close(self) -> None:
        pass


class _PrettyReporter:
    """Live lane-table renderer for `--pretty` (requires rich).

    One row per lane: status glyph, lane name, completed steps with their
    times, and the currently running step with a live elapsed counter.
    Consumes the same event stream _PlainReporter prints line-by-line.
    """

    def __init__(self, active: set, skipped: dict, descs: dict):
        from rich.console import Console
        from rich.live import Live
        from rich.table import Table
        from rich.text import Text

        self._Table, self._Text = Table, Text
        self._lock = threading.Lock()
        self._t0 = time.monotonic()
        self._infos: list[str] = []
        self._lanes: dict[str, dict] = {}
        for name in sorted(active):
            self._lanes[name] = {
                "status": "pending", "steps": [], "current": None,
                "t_start": None, "t_end": None, "desc": descs.get(name, ""),
            }
        for name in sorted(skipped):
            self._lanes[name] = {
                "status": "skipped", "reason": skipped[name], "steps": [],
                "current": None, "t_start": None, "t_end": None,
                "desc": descs.get(name, ""),
            }
        # Live starts lazily on the first lane_start: everything before the
        # fan-out (env probes, dep bootstrap, codegen) prints directly to
        # stdout, and an active Live would garble those lines.
        self._live = Live(
            get_renderable=self._render, refresh_per_second=8,
            console=Console(), transient=False,
        )
        self._started = False

    def _ensure_live(self) -> None:
        if not self._started:
            self._started = True
            self._live.start()

    _SPINNER = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"

    def _render(self):
        with self._lock:
            now = time.monotonic()
            frame = self._SPINNER[int(now * 8) % len(self._SPINNER)]
            table = self._Table(box=None, pad_edge=False, show_header=False, padding=(0, 1))
            table.add_column("st", width=1)
            table.add_column("lane", min_width=14)
            table.add_column("detail", overflow="fold")
            table.add_column("time", justify="right", min_width=7)
            for info in self._infos:
                table.add_row("", self._Text(info, style="dim"), "", "")
            for name, st in self._lanes.items():
                steps = self._Text()
                for step, rc, elapsed in st["steps"]:
                    steps.append(("✓" if rc == 0 else "✗") + step,
                                 style="green" if rc == 0 else "red")
                    steps.append(f" {elapsed:.1f}s  ", style="dim")
                if st["current"]:
                    cur, cur_t0 = st["current"]
                    steps.append(f"{frame} {cur}", style="yellow")
                    steps.append(f" {now - cur_t0:.0f}s", style="dim")
                if st["status"] == "skipped":
                    glyph, style = "-", "dim"
                    steps = self._Text(f"skipped — {st['reason']}", style="dim")
                elif st["status"] == "pending":
                    glyph, style = "·", "dim"
                    steps = self._Text(st["desc"], style="dim")
                elif st["status"] == "running":
                    glyph, style = frame, "yellow"
                elif any(rc != 0 for _, rc, _ in st["steps"]):
                    glyph, style = "✗", "red"
                else:
                    glyph, style = "✓", "green"
                if st["t_end"] is not None and st["t_start"] is not None:
                    t = f"{st['t_end'] - st['t_start']:.1f}s"
                elif st["t_start"] is not None:
                    t = f"{now - st['t_start']:.0f}s"
                else:
                    t = ""
                table.add_row(self._Text(glyph, style=style),
                              self._Text(name, style=style), steps,
                              self._Text(t, style="dim"))
            return table

    def info(self, msg: str) -> None:
        with self._lock:
            self._infos.append(msg)

    def banner(self, msg: str) -> None:
        pass

    def lane_start(self, lane: str) -> None:
        self._ensure_live()
        with self._lock:
            st = self._lanes.setdefault(lane, {"steps": [], "current": None,
                                               "t_end": None, "desc": ""})
            st["status"] = "running"
            st["t_start"] = time.monotonic()

    def lane_done(self, lane: str) -> None:
        with self._lock:
            st = self._lanes[lane]
            st["status"] = "done"
            st["current"] = None
            st["t_end"] = time.monotonic()

    def lane_skipped(self, lane: str, reason: str) -> None:
        with self._lock:
            self._lanes[lane] = {"status": "skipped", "reason": reason,
                                 "steps": [], "current": None,
                                 "t_start": None, "t_end": None, "desc": ""}

    def step_start(self, lane: str, step: str) -> None:
        with self._lock:
            self._lanes[lane]["current"] = (step, time.monotonic())

    def step_skipped(self, lane: str, step: str, reason: str) -> None:
        with self._lock:
            self._infos.append(f"{step}: skipped — {reason}")

    def step_done(self, lane: str, step: str, rc: int, elapsed: float) -> None:
        with self._lock:
            st = self._lanes[lane]
            st["current"] = None
            st["steps"].append((step, rc, elapsed))

    def close(self) -> None:
        if self._started:
            self._live.stop()


def _make_reporter(pretty: bool, active: set, skipped: dict):
    """Build the requested reporter, degrading to plain if rich is missing.

    rich is deliberately NOT in this script's PEP 723 dependencies — the
    default (plain) path must never download it. main() re-execs through
    `uv run --with rich` when --pretty is requested; if that bootstrap
    didn't deliver an importable rich, fall back to plain with a notice.
    """
    if pretty:
        try:
            return _PrettyReporter(active, skipped, _LANE_DESCS)
        except ImportError:
            print("  i  rich not available — falling back to plain output",
                  file=sys.stderr)
    return _PlainReporter()


def _is_valid_git_config_key(key):
    """Match Git's section[.subsection].name shape without restricting subsections."""
    import re

    first_dot = key.find(".")
    last_dot = key.rfind(".")
    if first_dot <= 0 or last_dot == len(key) - 1 or "\0" in key or "\n" in key:
        return False
    component = re.compile(r"^[A-Za-z][A-Za-z0-9-]*$")
    return bool(component.fullmatch(key[:first_dot]) and component.fullmatch(key[last_dot + 1:]))


def _append_git_config_override(key, value, environ=None):
    """Append a complete process-level Git config entry, resetting malformed input."""
    env = os.environ if environ is None else environ
    parameters = env.get("GIT_CONFIG_PARAMETERS", "").strip()
    env["GIT_CONFIG_PARAMETERS"] = f"{parameters} 'commit.gpgsign=false'".strip()
    try:
        count = int(env.get("GIT_CONFIG_COUNT", "0"))
        if count < 0:
            raise ValueError
        if any(
            f"GIT_CONFIG_KEY_{index}" not in env
            or f"GIT_CONFIG_VALUE_{index}" not in env
            or not _is_valid_git_config_key(env[f"GIT_CONFIG_KEY_{index}"])
            for index in range(count)
        ):
            raise ValueError
    except ValueError:
        for name in list(env):
            if name == "GIT_CONFIG_COUNT" or name.startswith(("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_")):
                env.pop(name, None)
        count = 0

    if key != "commit.gpgsign" or value != "false":
        env[f"GIT_CONFIG_KEY_{count}"] = key
        env[f"GIT_CONFIG_VALUE_{count}"] = value
        env["GIT_CONFIG_COUNT"] = str(count + 1)
    else:
        env["GIT_CONFIG_COUNT"] = str(count)


def cmd_check(gate: bool = True, lanes: str | None = None, pretty: bool = False):
    """Run lint, format check, tests, and task validation in parallel.

    `lanes` is an optional comma-separated lane subset (CI splits the check
    across runners with it); gating still applies within the subset unless
    gate=False. `pretty` renders a live lane table instead of line-per-event
    output.
    """
    results = []  # (name, returncode, elapsed, output)
    results_lock = threading.Lock()
    t_start = time.monotonic()

    # Per-step kill-and-fail budget. 600s covers cold `cargo test compile`
    # on a 4-vCPU GitHub Actions runner without sccache (observed ~5min);
    # local M-class hardware finishes in ~90s. Bump only if a single step
    # legitimately takes longer — never to mask flakes.
    CHECK_TIMEOUT = 600

    # Decide which lanes to run (Option A path-gating) BEFORE any
    # lane-specific setup runs. A skipped lane must not drag in its setup:
    # a markdown/spec/task-only change must not require Corepack/pnpm or a
    # working cargo toolchain to be present. --all / PHOENIX_CHECK_ALL=1
    # (handled in _gate_lanes) forces every lane.
    active, skipped = _resolve_check_lanes(gate=gate, lanes=lanes)
    if lanes is not None and not active:
        print("  i  no requested lane is active for this change set")

    reporter = _make_reporter(pretty, active, skipped)
    if lanes is not None:
        reporter.info(f"lane filter: [{', '.join(sorted(active | set(skipped)))}]")
    if skipped:
        ran = ", ".join(sorted(active))
        reporter.info(f"incremental gating: running [{ran}]")
        for lane in sorted(skipped):
            with results_lock:
                results.append((lane, 0, 0.0, ""))
            reporter.lane_skipped(lane, skipped[lane])

    # Lane groups whose shared, expensive setup is gated below. UI lanes need
    # Corepack/pnpm (ensure_ui_deps); cargo lanes need the rustc/sccache env
    # and the env-classification probes. Only `rust` consumes the nextest
    # probe + thread sizing + codegen commands.
    ui_active = bool(active & {"tsc", "ui-lint", "vitest"})
    cargo_active = bool(active & {"rust", "e2e"})

    # Stage 3 / per-crate gating: when the rust lane runs under gating, try to
    # narrow clippy + test to the changed crate(s) and their reverse-dep
    # closure instead of the whole workspace. `rust_scope` is either a list of
    # crate names to pass as `-p <crate>` flags, or None meaning "no safe
    # narrowing — run the full workspace". Both escape hatches force full:
    # the `--all` flag (gate=False) and the PHOENIX_CHECK_ALL=1 env var — kept
    # in lockstep with _gate_lanes(), which honors the same env var.
    _force_full = (not gate) or os.environ.get("PHOENIX_CHECK_ALL") == "1"
    rust_scope = None
    if not _force_full and "rust" in active:
        _scope = _rust_scope_crates(_changed_paths_vs_base() or set())
        if _scope is not None:
            rust_scope = sorted(_scope)
            reporter.info(f"rust scope: -p {' -p '.join(rust_scope)} "
                          f"(changed crate(s) + reverse-deps)")

    def _pflags():
        """`-p crate` flags for the current rust scope, or [] for full workspace."""
        if not rust_scope:
            return []
        out = []
        for c in rust_scope:
            out += ["-p", c]
        return out

    def run_step(name, cmd, cwd=ROOT, env_extra=None):
        # Stream stdout+stderr line-by-line into a bounded buffer so that on
        # timeout we keep the last N lines of output instead of throwing it
        # away. Process is launched in its own session so SIGKILL can target
        # the whole group \u2014 vitest, eslint, etc. fork worker children; killing
        # only the parent leaves orphans whose pipes the harness can still
        # be blocked on, masking the original hang.
        from collections import deque
        TAIL_LINES = 200
        t0 = time.monotonic()
        # Lane attribution for the reporter: steps always run on their lane's
        # thread, and lane threads are named after their lane.
        lane = threading.current_thread().name
        reporter.step_start(lane, name)
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
        if env_extra:
            env.update(env_extra)
        buf: deque[str] = deque(maxlen=TAIL_LINES)
        truncated = False
        # Cargo target-lock telemetry: cargo prints "Blocking waiting for
        # file lock on <what>" the moment it blocks, and the next output line
        # marks acquisition. Accumulating the gaps separates lock-wait from
        # build time in the step's elapsed figure (see tasks/76001).
        lock_wait = 0.0
        lock_t0: float | None = None

        proc = subprocess.Popen(
            cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, env=env, start_new_session=True, bufsize=1,
        )

        def reader():
            nonlocal truncated, lock_wait, lock_t0
            assert proc.stdout is not None
            for line in proc.stdout:
                now = time.monotonic()
                if lock_t0 is not None:
                    lock_wait += now - lock_t0
                    lock_t0 = None
                if len(buf) == buf.maxlen:
                    truncated = True
                # Strip terminal control sequences before buffering. The env
                # above disables color for tools that honour it, but rustfmt's
                # --check diff colours via the `term` crate keyed on $TERM and
                # ignores both NO_COLOR and CARGO_TERM_COLOR; this is the
                # tool-agnostic backstop so the reprinted buffer stays clean.
                clean = _CONTROL_SEQ_RE.sub("", line).rstrip("\n")
                if "Blocking waiting for file lock" in clean:
                    lock_t0 = now
                buf.append(clean)
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
        # Still-open block (process killed while waiting): count to the end.
        if lock_t0 is not None:
            lock_wait += elapsed - (lock_t0 - t0)
        if lock_wait >= 1.0:
            reporter.info(
                f"{name}: {lock_wait:.1f}s of {elapsed:.1f}s spent blocked "
                "on a cargo file lock"
            )

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
            results.append((name, rc, elapsed, output))
        reporter.step_done(lane, name, rc, elapsed)
        return rc

    def lane_rust():
        """Rust lane: test compile → codegen export → staleness diff →
        test run → [musl smoke check].

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

        Test compile and the later steps are split so each gets its own
        CHECK_TIMEOUT budget. Cold test-binary compiles on this codebase can
        approach 300s on their own, and when bundled with ~50s of test runtime
        the combined step exceeds the timeout even though nothing is wrong.

        Codegen runs here, but never touches `ui/src/generated/`: the ts-rs
        `export_bindings_*` tests run with TS_RS_EXPORT_DIR pointed at a
        scratch dir under target/, and the staleness step diffs that scratch
        export against the committed tree. The committed tree is therefore
        frozen for the whole check — the tsc, vitest, eslint, and ast-grep
        lanes read it concurrently with no ordering dependency on this lane.
        `test_cmd` excludes `export_bindings` (the codegen step just ran
        them — no lost coverage). `compile_cmd`/`test_cmd`/the thread cap are
        computed once in cmd_check and closed over here.
        """
        # clippy is NOT in this lane — it runs in lane_clippy with its own
        # target dir, so its RUSTC_WORKSPACE_WRAPPER=clippy-driver fingerprint
        # churn can never invalidate this dir's build (and the two builds
        # don't contend on the shared target lock).
        compile_rc = run_step("cargo test compile", compile_cmd)
        if compile_rc == 0:
            # TS_RS_EXPORT_DIR is read at test runtime (not macro expansion),
            # so this run reuses the harnesses compiled above verbatim.
            rc = run_step("codegen", codegen_cmd,
                          env_extra={"TS_RS_EXPORT_DIR": str(codegen_export_base)})
            if rc == 0:
                codegen_stale_step()
        run_step("cargo test", test_cmd)
        if sys.platform == "darwin":
            # macOS prod deploy uses native target (launchd_prod_deploy → prod_build target=None),
            # so the musl smoke check is opt-in: skip cleanly if the cross toolchain isn't installed.
            # See task 60001 for installing musl-cross-make on this machine.
            # (--target musl has its own fingerprint namespace, so it neither
            # disturbs nor is disturbed by the host-target steps around it.)
            if shutil.which("x86_64-linux-musl-gcc"):
                run_step("cargo check musl", [
                    "cargo", "check", "--target", "x86_64-unknown-linux-musl",
                ])
            else:
                reporter.step_skipped("rust", "cargo check musl",
                                      "x86_64-linux-musl-gcc not on PATH; see task 60001")

    def lane_clippy():
        """Clippy in its own target dir so it runs fully parallel to lane_rust.

        `cargo clippy` builds workspace crates through clippy-driver
        (RUSTC_WORKSPACE_WRAPPER), which carries a different fingerprint than the
        normal-rustc build lane_rust/codegen produce. Sharing one target dir
        would (a) make the two builds thrash each other's workspace fingerprints
        and (b) serialize them on cargo's exclusive target lock. A dedicated
        CARGO_TARGET_DIR sidesteps both, so clippy overlaps the test build/run
        instead of tailing it. Dependency compiles in the fresh dir are served
        by sccache (CI); the workspace check itself is cheap and rebuilds each
        run (this dir is outside rust-cache's saved target/).
        """
        # Scope to the changed crate(s)+rdeps when gating narrowed it; otherwise
        # lint the whole workspace. `-p` flags go before `--`. `--all-targets`
        # lints test/bench/example targets too, so test code is gated the same
        # as library code (a pedantic violation in a #[cfg(test)] module fails
        # CI); it composes with `-p` so the changed-crate scope still applies.
        run_step("cargo clippy",
                 ["cargo", "clippy", *_pflags(), "--all-targets", "--", "-D", "warnings"],
                 env_extra={"CARGO_TARGET_DIR": str(ROOT / "target" / "clippy")})

    def lane_ui_lint():
        """UI lint lane: eslint (TS/TSX) → stylelint (CSS).

        Both run on the same source tree and need no build artifact, so
        bundling them into one thread avoids spending a parallel slot on
        a sub-second stylelint pass. Each emits its own result entry.
        """
        run_step("eslint", ["pnpm", "run", "lint"], UI_DIR)
        run_step("stylelint", ["pnpm", "run", "lint:css"], UI_DIR)

    def lane_cargo_fmt():
        run_step("cargo fmt", ["cargo", "fmt", "--check"])

    def lane_task():
        t0 = time.monotonic()
        reporter.step_start("task", "task validation")
        ok, detail = cmd_tasks_validate(quiet=True)
        elapsed = time.monotonic() - t0
        with results_lock:
            results.append(("task validation", 0 if ok else 1, elapsed, detail))
        reporter.step_done("task", "task validation", 0 if ok else 1, elapsed)

    def lane_e2e():
        """E2E API-boundary tests driven through a real running binary.

        Spawns phoenix-ide on an ephemeral port with PHOENIX_ENABLE_MOCK_MODEL=1
        and an isolated temp DB, then runs a battery of scripted conversations
        through the same HTTP/SSE surface phoenix-client.py uses.

        Target-lock sharing with lane_rust. run.py's `cargo build --bin
        phoenix_ide` and lane_rust's cargo invocations share the one workspace
        target dir, whose exclusive lock serializes all builds. The `--bin`
        artifact is NOT a cheap link riding on lane_rust's output: clippy emits
        only check-mode .rmeta, and `cargo test --no-run` emits cfg(test)
        harness binaries under deps/ — neither produces target/debug/phoenix_ide,
        so this build does a full non-test crate codegen + link. Only the
        external dependency crates are reused.

        The overlap that keeps this lane inside lane_rust's wall-clock shadow
        comes from nextest, not artifact reuse: `cargo nextest run` splits
        build from run and releases the target lock for the run phase, so this
        bin build proceeds in parallel with the test run. On the plain
        `cargo test` fallback (no nextest installed) the run phase holds the
        lock end-to-end, so this build serializes after it and adds a full
        bin codegen to the tail of the critical path — the fallback is slower
        by design, not broken. clippy no longer contends here: it runs in
        lane_clippy with its own CARGO_TARGET_DIR, so the only builds on the
        shared workspace lock are lane_rust's test compile and this bin build.
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
        """Run every structural lint rule in a single ast-grep pass.

        One `scan --inline-rules` (rule files joined with `---`) parses each
        source file once and applies every applicable rule to it, instead of
        one scan — and one full re-parse of the target tree — per rule file.
        ast-grep routes rules by language internally, so Rust rules never run
        against ui/src/ files and vice versa, and findings carry their rule
        id, so per-rule attribution survives the batching.
        """
        import shutil
        if not shutil.which("ast-grep"):
            with results_lock:
                results.append(("ast-grep", 0, 0.0, ""))
            reporter.step_skipped("ast-grep", "ast-grep", "not installed")
            return
        rules_dir = ROOT / "ast-grep-rules"
        if not rules_dir.exists():
            return
        rule_files = sorted(rules_dir.glob("*.yml"))
        if not rule_files:
            return
        inline_rules = "\n---\n".join(f.read_text() for f in rule_files)
        run_step("ast-grep", [
            "ast-grep", "scan", "--inline-rules", inline_rules,
            "crates/", "ui/src/",
        ])
        comparison_commit = "HEAD^" if _on_integration_base() else _resolve_check_merge_base()
        if comparison_commit is None:
            timing_command = [
                sys.executable, "-c",
                "import sys; print('cannot resolve comparison commit for Rust test timing lint', file=sys.stderr); sys.exit(1)",
            ]
        else:
            timing_command = [
                "uv", "run", "scripts/check_rust_test_timing.py",
                "--base-sha", comparison_commit, "crates/",
            ]
        run_step("rust-test-timing", timing_command)

    def check_allium():
        """Validate every specs/<name>/<name>.allium parses under v3 grammar.
        `allium analyse` always exits 1 (findings or warnings), so we parse the
        JSON-stream output and fail on error-severity diagnostics and on
        findings not in the accepted baseline (specs/.allium-findings-baseline.json)."""
        import shutil, json
        if not shutil.which("allium"):
            with results_lock:
                results.append(("allium specs", 0, 0.0, ""))
            reporter.step_skipped("allium", "allium specs",
                                  "install via 'cargo install allium-cli'")
            return
        spec_files = sorted((ROOT / "specs").glob("*/*.allium"))
        if not spec_files:
            with results_lock:
                results.append(("allium specs", 0, 0.0, ""))
            reporter.step_skipped("allium", "allium specs", "no .allium files")
            return
        t0 = time.monotonic()
        reporter.step_start("allium", "allium specs")
        try:
            proc = subprocess.run(
                ["allium", "analyse", *[str(p) for p in spec_files]],
                capture_output=True, text=True, timeout=60,
            )
        except subprocess.TimeoutExpired:
            elapsed = time.monotonic() - t0
            with results_lock:
                results.append(("allium specs", 1, elapsed, "allium analyse timed out after 60s"))
            reporter.step_done("allium", "allium specs", 1, elapsed)
            return
        # Parse the concatenated JSON-doc stream that allium-cli emits
        # (one {...} per file passed). Use raw_decode to walk the stream
        # without depending on whitespace conventions.
        decoder = json.JSONDecoder()
        text = proc.stdout
        idx = 0
        failures = []
        all_findings = []          # (spec_file, trigger) tuples
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
            spec_file = doc.get("spec_file", "?")
            try:
                spec_file = str(Path(spec_file).relative_to(ROOT))
            except (ValueError, TypeError):
                pass
            errs = [d for d in doc.get("diagnostics", []) if d.get("severity") == "error"]
            if errs:
                failures.append((doc.get("diagnostics", [{}])[0].get("location", {}).get("file") or "?", errs))
            for f in doc.get("findings", []):
                rules = tuple(sorted(f.get("listening_rules", [])))
                all_findings.append((spec_file, f.get("trigger", "?"), rules))
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
                f"allium analyse produced no parseable diagnostic docs "
                f"(passed {len(spec_files)} files; stdout {len(text)} bytes)"
            )
        elif docs_parsed != len(spec_files):
            gate_problems.append(
                f"allium analyse returned {docs_parsed} diagnostic doc(s) but "
                f"{len(spec_files)} spec(s) were passed -- some specs were not validated"
            )

        # Compare findings against the accepted baseline.  The baseline is a
        # golden-file snapshot of known-accepted findings (cross-spec triggers
        # the per-spec analyser cannot resolve, and intentionally implicit
        # system signals).  Each finding is keyed on (spec, trigger, sorted
        # listening_rules) so a new rule listening on an already-baselined
        # trigger is still caught as a regression.  Fail only on findings NOT
        # in the baseline.
        baseline_path = ROOT / "specs" / ".allium-findings-baseline.json"
        new_findings = []
        if all_findings:
            if baseline_path.exists():
                try:
                    with open(baseline_path) as bf:
                        baseline = json.load(bf)
                    baseline_keys = set()
                    for f in baseline.get("findings", []):
                        rules = tuple(sorted(f.get("listening_rules", [])))
                        baseline_keys.add((f.get("spec"), f.get("trigger"), rules))
                except Exception as e:
                    baseline_keys = set()
                    gate_problems.append(
                        f"could not parse baseline {baseline_path.name} ({e}); "
                        f"all {len(all_findings)} finding(s) treated as new"
                    )
            else:
                baseline_keys = set()
                gate_problems.append(
                    f"baseline file {baseline_path.name} not found; "
                    f"all {len(all_findings)} finding(s) treated as new"
                )
            new_findings = [f for f in all_findings if f not in baseline_keys]

        if gate_problems or failures or new_findings:
            lines = []
            if gate_problems:
                lines.append("allium analyse gate failure:")
                for p in gate_problems:
                    lines.append(f"  {p}")
                if proc.stderr.strip():
                    lines.append(f"stderr (first 500 chars):\n{proc.stderr[:500]}")
                if text.strip() and not failures and not new_findings:
                    lines.append(f"stdout (first 500 chars):\n{text[:500]}")
                if failures or new_findings:
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
            if new_findings:
                lines.append(f"{len(new_findings)} new finding(s) not in baseline:")
                for spec, trigger, rules in new_findings[:10]:
                    try:
                        rel = str(Path(spec).relative_to(ROOT))
                    except (ValueError, TypeError):
                        rel = spec
                    rule_names = ", ".join(rules) if rules else "?"
                    lines.append(f"  {rel}: trigger '{trigger}' (rules: {rule_names})")
                if len(new_findings) > 10:
                    lines.append(f"  … and {len(new_findings) - 10} more")
                lines.append(
                    f"  To accept these, regenerate the baseline: "
                    f"allium analyse specs/*/*.allium 2>/dev/null | jq -s ... "
                    f"(see dev.py check_allium)"
                )
            out = "\n".join(lines)
            with results_lock:
                results.append(("allium specs", 1, elapsed, out))
            reporter.step_done("allium", "allium specs", 1, elapsed)
        else:
            with results_lock:
                results.append(("allium specs", 0, elapsed, ""))
            reporter.step_done("allium", "allium specs", 0, elapsed)

    def check_spec_anchors():
        """REQ-* anchor cross-validator. Fails on orphan code anchors —
        REQ-IDs referenced in source code but not canonically declared
        in any executive.md status row or requirements.md heading.
        Same logic as `./dev.py audit-specs`; the verbose /
        unanchored-half is on the standalone subcommand only (too
        noisy as a build gate)."""
        t0 = time.monotonic()
        reporter.step_start("spec-anchors", "spec anchors")
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
            reporter.step_done("spec-anchors", "spec anchors", 1, elapsed)
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
            reporter.step_done("spec-anchors", "spec anchors", 1, elapsed)
        else:
            with results_lock:
                results.append(("spec anchors", 0, elapsed, ""))
            reporter.step_done("spec-anchors", "spec anchors", 0, elapsed)

    def check_spec_shape():
        """spEARS v2 artifact-shape validator. Fails on malformed shared
        ADR-chain structure, but treats existing specs/*/design.md files
        as legacy inventory rather than current-shape failures."""
        t0 = time.monotonic()
        reporter.step_start("spec-shape", "spec shape")
        try:
            result = _validate_spears_v2_shape(ROOT / "specs")
        except Exception as e:
            elapsed = time.monotonic() - t0
            with results_lock:
                results.append(("spec shape", 1, elapsed, f"scan failed: {e}"))
            reporter.step_done("spec-shape", "spec shape", 1, elapsed)
            return

        elapsed = time.monotonic() - t0
        if result.errors:
            out = "\n".join(["spEARS v2 shape errors:", *[f"  {e}" for e in result.errors]])
            with results_lock:
                results.append(("spec shape", 1, elapsed, out))
            reporter.step_done("spec-shape", "spec shape", 1, elapsed)
        else:
            detail = ""
            if result.legacy_design_docs:
                detail = f"legacy design.md files classified: {len(result.legacy_design_docs)}"
            with results_lock:
                results.append(("spec shape", 0, elapsed, detail))
            reporter.step_done("spec-shape", "spec shape", 0, elapsed)
            run_step("dev.py unit tests", [sys.executable, "-m", "unittest", "discover", "tests/devpy"])

    # Bootstrap UI deps so eslint / tsc / vitest can run on a fresh checkout.
    # Skipped when no UI lane runs, so a non-UI change never needs pnpm.
    if ui_active:
        ensure_ui_deps()

    # Enable sccache (if installed) as the rustc wrapper so deps' object files
    # are shared across worktrees / `cargo clean` cycles. Honored by every
    # cargo invocation below because the env is inherited by run_step's
    # subprocesses. Skip cleanly if sccache isn't on PATH or the user has
    # explicitly set RUSTC_WRAPPER already. Only relevant when a cargo lane runs.
    if cargo_active and shutil.which("sccache") and "RUSTC_WRAPPER" not in os.environ:
        os.environ["RUSTC_WRAPPER"] = "sccache"
        # Default cache dir + 20G cap. Devs can override via SCCACHE_DIR /
        # SCCACHE_CACHE_SIZE before invoking dev.py.
        os.environ.setdefault("SCCACHE_CACHE_SIZE", "20G")

    # Environment classification and Git subprocess safety only matter when a
    # cargo lane runs.
    if cargo_active:
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

        # Tests create temporary commits and must never consult an interactive
        # signing agent. Add a process-only override without probing the signer
        # or modifying any Git config file. Append so caller-provided config
        # entries remain intact while this final entry takes precedence.
        _append_git_config_override("commit.gpgsign", "false")

    # nextest probe, thread sizing, and codegen command shapes are rust-only.
    if "rust" in active:
        # Probe nextest and size the test-thread cap here in cmd_check scope
        # (not in lane_rust) so the closed-over commands are built once.
        # Neither probe touches the cargo target lock (`cargo nextest
        # --version` only prints a version; the /proc/meminfo read is pure
        # Python).
        #
        # This probe runs on the main path before any lane (and its
        # LANE_JOIN_TIMEOUT) exists, so a rustup/network/cargo-home stall here
        # would hang the whole check with no timeout or summary. Bound it and
        # treat any stall/error as "no nextest" — the plain `cargo test` fallback
        # is always correct, just slower.
        try:
            has_nextest = subprocess.run(
                ["cargo", "nextest", "--version"],
                capture_output=True, timeout=30,
            ).returncode == 0
        except (subprocess.TimeoutExpired, OSError):
            has_nextest = False
            reporter.info("cargo nextest probe stalled/failed — using plain `cargo test`")
        # nextest defaults to available_parallelism (= num_cpus). On low-RAM
        # boxes, num_cpus parallel test threads can swap and stall sensitive
        # tests (e.g. browser tests where Chrome's CDP WebSocket handshake
        # times out if Chrome can't get CPU+RAM during launch). Cap the
        # thread count by ~1.5 GiB headroom per thread so resource-starved
        # machines back off, while leaving fast machines effectively unchanged.
        cpus = os.cpu_count() or 4
        try:
            # SC_PHYS_PAGES * SC_PAGE_SIZE is the POSIX-portable total-RAM
            # query (verified to match `sysctl hw.memsize` on macOS; the
            # same call also works on Linux). os.sysconf doesn't exist on
            # Windows at all, hence AttributeError alongside the value/OS
            # errors a missing sysconf name can raise.
            mem_gib = (
                os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
            ) / (1024**3)
            mem_cap = max(1, int(mem_gib // 1.5))
        except (ValueError, OSError, AttributeError):
            mem_cap = cpus
        test_threads = max(2, min(cpus - 1, mem_cap))
        if test_threads < cpus:
            try:
                load1, load5, load15 = os.getloadavg()
                load_note = f", loadavg={load1:.1f}/{load5:.1f}/{load15:.1f}"
            except OSError:
                load_note = ""
            reporter.info(f"cargo test: capping to {test_threads} threads "
                          f"(cpus={cpus}, mem_cap={mem_cap}{load_note})")

        # ts-rs emits a `#[test] export_bindings_*` per `#[ts(export)]` type;
        # those tests' side effect IS the codegen. In check they run with
        # TS_RS_EXPORT_DIR pointed at a scratch dir under target/, so the
        # committed ui/src/generated/ tree is never written during a check —
        # the lanes that read it (tsc, vitest, eslint, ast-grep) have no
        # ordering dependency on the rust lane. The staleness step then diffs
        # the scratch export against the committed tree. (In-place
        # regeneration is `./dev.py codegen`.)
        #
        # Discovery is preserved: every type still self-registers via its emitted
        # test, so a new #[ts(export)] type is regenerated automatically with no
        # hand-maintained root list to forget.
        #
        # Codegen must produce the COMPLETE generated tree (the staleness
        # guard diffs it against every committed file), and the crates that
        # can contribute to it are exactly the ts-rs dependents (see
        # _ts_export_crates). When the lane is crate-scoped, the codegen run
        # narrows to those crates and the harness compile to scope ∪ export
        # crates — the identical scratch tree, without building test
        # harnesses whose tests won't run. An undeterminable export set
        # falls back to full workspace: narrowing must never guess.
        codegen_scope = _ts_export_crates() if rust_scope else None
        if rust_scope and codegen_scope is not None:
            compile_p = [f for c in sorted(set(rust_scope) | codegen_scope)
                         for f in ("-p", c)]
            codegen_p = [f for c in sorted(codegen_scope) for f in ("-p", c)]
        else:
            compile_p, codegen_p = [], []
        if has_nextest:
            compile_cmd = ["cargo", "nextest", "run", *compile_p, "--no-run"]
            test_cmd = ["cargo", "nextest", "run", *_pflags(),
                        "-E", "not test(/export_bindings/)",
                        "--test-threads", str(test_threads)]
            codegen_cmd = ["cargo", "nextest", "run", *codegen_p,
                           "-E", "test(/export_bindings/)"]
        else:
            compile_cmd = ["cargo", "test", *compile_p, "--no-run"]
            test_cmd = ["cargo", "test", *_pflags(), "--",
                        "--test-threads", str(test_threads), "--skip", "export_bindings"]
            codegen_cmd = ["cargo", "test", *codegen_p, "export_bindings"]

        # Scratch base for the ts-rs export. Every `#[ts(export_to)]` in the
        # workspace uses a `../../../ui/src/generated/` path relative to the
        # export base, so three sacrificial levels keep the resolved output
        # inside the scratch dir: <scratch>/x/x/x/../../../ -> <scratch>/.
        codegen_scratch = ROOT / "target" / "codegen-scratch"
        if codegen_scratch.exists():
            shutil.rmtree(codegen_scratch)
        codegen_export_base = codegen_scratch / "x" / "x" / "x"
        codegen_export_base.mkdir(parents=True)

        def codegen_stale_step():
            """Diff the scratch ts-rs export against committed ui/src/generated/.

            A mismatch means the developer's Rust types and the committed TS
            don't line up. Recorded (not aborted) so the rest of the lane and
            the parallel phase still run and every failure lands in one pass.
            """
            step, t0 = "codegen-stale", time.monotonic()
            reporter.step_start("rust", step)
            # Only ts-rs-generated files participate: the directory also
            # holds the hand-authored sse.ts barrel, which ts-rs never
            # exports. Generated files are identified by the header ts-rs
            # writes as their first line.
            def _is_generated(p: Path) -> bool:
                with open(p, encoding="utf-8") as f:
                    return f.readline().startswith("// This file was generated by")
            committed = {p.name: p
                         for p in (ROOT / "ui" / "src" / "generated").glob("*.ts")
                         if _is_generated(p)}
            exported = {p.name: p for p in codegen_scratch.rglob("*.ts")}
            problems = []
            if not exported:
                problems.append(
                    f"no .ts files found under {codegen_scratch} — "
                    "TS_RS_EXPORT_DIR was not honored by the export tests?"
                )
            problems += [f"not committed: ui/src/generated/{n}"
                         for n in sorted(exported.keys() - committed.keys())]
            problems += [f"committed but no longer exported: ui/src/generated/{n}"
                         for n in sorted(committed.keys() - exported.keys())]
            problems += [f"stale: ui/src/generated/{n}"
                         for n in sorted(exported.keys() & committed.keys())
                         if exported[n].read_bytes() != committed[n].read_bytes()]
            rc, detail = (1, (
                "ui/src/generated/ does not match a fresh ts-rs export:\n"
                + "\n".join(f"  {p}" for p in problems)
                + "\n\nRun './dev.py codegen' and commit the result."
            )) if problems else (0, "")
            elapsed = time.monotonic() - t0
            with results_lock:
                results.append((step, rc, elapsed, detail))
            reporter.step_done("rust", step, rc, elapsed)

    reporter.banner("\nRunning checks in parallel...\n")

    def _lane(fn):
        """Wrap a lane body with reporter lifecycle events."""
        def wrapped():
            lane = threading.current_thread().name
            reporter.lane_start(lane)
            try:
                fn()
            finally:
                reporter.lane_done(lane)
        return wrapped

    # Threads carry the lane name so a hung-thread report names the lane
    # (and so run_step can attribute steps to lanes for the reporter).
    _lane_targets = [
        ("rust", lane_rust),
        ("clippy", lane_clippy),
        # tsc: use the `typecheck` script so contributors running
        # `pnpm typecheck` locally exercise the same `tsc -b --noEmit`
        # invocation this lane uses. Project references +
        # exactOptionalPropertyTypes only fire under `-b`; bare
        # `pnpm exec tsc --noEmit` silently misses them.
        ("tsc", lambda: run_step("tsc typecheck", ["pnpm", "run", "typecheck"], UI_DIR)),
        ("ui-lint", lane_ui_lint),
        ("vitest", lambda: run_step("vitest", ["pnpm", "exec", "vitest", "run"], UI_DIR)),
        ("cargo-fmt", lane_cargo_fmt),
        ("task", lane_task),
        ("ast-grep", check_ast_grep),
        ("allium", check_allium),
        ("spec-shape", check_spec_shape),
        ("spec-anchors", check_spec_anchors),
        ("pkglock", check_package_lock_clean),
        ("e2e", lane_e2e),
    ]
    threads = [threading.Thread(target=_lane(fn), name=name)
               for name, fn in _lane_targets if name in active]
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
    # Stop the live renderer before any direct printing below (hung-lane
    # lines, failure dumps, summary) — plain mode's close() is a no-op.
    reporter.close()
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


@dataclasses.dataclass
class SpearsV2ShapeResult:
    errors: list[str]
    legacy_design_docs: list[str]


def _validate_spears_v2_shape(specs_root: Path) -> SpearsV2ShapeResult:
    """Validate repository-level spEARS v2 artifact shape.

    This is deliberately narrow: it enforces the shared ADR-chain contract
    and records legacy design docs as inventory. It does not require feature
    specs to have design.md, and it does not fail the existing legacy corpus
    just because design.md is present.
    """
    errors: list[str] = []
    legacy_design_docs: list[str] = []

    def rel(path: Path) -> str:
        try:
            return str(path.relative_to(ROOT))
        except ValueError:
            return str(path)

    if not specs_root.is_dir():
        return SpearsV2ShapeResult(errors=[f"{specs_root}: specs directory not found"], legacy_design_docs=[])

    for design in sorted(specs_root.glob("*/design.md")):
        legacy_design_docs.append(rel(design))

    adrs = specs_root / "adrs"
    if not adrs.is_dir():
        errors.append("specs/adrs/: shared ADR chain directory is required")
        return SpearsV2ShapeResult(errors=errors, legacy_design_docs=legacy_design_docs)

    template = adrs / "_TEMPLATE.md"
    readme = adrs / "README.md"
    if not template.is_file():
        errors.append("specs/adrs/_TEMPLATE.md is required")
    if not readme.is_file():
        errors.append("specs/adrs/README.md is required")

    adr_re = re.compile(r"^(\d{3})_[a-z0-9][a-z0-9-]*\.md$")
    numbered: list[tuple[int, Path]] = []
    for path in sorted(adrs.glob("*.md")):
        if path.name in {"README.md", "_TEMPLATE.md"}:
            continue
        m = adr_re.match(path.name)
        if not m:
            errors.append(f"{rel(path)}: ADR filename must be NNN_<slug>.md")
            continue
        numbered.append((int(m.group(1)), path))

    if numbered:
        nums = [n for n, _ in numbered]
        expected = list(range(0, max(nums) + 1))
        if nums != expected:
            errors.append(
                "specs/adrs/: ADR numbers must be sequential without gaps "
                f"(found {', '.join(f'{n:03d}' for n in nums)})"
            )
    else:
        errors.append("specs/adrs/: at least one numbered ADR is required")

    for _num, path in numbered:
        try:
            text = path.read_text()
        except (UnicodeDecodeError, OSError) as e:
            errors.append(f"{rel(path)}: cannot read ADR: {e}")
            continue
        if not re.search(r"^- \*\*Status:\*\*\s+\S+", text, re.MULTILINE):
            errors.append(f"{rel(path)}: missing '- **Status:** ...'")
        if not re.search(r"^- \*\*Date:\*\*\s+\d{4}-\d{2}-\d{2}", text, re.MULTILINE):
            errors.append(f"{rel(path)}: missing '- **Date:** YYYY-MM-DD'")
        if not re.search(r"^- \*\*Affects:\*\*\s+\S+", text, re.MULTILINE):
            errors.append(f"{rel(path)}: missing '- **Affects:** ...'")
        for heading in ("## Context", "## Options considered", "## Decision", "## Consequences"):
            if heading not in text:
                errors.append(f"{rel(path)}: missing '{heading}'")

    if readme.is_file():
        try:
            readme_text = readme.read_text()
        except (UnicodeDecodeError, OSError) as e:
            errors.append(f"specs/adrs/README.md: cannot read ADR index: {e}")
        else:
            indexed = set(re.findall(r"\[(\d{3})\]\((\d{3}_[^)]+\.md)\)", readme_text))
            for num, path in numbered:
                key = (f"{num:03d}", path.name)
                if key not in indexed:
                    errors.append(f"specs/adrs/README.md: missing index row/link for {path.name}")
            numbered_keys = {(f"{num:03d}", path.name) for num, path in numbered}
            for num, filename in sorted(indexed - numbered_keys):
                errors.append(
                    "specs/adrs/README.md: index links to missing ADR file "
                    f"{filename} ({num})"
                )

    return SpearsV2ShapeResult(errors=errors, legacy_design_docs=legacy_design_docs)


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


def cmd_drive_turn(drive_turn_args: list[str]) -> None:
    """Build and exec drive-turn with the normal Phoenix development env."""
    env = _resolved_phoenix_env()
    build = subprocess.run(
        ["cargo", "build", "-p", "phoenix-drive-turn"],
        cwd=ROOT,
        env=env,
    )
    if build.returncode != 0:
        sys.exit(build.returncode)

    metadata = _workspace_metadata(env=env)
    if metadata is None:
        die("cargo metadata failed after building drive-turn")
    binary = Path(metadata["target_directory"]) / "debug" / "drive-turn"
    os.execve(binary, [str(binary), *drive_turn_args], env)


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
# Build Graph (./dev.py graph)
# =============================================================================
# Renders the project's complete build dependency graph as a self-contained
# HTML page laid out in topological layers (sources at the top, the final
# artifact at the bottom). Three real, data-derived subgraphs:
#
#   1. Cargo workspace crate DAG     — live from `cargo metadata`
#   2. cross-language build pipeline — toolchain → ui-deps / ts-rs codegen →
#                                      vite bundle → RustEmbed release binary
#   3. `./dev.py check` lane graph   — input categories → lanes (from _LANE_DEFS)
#
# Layer index is the longest path from a source, so a node sits below every
# input it depends on. Output is dependency-free (inline SVG + CSS + JS), so the
# snapshot opens anywhere and the --serve mode needs no assets.

_GRAPH_KIND_COLORS = {
    # kind     -> (stroke, fill)
    "tool":     ("#475569", "#e2e8f0"),
    "crate":    ("#2563eb", "#dbeafe"),
    "codegen":  ("#7c3aed", "#ede9fe"),
    "ui":       ("#d97706", "#fef3c7"),
    "artifact": ("#059669", "#d1fae5"),
    "category": ("#0d9488", "#ccfbf1"),
    "lane":     ("#4f46e5", "#e0e7ff"),
    "always":   ("#6b7280", "#f3f4f6"),
}
_GRAPH_KIND_LABELS = {
    "tool": "toolchain",
    "crate": "workspace crate",
    "codegen": "codegen",
    "ui": "UI build",
    "artifact": "release artifact",
    "category": "check trigger (changed path)",
    "lane": "check lane",
    "always": "always-on lane",
}


def _human_loc(n: int) -> str:
    return f"{n / 1000:.1f}k" if n >= 1000 else str(n)


def _count_rust_source(dirpath: "Path") -> "tuple[int, int]":
    """(lines, files) across *.rs under dirpath, skipping any target/ subtree."""
    loc = files = 0
    for root, dirs, names in os.walk(dirpath):
        dirs[:] = [d for d in dirs if d != "target"]
        for nm in names:
            if nm.endswith(".rs"):
                files += 1
                try:
                    with open(Path(root) / nm, encoding="utf-8", errors="ignore") as fh:
                        loc += sum(1 for _ in fh)
                except OSError:
                    pass
    return loc, files


def _parse_cargo_timings() -> "dict | None":
    """Parse the newest target/cargo-timings/*.html into {crate_name: seconds}.

    Sums per-package compile durations (every unit except run-custom-build,
    which is build-script *execution*, not compilation). Names are normalized
    to underscore form so callers can match either spelling. Returns None when
    no report is present or it can't be parsed.
    """
    tdir = ROOT / "target" / "cargo-timings"
    if not tdir.is_dir():
        return None
    htmls = sorted(tdir.glob("cargo-timing*.html"), key=lambda p: p.stat().st_mtime)
    if not htmls:
        return None
    m = re.search(
        r"UNIT_DATA\s*=\s*(\[.*?\]);",
        htmls[-1].read_text(encoding="utf-8", errors="ignore"), re.S,
    )
    if not m:
        return None
    try:
        units = json.loads(m.group(1))
    except json.JSONDecodeError:
        return None
    out: dict = {}
    for u in units:
        if u.get("mode") == "run-custom-build":
            continue
        name = str(u.get("name", "")).replace("-", "_")
        dur = u.get("duration")
        if name and isinstance(dur, (int, float)):
            out[name] = out.get(name, 0.0) + float(dur)
    return out or None


def _collect_crate_timings(crates: "list[str]") -> "dict | None":
    """Force a from-scratch recompile of the workspace crates under --timings,
    then read back per-crate compile seconds. Dependency artifacts are kept
    (only `-p <crate>` is cleaned) so the rebuild is workspace-only, but a cold
    target still pays the full external-dependency compile first."""
    clean = ["cargo", "clean"]
    for c in crates:
        clean += ["-p", c]
    subprocess.run(clean, cwd=ROOT, capture_output=True)
    # stderr: keeps a `graph --format json --timings` stdout pipe pure JSON.
    print("  building workspace with --timings (recompiles every crate)…",
          file=sys.stderr)
    build = subprocess.run(
        ["cargo", "build", "--timings"], cwd=ROOT, capture_output=True, text=True,
    )
    if build.returncode != 0:
        sys.stderr.write(build.stderr[-2000:])
        print("  ⚠ cargo build --timings failed; compile times omitted",
              file=sys.stderr)
        return None
    return _parse_cargo_timings()


def _crate_graph_info(with_timings: bool = False):
    """One `cargo metadata` pass → workspace crate graph plus per-crate metrics.

    Returns (crates_sorted, edges, info) or None if cargo metadata is
    unavailable. `edges` are (dep, dependent) tuples in build order. `info[name]`
    carries: dir, kind, ws_deps, ext_deps, rebuilds (crates recompiled on a
    change, incl. self), loc, files, and compile_s (None unless with_timings).

    Dependency edges count normal + build deps only (compile-relevant); dev-deps
    are excluded so the build DAG reflects what a release build links, not what
    the test harness pulls in.
    """
    try:
        out = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=ROOT, capture_output=True, text=True, timeout=60,
        )
        if out.returncode != 0:
            return None
        meta = json.loads(out.stdout)
    except (subprocess.TimeoutExpired, OSError, json.JSONDecodeError):
        return None

    ws = {p["name"] for p in meta["packages"]}
    edges: set = set()
    info: dict = {}
    for p in meta["packages"]:
        name = p["name"]
        compile_deps = [
            d for d in p["dependencies"] if d.get("kind") in (None, "build")
        ]
        ws_deps = {d["name"] for d in compile_deps if d["name"] in ws and d["name"] != name}
        ext_deps = {d["name"] for d in compile_deps if d["name"] not in ws}
        for dep in ws_deps:
            edges.add((dep, name))
        target_kinds = {k for t in p["targets"] for k in t["kind"]
                        if k in ("lib", "bin", "proc-macro")}
        kind = "+".join(sorted(target_kinds)) or "lib"
        try:
            rel = str(Path(p["manifest_path"]).parent.relative_to(ROOT))
        except ValueError:
            rel = ""
        loc, files = _count_rust_source(ROOT / rel) if rel else (0, 0)
        info[name] = {
            "dir": rel, "kind": kind,
            "ws_deps": len(ws_deps), "ext_deps": len(ext_deps),
            "loc": loc, "files": files, "rebuilds": 1, "compile_s": None,
        }

    # Blast radius: crates whose build transitively depends on `c` (incl. self).
    succ: dict = {c: set() for c in ws}
    for dep, dependent in edges:
        succ[dep].add(dependent)
    for c in ws:
        seen = {c}
        stack = [c]
        while stack:
            for nxt in succ[stack.pop()]:
                if nxt not in seen:
                    seen.add(nxt)
                    stack.append(nxt)
        info[c]["rebuilds"] = len(seen)

    if with_timings:
        timings = _collect_crate_timings(sorted(ws))
        if timings:
            for name in ws:
                info[name]["compile_s"] = timings.get(name.replace("-", "_"))

    return sorted(ws), sorted(edges), info


def _build_pipeline_model(with_timings: bool = False):
    """Nodes + edges for the build-artifact graph (toolchain, crate DAG,
    codegen, UI bundle, embedded release binary). Returns (nodes, edges, warn).

    Crate nodes carry per-crate metrics (LoC, deps, blast radius, optional
    compile time) in their tooltip, and a `meter` (0..1) encoding relative
    weight — compile time when --timings ran, else lines of code."""
    nodes: dict = {}
    edges: list = []

    def add(nid, label, kind, sub="", tip="", **extra):
        nodes[nid] = {"id": nid, "label": label, "kind": kind, "sub": sub,
                      "tip": tip, **extra}

    add("corepack", "corepack + pnpm", "tool", sub="toolchain pin",
        tip="ensure_corepack_pnpm(): pins pnpm via Corepack from "
            "ui/package.json#packageManager")
    add("ui-deps", "pnpm install", "ui", sub="ui/node_modules/",
        tip="ensure_ui_deps(): pnpm install. Success marker: "
            "node_modules/.modules.yaml")

    cg = _crate_graph_info(with_timings)
    warn = None
    top_crate = None
    weight_basis = "lines of code"
    if cg:
        crates, cedges, info = cg
        has_timings = any(info[c]["compile_s"] is not None for c in crates)
        weight_basis = "compile time" if has_timings else "lines of code"

        def weight(c):
            cs = info[c]["compile_s"]
            return cs if (has_timings and cs is not None) else info[c]["loc"]

        maxw = max((weight(c) for c in crates), default=1) or 1
        for c in crates:
            i = info[c]
            loc_h = _human_loc(i["loc"])
            if has_timings and i["compile_s"] is not None:
                sub = f"{i['compile_s']:.1f}s · {loc_h} loc"
            else:
                sub = f"{loc_h} loc · {i['ext_deps']} deps"
            tip_lines = [
                f"{c}  ({i['kind']})",
                f"{i['loc']:,} LoC across {i['files']} files",
                f"deps: {i['ws_deps']} workspace · {i['ext_deps']} external",
                f"rebuilds {i['rebuilds']} crate(s) on change",
            ]
            if i["compile_s"] is not None:
                tip_lines.append(f"compile: {i['compile_s']:.1f}s (debug, full rebuild)")
            add(f"crate:{c}", c, "crate", sub=sub, tip="\n".join(tip_lines),
                meter=weight(c) / maxw,
                metrics={
                    "kind": i["kind"], "dir": i["dir"],
                    "loc": i["loc"], "files": i["files"],
                    "ws_deps": i["ws_deps"], "ext_deps": i["ext_deps"],
                    "rebuilds": i["rebuilds"], "compile_s": i["compile_s"],
                })
        for dep, dependent in cedges:
            edges.append((f"crate:{dep}", f"crate:{dependent}"))
        # The root binary crate — nothing depends on it; it transitively pulls
        # the whole workspace, so codegen/release hang off it.
        for cand in ("phoenix_ide", "phoenix-ide"):
            if cand in crates:
                top_crate = f"crate:{cand}"
                break
        if top_crate is None:
            top_crate = f"crate:{max(crates, key=lambda c: info[c]['rebuilds'])}"
    else:
        warn = "cargo metadata unavailable — workspace crate DAG omitted"

    add("codegen", "ts-rs codegen", "codegen", sub="ui/src/generated/",
        tip="cmd_codegen(): cargo test export_bindings — exports TS types from "
            "Rust ts-rs derives into ui/src/generated/")
    add("ui-build", "vite build", "ui", sub="ui/dist/",
        tip="pnpm run build → ui/dist/. Consumes node_modules + generated TS")
    add("release-bin", "cargo build --release", "artifact", sub="phoenix_ide binary",
        tip="RustEmbed embeds ui/dist/ into the static binary "
            "(crates/phoenix-ide/src/api/assets.rs)")

    edges.append(("corepack", "ui-deps"))
    if top_crate:
        edges.append((top_crate, "codegen"))
        edges.append((top_crate, "release-bin"))
    edges.append(("codegen", "ui-build"))
    edges.append(("ui-deps", "ui-build"))
    edges.append(("ui-build", "release-bin"))
    return nodes, edges, warn, weight_basis


# Per-lane step lists mirror the lane_* / check_* functions in cmd_check; shown
# in node tooltips. Kept beside _LANE_DEFS conceptually — update both together.
_GRAPH_LANE_STEPS = {
    "rust": ["cargo test (compile)", "codegen export", "codegen-stale diff",
             "cargo test (run)", "cargo check musl (macOS, optional)"],
    "clippy": ["cargo clippy --all-targets -D warnings"],
    "tsc": ["tsc -b --noEmit"],
    "ui-lint": ["eslint", "stylelint"],
    "vitest": ["vitest run"],
    "ast-grep": ["structural rules over ui/src/ + crates/"],
    "allium": ["allium analyse specs/*/*.allium"],
    "spec-shape": ["spEARS v2 artifact-shape validation", "python unittest discover tests/devpy"],
    "spec-anchors": ["REQ-* anchor cross-validation"],
    "e2e": ["uv run tests/e2e/run.py (real binary)"],
    "cargo-fmt": ["cargo fmt --check"],
    "task": ["task validation"],
    "pkglock": ["pnpm-lock drift tripwire"],
}
_GRAPH_CATEGORY_LABELS = {
    "RUST": "RUST · crates/ Cargo.*",
    "UI": "UI · ui/",
    "TASKS": "TASKS · tasks/",
    "SPECS": "SPECS · specs/",
    "ASTGREP": "ASTGREP · ast-grep-rules/",
    "E2E": "E2E · tests/e2e/",
    "SELF": "SELF · dev.py/.github workflows",
}



def _check_graph_model():
    """Nodes + edges for the `./dev.py check` gating graph: changed-path input
    categories (plus an always-on trigger) → lanes. Sourced from _LANE_DEFS."""
    nodes: dict = {}
    edges: list = []
    nodes["__always__"] = {
        "id": "__always__", "label": "always-on", "kind": "always",
        "sub": "every check run",
        "tip": "Sub-second, correctness-sensitive lanes that run on every "
               "invocation regardless of which paths changed.",
    }
    # SELF (dev.py) is not a _LANE_DEFS input: _gate_lanes short-circuits a
    # dev.py change to run *every* lane (the gating logic itself may have
    # changed), so it fans out to all lanes rather than a subset.
    nodes["cat:SELF"] = {
        "id": "cat:SELF", "label": _GRAPH_CATEGORY_LABELS["SELF"],
        "kind": "category", "sub": "runs all lanes", "category": "SELF",
        "tip": "Editing dev.py changes the gating logic itself, so every lane "
               "runs regardless of what else changed.",
    }
    cats: set = set()
    for _name, inputs, _desc in _LANE_DEFS:
        if inputs:
            cats |= inputs
    for c in sorted(cats):
        nodes[f"cat:{c}"] = {
            "id": f"cat:{c}", "label": _GRAPH_CATEGORY_LABELS.get(c, c),
            "kind": "category", "sub": "changed-path trigger", "category": c,
            "tip": f"Touching {c} paths activates the dependent lanes "
                   "(incremental gating).",
        }
    for name, inputs, desc in _LANE_DEFS:
        steps = _GRAPH_LANE_STEPS.get(name, [])
        tip = desc + ("\n• " + "\n• ".join(steps) if steps else "")
        nodes[f"lane:{name}"] = {
            "id": f"lane:{name}", "label": name, "kind": "lane",
            "sub": (desc[:40] + "…") if len(desc) > 41 else desc, "tip": tip,
            "desc": desc, "steps": steps,
            "inputs": sorted(inputs) if inputs else [],
            "always_on": not inputs,
        }
        if inputs:
            for c in sorted(inputs):
                edges.append((f"cat:{c}", f"lane:{name}"))
        else:
            edges.append(("__always__", f"lane:{name}"))
        # A dev.py change runs every lane, gated or always-on alike.
        edges.append(("cat:SELF", f"lane:{name}"))
    return nodes, edges


def _topo_layout(nodes, edges):
    """Assign each node a topological layer (longest path from a source) and an
    intra-layer order chosen by a barycenter heuristic to reduce edge crossings.

    Returns (layer: dict[id->int], rows: dict[int->list[id]], depth: int).
    A cycle (not expected for a build DAG) drops its residual nodes onto a
    trailing layer rather than failing, so rendering always succeeds.
    """
    from collections import deque

    ids = list(nodes)
    succ = {i: [] for i in ids}
    indeg = {i: 0 for i in ids}
    for a, b in edges:
        if a in succ and b in succ:
            succ[a].append(b)
            indeg[b] += 1

    layer = {i: 0 for i in ids}
    residual = dict(indeg)
    q = deque(i for i in ids if indeg[i] == 0)
    seen = 0
    while q:
        n = q.popleft()
        seen += 1
        for m in succ[n]:
            if layer[n] + 1 > layer[m]:
                layer[m] = layer[n] + 1
            residual[m] -= 1
            if residual[m] == 0:
                q.append(m)
    if seen != len(ids):  # cycle fallback
        trailing = max(layer.values(), default=0) + 1
        for i in ids:
            if residual[i] > 0:
                layer[i] = trailing

    depth = max(layer.values(), default=0) + 1
    rows = {k: [] for k in range(depth)}
    for i in ids:
        rows[layer[i]].append(i)

    preds = {i: [] for i in ids}
    for a, b in edges:
        if a in succ and b in succ:
            preds[b].append(a)

    for sweep in range(6):
        downward = sweep % 2 == 0
        klist = range(1, depth) if downward else range(depth - 2, -1, -1)
        for k in klist:
            adj = preds if downward else succ
            ref = rows[k - 1] if downward else rows[k + 1]
            ref_pos = {n: idx for idx, n in enumerate(ref)}
            cur_pos = {n: idx for idx, n in enumerate(rows[k])}

            def bary(n, _adj=adj, _ref=ref_pos, _cur=cur_pos):
                ns = [_ref[p] for p in _adj[n] if p in _ref]
                return (sum(ns) / len(ns)) if ns else _cur[n]

            rows[k].sort(key=bary)
    return layer, rows, depth


def _svg_for_graph(nodes, edges):
    """Render one graph model to an inline SVG string (topologically layered,
    top→bottom; edges point from a dependency down to its dependent)."""
    import html

    _layer, rows, depth = _topo_layout(nodes, edges)
    NODE_H, V_GAP, H_GAP, TOP, SIDE, CHAR = 52, 64, 30, 30, 44, 7.3

    width = {}
    for nid, n in nodes.items():
        chars = max(len(n["label"]), len(n.get("sub", "")))
        width[nid] = max(150, int(chars * CHAR) + 30)

    row_w = {}
    for k in range(depth):
        ns = rows[k]
        row_w[k] = sum(width[n] for n in ns) + H_GAP * max(0, len(ns) - 1)
    canvas_w = max(row_w.values(), default=200) + 2 * SIDE
    canvas_h = TOP * 2 + depth * NODE_H + max(0, depth - 1) * V_GAP

    x, y = {}, {}
    for k in range(depth):
        cx = (canvas_w - row_w[k]) / 2
        for n in rows[k]:
            x[n] = cx
            y[n] = TOP + k * (NODE_H + V_GAP)
            cx += width[n] + H_GAP

    def eid(s):
        return re.sub(r"[^A-Za-z0-9_-]", "-", s)

    out = [
        f'<svg viewBox="0 0 {int(canvas_w)} {int(canvas_h)}" '
        f'width="{int(canvas_w)}" height="{int(canvas_h)}" class="graph">',
        '<defs><marker id="arr" markerWidth="9" markerHeight="9" refX="7.5" '
        'refY="3" orient="auto"><path d="M0,0 L7,3 L0,6 Z" fill="#64748b"/>'
        "</marker></defs>",
    ]
    for a, b in edges:
        if a not in x or b not in x:
            continue
        sx, sy = x[a] + width[a] / 2, y[a] + NODE_H
        tx, ty = x[b] + width[b] / 2, y[b]
        my = (sy + ty) / 2
        out.append(
            f'<path class="edge" data-a="{eid(a)}" data-b="{eid(b)}" '
            f'd="M {sx:.1f} {sy:.1f} C {sx:.1f} {my:.1f} {tx:.1f} {my:.1f} '
            f'{tx:.1f} {ty:.1f}" marker-end="url(#arr)"/>'
        )
    for nid, n in nodes.items():
        if nid not in x:
            continue
        stroke, fill = _GRAPH_KIND_COLORS.get(n["kind"], ("#334155", "#e2e8f0"))
        w, gx, gy = width[nid], x[nid], y[nid]
        sub = n.get("sub", "")
        meter = n.get("meter")
        lbl_y = gy + 19 if sub else gy + 30
        out.append(f'<g class="node" data-id="{eid(nid)}">')
        out.append(f"<title>{html.escape(n.get('tip') or n['label'])}</title>")
        out.append(
            f'<rect x="{gx:.1f}" y="{gy:.1f}" rx="8" width="{w}" '
            f'height="{NODE_H}" fill="{fill}" stroke="{stroke}"/>'
        )
        out.append(
            f'<text class="lbl" x="{gx + w / 2:.1f}" y="{lbl_y:.1f}" '
            f'fill="{stroke}">{html.escape(n["label"])}</text>'
        )
        if sub:
            out.append(
                f'<text class="sub" x="{gx + w / 2:.1f}" y="{gy + 34:.1f}">'
                f"{html.escape(sub)}</text>"
            )
        if meter is not None:
            mx, mw, my = gx + 12, w - 24, gy + NODE_H - 9
            fillw = max(2.0, mw * max(0.0, min(1.0, meter)))
            out.append(
                f'<rect class="meter-bg" x="{mx:.1f}" y="{my:.1f}" width="{mw:.1f}" '
                f'height="3.5" rx="1.75" fill="#0b1220" opacity="0.35"/>'
            )
            out.append(
                f'<rect class="meter" x="{mx:.1f}" y="{my:.1f}" width="{fillw:.1f}" '
                f'height="3.5" rx="1.75" fill="{stroke}"/>'
            )
        out.append("</g>")
    out.append("</svg>")
    return "".join(out)


_GRAPH_CSS = """
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body { font-family: -apple-system, "Segoe UI", Roboto, sans-serif; margin: 0;
       background: #0b1220; color: #e2e8f0; }
header { padding: 18px 24px; border-bottom: 1px solid #1e293b; }
h1 { margin: 0; font-size: 18px; }
.meta { color: #94a3b8; font-size: 12px; margin-top: 5px; }
section { padding: 16px 24px 28px; }
h2 { font-size: 15px; color: #cbd5e1; border-left: 3px solid #38bdf8;
     padding-left: 9px; margin: 4px 0 6px; }
.hint { color: #94a3b8; font-size: 12px; margin: 0 0 10px; }
.warn { color: #fbbf24; font-size: 12px; margin: 0 0 10px; }
.scroll { overflow: auto; border: 1px solid #1e293b; border-radius: 10px;
          background: #0f172a; padding: 8px; }
svg.graph { display: block; }
text.lbl { font-size: 12.5px; font-weight: 600; text-anchor: middle; }
text.sub { font-size: 10px; fill: #64748b; text-anchor: middle; }
.node rect { transition: opacity .12s; }
.node { cursor: default; }
path.edge { fill: none; stroke: #475569; stroke-width: 1.4;
            transition: stroke .12s, opacity .12s; }
.dim { opacity: .12; }
.hot { stroke: #38bdf8 !important; stroke-width: 2.3 !important; opacity: 1 !important; }
.legend { display: flex; gap: 16px; flex-wrap: wrap; margin: 4px 0 12px;
          font-size: 12px; color: #94a3b8; }
.legend span { display: inline-flex; align-items: center; gap: 6px; }
.legend i { width: 13px; height: 13px; border-radius: 3px; display: inline-block;
            border: 1px solid #0b1220; }
"""

_GRAPH_JS = """
document.querySelectorAll('svg.graph').forEach(function (svg) {
  var nodes = Array.prototype.slice.call(svg.querySelectorAll('.node'));
  var edges = Array.prototype.slice.call(svg.querySelectorAll('.edge'));
  function clear() {
    nodes.forEach(function (n) { n.classList.remove('dim'); });
    edges.forEach(function (e) { e.classList.remove('dim', 'hot'); });
  }
  nodes.forEach(function (n) {
    n.addEventListener('mouseenter', function () {
      var id = n.dataset.id;
      var inc = {};
      inc[id] = true;
      edges.forEach(function (e) {
        if (e.dataset.a === id || e.dataset.b === id) {
          inc[e.dataset.a] = true; inc[e.dataset.b] = true;
        }
      });
      nodes.forEach(function (m) { m.classList.toggle('dim', !inc[m.dataset.id]); });
      edges.forEach(function (e) {
        var hit = e.dataset.a === id || e.dataset.b === id;
        e.classList.toggle('hot', hit);
        e.classList.toggle('dim', !hit);
      });
    });
    n.addEventListener('mouseleave', clear);
  });
});
"""


def _render_graph_html(build_svg, build_warn, check_svg, kinds_used, meta_line,
                       weight_basis="lines of code"):
    import html

    legend_items = []
    for kind in ("tool", "crate", "codegen", "ui", "artifact",
                 "category", "lane", "always"):
        if kind not in kinds_used:
            continue
        _stroke, fill = _GRAPH_KIND_COLORS[kind]
        legend_items.append(
            f'<span><i style="background:{fill};border-color:{_stroke}"></i>'
            f"{html.escape(_GRAPH_KIND_LABELS[kind])}</span>"
        )
    legend = '<div class="legend">' + "".join(legend_items) + "</div>"
    warn_html = (
        f'<p class="warn">⚠ {html.escape(build_warn)}</p>' if build_warn else ""
    )
    return (
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">"
        '<meta name="viewport" content="width=device-width, initial-scale=1">'
        "<title>phoenix-ide build graph</title><style>" + _GRAPH_CSS
        + "</style></head><body>"
        "<header><h1>phoenix-ide · build graph</h1>"
        f'<div class="meta">{html.escape(meta_line)} · hover a node to trace its edges</div>'
        "</header>" + legend
        + '<section><h2>Build pipeline &amp; workspace crate DAG</h2>'
        '<p class="hint">Topological: each node sits below every input it '
        "depends on. Edges point from a dependency down to its dependent; the "
        "release binary at the bottom embeds the UI bundle. The bar under each "
        f"crate scales with its {html.escape(weight_basis)} — hover for LoC, "
        "deps, blast radius, and compile time.</p>"
        + warn_html
        + '<div class="scroll">' + build_svg + "</div></section>"
        + '<section><h2><code>./dev.py check</code> lane graph</h2>'
        '<p class="hint">Which changed-path categories activate which lanes '
        "(incremental gating). Hover a lane to see its steps.</p>"
        '<div class="scroll">' + check_svg + "</div></section>"
        "<script>" + _GRAPH_JS + "</script></body></html>"
    )


def _graph_to_dict(nodes, edges):
    """Serialize one graph model to a plain dict: topological layer per node,
    structured metric/step fields preserved, edges as [from, to] pairs."""
    layer, _rows, _depth = _topo_layout(nodes, edges)
    out_nodes = []
    for nid, n in nodes.items():
        d = {"id": n["id"], "label": n["label"], "kind": n["kind"],
             "layer": layer[nid]}
        if n.get("sub"):
            d["sub"] = n["sub"]
        for k in ("metrics", "steps", "inputs", "category", "always_on", "meter"):
            if n.get(k) is not None:
                d[k] = n[k]
        out_nodes.append(d)
    out_nodes.sort(key=lambda d: (d["layer"], d["id"]))
    return {"nodes": out_nodes, "edges": [[a, b] for a, b in edges]}


def _build_graph_json(bnodes, bedges, bwarn, cnodes, cedges,
                      head, generated, weight_basis):
    """Assemble the full machine-readable graph payload (minified by caller).

    Edges are oriented dependency→dependent (the dependency builds first);
    `layer` is the longest path from a source, so a node's layer exceeds every
    input it depends on. `meter` is each node's weight (compile time or LoC,
    per `weight_basis`) normalized to the heaviest node in [0,1]."""
    return {
        "schema": "phoenix-build-graph/1",
        "head": head,
        "generated": generated,
        "weight_basis": weight_basis,
        "warnings": [bwarn] if bwarn else [],
        "graphs": {
            "build": {
                "description": "Build-artifact DAG: toolchain + workspace crate "
                               "graph → ts-rs codegen → vite bundle → RustEmbed "
                               "release binary. Edge [a,b] = a builds before b.",
                **_graph_to_dict(bnodes, bedges),
            },
            "check": {
                "description": "`./dev.py check` gating graph: changed-path "
                               "categories (+ always-on) → lanes. Edge [a,b] = "
                               "touching a activates lane b.",
                **_graph_to_dict(cnodes, cedges),
            },
        },
    }


def cmd_graph(out: "Path | None" = None, serve: bool = False,
              port: "int | None" = None, open_browser: bool = False,
              timings: bool = False, fmt: str = "html") -> None:
    """Emit the project build graph as HTML or machine-readable JSON."""
    bnodes, bedges, bwarn, weight_basis = _build_pipeline_model(with_timings=timings)
    cnodes, cedges = _check_graph_model()

    sha = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout.strip() or "unknown"
    now = datetime.datetime.now()

    if fmt == "json":
        payload = json.dumps(
            _build_graph_json(bnodes, bedges, bwarn, cnodes, cedges,
                              sha, now.isoformat(timespec="seconds"), weight_basis),
            separators=(",", ":"),
        )
        content_type = "application/json; charset=utf-8"
        default_name = "build-graph.json"
    else:
        build_svg = _svg_for_graph(bnodes, bedges)
        check_svg = _svg_for_graph(cnodes, cedges)
        meta_line = f"HEAD {sha} · generated {now.strftime('%Y-%m-%d %H:%M')}"
        kinds_used = {n["kind"] for n in {**bnodes, **cnodes}.values()}
        payload = _render_graph_html(build_svg, bwarn, check_svg, kinds_used,
                                     meta_line, weight_basis)
        content_type = "text/html; charset=utf-8"
        default_name = "build-graph.html"
    payload_bytes = payload.encode("utf-8")

    # Summary goes to stderr so a JSON-to-stdout pipe stays pure data.
    log = sys.stderr if (fmt == "json" and out is None and not serve) else sys.stdout

    if serve:
        import http.server
        import webbrowser

        class _Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(payload_bytes)))
                self.end_headers()
                self.wfile.write(payload_bytes)

            def log_message(self, *a):
                pass

        httpd = http.server.ThreadingHTTPServer(("127.0.0.1", port or 0), _Handler)
        url = f"http://127.0.0.1:{httpd.server_address[1]}/"
        print(f"✓ serving build graph ({fmt}) at {url}  (Ctrl-C to stop)")
        print(f"  {len(bnodes)} build nodes, {len(cnodes)} check nodes")
        if bwarn:
            print(f"  ⚠ {bwarn}")
        if open_browser:
            webbrowser.open(url)
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nstopped")
        finally:
            httpd.server_close()
        return

    if fmt == "json" and out is None:
        sys.stdout.write(payload + "\n")
    else:
        # resolve() so a relative --out still yields an absolute path for both
        # the printed location and as_uri() below (which rejects relative paths).
        out = (out or (ROOT / "target" / default_name)).resolve()
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(payload, encoding="utf-8")
        print(f"✓ build graph ({fmt}) → {out}", file=log)
        if open_browser:
            import webbrowser
            webbrowser.open(out.as_uri())
    print(f"  {len(bnodes)} build nodes, {len(cnodes)} check nodes", file=log)
    if bwarn:
        print(f"  ⚠ {bwarn}", file=log)


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


def prod_build(strip: bool = True, target: str | None = "x86_64-unknown-linux-musl") -> Path:
    """Build the production binary from the invoking checkout's exact HEAD."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True
    )
    commit = result.stdout.strip()
    ref = commit
    dirty = subprocess.run(
        ["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True
    ).stdout.strip()
    if dirty:
        print("⚠ Warning: uncommitted changes will NOT be included in the build:")
        for line in dirty.splitlines()[:10]:
            print(f"    {line}")
        if len(dirty.splitlines()) > 10:
            print(f"    ... and {len(dirty.splitlines()) - 10} more")
        print()
    print(f"Building from HEAD: {commit[:12]}")
    
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
    subprocess.run(["git", "restore", "ui/dist/.gitkeep"], cwd=worktree, check=True)
    build_tree_status = subprocess.run(
        ["git", "status", "--porcelain"], cwd=worktree, capture_output=True, text=True, check=True
    ).stdout.strip()
    if build_tree_status:
        raise SystemExit(f"production build worktree is dirty before Rust compilation:\n{build_tree_status}")
    
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


def _serialize_env_snapshot(env: dict[str, str]) -> str:
    escaped_newline = "\\n"
    return "".join(f"{key}={value.replace(chr(10), escaped_newline)}\n" for key, value in env.items())


def _systemd_artifact(target: Path | None, source: Path | None = None) -> dict[str, str | None]:
    if target is None:
        return {"path": None, "sha256": None}
    return {"path": str(target), "sha256": _file_sha256(source or target)}


def _systemd_current_identity(env: dict[str, str]) -> "RuntimeIdentity | None":
    identity = _current_prod_identity(env)
    return RuntimeIdentity.from_value(identity) if identity is not None else None


def _systemd_copy_rollback(source: Path, destination: Path) -> Path | None:
    if not source.exists():
        return None
    shutil.copy2(source, destination)
    destination.chmod(0o600)
    return destination


def _stage_systemd_root_handoff(
    staging: Path,
    transaction_id: str,
    helper: Path,
    files: list[tuple[str, Path]],
) -> Path:
    bundle = staging / "bundle.json"
    _write_json_atomic(bundle, {
        "transaction_id": transaction_id,
        "files": [
            {"name": name, "source": str(path), "sha256": _file_sha256(path)}
            for name, path in files
        ],
    })
    bootstrap = SYSTEMD_DEPLOY_DIR / f"bootstrap-{transaction_id}"
    root_helper = bootstrap / "helper.py"
    subprocess.run(["sudo", "install", "-d", "-m", "0700", str(bootstrap)], check=True)
    subprocess.run([
        "sudo", "install", "-o", "root", "-g", "root", "-m", "0700",
        str(helper), str(root_helper),
    ], check=True)
    result = subprocess.run(
        [
            "sudo", "python3", str(root_helper), "stage",
            "--bundle", str(bundle), "--source-uid", str(os.getuid()),
        ],
        capture_output=True,
        text=True,
    )
    subprocess.run(["sudo", "rm", "-rf", str(bootstrap)], check=False)
    if result.returncode != 0:
        raise SystemExit(f"systemd handoff staging failed: {(result.stderr or result.stdout).strip()}")
    manifest_path = Path(result.stdout.strip())
    expected = SYSTEMD_TRANSACTION_ROOT / transaction_id / "manifest.json"
    if manifest_path != expected:
        raise SystemExit(f"systemd handoff returned unexpected manifest path {manifest_path}")
    return manifest_path


def _read_systemd_installed_env() -> dict[str, str]:
    import tempfile

    result = subprocess.run(["sudo", "cat", str(PROD_ENV_FILE)], capture_output=True, text=True)
    if result.returncode != 0:
        return {}
    with tempfile.NamedTemporaryFile(mode="w", delete=False) as snapshot:
        snapshot.write(result.stdout)
        path = Path(snapshot.name)
    try:
        return _load_installed_env(path)
    finally:
        path.unlink(missing_ok=True)


def _systemd_installed_runtime() -> tuple["RuntimeIdentity | None", str | None]:
    binary = PROD_INSTALL_DIR / "phoenix-ide"
    if not binary.is_file():
        return None, None
    identity = _binary_identity(binary)
    if not identity.is_exact():
        raise SystemExit(f"installed systemd binary has incomplete identity: {identity.display()}")
    return identity, _prod_api_health_url(_read_systemd_installed_env())


def _launch_systemd_activation(transaction_id: str, root_manifest: Path) -> None:
    root_transaction = root_manifest.parent
    activation_unit = f"phoenix-ide-deploy-{transaction_id}"
    try:
        subprocess.run([
            "sudo", "systemd-run", "--no-block", f"--unit={activation_unit}",
            "--property=Type=oneshot", "--",
            "python3", str(root_transaction / "helper.py"), "activate", "--manifest", str(root_manifest),
        ], check=True)
    except subprocess.CalledProcessError:
        cleanup = subprocess.run([
            "sudo", "python3", str(root_transaction / "helper.py"),
            "abandon", "--manifest", str(root_manifest),
        ], capture_output=True, text=True)
        if cleanup.returncode != 0:
            raise SystemExit(
                "systemd transient activation failed and its claim could not be released: "
                f"{(cleanup.stderr or cleanup.stdout).strip()}"
            )
        raise SystemExit("systemd transient activation unit did not start; staged claim released")


def native_prod_deploy(
    release: str | None = None,
    *,
    controller: "ProdDeployControllerOptions | None" = None,
):
    """Prepare and hand systemd activation to an independent root transient unit."""
    import tempfile
    import uuid

    controller = controller or ProdDeployControllerOptions()
    if controller.enabled:
        release, _expected_full_commit = controller.require_exact_release(release)
        _require_noninteractive_sudo_ready()
    if not check_systemd_available():
        raise SystemExit("systemd is not available on this Linux host")

    env_snapshot = _read_systemd_installed_env() if controller.enabled else {}
    env_file_loaded = str(PROD_ENV_FILE) if controller.enabled else _load_env_file(env_snapshot)
    _preflight_prod_bind_auth(env_snapshot, socket_activated=True)
    service_user = detect_service_user()
    transaction_id = controller.transaction_id or uuid.uuid4().hex

    with tempfile.TemporaryDirectory(prefix=f"phoenix-systemd-{transaction_id}-") as temporary:
        staging = Path(temporary)
        prepared = (
            _prepare_release_candidate(release, staging, expected_full_commit=controller.expected_full_commit, expected_asset_name=controller.expected_asset_name, expected_asset_sha256=controller.expected_asset_sha256)
            if release
            else _prepare_local_candidate(target=_linux_musl_target())
        )
        candidate_binary = staging / "candidate-binary"
        if prepared.binary != candidate_binary:
            shutil.copy2(prepared.binary, candidate_binary)
        candidate_binary.chmod(0o700)

        candidate_env = staging / "candidate.env"
        candidate_env.write_text(_serialize_env_snapshot(env_snapshot))
        candidate_env.chmod(0o600)
        try:
            candidate_port = int(env_snapshot.get("PHOENIX_PORT", str(PROD_PORT)))
            if not 1 <= candidate_port <= 65535:
                raise ValueError
        except ValueError as exc:
            raise SystemExit("PHOENIX_PORT must be an integer from 1 through 65535") from exc
        config = dataclasses.replace(
            NATIVE_SYSTEMD_CONFIG,
            user=service_user,
            db_path=str(SYSTEMD_DB_PATH),
            port=candidate_port,
            home_dir=str(Path.home()),
            env_file_path=str(PROD_ENV_FILE) if env_snapshot else None,
        )
        candidate_service = staging / "candidate.service"
        candidate_service.write_text(generate_systemd_service(config, prepared.identity.version))
        candidate_service.chmod(0o600)
        candidate_socket = staging / "candidate.socket"
        candidate_socket.write_text(generate_systemd_socket(config))
        candidate_socket.chmod(0o600)

        helper = staging / "helper.py"
        _materialize_source_file(
            prepared.source_commit,
            "scripts/systemd_deploy_helper.py",
            helper,
            prepared.source_kind.value,
        )
        protocol = subprocess.run(
            [sys.executable, str(helper), "--protocol-version"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        if protocol != str(SYSTEMD_HANDOFF_PROTOCOL_VERSION):
            raise SystemExit(f"systemd helper protocol mismatch: expected {SYSTEMD_HANDOFF_PROTOCOL_VERSION}, got {protocol!r}")

        root_transaction = SYSTEMD_TRANSACTION_ROOT / transaction_id
        root_paths = {name: root_transaction / name for name in (
            "candidate-binary", "candidate.service", "candidate.socket", "candidate.env",
            "rollback-binary", "rollback.service", "rollback.socket", "rollback.env", "helper.py",
        )}
        manifest = {
            "manifest_version": SYSTEMD_HANDOFF_PROTOCOL_VERSION,
            "transaction_id": transaction_id,
            "unit_name": PROD_SERVICE_NAME,
            "service_user": service_user,
            "source_kind": prepared.source_kind.value,
            "source_commit": prepared.source_commit,
            "release_tag": prepared.release_tag,
            "release_commit": prepared.release_commit,
            "expected": prepared.identity.as_dict(),
            "previous": None,
            "expected_health_url": _prod_api_health_url(env_snapshot),
            "previous_health_url": None,
            "candidate": {
                "binary": _systemd_artifact(root_paths["candidate-binary"], candidate_binary),
                "service": _systemd_artifact(root_paths["candidate.service"], candidate_service),
                "socket": _systemd_artifact(root_paths["candidate.socket"], candidate_socket),
                "environment": _systemd_artifact(root_paths["candidate.env"], candidate_env) if env_snapshot else _systemd_artifact(None),
            },
            "rollback": {
                "binary": _systemd_artifact(None),
                "service": _systemd_artifact(None),
                "socket": _systemd_artifact(None),
                "environment": _systemd_artifact(None),
            },
            "targets": {
                "binary": str(PROD_INSTALL_DIR / "phoenix-ide"),
                "service": str(SYSTEMD_SERVICE_PATH),
                "socket": str(SYSTEMD_SOCKET_PATH),
                "environment": str(PROD_ENV_FILE),
                "deployed_sha": str(SYSTEMD_DEPLOYED_SHA_PATH),
            },
            "status_path": str(SYSTEMD_STATUS_PATH),
            "active_path": str(SYSTEMD_ACTIVE_PATH),
            "activation_lock_path": str(SYSTEMD_ACTIVATION_LOCK_PATH),
            "claim_lock_path": str(SYSTEMD_CLAIM_LOCK_PATH),
            "previous_deployed_sha": None,
            "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        }
        local_manifest = staging / "manifest.json"
        _write_json_atomic(local_manifest, manifest)
        files = [
            ("candidate-binary", candidate_binary),
            ("candidate.service", candidate_service),
            ("candidate.socket", candidate_socket),
            ("helper.py", helper),
            ("manifest.json", local_manifest),
        ]
        if env_snapshot:
            files.append(("candidate.env", candidate_env))
        root_manifest = _stage_systemd_root_handoff(staging, transaction_id, helper, files)

    _launch_systemd_activation(transaction_id, root_manifest)
    print("\n✓ Activation handed to an independent root systemd unit")
    print(f"  Transaction: {transaction_id}")
    print(f"  Candidate: {prepared.identity.version} ({prepared.identity.git_sha})")
    print("  After reconnecting, run: ./dev.py prod status")

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


def _bind_is_loopback(effective_env: dict[str, str]) -> bool:
    """Mirror the binary's fallback-bind resolution to decide if it binds loopback.

    The binary defaults the bind IP to 0.0.0.0 (non-loopback) and lets
    PHOENIX_BIND_ADDR override it; an unparseable PHOENIX_BIND_ADDR also falls
    back to 0.0.0.0. So the bind is loopback only when PHOENIX_BIND_ADDR is a
    valid loopback IP (127.0.0.0/8 or ::1).
    """
    raw = effective_env.get("PHOENIX_BIND_ADDR")
    if not raw:
        return False  # default is 0.0.0.0, non-loopback
    try:
        return ipaddress.ip_address(raw.strip()).is_loopback
    except ValueError:
        return False  # binary falls back to 0.0.0.0 on an invalid value


def _preflight_prod_bind_auth(effective_env: dict[str, str], socket_activated: bool) -> None:
    """Refuse to deploy an unauthenticated, network-reachable prod server.

    `effective_env` is the single `.phoenix-ide.env` snapshot installed for the
    candidate. Backend-specific legacy override stores are not consulted.

    `socket_activated` is True for the systemd (.socket) and launchd (Sockets)
    paths: the listener fd comes from the init system, which binds all
    interfaces (`ListenStream={port}` / the launchd socket), and the binary
    IGNORES PHOENIX_BIND_ADDR. So a loopback PHOENIX_BIND_ADDR does NOT make a
    socket-activated deploy safe — only a password or the insecure override
    does. The non-socket-activated daemon binds via the binary's own fallback,
    so there PHOENIX_BIND_ADDR=127.0.0.1 genuinely yields a loopback listener.

    The binary fails closed at startup on a non-loopback passwordless bind. This
    turns that post-restart crash into a clear deploy-time error, mirroring the
    binary's own guard.
    """
    if not socket_activated and _bind_is_loopback(effective_env):
        return
    if effective_env.get("PHOENIX_PASSWORD"):
        return
    if effective_env.get("PHOENIX_ALLOW_INSECURE_BIND") == "1":
        return
    loopback_hint = (
        ""
        if socket_activated
        else "  To bind loopback-only instead, set PHOENIX_BIND_ADDR=127.0.0.1.\n"
    )
    print(
        "ERROR: refusing to deploy an unauthenticated, network-reachable Phoenix.\n"
        "  The prod server binds all interfaces; without a password anyone on the\n"
        "  network can drive an agent that runs arbitrary commands as the service user.\n"
        "\n"
        "  Set PHOENIX_PASSWORD=<secret> in .phoenix-ide.env (the deployed service\n"
        "  reads its environment from that file, not your shell), then redeploy.\n"
        f"{loopback_hint}"
        "  To deploy passwordless anyway (e.g. Phoenix sits behind your own auth\n"
        "  proxy), set PHOENIX_ALLOW_INSECURE_BIND=1 in .phoenix-ide.env.",
        file=sys.stderr,
    )
    sys.exit(1)


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


def _llm_mode_summary(env: dict[str, str]) -> str:
    """Human-readable description of how the deployed server will reach an LLM,
    for the post-deploy summary line."""
    if env.get("LLM_API_KEY_HELPER"):
        return "api_key_helper (from .phoenix-ide.env)"
    keys = [k for k in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY") if env.get(k)]
    if keys:
        return f"{' + '.join(keys)} (from .phoenix-ide.env)"
    codex = _detect_codex_auth(env)
    if codex:
        return codex
    return "none detected — server has no LLM configured"


def _configure_llm_env(env: dict[str, str]) -> str:
    """Configure LLM environment variables. Returns a human-readable mode string.

    Priority:
    1. .phoenix-ide.env overrides (LLM_API_KEY_HELPER, API keys, Codex auth, etc.)
    2. ANTHROPIC_API_KEY / OPENAI_API_KEY from shell environment
    3. Codex auth from Phoenix or Codex CLI
    """
    # If env file provided LLM config, respect it — skip auto-detection
    if env.get("LLM_API_KEY_HELPER"):
        helper = env["LLM_API_KEY_HELPER"]
        return f"api_key_helper ({helper})"
    keys = [k for k in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY") if env.get(k)]
    if keys:
        return f"direct API key ({' + '.join(keys)})"
    if codex := _detect_codex_auth(env):
        return codex

    # Last resort: check shell env for API keys
    for key in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY"):
        api_key = os.environ.get(key)
        if api_key:
            env[key] = api_key
            return f"direct API key ({key})"

    print("ERROR: No LLM configuration found.", file=sys.stderr)
    print("  Options:", file=sys.stderr)
    print("    1. Create .phoenix-ide.env with LLM_API_KEY_HELPER, ANTHROPIC_API_KEY, or OPENAI_API_KEY", file=sys.stderr)
    print("    2. Sign in with Codex or set OPENAI_USE_CODEX_AUTH=1 with Codex CLI auth", file=sys.stderr)
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
    port = env.get("PHOENIX_PORT", str(PROD_PORT))
    return f"{scheme}://localhost:{port}"


def _prod_local_health_url(env: dict[str, str] | None = None) -> str:
    env = _repo_env() if env is None else env
    scheme = "https" if tls_enabled_from_env(env) else "http"
    port = env.get("PHOENIX_PORT", str(PROD_PORT))
    return f"{scheme}://localhost:{port}/version"


def _prod_api_health_url(env: dict[str, str] | None = None) -> str:
    return _prod_local_health_url(env).removesuffix("/version") + "/api/version"


def _bare_api_health_url(env: dict[str, str]) -> str:
    import ipaddress

    bind = env.get("PHOENIX_BIND_ADDR", "127.0.0.1")
    try:
        address = ipaddress.ip_address(bind)
    except ValueError as exc:
        raise SystemExit("PHOENIX_BIND_ADDR must be an IP address") from exc
    if address.is_unspecified:
        address = ipaddress.ip_address("::1" if address.version == 6 else "127.0.0.1")
    host = f"[{address}]" if address.version == 6 else str(address)
    scheme = "https" if tls_enabled_from_env(env) else "http"
    port = env.get("PHOENIX_PORT", str(PROD_PORT))
    return f"{scheme}://{host}:{port}/api/version"


def _load_installed_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    if not path.is_file():
        return env
    for raw in path.read_text().splitlines():
        if not raw or raw.lstrip().startswith("#"):
            continue
        key, separator, value = raw.partition("=")
        if not separator or not key:
            raise SystemExit(f"invalid installed production environment snapshot: {path}")
        env[key] = value.replace("\\n", "\n")
    return env


def _installed_runtime(binary: Path, environment: Path) -> tuple["RuntimeIdentity | None", str | None]:
    if not binary.is_file():
        return None, None
    identity = _binary_identity(binary)
    if not identity.is_exact():
        raise SystemExit(f"installed production binary has incomplete identity: {identity.display()}")
    return identity, _prod_api_health_url(_load_installed_env(environment))


def _installed_bare_runtime(binary: Path, environment: Path) -> tuple["RuntimeIdentity | None", str | None]:
    if not binary.is_file():
        return None, None
    identity = _binary_identity(binary)
    if not identity.is_exact():
        raise SystemExit(f"installed bare binary has incomplete identity: {identity.display()}")
    return identity, _bare_api_health_url(_load_installed_env(environment))


def _open_prod_health(env: dict[str, str] | None = None, timeout: float = 5.0):
    import ssl
    import urllib.request

    env = env or _repo_env()
    context = ssl._create_unverified_context() if tls_enabled_from_env(env) else None
    return urllib.request.urlopen(_prod_local_health_url(env), timeout=timeout, context=context)


def _wait_for_health(
    t0: float,
    env: dict[str, str] | None = None,
    timeout_secs: float = 20.0,
    poll_interval: float = 0.1,
) -> tuple[str | None, float]:
    """Poll the health endpoint until it responds or timeout_secs elapses.

    Returns (version_string_or_None, elapsed_seconds_from_t0).
    t0 should be time.monotonic() captured just before the launch command.
    """
    deadline = t0 + timeout_secs
    while time.monotonic() < deadline:
        try:
            with _open_prod_health(env, timeout=poll_interval * 5) as resp:
                version = resp.read().decode().strip()
                return version, time.monotonic() - t0
        except Exception:
            time.sleep(poll_interval)
    return None, time.monotonic() - t0


def _bare_layout() -> dict[str, Path]:
    root = Path.home() / ".phoenix-ide"
    return {
        "root": root,
        "supervisor": root / "bin/phoenix-supervisor.py",
        "binary": root / "bin/phoenix-ide",
        "environment": root / "config/phoenix.env",
        "transactions": root / "deploy/transactions",
        "status": root / "deploy/status.json",
        "socket": root / "run/supervisor.sock",
        "deployed_sha": root / "deployed.sha",
    }


def _start_bare_supervisor(layout: dict[str, Path], protocol: str, selected_source: Path) -> None:
    if layout["socket"].exists():
        running = subprocess.run(
            [sys.executable, str(layout["supervisor"]), "--root", str(layout["root"]), "status"],
            capture_output=True,
            text=True,
        )
        if running.returncode == 0:
            try:
                running_protocol = str(json.loads(running.stdout)["protocol_version"])
            except (KeyError, TypeError, json.JSONDecodeError) as exc:
                raise SystemExit("running bare supervisor did not report its protocol version") from exc
            if running_protocol != protocol:
                raise SystemExit(
                    "running bare supervisor uses an incompatible protocol; stop it from an external shell "
                    "with `python3 ~/.phoenix-ide/bin/phoenix-supervisor.py shutdown-supervisor`, then redeploy"
                )
            if layout["supervisor"].is_file() and _file_sha256(layout["supervisor"]) == _file_sha256(selected_source):
                return
            raise SystemExit(
                "running bare supervisor differs from the selected deployment source; production was left running. "
                "Stop the supervisor from an external shell with "
                "`python3 ~/.phoenix-ide/bin/phoenix-supervisor.py shutdown-supervisor`, then redeploy"
            )
        layout["socket"].unlink(missing_ok=True)
    layout["supervisor"].parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(selected_source, layout["supervisor"])
    layout["supervisor"].chmod(0o700)
    with (layout["root"] / "supervisor.log").open("ab") as log:
        subprocess.Popen(
            _bare_supervisor_command(layout),
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            close_fds=True,
        )
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if layout["socket"].exists():
            return
        time.sleep(0.05)
    raise SystemExit("bare Linux supervisor did not become ready")


def _bare_supervisor_command(layout: dict[str, Path]) -> list[str]:
    return [
        sys.executable,
        str(layout["supervisor"]),
        "--root",
        str(layout["root"]),
        "run",
    ]


def _configure_bare_reboot_persistence(layout: dict[str, Path]) -> bool:
    import shlex

    command = " ".join(shlex.quote(part) for part in _bare_supervisor_command(layout))
    marker = "# phoenix-ide persistent supervisor"
    crontab = shutil.which("crontab")
    if crontab is not None:
        current = subprocess.run([crontab, "-l"], capture_output=True, text=True)
        if current.returncode in {0, 1}:
            lines = [line for line in current.stdout.splitlines() if marker not in line]
            lines.extend([
                f"@reboot {command} >> {shlex.quote(str(layout['root'] / 'supervisor.log'))} 2>&1 {marker}",
                "",
            ])
            installed = subprocess.run([crontab, "-"], input="\n".join(lines), text=True, capture_output=True)
            if installed.returncode == 0:
                print("  Reboot persistence: owner crontab @reboot entry installed")
                return True
    print("  Reboot persistence: not configured (compatible owner crontab unavailable)")
    print("  Add this command to the host's same-user boot/rc mechanism:")
    print(f"    {command}")
    return False


def _bare_child_running(layout: dict[str, Path]) -> bool:
    result = subprocess.run(
        [sys.executable, str(layout["supervisor"]), "--root", str(layout["root"]), "status"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"cannot inspect bare supervisor child state: {(result.stderr or result.stdout).strip()}")
    return json.loads(result.stdout).get("child") is not None


def _discard_unclaimed_bare_transaction(transaction: Path) -> None:
    if not transaction.exists():
        return
    transaction.chmod(0o700)
    for artifact in transaction.iterdir():
        artifact.chmod(0o600)
    shutil.rmtree(transaction)


def prod_daemon_deploy(
    release: str | None = None,
    *,
    controller: "ProdDeployControllerOptions | None" = None,
):
    """Deploy through the persistent same-user supervisor on Linux without systemd."""
    import tempfile
    import uuid

    controller = controller or ProdDeployControllerOptions()
    if controller.enabled:
        release, _expected_full_commit = controller.require_exact_release(release)
    layout = _bare_layout()
    env_snapshot = _load_installed_env(layout["environment"]) if controller.enabled else {}
    if not controller.enabled:
        _load_env_file(env_snapshot)
    env_snapshot.setdefault("HOME", str(Path.home()))
    env_snapshot.setdefault("PATH", os.environ.get("PATH", os.defpath))
    env_snapshot.setdefault("PHOENIX_PORT", str(PROD_PORT))
    env_snapshot.setdefault("PHOENIX_DB_PATH", str(Path.home() / ".phoenix-ide/prod.db"))
    env_snapshot.setdefault("PHOENIX_LOG_FILE", str(Path.home() / ".phoenix-ide/prod.log"))
    env_snapshot.setdefault("PHOENIX_LOG_STDOUT", "false")
    _preflight_prod_bind_auth(env_snapshot, socket_activated=False)
    transaction_id = controller.transaction_id or uuid.uuid4().hex

    with tempfile.TemporaryDirectory(prefix=f"phoenix-bare-{transaction_id}-") as td:
        staging = Path(td)
        prepared = (
            _prepare_release_candidate(release, staging, expected_full_commit=controller.expected_full_commit, expected_asset_name=controller.expected_asset_name, expected_asset_sha256=controller.expected_asset_sha256)
            if release else _prepare_local_candidate(target=_linux_musl_target())
        )
        supervisor_source = staging / "bare-supervisor.py"
        _materialize_source_file(
            prepared.source_commit,
            "scripts/bare_supervisor.py",
            supervisor_source,
            prepared.source_kind.value,
        )
        protocol = subprocess.run(
            [sys.executable, str(supervisor_source), "--protocol-version"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        if protocol != "1":
            raise SystemExit(f"bare supervisor protocol mismatch: {protocol!r}")

        _start_bare_supervisor(layout, protocol, supervisor_source)
        _configure_bare_reboot_persistence(layout)
        previous_running = _bare_child_running(layout)

        transaction = layout["transactions"] / transaction_id
        transaction.mkdir(parents=True, mode=0o700)
        candidate = transaction / "candidate-binary"
        shutil.copy2(prepared.binary, candidate)
        candidate.chmod(0o600)
        candidate_env = transaction / "candidate.env"
        candidate_env.write_text(_serialize_env_snapshot(env_snapshot))
        candidate_env.chmod(0o600)
        previous_identity, previous_health_url = _installed_bare_runtime(layout["binary"], layout["environment"])
        rollback_binary = None
        rollback_env = None
        if previous_identity is not None:
            if not layout["environment"].is_file():
                raise SystemExit("installed bare production lacks rollback environment")
            rollback_binary = transaction / "rollback-binary"
            rollback_env = transaction / "rollback.env"
            shutil.copy2(layout["binary"], rollback_binary)
            shutil.copy2(layout["environment"], rollback_env)
            rollback_binary.chmod(0o600)
            rollback_env.chmod(0o600)
        manifest = {
            "manifest_version": 1,
            "transaction_id": transaction_id,
            "expected": prepared.identity.as_dict(),
            "previous": previous_identity.as_dict() if previous_identity else None,
            "expected_health_url": _bare_api_health_url(env_snapshot),
            "previous_health_url": previous_health_url,
            "candidate_binary": {"name": candidate.name, "sha256": _file_sha256(candidate)},
            "candidate_environment": {"name": candidate_env.name, "sha256": _file_sha256(candidate_env)},
            "rollback_binary": {"name": rollback_binary.name, "sha256": _file_sha256(rollback_binary)} if rollback_binary else None,
            "rollback_environment": {"name": rollback_env.name, "sha256": _file_sha256(rollback_env)} if rollback_env else None,
            "source_commit": prepared.source_commit,
            "source_kind": prepared.source_kind.value,
            "release_tag": prepared.release_tag,
            "previous_deployed_sha": layout["deployed_sha"].read_text().strip() if layout["deployed_sha"].exists() else None,
            "previous_running": previous_running,
            "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        }
        manifest_path = transaction / "manifest.json"
        _write_json_atomic(manifest_path, manifest)
        manifest_hash = _file_sha256(manifest_path)
        for artifact in transaction.iterdir():
            artifact.chmod(0o400)
        transaction.chmod(0o500)

    result = subprocess.run([
        sys.executable, str(layout["supervisor"]), "--root", str(layout["root"]),
        "activate", "--transaction-id", transaction_id, "--manifest-sha256", manifest_hash,
    ], capture_output=True, text=True)
    if result.returncode != 0:
        if not layout["active"].exists():
            _discard_unclaimed_bare_transaction(layout["transactions"] / transaction_id)
        raise SystemExit(f"bare deployment failed: {(result.stderr or result.stdout).strip()}")
    response = json.loads(result.stdout)
    state = response.get("state")
    if state != "committed":
        raise SystemExit(f"bare deployment ended in {state}; run ./dev.py prod status")
    print("\n✓ Deployed through persistent bare Linux supervisor")
    print(f"  Transaction: {transaction_id}")
    print(f"  Candidate: {prepared.identity.version} ({prepared.identity.git_sha})")
    print(f"  Database: {Path.home() / '.phoenix-ide/prod.db'}")
    print(f"  Logs: {Path.home() / '.phoenix-ide/prod.log'}")


def prod_daemon_status():
    """Show supervisor and managed-child status."""
    layout = _bare_layout()
    if not layout["socket"].exists():
        print("Status: Supervisor not running")
        if layout["status"].exists():
            status = json.loads(layout["status"].read_text())
            print(f"  Deployment: {status.get('state')} ({status.get('transaction_id')})")
        if layout["deployed_sha"].exists():
            print(f"  Commit: {layout['deployed_sha'].read_text().strip()}")
        return
    result = subprocess.run(
        [sys.executable, str(layout["supervisor"]), "--root", str(layout["root"]), "status"],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        print(f"Status: Supervisor unreachable ({(result.stderr or result.stdout).strip()})")
        return
    value = json.loads(result.stdout)
    child = value.get("child")
    print(f"Supervisor: running (PID {value.get('supervisor_pid')})")
    if child is None:
        print("Phoenix: stopped")
    else:
        runtime = child["runtime"]
        print(f"Phoenix: running (PID {child['pid']}, start {child['proc_start_time']})")
        print(f"  Identity: {runtime['version']} ({runtime['git_sha']})")
    if layout["status"].exists():
        status = json.loads(layout["status"].read_text())
        print(f"  Deployment: {status.get('state')} ({status.get('transaction_id')})")
    print(f"  Logs: {Path.home() / '.phoenix-ide/prod.log'}")


def prod_daemon_stop():
    """Stop the supervisor-owned Phoenix child while preserving the supervisor."""
    layout = _bare_layout()
    if not layout["socket"].exists():
        print("No bare Linux supervisor running")
        return
    result = subprocess.run(
        [sys.executable, str(layout["supervisor"]), "--root", str(layout["root"]), "stop"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"failed to stop supervised Phoenix: {(result.stderr or result.stdout).strip()}")
    print("✓ Phoenix stopped; supervisor remains running")



def _read_systemd_deploy_status() -> dict[str, object] | None:
    result = subprocess.run(
        ["sudo", "cat", str(SYSTEMD_STATUS_PATH)],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    try:
        status = json.loads(result.stdout)
        return status if isinstance(status, dict) else None
    except json.JSONDecodeError:
        return None


def _read_systemd_deployed_sha() -> str | None:
    result = subprocess.run(
        ["sudo", "cat", str(SYSTEMD_DEPLOYED_SHA_PATH)], capture_output=True, text=True
    )
    return result.stdout.strip() if result.returncode == 0 and result.stdout.strip() else None


def native_prod_status():
    """Show production service status (native Linux)."""
    # Check if service exists
    result = subprocess.run(
        ["systemctl", "is-active", PROD_SERVICE_NAME],
        capture_output=True, text=True
    )
    status = result.stdout.strip()
    
    if status == "active":
        installed_env = _read_systemd_installed_env()
        installed_port = installed_env.get("PHOENIX_PORT", str(PROD_PORT))
        print(f"Production: running")
        print(f"  Port: {installed_port}")
        print(f"  URL: {_prod_local_health_url(installed_env).removesuffix('/version')}")
        print(f"  Database: {installed_env.get('PHOENIX_DB_PATH', str(SYSTEMD_DB_PATH))}")

        # Health check
        try:
            _open_prod_health(installed_env, timeout=2).close()
            print(f"  Health: OK")
        except Exception:
            print(f"  Health: not responding")

        if sha := _read_systemd_deployed_sha():
            print(f"  Commit: {sha}")
    else:
        print(f"Production: {status}")

    deploy_status = _read_systemd_deploy_status()
    if deploy_status is not None:
        print("  Deployment:")
        print(f"    Transaction: {deploy_status.get('transaction_id', '<unknown>')}")
        print(f"    State: {deploy_status.get('state', '<unknown>')}")
        expected_version = deploy_status.get("expected_version")
        expected_sha = deploy_status.get("expected_git_sha")
        if expected_version and expected_sha:
            print(f"    Candidate: {expected_version} ({expected_sha})")
        if deploy_status.get("failure"):
            print(f"    Failure: {deploy_status['failure']}")
        if deploy_status.get("rollback_failure"):
            print(f"    Rollback failure: {deploy_status['rollback_failure']}")
    
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
        # Unified logging: the binary appends JSON to this file directly. stdout
        # is off, so the plist's StandardOut/ErrorPath (same file) only carries
        # pre-logger / panic output. newsyslog handles rotation.
        "PHOENIX_LOG_FILE": str(LAUNCHD_LOG_PATH),
        "PHOENIX_LOG_STDOUT": "false",
    }
    # Merge .phoenix-ide.env overrides (LLM_API_KEY_HELPER, base URLs, etc.)
    if extra_env:
        env_vars.update(extra_env)
    launchd_socket_port = env_vars["PHOENIX_PORT"]

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

  <key>Sockets</key>
  <dict>
    <key>Listeners</key>
    <dict>
      <key>SockFamily</key>
      <string>IPv4v6</string>
      <key>SockProtocol</key>
      <string>TCP</string>
      <key>SockServiceName</key>
      <string>{_xml_escape(launchd_socket_port)}</string>
      <key>SockType</key>
      <string>stream</string>
    </dict>
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


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class ProdSourceKind(enum.Enum):
    LOCAL_HEAD = "local_head"
    PUBLISHED_RELEASE = "published_release"


@dataclasses.dataclass(frozen=True)
class RuntimeIdentity:
    version: str
    git_sha: str

    @classmethod
    def from_value(cls, value: "RuntimeIdentity | dict[str, str]") -> "RuntimeIdentity":
        if isinstance(value, cls):
            return value
        return cls(version=value["version"], git_sha=value["git_sha"])

    def is_exact(self) -> bool:
        return bool(self.version and self.git_sha)

    def display(self) -> str:
        return f"{self.version or '<unknown>'} ({self.git_sha or '<unknown>'})"

    def as_dict(self) -> dict[str, str]:
        if self.version is None or self.git_sha is None:
            raise ValueError("exact runtime identity requires both version and git SHA")
        return {"version": self.version, "git_sha": self.git_sha}


@dataclasses.dataclass(frozen=True)
class PreparedCandidate:
    binary: Path
    source_kind: ProdSourceKind
    source_commit: str
    identity: RuntimeIdentity
    release_tag: str | None = None
    release_commit: str | None = None


@dataclasses.dataclass(frozen=True)
class ProdDeployControllerOptions:
    enabled: bool = False
    exact_release_tag: str | None = None
    expected_full_commit: str | None = None
    expected_asset_name: str | None = None
    expected_asset_sha256: str | None = None
    transaction_id: str | None = None

    def require_exact_release(self, release: str | None) -> tuple[str, str]:
        if release is None:
            raise SystemExit("controller mode requires --release")
        if not self.enabled:
            return release, ""
        if self.exact_release_tag is None:
            raise SystemExit("controller mode requires --controller-release-tag")
        if release != self.exact_release_tag:
            raise SystemExit(
                f"controller mode requires --release to exactly match --controller-release-tag ({self.exact_release_tag})"
            )
        expected = self.expected_full_commit
        if expected is None:
            raise SystemExit("controller mode requires --controller-expected-full-commit")
        if re.fullmatch(r"[0-9a-f]{40}", expected) is None:
            raise SystemExit("--controller-expected-full-commit must be a full 40-character lowercase git SHA")
        return release, expected



def _noninteractive_sudo_prefix() -> list[str]:
    return ["sudo", "-n"]



def _require_noninteractive_sudo_ready() -> None:
    result = subprocess.run(_noninteractive_sudo_prefix() + ["true"], capture_output=True, text=True)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        if detail:
            raise SystemExit(f"non-interactive sudo is required before disruption: {detail}")
        raise SystemExit("non-interactive sudo is required before disruption")


def _binary_identity(binary: Path) -> RuntimeIdentity:
    result = subprocess.run(
        [str(binary), "--build-identity"], capture_output=True, text=True, timeout=10, check=True
    )
    try:
        identity = json.loads(result.stdout)
        version = identity["version"]
        git_sha = identity["git_sha"]
    except (json.JSONDecodeError, KeyError, TypeError) as exc:
        raise SystemExit("candidate binary did not report a valid embedded build identity") from exc
    if not isinstance(version, str) or not version or not isinstance(git_sha, str) or not git_sha or git_sha == "unknown":
        raise SystemExit("candidate binary has an incomplete embedded build identity")
    return RuntimeIdentity(version=version, git_sha=git_sha)


def _launchd_health_probe(env: dict[str, str]) -> tuple[str, bool]:
    return (
        _prod_local_health_url(env).replace("/version", "/api/version"),
        tls_enabled_from_env(env),
    )


def _launchd_env_from_plist(path: Path) -> dict[str, str]:
    with path.open("rb") as stream:
        plist = plistlib.load(stream)
    env = {str(key): str(value) for key, value in plist.get("EnvironmentVariables", {}).items()}
    try:
        socket_port = plist["Sockets"]["Listeners"]["SockServiceName"]
        env["PHOENIX_PORT"] = str(socket_port)
    except (KeyError, TypeError):
        pass
    return env


def _rollback_identity_matches(
    rollback_identity: RuntimeIdentity | dict[str, str],
    previous_identity: RuntimeIdentity | dict[str, str],
    previous_health_json: bool,
) -> bool:
    rollback_identity = RuntimeIdentity.from_value(rollback_identity)
    previous_identity = RuntimeIdentity.from_value(previous_identity)
    rollback_sha = rollback_identity.git_sha
    if re.fullmatch(r"[0-9a-f]{12}", rollback_sha) is None:
        return False
    if previous_health_json:
        return rollback_identity == previous_identity
    return (
        rollback_identity.version == previous_identity.version
        and previous_identity.git_sha.startswith(rollback_sha)
    )


def _parse_legacy_version_body(body: str) -> str | None:
    value = body.strip()
    prefix = "phoenix-ide "
    if value.startswith(prefix):
        value = value[len(prefix):].strip()
    return value or None


def _legacy_prod_identity(env: dict[str, str]) -> tuple[RuntimeIdentity, str, bool] | None:
    import ssl
    import urllib.request

    source_sha = PROD_SHA_PATH.read_text().strip() if PROD_SHA_PATH.exists() else ""
    if not source_sha:
        return None
    url = _prod_local_health_url(env)
    context = ssl._create_unverified_context() if tls_enabled_from_env(env) else None
    try:
        with urllib.request.urlopen(url, timeout=2, context=context) as response:
            version = _parse_legacy_version_body(response.read().decode())
        if version is None:
            return None
        return RuntimeIdentity(version=version, git_sha=source_sha), url, tls_enabled_from_env(env)
    except Exception:
        return None


def _current_prod_identity(env: dict[str, str]) -> RuntimeIdentity | None:
    import ssl
    import urllib.request
    try:
        url, insecure_tls = _launchd_health_probe(env)
        context = ssl._create_unverified_context() if insecure_tls else None
        with urllib.request.urlopen(url, timeout=2, context=context) as response:
            value = json.load(response)
        identity = RuntimeIdentity(version=str(value["version"]), git_sha=str(value["git_sha"]))
        return identity if identity.is_exact() else None
    except Exception:
        return None


def _resolve_rollback_identity(
    rollback_binary: Path,
    previous_env: dict[str, str],
) -> tuple[RuntimeIdentity, str, bool, bool]:
    previous_identity = _current_prod_identity(previous_env)
    previous_health_url, previous_health_insecure_tls = _launchd_health_probe(previous_env)
    previous_health_json = previous_identity is not None
    rollback_identity = None
    try:
        rollback_identity = _binary_identity(rollback_binary)
    except (OSError, subprocess.SubprocessError, SystemExit):
        if previous_identity is not None:
            raise SystemExit(
                "installed production identity is unavailable; refusing an unverifiable rollback"
            )
    if previous_identity is None:
        legacy = _legacy_prod_identity(previous_env)
        if legacy is not None:
            previous_identity, previous_health_url, previous_health_insecure_tls = legacy
            previous_health_json = False
        elif rollback_identity is not None:
            previous_identity = rollback_identity
            previous_health_json = True
        else:
            raise SystemExit(
                "installed production identity is unavailable; refusing an unverifiable rollback"
            )
    if rollback_identity is not None and not _rollback_identity_matches(
        rollback_identity,
        previous_identity,
        previous_health_json,
    ):
        raise SystemExit(
            "staged rollback binary identity does not exactly match the previous production identity"
        )
    return (
        previous_identity,
        previous_health_url,
        previous_health_insecure_tls,
        previous_health_json,
    )


def _linux_musl_target() -> str:
    import platform

    architecture = {
        "arm64": "aarch64",
        "aarch64": "aarch64",
        "x86_64": "x86_64",
        "amd64": "x86_64",
    }.get(platform.machine().lower())
    if architecture is None:
        raise SystemExit(f"no supported Linux musl target for architecture {platform.machine()!r}")
    return f"{architecture}-unknown-linux-musl"


def _release_asset_name() -> str:
    import platform

    machine = platform.machine().lower()
    architecture = {
        "arm64": "aarch64",
        "aarch64": "aarch64",
        "x86_64": "x86_64",
        "amd64": "x86_64",
    }.get(machine)
    platform_target = {
        "darwin": "apple-darwin",
        "linux": "unknown-linux-musl",
    }.get(sys.platform)
    if architecture is None or platform_target is None:
        raise SystemExit(
            f"no published release asset for host platform {sys.platform!r} architecture {machine!r}"
        )
    return f"phoenix_ide-{architecture}-{platform_target}"


def _prepare_release_candidate(
    requested: str,
    staging: Path,
    *,
    expected_full_commit: str | None = None,
    expected_asset_name: str | None = None,
    expected_asset_sha256: str | None = None,
) -> PreparedCandidate:
    if expected_full_commit is not None and requested == "latest":
        raise SystemExit("controller mode requires an exact release tag, not 'latest'")
    if requested == "latest":
        view = subprocess.run(
            ["gh", "release", "view", "--repo", "scottopell/phoenix-ide", "--json", "tagName,isPrerelease"],
            capture_output=True, text=True, check=True,
        )
    else:
        view = subprocess.run(
            ["gh", "release", "view", requested, "--repo", "scottopell/phoenix-ide", "--json", "tagName,isPrerelease"],
            capture_output=True, text=True, check=True,
        )
    release = json.loads(view.stdout)
    tag = release["tagName"]
    if requested == "latest" and release.get("isPrerelease"):
        raise SystemExit("latest resolved to a prerelease; name an exact prerelease tag to opt in")
    if requested != "latest" and tag != requested:
        raise SystemExit(f"release resolution mismatch: requested {requested}, resolved {tag}")

    if expected_full_commit is None:
        commit_result = subprocess.run(
            ["gh", "api", f"repos/scottopell/phoenix-ide/commits/{tag}", "--jq", ".sha"],
            capture_output=True, text=True, check=True,
        )
        release_commit = commit_result.stdout.strip()
    else:
        release_commit = expected_full_commit
    if not re.fullmatch(r"[0-9a-f]{40}", release_commit):
        raise SystemExit(f"release {tag} did not resolve to an immutable commit SHA")
    if expected_full_commit is not None:
        tagged_commit_result = subprocess.run(
            ["gh", "api", f"repos/scottopell/phoenix-ide/commits/{tag}", "--jq", ".sha"],
            capture_output=True, text=True, check=True,
        )
        tagged_commit = tagged_commit_result.stdout.strip()
        if tagged_commit != expected_full_commit:
            raise SystemExit(
                f"release {tag} resolved to {tagged_commit}, expected exact commit {expected_full_commit}"
            )

    asset_name = _release_asset_name()
    if expected_asset_name is not None and asset_name != expected_asset_name:
        raise SystemExit(
            f"release asset selection changed: expected {expected_asset_name}, selected {asset_name}"
        )
    subprocess.run(
        ["gh", "release", "download", tag, "--repo", "scottopell/phoenix-ide", "--dir", str(staging),
         "--pattern", asset_name, "--pattern", "SHA256SUMS"],
        check=True,
    )
    binary = staging / asset_name
    sums = staging / "SHA256SUMS"
    if not binary.is_file() or not sums.is_file():
        raise SystemExit(f"release {tag} is missing {asset_name} or SHA256SUMS")
    entries = {}
    for line in sums.read_text().splitlines():
        fields = line.split()
        if len(fields) == 2:
            entries[fields[1].lstrip("*")] = fields[0]
    expected = entries.get(asset_name)
    if not expected:
        raise SystemExit(f"SHA256SUMS has no entry for {asset_name}")
    if expected_asset_sha256 is not None and expected.lower() != expected_asset_sha256:
        raise SystemExit(
            f"release checksum changed: expected {expected_asset_sha256}, published {expected.lower()}"
        )
    if _file_sha256(binary) != expected.lower():
        raise SystemExit(f"checksum mismatch for release asset {asset_name}")
    binary.chmod(0o755)
    identity = RuntimeIdentity.from_value(_binary_identity(binary))
    expected_version = tag.removeprefix("v")
    if identity.version != expected_version:
        raise SystemExit(
            f"release {tag} embeds version {identity.version}, expected {expected_version}"
        )
    if identity.git_sha.endswith("-dirty"):
        raise SystemExit(f"release {tag} asset embeds a dirty git identity")
    if not re.fullmatch(r"[0-9a-f]{12}", identity.git_sha):
        raise SystemExit(
            f"release {tag} asset embeds malformed git identity {identity.git_sha!r}; expected 12 lowercase hex characters"
        )
    if not release_commit.startswith(identity.git_sha):
        raise SystemExit(
            f"release {tag} resolves to {release_commit}, but the asset embeds {identity.git_sha}"
        )
    return PreparedCandidate(
        binary=binary,
        source_kind=ProdSourceKind.PUBLISHED_RELEASE,
        source_commit=release_commit,
        identity=identity,
        release_tag=tag,
        release_commit=release_commit,
    )


def _prepare_local_candidate(*, target: str | None) -> PreparedCandidate:
    binary = prod_build(target=target)
    source_commit = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.strip()
    identity = RuntimeIdentity.from_value(_binary_identity(binary))
    if not source_commit.startswith(identity.git_sha.removesuffix("-dirty")):
        raise SystemExit(
            f"local candidate identity {identity.git_sha} does not match selected HEAD {source_commit[:12]}"
        )
    return PreparedCandidate(
        binary=binary,
        source_kind=ProdSourceKind.LOCAL_HEAD,
        source_commit=source_commit,
        identity=identity,
    )


_DEPLOY_TERMINAL_STATES = {
    "committed", "precondition_failed", "activation_failed_rolled_back",
    "activation_failed_rollback_failed", "rejected_concurrent",
}


@contextlib.contextmanager
def _launchd_claim_lock():
    import fcntl

    LAUNCHD_DEPLOY_DIR.mkdir(parents=True, exist_ok=True, mode=0o700)
    with LAUNCHD_DEPLOY_CLAIM_LOCK_PATH.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def _deploy_claim_owner() -> str | None:
    try:
        return LAUNCHD_DEPLOY_ACTIVE_PATH.read_text().strip() or None
    except OSError:
        return None


def _release_launchd_deploy_claim_unlocked(transaction_id: str) -> bool:
    if _deploy_claim_owner() != transaction_id:
        return False
    try:
        LAUNCHD_DEPLOY_ACTIVE_PATH.unlink()
        return True
    except FileNotFoundError:
        return False


def _release_launchd_deploy_claim(transaction_id: str) -> bool:
    with _launchd_claim_lock():
        return _release_launchd_deploy_claim_unlocked(transaction_id)


def _claim_launchd_deploy(transaction_id: str) -> None:
    LAUNCHD_DEPLOY_DIR.mkdir(parents=True, exist_ok=True, mode=0o700)
    LAUNCHD_DEPLOY_DIR.chmod(0o700)
    with _launchd_claim_lock():
        owner = _deploy_claim_owner()
        if owner and LAUNCHD_DEPLOY_STATUS_PATH.exists():
            try:
                status = json.loads(LAUNCHD_DEPLOY_STATUS_PATH.read_text())
                if status.get("transaction_id") == owner and status.get("state") in _DEPLOY_TERMINAL_STATES:
                    _release_launchd_deploy_claim_unlocked(owner)
                    owner = None
            except (OSError, json.JSONDecodeError):
                pass
        if owner is not None or LAUNCHD_DEPLOY_ACTIVE_PATH.exists():
            raise SystemExit(
                f"another launchd deployment ({owner or 'unknown'}) is active or needs recovery. "
                "Run './dev.py prod status'; remove the active marker only after confirming no helper is running."
            )
        fd = os.open(LAUNCHD_DEPLOY_ACTIVE_PATH, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as stream:
            stream.write(transaction_id + "\n")
            stream.flush()
            os.fsync(stream.fileno())


def _write_json_atomic(path: Path, value: dict, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def _materialize_source_file(
    source_commit: str,
    source_path: str,
    destination: Path,
    source_kind: str,
) -> None:
    if source_kind == "local_head":
        result = subprocess.run(
            ["git", "show", f"{source_commit}:{source_path}"],
            cwd=ROOT, capture_output=True,
        )
    else:
        result = subprocess.run(
            ["gh", "api", f"repos/scottopell/phoenix-ide/contents/{source_path}?ref={source_commit}",
             "-H", "Accept: application/vnd.github.raw+json"],
            capture_output=True,
        )
    if result.returncode != 0 or not result.stdout:
        raise SystemExit(f"selected source {source_commit} has no {source_path}")
    destination.write_bytes(result.stdout)
    destination.chmod(0o700)


def _materialize_helper(source_commit: str, destination: Path, source_kind: str) -> None:
    _materialize_source_file(
        source_commit,
        "scripts/launchd_deploy_helper.py",
        destination,
        source_kind,
    )


def _helper_plist(
    label: str,
    helper: Path,
    manifest: Path,
    log_path: Path,
    python_executable: Path,
) -> bytes:
    return plistlib.dumps({
        "Label": label,
        "ProgramArguments": [
            str(python_executable), str(helper), "activate", "--manifest", str(manifest),
            "--helper-label", label, "--uid", str(os.getuid()),
        ],
        "RunAtLoad": True,
        "StandardOutPath": str(log_path),
        "StandardErrorPath": str(log_path),
    }, fmt=plistlib.FMT_XML)


def _report_launchd_handoff(transaction_id: str, identity: RuntimeIdentity) -> None:
    try:
        print("\n✓ Activation handed to an independent launchd helper")
        print(f"  Transaction: {transaction_id}")
        print(f"  Candidate: {identity.version} ({identity.git_sha})")
        print("  The Phoenix connection may close while the service is replaced.")
        print("  After reconnecting, run: ./dev.py prod status")
    except BrokenPipeError:
        pass


def _launchd_candidate_env(controller: "ProdDeployControllerOptions | None" = None) -> tuple[dict[str, str], Path | None]:
    env: dict[str, str] = {}
    if controller is not None and controller.enabled:
        try:
            return _launchd_env_from_plist(LAUNCHD_PLIST_PATH), None
        except (OSError, plistlib.InvalidFileException, ValueError) as exc:
            raise SystemExit(
                f"installed launchd configuration is unreadable: {type(exc).__name__}"
            ) from exc
    env_file = _load_env_file(env)
    return env, env_file


def launchd_prod_deploy(
    release: str | None = None,
    *,
    controller: "ProdDeployControllerOptions | None" = None,
):
    """Prepare a candidate, then hand transactional activation to launchd."""
    import uuid

    controller = controller or ProdDeployControllerOptions()
    if controller.enabled:
        release, _expected_full_commit = controller.require_exact_release(release)
    launchd_env, _env_file = _launchd_candidate_env(controller)
    _preflight_prod_bind_auth(launchd_env, socket_activated=True)

    transaction_id = controller.transaction_id or f"{datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')}-{uuid.uuid4().hex[:8]}"
    _claim_launchd_deploy(transaction_id)
    claimed_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
    staging = LAUNCHD_DEPLOY_DIR / "transactions" / transaction_id
    try:
        _write_json_atomic(LAUNCHD_DEPLOY_STATUS_PATH, {
            "transaction_id": transaction_id, "state": "preparing", "source_kind": None,
            "source_commit": None, "release_commit": None, "release_tag": release,
            "expected_version": None, "expected_git_sha": None,
            "created_at": claimed_at, "updated_at": claimed_at,
            "failure": None, "rollback_failure": None,
        })
        transactions_dir = LAUNCHD_DEPLOY_DIR / "transactions"
        transactions_dir.mkdir(parents=True, exist_ok=True)
        old_transactions = sorted(
            (path for path in transactions_dir.iterdir() if path.is_dir()),
            key=lambda path: path.stat().st_mtime,
            reverse=True,
        )
        for old in old_transactions[5:]:
            shutil.rmtree(old, ignore_errors=True)
        staging.mkdir(parents=True)
        staging.chmod(0o700)
        prepared = (
            _prepare_release_candidate(release, staging, expected_full_commit=controller.expected_full_commit, expected_asset_name=controller.expected_asset_name, expected_asset_sha256=controller.expected_asset_sha256)
            if release
            else _prepare_local_candidate(target=None)
        )
        binary = prepared.binary
        release_tag = prepared.release_tag
        source_commit = prepared.source_commit
        source_kind = prepared.source_kind.value
        release_commit = prepared.release_commit

        candidate_binary = staging / "candidate-phoenix-ide"
        if binary != candidate_binary:
            shutil.copy2(binary, candidate_binary)
        candidate_binary.chmod(0o755)
        subprocess.run(
            ["codesign", "--force", "--sign", "-", "--identifier", LAUNCHD_LABEL, str(candidate_binary)],
            check=True,
        )
        subprocess.run(["codesign", "--verify", "--strict", str(candidate_binary)], check=True)
        identity = RuntimeIdentity.from_value(_binary_identity(candidate_binary))
        if (
            identity.version != prepared.identity.version
            or identity.git_sha != prepared.identity.git_sha
        ):
            raise SystemExit("staged candidate identity changed after signing")

        env_overrides = dict(launchd_env)
        env_file = _env_file
        if env_file:
            print(f"  Loaded env from {env_file}")
        path_str, path_source = capture_login_shell_path()
        print_launchd_path_report(path_str, path_source)
        plist_content = generate_launchd_plist(identity.version, extra_env=env_overrides, path_override=path_str)
        candidate_plist = staging / "candidate.plist"
        candidate_plist.write_text(plist_content)
        candidate_plist.chmod(0o600)
        subprocess.run(["plutil", "-lint", str(candidate_plist)], check=True, capture_output=True)

        rollback_binary = staging / "rollback-phoenix-ide"
        rollback_plist = staging / "rollback.plist"
        target_binary = LAUNCHD_INSTALL_DIR / "phoenix-ide"
        if target_binary.exists() != LAUNCHD_PLIST_PATH.exists():
            raise SystemExit("production binary/plist rollback inputs are incomplete; refusing disruption")
        if target_binary.exists():
            shutil.copy2(target_binary, rollback_binary)
            shutil.copy2(LAUNCHD_PLIST_PATH, rollback_plist)
            rollback_plist.chmod(0o600)
        previous_env = _launchd_env_from_plist(rollback_plist) if rollback_plist.exists() else {}
        previous_identity = None
        previous_health_url, previous_health_insecure_tls = _launchd_health_probe(previous_env)
        previous_health_json = False
        if target_binary.exists():
            (
                previous_identity,
                previous_health_url,
                previous_health_insecure_tls,
                previous_health_json,
            ) = _resolve_rollback_identity(rollback_binary, previous_env)

        helper = staging / "activate.py"
        _materialize_helper(source_commit, helper, source_kind)
        python_executable = Path(sys.executable).resolve()
        protocol = subprocess.run(
            [str(python_executable), str(helper), "--protocol-version"],
            capture_output=True, text=True,
        )
        if protocol.returncode != 0 or protocol.stdout.strip() != str(LAUNCHD_HANDOFF_PROTOCOL_VERSION):
            raise SystemExit(
                f"selected helper uses incompatible handoff protocol {protocol.stdout.strip()!r}; "
                f"expected {LAUNCHD_HANDOFF_PROTOCOL_VERSION}"
            )
        interpreter_check = subprocess.run(
            [str(python_executable), "-c", "import fcntl, plistlib, ssl, urllib.request"],
            capture_output=True,
        )
        if interpreter_check.returncode != 0:
            raise SystemExit(f"active Python interpreter cannot run the launchd helper: {python_executable}")
        helper_log = LAUNCHD_DEPLOY_DIR / "activation.log"
        helper_label = f"{LAUNCHD_DEPLOY_HELPER_PREFIX}.{transaction_id}"
        helper_plist = staging / "helper.plist"
        helper_plist.write_bytes(
            _helper_plist(
                helper_label,
                helper,
                staging / "manifest.json",
                helper_log,
                python_executable,
            )
        )
        helper_plist.chmod(0o600)
        subprocess.run(["plutil", "-lint", str(helper_plist)], check=True, capture_output=True)

        health_url, health_insecure_tls = _launchd_health_probe(env_overrides)
        previous_deployed_sha = PROD_SHA_PATH.read_text().strip() if PROD_SHA_PATH.exists() else None
        manifest = {
            "manifest_version": LAUNCHD_HANDOFF_PROTOCOL_VERSION,
            "transaction_id": transaction_id,
            "source_kind": source_kind,
            "source_commit": source_commit,
            "release_tag": release_tag,
            "release_commit": release_commit,
            "expected": identity.as_dict(),
            "previous": previous_identity.as_dict() if previous_identity is not None else None,
            "previous_deployed_sha": previous_deployed_sha,
            "candidate_binary": str(candidate_binary),
            "candidate_binary_sha256": _file_sha256(candidate_binary),
            "candidate_plist": str(candidate_plist),
            "candidate_plist_sha256": _file_sha256(candidate_plist),
            "rollback_binary": str(rollback_binary) if rollback_binary.exists() else None,
            "rollback_binary_sha256": _file_sha256(rollback_binary) if rollback_binary.exists() else None,
            "rollback_plist": str(rollback_plist) if rollback_plist.exists() else None,
            "rollback_plist_sha256": _file_sha256(rollback_plist) if rollback_plist.exists() else None,
            "target_binary": str(target_binary),
            "target_plist": str(LAUNCHD_PLIST_PATH),
            "label": LAUNCHD_LABEL,
            "helper_label": helper_label,
            "uid": os.getuid(),
            "health_url": health_url,
            "health_insecure_tls": health_insecure_tls,
            "previous_health_url": previous_health_url if previous_identity is not None else None,
            "previous_health_insecure_tls": previous_health_insecure_tls if previous_identity is not None else None,
            "previous_health_json": previous_health_json if previous_identity is not None else None,
            "active_path": str(LAUNCHD_DEPLOY_ACTIVE_PATH),
            "status_path": str(LAUNCHD_DEPLOY_STATUS_PATH),
            "deployed_sha_path": str(PROD_SHA_PATH),
            "lock_path": str(LAUNCHD_DEPLOY_LOCK_PATH),
            "claim_lock_path": str(LAUNCHD_DEPLOY_CLAIM_LOCK_PATH),
            "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        }
        _write_json_atomic(staging / "manifest.json", manifest)
        (staging / "manifest.json").chmod(0o400)
        _write_json_atomic(LAUNCHD_DEPLOY_STATUS_PATH, {
            "transaction_id": transaction_id, "state": "prepared", "source_kind": source_kind,
            "source_commit": source_commit, "release_commit": release_commit, "release_tag": release_tag,
            "expected_version": identity.version, "expected_git_sha": identity.git_sha,
            "created_at": manifest["created_at"], "updated_at": manifest["created_at"],
            "failure": None, "rollback_failure": None,
        })
        _ensure_newsyslog_config()
        result = subprocess.run(
            ["launchctl", "bootstrap", f"gui/{os.getuid()}", str(helper_plist)],
            capture_output=True, text=True,
        )
        if result.returncode != 0:
            raise SystemExit(f"could not hand activation to launchd (exit {result.returncode})")
    except BaseException as exc:
        failed_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
        try:
            _write_json_atomic(LAUNCHD_DEPLOY_STATUS_PATH, {
                "transaction_id": transaction_id, "state": "precondition_failed",
                "source_kind": locals().get("source_kind"), "source_commit": locals().get("source_commit"),
                "release_commit": locals().get("release_commit"), "release_tag": locals().get("release_tag", release),
                "expected_version": locals().get("identity", {}).get("version"),
                "expected_git_sha": locals().get("identity", {}).get("git_sha"),
                "created_at": claimed_at, "updated_at": failed_at,
                "failure": f"{type(exc).__name__}: preparation failed before handoff",
                "rollback_failure": None,
            })
        finally:
            _release_launchd_deploy_claim(transaction_id)
        raise
    _report_launchd_handoff(transaction_id, identity)



def _print_launchd_deploy_status() -> None:
    if not LAUNCHD_DEPLOY_STATUS_PATH.exists():
        return
    try:
        deploy = json.loads(LAUNCHD_DEPLOY_STATUS_PATH.read_text())
        print(f"  Last deploy: {deploy.get('state', 'unknown')} ({deploy.get('transaction_id', 'unknown')})")
        source = deploy.get("source_kind") or "unknown"
        if deploy.get("release_tag"):
            source += f" {deploy['release_tag']}"
        if deploy.get("release_commit"):
            source += f" ({deploy['release_commit'][:12]})"
        print(f"    Source: {source}")
        print(f"    Expected: {deploy.get('expected_version', 'unknown')} ({deploy.get('expected_git_sha', 'unknown')})")
        print(f"    Updated: {deploy.get('updated_at', 'unknown')}")
        if deploy.get("failure"):
            print(f"    Failure: {deploy['failure']}")
        if deploy.get("rollback_failure"):
            print(f"    Rollback failure: {deploy['rollback_failure']}")
        if deploy.get("state") in {"preparing", "prepared", "activating"}:
            age = datetime.datetime.now(datetime.timezone.utc) - datetime.datetime.fromisoformat(deploy["updated_at"])
            if age.total_seconds() > 120:
                print("    STALE: inspect ~/.phoenix-ide/deploy/activation.log and confirm no helper is running before clearing the active marker")
    except Exception as exc:
        print(f"  Last deploy: unreadable status ({type(exc).__name__})")


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
        _print_launchd_deploy_status()
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

    try:
        status_env = _launchd_env_from_plist(LAUNCHD_PLIST_PATH)
    except (OSError, plistlib.InvalidFileException, ValueError) as exc:
        status_env = {}
        print(f"  Config: unreadable launchd plist ({type(exc).__name__})")
    identity = _current_prod_identity(status_env)
    if identity is None:
        legacy = _legacy_prod_identity(status_env)
        identity = legacy[0] if legacy is not None else None
    if identity:
        print(f"  Version: {identity.version} ({identity.git_sha})")
    else:
        print("  Health: not responding")

    _print_launchd_deploy_status()

    if sha := read_deployed_sha():
        print(f"  Commit: {sha}")
    print(f"  Port: {status_env.get('PHOENIX_PORT', str(PROD_PORT))}")
    print(f"  Database: {PROD_DB_PATH}")
    print(f"  Logs: {LAUNCHD_LOG_PATH}")
    print(f"  URL: {_prod_display_url(status_env)}")


def launchd_prod_stop():
    """Stop the launchd service."""
    _launchd_stop_if_loaded()
    print(f"Stopped {LAUNCHD_LABEL}")


def cmd_prod_build():
    """Build the production binary from local HEAD."""
    if sys.platform == "darwin":
        prod_build(target=None)
    elif sys.platform == "linux":
        prod_build()
    else:
        print(f"Unsupported platform: {sys.platform}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_deploy(
    release: str | None = None,
    pretty: bool = False,
    *,
    controller: "ProdDeployControllerOptions | None" = None,
):
    """Deploy local HEAD or an immutable published release."""
    controller = controller or ProdDeployControllerOptions()
    env = detect_prod_env()
    if controller.enabled and not release:
        raise SystemExit("controller mode requires --release")
    if not release:
        print("Running pre-deploy checks...\n")
        cmd_check(gate=False, pretty=pretty)
        print()

    if env == "launchd":
        if controller.enabled:
            launchd_prod_deploy(release, controller=controller)
        else:
            launchd_prod_deploy(release)

    elif env == "native":
        if controller.enabled:
            native_prod_deploy(release, controller=controller)
        else:
            native_prod_deploy(release)

    elif env == "daemon":
        print("Detected: Bare Linux (persistent supervisor mode)")
        print("    Deploying through the same-user Phoenix supervisor")
        print()
        if controller.enabled:
            prod_daemon_deploy(release, controller=controller)
        else:
            prod_daemon_deploy(release)

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


def _reject_prod_override_command() -> None:
    raise SystemExit(
        "prod set/unset no longer mutate production configuration; "
        "edit .phoenix-ide.env directly, then run './dev.py prod deploy'"
    )


def cmd_prod_override_set(_name: str, _value: str):
    """Reject legacy override mutation in favor of the environment file."""
    _reject_prod_override_command()


def cmd_prod_override_unset(_name: str):
    """Reject legacy override mutation in favor of the environment file."""
    _reject_prod_override_command()


# =============================================================================
# Main
# =============================================================================

def _bootstrap_rich() -> None:
    """Make rich importable for --pretty without it being a standing dep.

    rich is deliberately absent from this script's PEP 723 dependencies so
    the default path never downloads it. When --pretty is requested and rich
    isn't importable, re-exec through `uv run --with rich` (uv caches the
    resulting environment, so this costs one resolve on first use). If the
    bootstrap already ran and rich still isn't importable, fall through —
    _make_reporter degrades to plain output with a notice.
    """
    try:
        import rich  # noqa: F401
        return
    except ImportError:
        pass
    if os.environ.get("_PHOENIX_PRETTY_BOOTSTRAP") == "1":
        return
    env = dict(os.environ, _PHOENIX_PRETTY_BOOTSTRAP="1")
    os.execvpe(
        "uv",
        ["uv", "run", "--with", "rich>=13,<15", str(Path(__file__).resolve()),
         *sys.argv[1:]],
        env,
    )


def main():
    # Verbatim passthrough commands are intercepted before argparse so their
    # flags (especially --help) reach the underlying CLI unchanged.
    if len(sys.argv) >= 2 and sys.argv[1] == "taskmd":
        cmd_taskmd(sys.argv[2:])
        return
    if len(sys.argv) >= 2 and sys.argv[1] == "drive-turn":
        cmd_drive_turn(sys.argv[2:])
        return

    parser = argparse.ArgumentParser(prog="dev.py", description="Phoenix development tasks")
    parser.add_argument(
        "--pretty", dest="pretty_global", action="store_true", default=False,
        help="Render long-running commands (check, prod deploy) as a live lane table",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    # up
    up_parser = sub.add_parser("up", help="Build and start servers")
    up_parser.add_argument("--port", type=int, default=None, help="Phoenix port (default: auto from worktree hash)")
    up_parser.add_argument("--vite-port", type=int, default=None, help="Vite port (default: auto from worktree hash)")
    up_parser.add_argument("--no-seed", action="store_true", default=False, help="Skip auto-seeding the dev DB on startup")
    up_parser.add_argument("--https", dest="https", action="store_true", help="Serve dev over auto-managed HTTPS/HTTP2 (default)")
    up_parser.add_argument("--no-https", dest="https", action="store_false", help="Serve dev over plain HTTP/1.1")
    up_parser.add_argument("--tls", dest="https", action="store_true", help=argparse.SUPPRESS)
    up_parser.add_argument("--no-tls", dest="https", action="store_false", help=argparse.SUPPRESS)
    up_parser.set_defaults(https=True)

    # down
    sub.add_parser("down", help="Stop all servers")

    reap_parser = sub.add_parser("reap", help="Kill dev servers orphaned by deleted worktrees")
    reap_parser.add_argument("--dry-run", action="store_true", help="List what would be reaped without killing anything")

    # restart
    restart_parser = sub.add_parser("restart", help="Rebuild Rust and restart Phoenix")
    restart_parser.add_argument("--port", type=int, default=None, help="Phoenix port (default: auto from worktree hash)")
    restart_parser.add_argument("--https", dest="https", action="store_true", help="Serve dev over auto-managed HTTPS/HTTP2 (default)")
    restart_parser.add_argument("--no-https", dest="https", action="store_false", help="Serve dev over plain HTTP/1.1")
    restart_parser.add_argument("--tls", dest="https", action="store_true", help=argparse.SUPPRESS)
    restart_parser.add_argument("--no-tls", dest="https", action="store_false", help=argparse.SUPPRESS)
    restart_parser.set_defaults(https=True)

    # status
    sub.add_parser("status", help="Check what's running")

    # check
    check_parser = sub.add_parser("check", help="Run lint, fmt check, and tests")
    check_parser.add_argument(
        "--all", dest="check_all", action="store_true", default=False,
        help="Run every lane, disabling incremental path-gating (also: PHOENIX_CHECK_ALL=1)",
    )
    check_parser.add_argument(
        "--lanes", default=None, metavar="A,B,...",
        help="Run only this comma-separated lane subset (CI splits the check "
             "across runners with it); gating still applies within the subset",
    )
    check_parser.add_argument(
        "--pretty", action="store_true", default=False,
        help="Render lanes as a live table instead of line-per-step output",
    )

    check_plan_parser = sub.add_parser(
        "check-plan",
        help="Print the incremental check lane/group plan without running lanes",
    )
    check_plan_parser.add_argument(
        "--all", dest="check_all", action="store_true", default=False,
        help="Plan every lane, disabling incremental path-gating (also: PHOENIX_CHECK_ALL=1)",
    )
    check_plan_parser.add_argument(
        "--lanes", default=None, metavar="A,B,...",
        help="Restrict the plan to this comma-separated lane subset",
    )
    check_plan_parser.add_argument(
        "--format", choices=("json", "github-env"), default="json",
        help="Output format (default: json)",
    )

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

    # graph
    graph_parser = sub.add_parser(
        "graph",
        help="Emit an HTML view of the project build graph (topologically layered)",
    )
    graph_parser.add_argument(
        "--format", choices=("html", "json"), default="html",
        help="Output format. json is minified, LLM/pipe-friendly, and prints "
             "to stdout unless --out is given (default: html)",
    )
    graph_parser.add_argument(
        "--out", type=Path, default=None,
        help="Output path (default: target/build-graph.{html,json}; json "
             "without --out prints to stdout)",
    )
    graph_parser.add_argument(
        "--serve", action="store_true",
        help="Host the graph over HTTP on localhost instead of writing a file",
    )
    graph_parser.add_argument(
        "--port", type=int, default=None,
        help="Port for --serve (default: ephemeral)",
    )
    graph_parser.add_argument(
        "--open", dest="open_browser", action="store_true",
        help="Open the result in a browser",
    )
    graph_parser.add_argument(
        "--timings", action="store_true",
        help="Measure real per-crate compile time (recompiles the workspace "
             "with cargo --timings; slow). Default weight is lines of code.",
    )

    # seed (offline)
    sub.add_parser("seed", help="Populate dev DB with representative conversations (offline; refuses if Phoenix is running)")

    # qa
    qa_parser = sub.add_parser("qa", help="Run local QA capture workflows")
    qa_sub = qa_parser.add_subparsers(dest="qa_command", required=True)
    qa_sub.add_parser("grounding-panel", help="Capture grounding panel Ladle screenshots")
    qa_sub.add_parser("meta-viewer", help="Capture MetaViewer edge-state Ladle screenshots")
    qa_sub.add_parser("conversation-panel", help="Capture conversation side panel Ladle screenshots")
    qa_sub.add_parser("sidebar", help="Capture sidebar Ladle screenshots")
    qa_sub.add_parser("task-approval", help="Capture task approval Ladle screenshots")
    qa_sub.add_parser("mobile-conversation-list", help="Capture mobile conversation list Ladle screenshots")
    qa_sub.add_parser("coordinator", help="Capture Coordinator screenshots across responsive viewports")
    qa_sub.add_parser("mobile-multi-pr-conversation", help="Capture a mobile conversation with two open PRs")
    qa_sub.add_parser("desktop-multi-pr-conversation", help="Capture a desktop conversation with multiple open PRs")
    qa_sub.add_parser("new-conversation", help="Capture the /new page at desktop and mobile sizes")
    qa_sub.add_parser("message-list", help="Capture message list Ladle screenshots")
    qa_sub.add_parser("tool-results", help="Capture tool-result Ladle screenshots at desktop and mobile sizes")
    qa_sub.add_parser("work-actions", help="Capture Work Actions Ladle screenshots")
    qa_sub.add_parser("commission-review", help="Capture commission review Ladle screenshots at desktop and mobile sizes")

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
    prod_sub.add_parser("build", help="Build production binary from local HEAD")
    deploy_parser = prod_sub.add_parser(
        "deploy",
        help="Deploy local HEAD, or use --release TAG|latest for a published macOS release",
        epilog="Positional versions were removed; use --release vX.Y.Z (there is no local build-from-tag mode).",
    )
    deploy_parser.add_argument(
        "--release", metavar="TAG|latest",
        help="Install an immutable published GitHub release without checks or compilation",
    )
    deploy_parser.add_argument("legacy_version", nargs="?", help=argparse.SUPPRESS)
    deploy_parser.add_argument(
        "--pretty", action="store_true", default=False,
        help="Render the pre-deploy check as a live lane table",
    )
    deploy_parser.add_argument("--controller-mode", action="store_true", help=argparse.SUPPRESS)
    deploy_parser.add_argument("--controller-release-tag", help=argparse.SUPPRESS)
    deploy_parser.add_argument("--controller-expected-full-commit", help=argparse.SUPPRESS)
    deploy_parser.add_argument("--controller-expected-asset-name", help=argparse.SUPPRESS)
    deploy_parser.add_argument("--controller-expected-asset-sha256", help=argparse.SUPPRESS)
    deploy_parser.add_argument("--transaction-id", help=argparse.SUPPRESS)
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

    # drive-turn — verbatim passthrough, intercepted before argparse.
    sub.add_parser(
        "drive-turn",
        help="Build and run one production-runtime turn with the development env",
        add_help=False,
    )

    # taskmd — passthrough to the bundled taskmd CLI (intercepted above before
    # argparse runs; this entry exists only so it shows up in `./dev.py --help`)
    sub.add_parser(
        "taskmd",
        help="Run the bundled taskmd CLI; all args forwarded (e.g. ./dev.py taskmd new --slug fix-x --priority p1)",
        add_help=False,
    )

    args = parser.parse_args()

    # --pretty composes from the global flag and the per-subcommand flag.
    pretty = args.pretty_global or getattr(args, "pretty", False)
    if pretty:
        _bootstrap_rich()

    if args.command == "up":
        cmd_up(
            phoenix_port=args.port,
            vite_port=args.vite_port,
            no_seed=args.no_seed,
            tls=args.https,
        )
    elif args.command == "down":
        cmd_down()
    elif args.command == "reap":
        cmd_reap(dry_run=args.dry_run)
    elif args.command == "restart":
        cmd_restart(phoenix_port=args.port, tls=args.https)
    elif args.command == "status":
        cmd_status()
    elif args.command == "check":
        cmd_check(gate=not args.check_all, lanes=args.lanes, pretty=pretty)
    elif args.command == "check-plan":
        cmd_check_plan(gate=not args.check_all, lanes=args.lanes, fmt=args.format)
    elif args.command == "codegen":
        if not cmd_codegen():
            sys.exit(1)
    elif args.command == "graph":
        cmd_graph(
            out=args.out, serve=args.serve, port=args.port,
            open_browser=args.open_browser, timings=args.timings,
            fmt=args.format,
        )
    elif args.command == "seed":
        cmd_seed()
    elif args.command == "qa":
        if args.qa_command == "grounding-panel":
            cmd_qa_grounding_panel()
        elif args.qa_command == "meta-viewer":
            cmd_qa_meta_viewer()
        elif args.qa_command == "conversation-panel":
            cmd_qa_conversation_panel()
        elif args.qa_command == "sidebar":
            cmd_qa_sidebar()
        elif args.qa_command == "task-approval":
            cmd_qa_task_approval()
        elif args.qa_command == "mobile-conversation-list":
            cmd_qa_mobile_conversation_list()
        elif args.qa_command == "coordinator":
            cmd_qa_coordinator()
        elif args.qa_command == "mobile-multi-pr-conversation":
            cmd_qa_mobile_multi_pr_conversation()
        elif args.qa_command == "desktop-multi-pr-conversation":
            cmd_qa_desktop_multi_pr_conversation()
        elif args.qa_command == "new-conversation":
            cmd_qa_new_conversation()
        elif args.qa_command == "message-list":
            cmd_qa_message_list()
        elif args.qa_command == "tool-results":
            cmd_qa_tool_results()
        elif args.qa_command == "work-actions":
            cmd_qa_work_actions()
        elif args.qa_command == "commission-review":
            cmd_qa_commission_review()
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
            cmd_prod_build()
        elif args.prod_command == "deploy":
            if args.legacy_version:
                parser.error(
                    f"positional deploy version {args.legacy_version!r} was removed; "
                    "use 'prod deploy --release vX.Y.Z' (local build-from-tag no longer exists)"
                )
            cmd_prod_deploy(
                args.release,
                pretty=pretty,
                controller=ProdDeployControllerOptions(
                    enabled=args.controller_mode,
                    exact_release_tag=args.controller_release_tag,
                    expected_full_commit=args.controller_expected_full_commit,
                    expected_asset_name=args.controller_expected_asset_name,
                    expected_asset_sha256=args.controller_expected_asset_sha256,
                    transaction_id=args.transaction_id,
                ),
            )
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

