#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# ///
"""Development tasks for phoenix-ide."""

import argparse
import fcntl
import hashlib
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).parent.resolve()
TASKS_DIR = ROOT / "tasks"
UI_DIR = ROOT / "ui"
PHOENIX_PID_FILE = ROOT / ".phoenix.pid"
VITE_PID_FILE = ROOT / ".vite.pid"
LOG_FILE = ROOT / "phoenix.log"

# exe.dev LLM gateway configuration
EXE_DEV_CONFIG = Path("/exe.dev/shelley.json")
DEFAULT_GATEWAY = "http://169.254.169.254/gateway/llm"

# Base ports - offset added based on worktree path hash
BASE_PHOENIX_PORT = 8000
BASE_VITE_PORT = 5173
PORT_RANGE = 1000  # Ports will be base + (0 to 999)

# Database directory
DB_DIR = Path.home() / ".phoenix-ide"


def get_llm_gateway() -> str:
    """Get LLM gateway URL from exe.dev config or default."""
    if EXE_DEV_CONFIG.exists():
        try:
            config = json.loads(EXE_DEV_CONFIG.read_text())
            return config.get("llm_gateway", DEFAULT_GATEWAY)
        except (json.JSONDecodeError, KeyError):
            pass
    return DEFAULT_GATEWAY


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
    env["LLM_GATEWAY"] = get_llm_gateway()
    env["PHOENIX_PORT"] = str(port)
    env["PHOENIX_DB_PATH"] = str(db_path)

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
    
    # Start Vite in background
    proc = subprocess.Popen(
        ["npm", "run", "dev", "--", "--port", str(port)],
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
    """Run lint, format check, and tests."""
    print("Running clippy...")
    result = subprocess.run(["cargo", "clippy", "--", "-D", "warnings"], cwd=ROOT)
    if result.returncode != 0:
        sys.exit(result.returncode)

    print("\nChecking format...")
    result = subprocess.run(["cargo", "fmt", "--check"], cwd=ROOT)
    if result.returncode != 0:
        sys.exit(result.returncode)

    print("\nRunning tests...")
    result = subprocess.run(["cargo", "test"], cwd=ROOT)
    if result.returncode != 0:
        sys.exit(result.returncode)

    print("\n✓ All checks passed")


def cmd_tasks_ready():
    """List tasks ready to be implemented."""
    for task_file in sorted(TASKS_DIR.glob("[0-9]*.md")):
        content = task_file.read_text()
        if "status: ready" in content:
            priority = "p?"
            title = task_file.stem
            for line in content.splitlines():
                if line.startswith("priority:"):
                    priority = line.split(":", 1)[1].strip()
                elif line.startswith("# "):
                    title = line[2:]
                    break
            print(f"[{priority}] {task_file.stem}: {title}")


def cmd_tasks_close(task_id: str, wont_do: bool = False):
    """Close a task as complete or won't-do."""
    matches = list(TASKS_DIR.glob(f"{task_id}*.md"))
    if not matches:
        print(f"No task found matching '{task_id}'", file=sys.stderr)
        sys.exit(1)
    if len(matches) > 1:
        print(f"Ambiguous task id '{task_id}': {[m.name for m in matches]}", file=sys.stderr)
        sys.exit(1)

    task_file = matches[0]
    content = task_file.read_text()
    new_status = "wont-do" if wont_do else "done"
    new_content = content.replace("status: ready", f"status: {new_status}")

    if new_content == content:
        print(f"Task {task_file.name} not in 'ready' status", file=sys.stderr)
        sys.exit(1)

    task_file.write_text(new_content)
    print(f"Closed {task_file.name} as {new_status}")


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

    # tasks
    tasks_parser = sub.add_parser("tasks", help="Task management")
    tasks_sub = tasks_parser.add_subparsers(dest="tasks_command", required=True)
    tasks_sub.add_parser("ready", help="List ready tasks")
    close_parser = tasks_sub.add_parser("close", help="Close a task")
    close_parser.add_argument("task_id", help="Task ID (e.g., 001)")
    close_parser.add_argument("--wont-do", action="store_true", help="Mark as won't-do")

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
    elif args.command == "tasks":
        if args.tasks_command == "ready":
            cmd_tasks_ready()
        elif args.tasks_command == "close":
            cmd_tasks_close(args.task_id, args.wont_do)


if __name__ == "__main__":
    main()
