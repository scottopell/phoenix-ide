#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# ///
"""Development tasks for phoenix-ide."""

import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).parent
TASKS_DIR = ROOT / "tasks"


def cmd_lint():
    """Run clippy and fmt check."""
    subprocess.run(["cargo", "clippy", "--", "-D", "warnings"], check=True)
    subprocess.run(["cargo", "fmt", "--check"], check=True)


def cmd_build():
    """Build the project."""
    subprocess.run(["cargo", "build"], check=True)


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
    elif args.command == "tasks":
        if args.tasks_command == "ready":
            cmd_tasks_ready()
        elif args.tasks_command == "close":
            cmd_tasks_close(args.task_id, args.wont_do)


if __name__ == "__main__":
    main()
