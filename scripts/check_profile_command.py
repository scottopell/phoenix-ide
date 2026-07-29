#!/usr/bin/env python3
"""Dependency-free CPU window profiling helpers for Python check/test commands.

Preserves the profiled command's normal stdout/stderr while writing versioned
JSONL records when explicitly asked.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import signal
import sys
import tempfile
import time
import uuid
from pathlib import Path

SCHEMA_VERSION = 1
PROCESS_TREE_PROVENANCE = "exact_waited_descendants"
WINDOW_PROVENANCE = "windowed_process"


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


def _append_jsonl(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as output:
        json.dump(value, output, sort_keys=True)
        output.write("\n")


def _identity_suffix(identity: str) -> str:
    safe = []
    for char in identity:
        safe.append(char if char.isascii() and (char.isalnum() or char in "._-") else "-")
    suffix = "".join(safe).strip("-") or "record"
    digest = hashlib.sha256(identity.encode("utf-8")).hexdigest()[:12]
    return f"{suffix[:96]}-{digest}"


def _record(
    *,
    identity: str,
    started_wall_ns: int,
    started_monotonic_ns: int,
    finished_monotonic_ns: int,
    user_cpu_ms: float,
    system_cpu_ms: float,
    extra: dict | None = None,
    provenance: str = WINDOW_PROVENANCE,
) -> dict:
    record = {
        "schema_version": SCHEMA_VERSION,
        "provenance": provenance,
        "identity": identity,
        "started_unix_ns": started_wall_ns,
        "wall_ms": (finished_monotonic_ns - started_monotonic_ns) / 1_000_000.0,
        "user_cpu_ms": max(0.0, user_cpu_ms),
        "system_cpu_ms": max(0.0, system_cpu_ms),
    }
    record["total_cpu_ms"] = record["user_cpu_ms"] + record["system_cpu_ms"]
    if extra:
        record.update(extra)
    return record


def _write_record(
    *,
    record: dict,
    output: Path | None,
    output_dir: Path | None,
    output_jsonl: Path | None,
) -> Path | None:
    if output is not None:
        _write_atomic(output, record)
        return output
    if output_dir is not None:
        path = output_dir / f"process-{os.getpid()}-{uuid.uuid4().hex}-{_identity_suffix(record['identity'])}.json"
        _write_atomic(path, record)
        return path
    if output_jsonl is not None:
        _append_jsonl(output_jsonl, record)
        return output_jsonl
    return None


def _forward_signal(child_pid: int, signum: int) -> None:
    try:
        os.kill(child_pid, signum)
    except ProcessLookupError:
        pass


def measure(
    command: list[str],
    output: Path | None = None,
    output_dir: Path | None = None,
    output_jsonl: Path | None = None,
    identity: str | None = None,
) -> int:
    if not command:
        raise ValueError("missing command after --")

    started_wall_ns = time.time_ns()
    started_monotonic_ns = time.monotonic_ns()
    child_pid = os.fork()
    if child_pid == 0:
        try:
            # Remain in the wrapper's process group. The owning check step can
            # then terminate the complete wrapped tree with one group signal.
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
    if os.WIFEXITED(status):
        returncode = os.WEXITSTATUS(status)
    elif os.WIFSIGNALED(status):
        returncode = -os.WTERMSIG(status)
    else:
        returncode = 1

    nextest_name = os.environ.get("NEXTEST_TEST_NAME")
    nextest_binary = os.environ.get("NEXTEST_BINARY_ID")
    if nextest_name is None and "--exact" in command:
        exact_index = command.index("--exact")
        if exact_index + 1 < len(command):
            nextest_name = command[exact_index + 1]
    if nextest_binary is None and nextest_name is not None:
        nextest_binary = Path(command[0]).name
    inferred_identity = (
        f"rust:{nextest_binary}:{nextest_name}"
        if nextest_name and nextest_binary else f"command:{command[0]}"
    )
    record = _record(
        identity=identity or inferred_identity,
        started_wall_ns=started_wall_ns,
        started_monotonic_ns=started_monotonic_ns,
        finished_monotonic_ns=finished_monotonic_ns,
        user_cpu_ms=usage.ru_utime * 1000.0,
        system_cpu_ms=usage.ru_stime * 1000.0,
        provenance=PROCESS_TREE_PROVENANCE,
        extra={
            "command": command,
            "pid": child_pid,
            "returncode": returncode,
            "kind": (
                "rust_test" if nextest_name
                else "step" if (identity or "").startswith("step:")
                else "command"
            ),
            "test_name": nextest_name,
            "binary_id": nextest_binary,
            "attempt": os.environ.get("NEXTEST_ATTEMPT"),
            # wait4 is exact for the command and descendants it reaped. A daemonized
            # descendant is outside that accounting boundary and cannot be proven
            # closed portably from this wrapper.
            "tree_closure": "waited_descendants_only_survivors_unverified",
        },
    )
    _write_record(record=record, output=output, output_dir=output_dir, output_jsonl=output_jsonl)
    return returncode if returncode >= 0 else 128 - returncode


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    destination = parser.add_mutually_exclusive_group(required=True)
    destination.add_argument("--output", type=Path)
    destination.add_argument("--output-dir", type=Path)
    destination.add_argument("--output-jsonl", type=Path)
    parser.add_argument("--identity")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("missing command after --")
    return measure(
        command,
        args.output,
        args.output_dir,
        args.output_jsonl,
        args.identity,
    )


if __name__ == "__main__":
    raise SystemExit(main())
