#!/usr/bin/env python3
"""Measure one command's consumed CPU without changing its output streams.

This is intentionally dependency-free: `dev.py check --profile-work` inserts it
between the check harness and each step.  `wait4` reports CPU charged to the
exited command, including descendants that command waited for.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import tempfile
import time
import uuid
from pathlib import Path

SCHEMA_VERSION = 1


def _write_atomic(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            json.dump(value, output, sort_keys=True)
            output.write("\n")
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _forward_signal(child_pid: int, signum: int) -> None:
    try:
        os.kill(child_pid, signum)
    except ProcessLookupError:
        pass


def measure(command: list[str], output: Path | None = None, output_dir: Path | None = None) -> int:
    if not command:
        raise ValueError("missing command after --")

    started_wall_ns = time.time_ns()
    started_monotonic_ns = time.monotonic_ns()
    child_pid = os.fork()
    if child_pid == 0:
        try:
            os.execvp(command[0], command)
        except OSError as error:
            print(f"check profile wrapper: cannot execute {command[0]}: {error}", file=sys.stderr)
            os._exit(127)

    previous_handlers: dict[int, object] = {}
    for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        previous_handlers[signum] = signal.getsignal(signum)
        signal.signal(signum, lambda received, _frame, pid=child_pid: _forward_signal(pid, received))

    try:
        waited_pid, status, usage = os.wait4(child_pid, 0)
        assert waited_pid == child_pid
    finally:
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)

    finished_monotonic_ns = time.monotonic_ns()
    user_cpu_ms = usage.ru_utime * 1000.0
    system_cpu_ms = usage.ru_stime * 1000.0
    if os.WIFEXITED(status):
        returncode = os.WEXITSTATUS(status)
    elif os.WIFSIGNALED(status):
        returncode = -os.WTERMSIG(status)
    else:
        returncode = 1

    if output is None:
        assert output_dir is not None
        output = output_dir / f"process-{os.getpid()}-{uuid.uuid4().hex}.json"
    _write_atomic(output, {
        "schema_version": SCHEMA_VERSION,
        "provenance": "exact_process_tree",
        "command": command,
        "pid": child_pid,
        "started_unix_ns": started_wall_ns,
        "duration_ms": (finished_monotonic_ns - started_monotonic_ns) / 1_000_000.0,
        "user_cpu_ms": user_cpu_ms,
        "system_cpu_ms": system_cpu_ms,
        "cpu_ms": user_cpu_ms + system_cpu_ms,
        # wait4 is exact for the command and descendants it reaped. A daemonized
        # descendant is outside that accounting boundary and cannot be proven
        # closed portably from this wrapper.
        "tree_closure": "command_reaped_descendants_unverified",
        "returncode": returncode,
    })
    return returncode if returncode >= 0 else 128 - returncode


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    destination = parser.add_mutually_exclusive_group(required=True)
    destination.add_argument("--output", type=Path)
    destination.add_argument("--output-dir", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("missing command after --")
    return measure(command, args.output, args.output_dir)


if __name__ == "__main__":
    raise SystemExit(main())
