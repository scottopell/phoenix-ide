#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# ///
"""Development tasks for phoenix-ide."""

import argparse
import dataclasses
import fcntl
import getpass
import hashlib
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).parent.resolve()

UI_DIR = ROOT / "ui"
PHOENIX_PID_FILE = ROOT / ".phoenix.pid"
VITE_PID_FILE = ROOT / ".vite.pid"
LOG_FILE = ROOT / "phoenix.log"

# Production paths
PROD_SERVICE_NAME = "phoenix-ide"
PROD_INSTALL_DIR = Path("/opt/phoenix-ide")
PROD_DB_PATH = Path.home() / ".phoenix-ide" / "prod.db"
PROD_PORT = 8031  # In workspaces-compatible 8000-8050 range

# Lima VM configuration
LIMA_VM_NAME = "phoenix-ide"
LIMA_YAML = ROOT / "lima" / "phoenix-ide.yaml"
LIMA_BUILD_DIR = "/opt/phoenix-build"
LIMA_ENV_FILE = "/etc/phoenix-ide/env"

# exe.dev LLM gateway configuration
EXE_DEV_CONFIG = Path("/exe.dev/shelley.json")
DEFAULT_GATEWAY = "http://169.254.169.254/gateway/llm"
LOCAL_AI_PROXY = "http://127.0.0.1:8462"

# Base ports - offset added based on worktree path hash
# Both ports must stay within exposed ranges: 6000-6010, 8000-8050, 8080, 8443
BASE_PHOENIX_PORT = 8000  # API will use 8000-8024
BASE_VITE_PORT = 8025     # Vite will use 8025-8049
PORT_RANGE = 25           # Reduced to fit both in 8000-8050 range

# Database directory
DB_DIR = Path.home() / ".phoenix-ide"


def _gateway_is_reachable(url: str) -> bool:
    """Probe a gateway with a quick HTTP request. Any response means it's up."""
    import urllib.request
    import urllib.error
    try:
        urllib.request.urlopen(url, timeout=0.5)
        return True
    except urllib.error.HTTPError:
        return True  # 404, 405, etc. — server is listening
    except Exception:
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
    h = hashlib.md5(str(ROOT).encode()).hexdigest()
    return int(h[:4], 16) % PORT_RANGE


def get_default_ports() -> tuple[int, int]:
    """Get default Phoenix and Vite ports for this worktree."""
    offset = get_port_offset()
    return (BASE_PHOENIX_PORT + offset, BASE_VITE_PORT + offset)


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
        os.kill(pid, signal.SIGTERM)
        # Wait briefly for graceful shutdown
        for _ in range(10):
            if not is_process_running(pid):
                break
            time.sleep(0.1)
        else:
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
        subprocess.run(["npm", "install"], cwd=UI_DIR, check=True)


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
    default_phoenix, _ = get_default_ports()
    phoenix_port = phoenix_port or default_phoenix
    
    build_rust(release=True)
    stop_process(PHOENIX_PID_FILE, "Phoenix")
    time.sleep(0.5)
    start_phoenix(port=phoenix_port)
    print("Phoenix restarted. Vite still running for UI hot reload.")


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
    """Run lint, format check, tests, and task validation."""
    print("Running clippy...")
    result = subprocess.run(["cargo", "clippy", "--", "-D", "warnings"], cwd=ROOT)
    if result.returncode != 0:
        sys.exit(result.returncode)

    print("\nChecking format...")
    result = subprocess.run(["cargo", "fmt", "--check"], cwd=ROOT)
    if result.returncode != 0:
        sys.exit(result.returncode)

    print("\nRunning Rust tests...")
    result = subprocess.run(["cargo", "test"], cwd=ROOT)
    if result.returncode != 0:
        sys.exit(result.returncode)

    print("\nRunning UI lint...")
    result = subprocess.run(["npm", "run", "lint"], cwd=UI_DIR)
    if result.returncode != 0:
        sys.exit(result.returncode)

    print("\nValidating task specs...")
    if not cmd_tasks_validate():
        sys.exit(1)

    print("\n✓ All checks passed")


# =============================================================================
# Task Validation
# =============================================================================

VALID_STATUSES = {"ready", "in-progress", "pending", "blocked", "done", "wont-do", "brainstorming"}
VALID_PRIORITIES = {"p0", "p1", "p2", "p3", "p4"}

# Task filename pattern: NNN-pX-status-slug.md
# Example: 001-p2-done-refactor-executor.md
TASK_FILENAME_PATTERN = r"^(\d{3})-(p[0-4])-(" + "|".join(VALID_STATUSES) + r")-(.+)\.md$"


def parse_task_file(path: Path) -> dict | None:
    """Parse a task file and return its metadata, or None if invalid.

    Returns dict with: number, priority, status, slug, fields (from frontmatter)
    """
    import re

    content = path.read_text()

    # Check YAML frontmatter exists
    if not content.startswith("---\n"):
        return None

    # Extract frontmatter
    end = content.find("\n---\n", 4)
    if end == -1:
        return None

    frontmatter = content[4:end]
    fields = {}
    for line in frontmatter.strip().split("\n"):
        if ":" in line:
            key, _, value = line.partition(":")
            fields[key.strip()] = value.strip()

    # Parse filename
    name = path.name
    match = re.match(TASK_FILENAME_PATTERN, name)
    if match:
        return {
            "path": path,
            "number": match.group(1),
            "file_priority": match.group(2),
            "file_status": match.group(3),
            "slug": match.group(4),
            "fields": fields,
        }

    # Try old format: NNN-slug.md
    old_match = re.match(r"^(\d{3})-(.+)\.md$", name)
    if old_match:
        return {
            "path": path,
            "number": old_match.group(1),
            "file_priority": None,
            "file_status": None,
            "slug": old_match.group(2),
            "fields": fields,
        }

    return None


def get_expected_filename(number: str, priority: str, status: str, slug: str) -> str:
    """Generate the expected filename from task metadata."""
    return f"{number}-{priority}-{status}-{slug}.md"


def cmd_tasks_validate() -> bool:
    """Validate all task files conform to the naming and frontmatter conventions.

    Filename format: NNN-pX-status-slug.md
    Example: 001-p2-done-refactor-executor.md

    Returns True if all tasks pass, False otherwise.
    """
    import re

    tasks_dir = ROOT / "tasks"
    if not tasks_dir.exists():
        print("No tasks/ directory found, skipping.")
        return True

    errors = []
    task_files = sorted(tasks_dir.glob("*.md"))
    template = tasks_dir / "_TEMPLATE.md"

    for path in task_files:
        if path == template:
            continue

        name = path.name
        content = path.read_text()

        # Check YAML frontmatter exists
        if not content.startswith("---\n"):
            errors.append(f"{name}: missing YAML frontmatter (must start with ---)")
            continue

        # Extract frontmatter
        end = content.find("\n---\n", 4)
        if end == -1:
            errors.append(f"{name}: malformed YAML frontmatter (no closing ---)")
            continue

        frontmatter = content[4:end]
        fields = {}
        for line in frontmatter.strip().split("\n"):
            if ":" in line:
                key, _, value = line.partition(":")
                fields[key.strip()] = value.strip()

        # Check required fields
        if "status" not in fields:
            errors.append(f"{name}: missing 'status' field")
        elif fields["status"] not in VALID_STATUSES:
            errors.append(
                f"{name}: invalid status '{fields['status']}' "
                f"(valid: {', '.join(sorted(VALID_STATUSES))})"
            )

        if "priority" not in fields:
            errors.append(f"{name}: missing 'priority' field")
        elif fields["priority"] not in VALID_PRIORITIES:
            errors.append(
                f"{name}: invalid priority '{fields['priority']}' "
                f"(valid: {', '.join(sorted(VALID_PRIORITIES))})"
            )

        if "created" not in fields:
            errors.append(f"{name}: missing 'created' field")
        elif not re.match(r"^\d{4}-\d{2}-\d{2}$", fields["created"]):
            errors.append(f"{name}: invalid 'created' date format (expected YYYY-MM-DD)")

        # Check filename matches frontmatter
        task = parse_task_file(path)
        if task and fields.get("status") and fields.get("priority"):
            expected = get_expected_filename(
                task["number"], fields["priority"], fields["status"], task["slug"]
            )
            if name != expected:
                errors.append(f"{name}: filename doesn't match frontmatter, expected: {expected}")

    if errors:
        print(f"✗ {len(errors)} task validation error(s):")
        for err in errors:
            print(f"  - {err}")
        print("\nRun './dev.py tasks fix' to auto-fix (injects missing 'created', renames files).")
        return False

    print(f"✓ {len(task_files) - 1} task files validated")  # -1 for template
    return True


def infer_created_date(path: Path) -> str:
    """Infer a creation date for a task file.

    Priority:
    1. Earliest git commit date for the file
    2. File mtime (on-disk creation approximation)
    3. Today's date
    """
    import subprocess
    from datetime import date, datetime

    # 1. Earliest git commit touching this file
    try:
        result = subprocess.run(
            ["git", "log", "--follow", "--diff-filter=A", "--format=%as", str(path)],
            capture_output=True,
            text=True,
            cwd=path.parent,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip().splitlines()[-1]
    except Exception:
        pass

    # 2. File mtime
    try:
        mtime = path.stat().st_mtime
        return datetime.fromtimestamp(mtime).strftime("%Y-%m-%d")
    except Exception:
        pass

    # 3. Today
    return date.today().isoformat()


def cmd_tasks_fix() -> bool:
    """Auto-fix task files: inject missing 'created' fields and rename to match frontmatter.

    Returns True if all files are now correct, False on errors.
    """
    import re

    tasks_dir = ROOT / "tasks"
    if not tasks_dir.exists():
        print("No tasks/ directory found.")
        return True

    task_files = sorted(tasks_dir.glob("*.md"))
    template = tasks_dir / "_TEMPLATE.md"
    renamed = 0
    patched = 0
    errors = []

    for path in task_files:
        if path == template:
            continue

        task = parse_task_file(path)
        if not task:
            errors.append(f"{path.name}: could not parse file")
            continue

        fields = task["fields"]

        # Inject missing 'created' field into frontmatter
        if "created" not in fields or not re.match(r"^\d{4}-\d{2}-\d{2}$", fields.get("created", "")):
            created = infer_created_date(path)
            content = path.read_text()
            # Insert after the opening --- line
            content = content.replace("---\n", f"---\ncreated: {created}\n", 1)
            path.write_text(content)
            print(f"  {path.name}: added created: {created}")
            fields["created"] = created
            patched += 1

        if not fields.get("status") or not fields.get("priority"):
            errors.append(f"{path.name}: missing status or priority in frontmatter")
            continue

        expected = get_expected_filename(
            task["number"], fields["priority"], fields["status"], task["slug"]
        )

        if path.name != expected:
            new_path = tasks_dir / expected
            if new_path.exists():
                errors.append(f"{path.name}: cannot rename to {expected}, file exists")
                continue

            path.rename(new_path)
            print(f"  {path.name} -> {expected}")
            renamed += 1

    if errors:
        print(f"\n✗ {len(errors)} error(s):")
        for err in errors:
            print(f"  - {err}")
        return False

    if patched or renamed:
        parts = []
        if patched:
            parts.append(f"patched {patched} file(s)")
        if renamed:
            parts.append(f"renamed {renamed} file(s)")
        print(f"\n✓ {', '.join(parts).capitalize()}")
    else:
        print("✓ All files already correctly named")
    return True



# =============================================================================
# Production Commands
# =============================================================================


def detect_prod_env() -> str | None:
    """Detect production environment: 'native', 'lima', 'daemon', or None.

    Returns:
        'native': Linux with systemd - full production deployment
        'lima': macOS with Lima VM - isolated VM deployment
        'daemon': Fallback - background daemon in ~/.phoenix-ide/
        None: macOS without Lima (error condition)
    """
    if sys.platform == "darwin":
        # macOS: ONLY check for Lima VM, never use daemon mode
        if lima_vm_exists():
            lima_ensure_running()
            return "lima"
        # No Lima = fail with helpful message, don't silently fall back
        return None

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
    except:
        return False


def prod_build(version: str | None = None, strip: bool = True) -> Path:
    """Build a production binary from a git tag or HEAD.

    Uses a separate git worktree to avoid disturbing the main working directory.
    Returns path to the built binary.

    Args:
        version: Git tag or None for HEAD
        strip: Whether to strip debug symbols (default True, False for debugging)
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
    print("Installing UI dependencies...")
    subprocess.run(["npm", "ci"], cwd=ui_dir, check=True, capture_output=True)
    
    print("Building UI...")
    subprocess.run(["npm", "run", "build"], cwd=ui_dir, check=True)
    
    # Build Rust with musl target
    print("Building Rust (musl, release)...")
    build_env = os.environ.copy()
    build_env["CC_x86_64_unknown_linux_musl"] = "x86_64-linux-musl-gcc"
    subprocess.run(
        ["cargo", "build", "--release", "--target", "x86_64-unknown-linux-musl"],
        cwd=worktree, check=True, env=build_env
    )
    
    binary = worktree / "target" / "x86_64-unknown-linux-musl" / "release" / "phoenix_ide"

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
# Systemd Unit Generation (shared by native and Lima)
# =============================================================================

@dataclasses.dataclass
class SystemdConfig:
    """Configuration for systemd unit generation.
    
    Using a dataclass ensures native and Lima deployments use identical
    unit file structure - they can only differ in these explicit parameters.
    """
    user: str
    db_path: str
    install_dir: str
    port: int
    # For Lima: uses EnvironmentFile for API key; for native: uses LLM_GATEWAY env var
    env_file: str | None = None
    llm_gateway: str | None = None


# Configs for each deployment target
NATIVE_SYSTEMD_CONFIG = SystemdConfig(
    user="exedev",
    db_path=str(PROD_DB_PATH),
    install_dir=str(PROD_INSTALL_DIR),
    port=PROD_PORT,
    llm_gateway=None,  # Set at deploy time via get_llm_gateway()
)

LIMA_SYSTEMD_CONFIG = SystemdConfig(
    user="phoenix-ide",
    db_path="/mnt/phoenix-data/prod.db",
    install_dir="/opt/phoenix-ide",
    port=PROD_PORT,
    env_file=LIMA_ENV_FILE,
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

    if config.env_file:
        # Lima mode: use EnvironmentFile for API key
        env_lines.insert(0, f"EnvironmentFile={config.env_file}")
    elif config.llm_gateway:
        # Native mode: use LLM_GATEWAY directly
        env_lines.append(f"Environment=LLM_GATEWAY={config.llm_gateway}")

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
        print("  - Use './dev.py lima create' to set up a Lima VM with systemd", file=sys.stderr)
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
    
    # Ensure database directory exists
    PROD_DB_PATH.parent.mkdir(parents=True, exist_ok=True)

    # Configure for native deployment
    config = dataclasses.replace(NATIVE_SYSTEMD_CONFIG, llm_gateway=get_llm_gateway())

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
            print(f"\n✓ Deployed {version} to production (zero-downtime upgrade)")
            print(f"  Service: {PROD_SERVICE_NAME}")
            print(f"  Port: {PROD_PORT}")
            print(f"  Socket: {PROD_SERVICE_NAME}.socket (keeps connections alive)")
            print(f"  Database: {PROD_DB_PATH}")
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
            print(f"\n✓ Deployed {version} to production")
            print(f"  Service: {PROD_SERVICE_NAME}")
            print(f"  Port: {PROD_PORT}")
            print(f"  Socket: {PROD_SERVICE_NAME}.socket (zero-downtime upgrades enabled)")
            print(f"  Database: {PROD_DB_PATH}")
        else:
            print(f"\n✗ Service failed to start", file=sys.stderr)
            subprocess.run(["sudo", "journalctl", "-u", PROD_SERVICE_NAME, "-n", "20", "--no-pager"])
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

    gateway = get_llm_gateway()
    if gateway:
        env["LLM_GATEWAY"] = gateway
    else:
        api_key = os.environ.get("ANTHROPIC_API_KEY")
        if not api_key:
            print("ERROR: No LLM gateway reachable and ANTHROPIC_API_KEY not set.", file=sys.stderr)
            sys.exit(1)
        env["ANTHROPIC_API_KEY"] = api_key

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

    print(f"\n✓ Deployed daemon to production")
    print(f"  Version: {version_info.get('version', 'unknown')}")
    print(f"  Port: {PROD_PORT}")
    print(f"  Database: {prod_db_path}")
    print(f"  Logs: {prod_log_path}")
    print(f"  PID: {proc.pid} (saved to {prod_pid_path})")
    llm_mode = f"gateway ({gateway})" if gateway else "Direct API (ANTHROPIC_API_KEY)"
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

        # Try to get version from health endpoint
        try:
            import urllib.request
            with urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=2) as resp:
                version_text = resp.read().decode().strip()
                print(f"  Version: {version_text}")
                print(f"  Port: {PROD_PORT}")
                print(f"  Health: OK")
        except Exception as e:
            print(f"  Health: Unreachable ({type(e).__name__}: {e})")

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
        # Get version from environment
        result = subprocess.run(
            ["systemctl", "show", PROD_SERVICE_NAME, "--property=Environment"],
            capture_output=True, text=True
        )
        for part in result.stdout.split():
            if part.startswith("PHOENIX_VERSION="):
                print(f"  Version: {part.split('=', 1)[1]}")
        print(f"  Port: {PROD_PORT}")
        print(f"  Database: {PROD_DB_PATH}")
        
        # Check if responding
        try:
            import urllib.request
            with urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=2) as resp:
                print(f"  Health: {resp.read().decode().strip()}")
        except Exception:
            print(f"  Health: not responding")
    else:
        print(f"Production: {status}")
    
    # Show systemd overrides
    overrides = list_systemd_overrides()
    if overrides:
        print(f"  Overrides:")
        for filename, content in overrides:
            # Extract the key=value from the content
            for line in content.split('\n'):
                if line.startswith('Environment='):
                    env_val = line.replace('Environment=', '')
                    print(f"    {filename}: {env_val}")


def native_prod_stop():
    """Stop production service (native Linux)."""
    subprocess.run(["sudo", "systemctl", "stop", PROD_SERVICE_NAME])
    print(f"Stopped {PROD_SERVICE_NAME}")


def cmd_prod_build(version: str | None = None):
    """Build production binary from git tag."""
    if sys.platform != "linux":
        print("Production builds happen inside the Lima VM during './dev.py prod deploy'.")
        return
    prod_build(version)


def cmd_prod_deploy(version: str | None = None):
    """Build and deploy to production (auto-detects environment)."""
    print("Running pre-deploy checks...\n")
    cmd_check()
    print()

    env = detect_prod_env()

    if env == "native":
        print("📦 Detected: Linux with systemd (native deployment)")
        native_prod_deploy(version)

    elif env == "lima":
        print("📦 Detected: macOS with Lima VM")
        # Lima clones the repo independently, so unpushed commits won't be visible.
        unpushed = subprocess.run(
            ["git", "log", "@{u}..", "--oneline"],
            cwd=ROOT, capture_output=True, text=True
        )
        if unpushed.stdout.strip():
            print("ERROR: unpushed commits will not be visible inside Lima VM:", file=sys.stderr)
            for line in unpushed.stdout.strip().splitlines():
                print(f"  {line}", file=sys.stderr)
            print("Run 'git push' first.", file=sys.stderr)
            sys.exit(1)
        lima_prod_deploy()

    elif env == "daemon":
        print("📦 Detected: No systemd (daemon mode)")
        print("    Running production build as background daemon")
        print()
        prod_daemon_deploy()

    elif env is None:
        # macOS without Lima
        print("ERROR: Lima VM not found", file=sys.stderr)
        print("", file=sys.stderr)
        print("Phoenix IDE requires Lima VM for production deployment on macOS.", file=sys.stderr)
        print("Lima provides an isolated Linux environment with systemd.", file=sys.stderr)
        print("", file=sys.stderr)
        print("To set up Lima VM:", file=sys.stderr)
        print("  1. Install Lima: brew install lima", file=sys.stderr)
        print("  2. Create VM: ./dev.py lima create", file=sys.stderr)
        print("  3. Deploy: ./dev.py prod deploy", file=sys.stderr)
        sys.exit(1)

    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_status():
    """Show production status (auto-detects environment)."""
    env = detect_prod_env()

    if env == "native":
        native_prod_status()
    elif env == "lima":
        lima_prod_status()
    elif env == "daemon":
        prod_daemon_status()
    elif env is None:
        print("ERROR: Lima VM not found. Cannot check status.", file=sys.stderr)
        print("Run './dev.py lima create' to set up Lima VM.", file=sys.stderr)
        sys.exit(1)
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_stop():
    """Stop production service (auto-detects environment)."""
    env = detect_prod_env()

    if env == "native":
        native_prod_stop()
    elif env == "lima":
        lima_prod_stop()
    elif env == "daemon":
        prod_daemon_stop()
    elif env is None:
        print("ERROR: Lima VM not found. Cannot stop.", file=sys.stderr)
        sys.exit(1)
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_override_set(name: str, value: str):
    """Set a systemd environment override (native Linux only)."""
    env = detect_prod_env()
    
    if env == "native":
        native_prod_override_set(name, value)
    elif env == "lima":
        print("ERROR: Overrides not yet supported for Lima deployments", file=sys.stderr)
        print("SSH into the VM and edit the service file directly.", file=sys.stderr)
        sys.exit(1)
    elif env == "daemon":
        print("ERROR: Overrides not supported for daemon mode", file=sys.stderr)
        print("Stop the daemon and restart with environment variables set.", file=sys.stderr)
        sys.exit(1)
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


def cmd_prod_override_unset(name: str):
    """Remove a systemd environment override (native Linux only)."""
    env = detect_prod_env()
    
    if env == "native":
        native_prod_override_unset(name)
    elif env == "lima":
        print("ERROR: Overrides not yet supported for Lima deployments", file=sys.stderr)
        sys.exit(1)
    elif env == "daemon":
        print("ERROR: Overrides not supported for daemon mode", file=sys.stderr)
        sys.exit(1)
    else:
        print(f"ERROR: Unknown environment: {env}", file=sys.stderr)
        sys.exit(1)


# =============================================================================
# Lima VM Commands
# =============================================================================

def lima_shell(cmd: str, check=True) -> subprocess.CompletedProcess:
    """Run a command inside the Lima VM via bash -lc (loads .bashrc for Rust PATH)."""
    return subprocess.run(
        ["limactl", "shell", "--workdir", "/", LIMA_VM_NAME, "--", "bash", "-lc", cmd],
        check=check,
    )


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


def lima_has_llm_config() -> bool:
    """Check if LLM config (API key or gateway) is configured in the VM."""
    result = lima_shell_quiet(f"test -f {LIMA_ENV_FILE} && sudo grep -qE '(ANTHROPIC_API_KEY|LLM_GATEWAY)' {LIMA_ENV_FILE}", check=False)
    return result.returncode == 0


def lima_prompt_api_key():
    """Prompt user for API key and write to VM env file.

    Falls back to ANTHROPIC_API_KEY env var if no terminal is available.
    """
    key = os.environ.get("ANTHROPIC_API_KEY", "")
    if key:
        print("Using ANTHROPIC_API_KEY from host environment.")
    else:
        try:
            key = getpass.getpass("ANTHROPIC_API_KEY: ")
        except EOFError:
            print("No terminal available and ANTHROPIC_API_KEY not set in environment.", file=sys.stderr)
            print("Set ANTHROPIC_API_KEY in your environment and re-run './dev.py prod deploy'.", file=sys.stderr)
            return
    if not key.strip():
        print("No key provided, skipping.", file=sys.stderr)
        return
    env_content = f"ANTHROPIC_API_KEY={key.strip()}\n"
    subprocess.run(
        ["limactl", "shell", "--workdir", "/", LIMA_VM_NAME, "--", "sudo", "tee", LIMA_ENV_FILE],
        input=env_content.encode(), check=True, capture_output=True,
    )
    lima_shell(f"sudo chmod 600 {LIMA_ENV_FILE}")
    print("API key saved.")


def lima_configure_llm() -> None:
    """Auto-detect host LLM gateway and write VM env file, or prompt for API key."""
    if lima_has_llm_config():
        return

    # Probe candidates on the host
    for url in _discover_gateway_candidates():
        if _gateway_is_reachable(url):
            # Translate host loopback to Lima's host-reachable address
            vm_url = url.replace("://127.0.0.1", "://host.lima.internal") \
                        .replace("://localhost", "://host.lima.internal")
            env_content = f"LLM_GATEWAY={vm_url}\n"
            subprocess.run(
                ["limactl", "shell", "--workdir", "/", LIMA_VM_NAME, "--", "sudo", "tee", LIMA_ENV_FILE],
                input=env_content.encode(), check=True, capture_output=True,
            )
            lima_shell(f"sudo chmod 600 {LIMA_ENV_FILE}")
            print(f"  LLM gateway: {vm_url} (auto-detected from host)")
            return

    # No gateway found — fall back to API key prompt
    print("\nNo LLM gateway detected on host.")
    lima_prompt_api_key()


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


def lima_prod_deploy():
    """Build and deploy phoenix-ide inside the Lima VM."""
    lima_ensure_running()

    # Ensure build dir is writable by the Lima default user
    lima_shell(f"sudo chown -R $(id -u):$(id -g) {LIMA_BUILD_DIR}")

    # Sync source via tar pipe (--no-mac-metadata avoids macOS ._ extended attr files)
    print("Syncing source to VM...")
    tar_cmd = [
        "tar", "-cf", "-", "--no-mac-metadata",
        "--exclude=target", "--exclude=node_modules", "--exclude=.git",
        ".",
    ]
    tar_proc = subprocess.Popen(tar_cmd, stdout=subprocess.PIPE, cwd=ROOT)
    # Suppress tar warnings about macOS extended header keywords on the receiving side
    subprocess.run(
        ["limactl", "shell", "--workdir", "/", LIMA_VM_NAME, "--",
         "tar", "xf", "-", "-C", LIMA_BUILD_DIR,
         "--warning=no-unknown-keyword"],
        stdin=tar_proc.stdout, check=True,
        stderr=subprocess.DEVNULL,
    )
    tar_proc.wait()
    if tar_proc.returncode != 0:
        print("Failed to create tar archive", file=sys.stderr)
        sys.exit(1)
    print("Source synced.")

    # Build UI
    print("\nBuilding UI...")
    lima_shell(f"cd {LIMA_BUILD_DIR}/ui && npm install && npm run build")

    # Build Rust
    print("\nBuilding Rust (release)...")
    lima_shell(f"cd {LIMA_BUILD_DIR} && cargo build --release")

    # Strip and install binary
    print("\nInstalling binary...")
    lima_shell(f"strip {LIMA_BUILD_DIR}/target/release/phoenix_ide")
    lima_shell(f"sudo cp {LIMA_BUILD_DIR}/target/release/phoenix_ide /opt/phoenix-ide/phoenix-ide")
    lima_shell("sudo chmod +x /opt/phoenix-ide/phoenix-ide")

    # LLM config — auto-detect gateway or prompt for API key
    lima_configure_llm()

    # Get version string
    result = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=ROOT, capture_output=True, text=True,
    )
    version = f"dev-{result.stdout.strip()}" if result.returncode == 0 else "dev-unknown"

    # Use shared systemd config for Lima
    config = LIMA_SYSTEMD_CONFIG

    # Install systemd socket unit (for socket activation)
    print("\nInstalling systemd socket unit...")
    socket_content = generate_systemd_socket(config)
    subprocess.run(
        ["limactl", "shell", "--workdir", "/", LIMA_VM_NAME, "--",
         "sudo", "tee", f"/etc/systemd/system/{PROD_SERVICE_NAME}.socket"],
        input=socket_content.encode(), check=True, capture_output=True,
    )

    # Install systemd service unit
    print("Installing systemd service unit...")
    unit_content = generate_systemd_service(config, version)
    subprocess.run(
        ["limactl", "shell", "--workdir", "/", LIMA_VM_NAME, "--",
         "sudo", "tee", f"/etc/systemd/system/{PROD_SERVICE_NAME}.service"],
        input=unit_content.encode(), check=True, capture_output=True,
    )

    lima_shell("sudo systemctl daemon-reload")
    lima_shell(f"sudo systemctl enable {PROD_SERVICE_NAME}.socket")
    lima_shell(f"sudo systemctl enable {PROD_SERVICE_NAME}")

    # Check if service is already running
    result = lima_shell_quiet(f"systemctl is-active {PROD_SERVICE_NAME}", check=False)
    if result.stdout.strip() == "active":
        # Service running - send SIGHUP for zero-downtime upgrade
        print("Sending reload signal (SIGHUP) for zero-downtime upgrade...")
        lima_shell(f"sudo systemctl reload {PROD_SERVICE_NAME}")
    else:
        # Service not running - start socket and service
        print("Starting socket and service...")
        lima_shell(f"sudo systemctl start {PROD_SERVICE_NAME}.socket")
        lima_shell(f"sudo systemctl start {PROD_SERVICE_NAME}")

    time.sleep(2)

    # Verify
    result = lima_shell_quiet(f"systemctl is-active {PROD_SERVICE_NAME}", check=False)
    status = result.stdout.strip()
    if status == "active":
        print(f"\n✓ Deployed {version} to Lima VM")
        print(f"  Service: {PROD_SERVICE_NAME}")
        print(f"  Port: {PROD_PORT}")
        print(f"  Socket: {PROD_SERVICE_NAME}.socket (zero-downtime upgrades enabled)")
    else:
        print(f"\n✗ Service failed to start (status: {status})", file=sys.stderr)
        lima_shell(f"sudo journalctl -u {PROD_SERVICE_NAME} -n 20 --no-pager")
        sys.exit(1)


def lima_prod_status():
    """Show Lima VM and service status."""
    if not lima_vm_exists():
        print(f"VM '{LIMA_VM_NAME}': not created")
        return

    if not lima_is_running():
        print(f"VM '{LIMA_VM_NAME}': stopped")
        return

    print(f"VM '{LIMA_VM_NAME}': running")

    # Kernel version
    result = lima_shell_quiet("uname -r", check=False)
    if result.returncode == 0:
        print(f"  Kernel: {result.stdout.strip()}")

    # Service status
    result = lima_shell_quiet(f"systemctl is-active {PROD_SERVICE_NAME}", check=False)
    svc_status = result.stdout.strip()
    print(f"  Service: {svc_status}")

    # LLM config
    if lima_has_llm_config():
        print("  LLM config: configured")
    else:
        print("  LLM config: not set")

    # Health check from host
    if svc_status == "active":
        try:
            import urllib.request
            with urllib.request.urlopen(f"http://localhost:{PROD_PORT}/version", timeout=2) as resp:
                print(f"  Health: {resp.read().decode().strip()}")
        except Exception:
            print("  Health: not responding on host (port forward may not be active)")


def lima_prod_stop():
    """Stop the phoenix-ide service in the Lima VM."""
    lima_ensure_running()
    lima_shell(f"sudo systemctl stop {PROD_SERVICE_NAME}", check=False)
    print(f"Stopped {PROD_SERVICE_NAME} in Lima VM.")


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

    # Stop service first (ignore errors)
    if lima_is_running():
        lima_shell(f"sudo systemctl stop {PROD_SERVICE_NAME}", check=False)

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
    override_set_parser = prod_sub.add_parser("set", help="Set systemd environment override")
    override_set_parser.add_argument("name", help="Environment variable name (e.g., RUST_LOG)")
    override_set_parser.add_argument("value", help="Environment variable value (e.g., debug)")
    override_unset_parser = prod_sub.add_parser("unset", help="Remove systemd environment override")
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
    main()
