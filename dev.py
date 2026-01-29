#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# ///
"""Development tasks for phoenix-ide."""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).parent
TASKS_DIR = ROOT / "tasks"
PID_FILE = ROOT / ".phoenix.pid"

# exe.dev LLM gateway configuration
EXE_DEV_CONFIG = Path("/exe.dev/shelley.json")
DEFAULT_GATEWAY = "http://169.254.169.254/gateway/llm"


def get_llm_gateway() -> str:
    """Get LLM gateway URL from exe.dev config or default."""
    if EXE_DEV_CONFIG.exists():
        try:
            config = json.loads(EXE_DEV_CONFIG.read_text())
            return config.get("llm_gateway", DEFAULT_GATEWAY)
        except (json.JSONDecodeError, KeyError):
            pass
    return DEFAULT_GATEWAY


def cmd_lint():
    """Run clippy and fmt check."""
    subprocess.run(["cargo", "clippy", "--", "-D", "warnings"], check=True)
    subprocess.run(["cargo", "fmt", "--check"], check=True)


def cmd_build():
    """Build the project."""
    subprocess.run(["cargo", "build"], check=True)


def cmd_start(release: bool = True, port: int = 8000):
    """Start the Phoenix server."""
    # Check if already running
    if PID_FILE.exists():
        pid = int(PID_FILE.read_text().strip())
        try:
            os.kill(pid, 0)  # Check if process exists
            print(f"Server already running (PID {pid})")
            return
        except OSError:
            PID_FILE.unlink()  # Stale PID file

    # Build first
    build_args = ["cargo", "build"]
    if release:
        build_args.append("--release")
    subprocess.run(build_args, check=True, cwd=ROOT)

    # Determine binary path
    binary = ROOT / "target" / ("release" if release else "debug") / "phoenix_ide"
    if not binary.exists():
        print(f"Binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    # Set up environment
    env = os.environ.copy()
    env["LLM_GATEWAY"] = get_llm_gateway()
    env["PHOENIX_PORT"] = str(port)

    # Start server in background
    log_file = ROOT / "phoenix.log"
    with open(log_file, "w") as log:
        proc = subprocess.Popen(
            [str(binary)],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        PID_FILE.write_text(str(proc.pid))
        print(f"Started Phoenix server (PID {proc.pid})")
        print(f"  Port: {port}")
        print(f"  Gateway: {env['LLM_GATEWAY']}")
        print(f"  Log: {log_file}")

    # Wait briefly and check it started
    time.sleep(1)
    try:
        os.kill(proc.pid, 0)
    except OSError:
        print("Server failed to start. Check phoenix.log", file=sys.stderr)
        PID_FILE.unlink()
        sys.exit(1)


def cmd_stop():
    """Stop the Phoenix server."""
    if not PID_FILE.exists():
        print("Server not running (no PID file)")
        return

    pid = int(PID_FILE.read_text().strip())
    try:
        os.kill(pid, signal.SIGTERM)
        print(f"Stopped server (PID {pid})")
    except OSError as e:
        print(f"Could not stop server: {e}")
    finally:
        PID_FILE.unlink()


def cmd_status():
    """Check Phoenix server status."""
    if not PID_FILE.exists():
        print("Server not running (no PID file)")
        return

    pid = int(PID_FILE.read_text().strip())
    try:
        os.kill(pid, 0)
        print(f"Server running (PID {pid})")
        # Try to get more info
        try:
            import urllib.request
            with urllib.request.urlopen("http://localhost:8000/api/models", timeout=2) as resp:
                data = json.loads(resp.read())
                print(f"  Models: {', '.join(data.get('models', []))}")
                print(f"  Default: {data.get('default', 'N/A')}")
        except Exception:
            print("  (Could not fetch API status)")
    except OSError:
        print(f"Server not running (stale PID {pid})")
        PID_FILE.unlink()


def cmd_restart(release: bool = True, port: int = 8000):
    """Restart the Phoenix server."""
    cmd_stop()
    time.sleep(1)
    cmd_start(release=release, port=port)


def cmd_tasks_ready():
    """List tasks ready to be implemented."""
    for task_file in sorted(TASKS_DIR.glob("[0-9]*.md")):
        content = task_file.read_text()
        if "status: ready" in content:
            # Extract priority and title
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


def main():
    parser = argparse.ArgumentParser(prog="dev.py", description="Development tasks")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("lint", help="Run linting")
    sub.add_parser("build", help="Build project")

    # Server commands
    start_parser = sub.add_parser("start", help="Start Phoenix server")
    start_parser.add_argument("--debug", action="store_true", help="Use debug build")
    start_parser.add_argument("--port", type=int, default=8000, help="Port (default: 8000)")

    sub.add_parser("stop", help="Stop Phoenix server")
    sub.add_parser("status", help="Check server status")

    restart_parser = sub.add_parser("restart", help="Restart Phoenix server")
    restart_parser.add_argument("--debug", action="store_true", help="Use debug build")
    restart_parser.add_argument("--port", type=int, default=8000, help="Port (default: 8000)")

    # Task commands
    tasks_parser = sub.add_parser("tasks", help="Task management")
    tasks_sub = tasks_parser.add_subparsers(dest="tasks_command", required=True)

    tasks_sub.add_parser("ready", help="List ready tasks")

    close_parser = tasks_sub.add_parser("close", help="Close a task")
    close_parser.add_argument("task_id", help="Task ID (e.g., 001)")
    close_parser.add_argument("--wont-do", action="store_true", help="Mark as won't-do instead of done")

    args = parser.parse_args()

    if args.command == "lint":
        cmd_lint()
    elif args.command == "build":
        cmd_build()
    elif args.command == "start":
        cmd_start(release=not args.debug, port=args.port)
    elif args.command == "stop":
        cmd_stop()
    elif args.command == "status":
        cmd_status()
    elif args.command == "restart":
        cmd_restart(release=not args.debug, port=args.port)
    elif args.command == "tasks":
        if args.tasks_command == "ready":
            cmd_tasks_ready()
        elif args.tasks_command == "close":
            cmd_tasks_close(args.task_id, args.wont_do)


if __name__ == "__main__":
    main()
