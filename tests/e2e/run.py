#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "httpx",
#     "httpx-sse",
# ]
# ///
"""E2E API-boundary tests using the mock LLM provider.

Spawns a real phoenix-ide binary on an ephemeral port with an isolated
SQLite DB and PHOENIX_ENABLE_MOCK_MODEL=1, then drives it through a
battery of scripted conversations using the same HTTP/SSE surface that
phoenix-client.py uses.

Tests select mock scenarios with the `[[scenario:NAME]]` marker
(see crates/phoenix-ide/src/llm/mock.rs).

Exit code 0 if all scenarios pass; 1 otherwise.


Adding a new scenario
---------------------
1. If you can express the test with one of the existing mock variants,
   skip to step 3. Grep `[[scenario:` in this file for what already
   exists; the marker name is the source-of-truth pointer — grep the
   same string in `crates/phoenix-ide/src/llm/mock.rs` to see the
   scripted response.

2. If you need a new mock response shape:
   - Add a variant to `enum Scenario` in mock.rs
   - Add a `"name" => Some(Scenario::Variant)` match arm in
     `parse_scenario`
   - Add a `Scenario::Variant => (content, streamable_text)` arm in
     `build_response`
   - List the new NAME in the doc comment above `parse_scenario`
   - Add a marker test in the `tests` module at the bottom

3. Add a `scenario_xxx(base_url)` function below. Use the helpers
   `_new_conv`, `_cancel`, `_get_conv`, `_poll_to_idle_with_messages`,
   `_send_chat_and_stream`, `_agent_text`, `_has_tool_use`,
   `_count_tool_use`, `_user_messages`, `_user_message_images`. Raise
   `AssertionError` (with a useful message) on failure.

4. Register it in the `SCENARIOS` list near the bottom of this file.
   The list is ordered: faster scenarios first so the slowest get the
   tail of the wall clock if you're iterating locally.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
try:
    import resource
except ImportError:  # Windows: profiling unavailable; normal E2E remains usable.
    resource = None
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock
import uuid
from contextlib import contextmanager
from pathlib import Path

import httpx
from httpx_sse import aconnect_sse

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target" / "debug" / "phoenix_ide"
STARTUP_ATTEMPTS = 3
# A crash is caught immediately via proc.poll() with its log, so this ceiling
# only bounds the "alive but not yet serving" case. 30s was too tight when the
# e2e server cold-starts while the rest of ./dev.py check saturates the machine;
# 60s covers a CPU-starved start without masking a real startup hang.
STARTUP_TIMEOUT_SECONDS = 60.0
SCENARIO_TIMEOUT_SECONDS = 45.0
PROFILE_ENV = "PHOENIX_CHECK_PROFILE_DIR"
SCHEMA_VERSION = 1
PROVENANCE = "windowed_process"



# 1x1 transparent PNG, base64-encoded.
TINY_PNG_B64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAA"
    "AAYAAjCB0C8AAAAASUVORK5CYII="
)


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _build_binary() -> None:
    print("[e2e] cargo build --bin phoenix-ide ...", flush=True)
    t0 = time.monotonic()
    # --quiet keeps the lane's stdout focused on test results; rustc errors
    # still surface on stderr and via the non-zero exit code.
    res = subprocess.run(
        ["cargo", "build", "--bin", "phoenix_ide", "--quiet"],
        cwd=ROOT,
    )
    if res.returncode != 0:
        sys.exit(res.returncode)
    print(f"[e2e] build done in {time.monotonic() - t0:.1f}s", flush=True)


def _profile_dir_from_env() -> Path | None:
    configured = os.environ.get(PROFILE_ENV, "").strip()
    if not configured:
        return None
    return Path(configured)


def _append_jsonl(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as output:
        json.dump(value, output, sort_keys=True)
        output.write("\n")


def _harness_cpu_times() -> tuple[float | None, float | None, float]:
    if resource is not None:
        own = resource.getrusage(resource.RUSAGE_SELF)
        return own.ru_utime, own.ru_stime, own.ru_utime + own.ru_stime
    process = os.times()
    user = process.user + process.children_user
    system = process.system + process.children_system
    return user, system, user + system


def _harness_with_waited_children_cpu_times() -> tuple[float | None, float | None, float]:
    own = _harness_cpu_times()
    if resource is None:
        return own
    children = resource.getrusage(resource.RUSAGE_CHILDREN)
    user = own[0] + children.ru_utime
    system = own[1] + children.ru_stime
    return user, system, user + system


def _linux_proc_cpu_times(stat: str, hz: float) -> tuple[float, float, float] | None:
    rparen = stat.rfind(")")
    if rparen < 0:
        return None
    fields = stat[rparen + 2 :].split()
    try:
        user = (int(fields[11]) + int(fields[13])) / hz
        system = (int(fields[12]) + int(fields[14])) / hz
    except (IndexError, ValueError, ZeroDivisionError):
        return None
    return user, system, user + system


def _darwin_rusage_cpu_times(info) -> tuple[float, float, float]:
    nanoseconds_per_second = 1_000_000_000
    user = (info.ri_user_time + info.ri_child_user_time) / nanoseconds_per_second
    system = (info.ri_system_time + info.ri_child_system_time) / nanoseconds_per_second
    return user, system, user + system


def _darwin_process_cpu_times(pid: int) -> tuple[float, float, float] | None:
    import ctypes

    class RusageInfoV2(ctypes.Structure):
        _fields_ = [
            ("ri_uuid", ctypes.c_uint8 * 16),
            ("ri_user_time", ctypes.c_uint64),
            ("ri_system_time", ctypes.c_uint64),
            ("ri_pkg_idle_wkups", ctypes.c_uint64),
            ("ri_interrupt_wkups", ctypes.c_uint64),
            ("ri_pageins", ctypes.c_uint64),
            ("ri_wired_size", ctypes.c_uint64),
            ("ri_resident_size", ctypes.c_uint64),
            ("ri_phys_footprint", ctypes.c_uint64),
            ("ri_proc_start_abstime", ctypes.c_uint64),
            ("ri_proc_exit_abstime", ctypes.c_uint64),
            ("ri_child_user_time", ctypes.c_uint64),
            ("ri_child_system_time", ctypes.c_uint64),
            ("ri_child_pkg_idle_wkups", ctypes.c_uint64),
            ("ri_child_interrupt_wkups", ctypes.c_uint64),
            ("ri_child_pageins", ctypes.c_uint64),
            ("ri_child_elapsed_abstime", ctypes.c_uint64),
            ("ri_diskio_bytesread", ctypes.c_uint64),
            ("ri_diskio_byteswritten", ctypes.c_uint64),
        ]

    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    proc_pid_rusage = libproc.proc_pid_rusage
    proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    proc_pid_rusage.restype = ctypes.c_int
    info = RusageInfoV2()
    if proc_pid_rusage(pid, 2, ctypes.byref(info)) != 0:
        return None
    return _darwin_rusage_cpu_times(info)


def _process_cpu_times(
    pid: int,
) -> tuple[float | None, float | None, float] | None:
    if pid == os.getpid():
        if resource is not None:
            usage = resource.getrusage(resource.RUSAGE_SELF)
            return usage.ru_utime, usage.ru_stime, usage.ru_utime + usage.ru_stime
        process = os.times()
        return process.user, process.system, process.user + process.system
    if sys.platform == "linux":
        try:
            with open(f"/proc/{pid}/stat", "r", encoding="utf-8") as stat_file:
                stat = stat_file.read()
        except FileNotFoundError:
            return None
        hz = os.sysconf(os.sysconf_names["SC_CLK_TCK"])
        return _linux_proc_cpu_times(stat, hz)
    if sys.platform == "darwin":
        return _darwin_process_cpu_times(pid)
    return None


def _profile_writer(profile_dir: Path | None):
    if profile_dir is None:
        return None
    return profile_dir / "e2e-scenario-cpu.jsonl"


def _cpu_window_record(
    *,
    identity: str,
    started_wall_ns: int,
    started_monotonic_ns: int,
    finished_monotonic_ns: int,
    start_cpu: tuple[float | None, float | None, float] | None,
    finish_cpu: tuple[float | None, float | None, float] | None,
    extra: dict | None = None,
) -> dict:
    available = start_cpu is not None and finish_cpu is not None
    components_available = (
        available
        and start_cpu[0] is not None and finish_cpu[0] is not None
        and start_cpu[1] is not None and finish_cpu[1] is not None
    )
    user_cpu_ms = (
        max(0.0, (finish_cpu[0] - start_cpu[0]) * 1000.0)
        if components_available else None
    )
    system_cpu_ms = (
        max(0.0, (finish_cpu[1] - start_cpu[1]) * 1000.0)
        if components_available else None
    )
    total_cpu_ms = (
        max(0.0, (finish_cpu[2] - start_cpu[2]) * 1000.0) if available else None
    )
    record = {
        "schema_version": SCHEMA_VERSION,
        "provenance": (
            PROVENANCE if components_available
            else "windowed_process_total_only" if available
            else "unavailable"
        ),
        "identity": identity,
        "started_unix_ns": started_wall_ns,
        "wall_ms": (finished_monotonic_ns - started_monotonic_ns) / 1_000_000.0,
        "user_cpu_ms": user_cpu_ms,
        "system_cpu_ms": system_cpu_ms,
        "total_cpu_ms": total_cpu_ms,
    }
    if extra:
        record.update(extra)
    return record


def _write_cpu_window(
    profile_dir: Path | None,
    *,
    identity: str,
    started_wall_ns: int,
    started_monotonic_ns: int,
    finished_monotonic_ns: int,
    start_cpu: tuple[float | None, float | None, float] | None,
    finish_cpu: tuple[float | None, float | None, float] | None,
    extra: dict | None = None,
) -> None:
    destination = _profile_writer(profile_dir)
    if destination is None:
        return
    _append_jsonl(
        destination,
        _cpu_window_record(
            identity=identity,
            started_wall_ns=started_wall_ns,
            started_monotonic_ns=started_monotonic_ns,
            finished_monotonic_ns=finished_monotonic_ns,
            start_cpu=start_cpu,
            finish_cpu=finish_cpu,
            extra=extra,
        ),
    )


class _StartupFailure(RuntimeError):
    def __init__(self, message: str, *, retryable: bool = False):
        super().__init__(message)
        self.retryable = retryable


def _is_addr_in_use(log_text: str) -> bool:
    return "kind: AddrInUse" in log_text or "Address already in use" in log_text


def _start_server_attempt(env: dict[str, str], tmpdir: Path, attempt: int):
    port = _free_port()
    attempt_env = env | {"PHOENIX_PORT": str(port)}
    log_path = tmpdir / f"phoenix-startup-{attempt}.log"
    log_file = log_path.open("w")
    proc = subprocess.Popen(
        [str(BINARY)],
        cwd=tmpdir,
        env=attempt_env,
        stdout=log_file,
        stderr=subprocess.STDOUT,
    )

    base_url = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + STARTUP_TIMEOUT_SECONDS
    last_err: Exception | None = None
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            log_file.close()
            log_text = log_path.read_text()
            raise _StartupFailure(
                f"phoenix-ide exited during startup (code {proc.returncode})\n"
                f"--- log ({log_path}) ---\n{log_text}",
                retryable=_is_addr_in_use(log_text),
            )
        try:
            response = httpx.get(f"{base_url}/version", timeout=2.0)
            if response.status_code == 200:
                return proc, base_url, log_path, log_file
        except Exception as error:
            last_err = error
        time.sleep(0.1)

    proc.kill()
    proc.wait(timeout=5)
    log_file.close()
    raise _StartupFailure(
        f"phoenix-ide did not become healthy in {STARTUP_TIMEOUT_SECONDS:g}s: {last_err}\n"
        f"--- log ({log_path}) ---\n{log_path.read_text()}"
    )


def _start_server_with_retries(env: dict[str, str], tmpdir: Path, start_attempt=_start_server_attempt):
    for attempt in range(1, STARTUP_ATTEMPTS + 1):
        try:
            return start_attempt(env, tmpdir, attempt)
        except _StartupFailure as error:
            if not error.retryable or attempt == STARTUP_ATTEMPTS:
                raise
            print(
                f"[e2e] startup port was taken; retrying with a fresh port "
                f"({attempt}/{STARTUP_ATTEMPTS})",
                flush=True,
            )
    raise AssertionError("bounded startup loop completed without a result")


def _server_env(tmpdir: Path, parent_env: dict[str, str] | None = None) -> dict[str, str]:
    env = dict(os.environ if parent_env is None else parent_env)
    isolated_home = tmpdir / "home"
    isolated_config = tmpdir / "config"
    isolated_codex = tmpdir / "codex"
    isolated_data = tmpdir / "data"
    for path in (isolated_home, isolated_config, isolated_codex, isolated_data):
        path.mkdir(parents=True, exist_ok=True)

    # The mock server must not discover personal providers, credentials, or MCP
    # configuration. Keep PATH and other process essentials from the caller, but
    # give every home/config lookup an empty per-run root.
    env.update(
        {
            "HOME": str(isolated_home),
            "USERPROFILE": str(isolated_home),
            "XDG_CONFIG_HOME": str(isolated_config),
            "CODEX_HOME": str(isolated_codex),
            "PHOENIX_DATA_DIR": str(isolated_data),
            "PHOENIX_ENABLE_MOCK_MODEL": "1",
            "DEFAULT_MODEL": "mock",
            "PHOENIX_DB_PATH": str(tmpdir / "phoenix.db"),
            "PHOENIX_BIND_ADDR": "127.0.0.1",
            "PHOENIX_LOG_STDOUT": "true",
            "PHOENIX_TRACE_EXPORTER": "none",
            "RUST_LOG": env.get("E2E_RUST_LOG", "warn"),
        }
    )
    for key in tuple(env):
        if key.startswith(("DD_", "OTEL_")):
            env.pop(key)
    for key in (
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "LLM_API_KEY_HELPER",
        "OPENAI_USE_CODEX_AUTH",
        "PHOENIX_PASSWORD",
        "PHOENIX_TLS",
        "PHOENIX_TLS_CERT_PATH",
        "PHOENIX_TLS_KEY_PATH",
        "PHOENIX_LOG_FILE",
    ):
        env.pop(key, None)
    return env


@contextmanager
def _server():
    tmpdir = Path(tempfile.mkdtemp(prefix="phoenix-e2e-"))
    env = _server_env(tmpdir)
    profile_dir = _profile_dir_from_env()
    startup_started_wall_ns = time.time_ns()
    startup_started_monotonic_ns = time.monotonic_ns()
    startup_started_cpu = _harness_cpu_times() if profile_dir is not None else None

    try:
        proc, base_url, log_path, log_file = _start_server_with_retries(env, tmpdir)
    except Exception:
        startup_finished_monotonic_ns = time.monotonic_ns()
        _write_cpu_window(
            profile_dir,
            identity="e2e:startup:harness",
            started_wall_ns=startup_started_wall_ns,
            started_monotonic_ns=startup_started_monotonic_ns,
            finished_monotonic_ns=startup_finished_monotonic_ns,
            start_cpu=startup_started_cpu,
            finish_cpu=_harness_cpu_times() if profile_dir is not None else None,
            extra={
                "kind": "e2e_startup", "process_role": "harness",
                "status": "failed",
            },
        )
        shutil.rmtree(tmpdir, ignore_errors=True)
        raise

    startup_finished_monotonic_ns = time.monotonic_ns()
    startup_finished_cpu = _harness_cpu_times() if profile_dir is not None else None
    _write_cpu_window(
        profile_dir,
        identity="e2e:startup:harness",
        started_wall_ns=startup_started_wall_ns,
        started_monotonic_ns=startup_started_monotonic_ns,
        finished_monotonic_ns=startup_finished_monotonic_ns,
        start_cpu=startup_started_cpu,
        finish_cpu=startup_finished_cpu,
        extra={"kind": "e2e_startup", "process_role": "harness"},
    )
    if profile_dir is not None:
        startup_server_cpu = _process_cpu_times(proc.pid)
        if startup_server_cpu is not None:
            startup_server_zero = (
                0.0 if startup_server_cpu[0] is not None else None,
                0.0 if startup_server_cpu[1] is not None else None,
                0.0,
            )
            _write_cpu_window(
                profile_dir,
                identity="e2e:startup:server",
                started_wall_ns=startup_started_wall_ns,
                started_monotonic_ns=startup_started_monotonic_ns,
                finished_monotonic_ns=startup_finished_monotonic_ns,
                start_cpu=startup_server_zero,
                finish_cpu=startup_server_cpu,
                extra={"kind": "e2e_startup", "process_role": "server"},
            )

    try:
        yield base_url, log_path, proc.pid, profile_dir
    finally:
        teardown_started_wall_ns = time.time_ns()
        teardown_started_monotonic_ns = time.monotonic_ns()
        teardown_harness_cpu = (
            _harness_cpu_times() if profile_dir is not None else None
        )
        teardown_server_cpu = (
            _process_cpu_times(proc.pid) if profile_dir is not None else None
        )
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
        teardown_finished_monotonic_ns = time.monotonic_ns()
        teardown_finished_harness_cpu = (
            _harness_cpu_times() if profile_dir is not None else None
        )
        _write_cpu_window(
            profile_dir,
            identity="e2e:teardown:harness",
            started_wall_ns=teardown_started_wall_ns,
            started_monotonic_ns=teardown_started_monotonic_ns,
            finished_monotonic_ns=teardown_finished_monotonic_ns,
            start_cpu=teardown_harness_cpu,
            finish_cpu=teardown_finished_harness_cpu,
            extra={"kind": "e2e_teardown", "process_role": "harness"},
        )
        # The exited server can no longer be sampled. Its last cumulative sample
        # is still emitted explicitly, with honest incomplete provenance.
        if profile_dir is not None and teardown_server_cpu is not None:
            _write_cpu_window(
                profile_dir,
                identity="e2e:teardown:server",
                started_wall_ns=teardown_started_wall_ns,
                started_monotonic_ns=teardown_started_monotonic_ns,
                finished_monotonic_ns=teardown_finished_monotonic_ns,
                start_cpu=teardown_server_cpu,
                finish_cpu=None,
                extra={
                    "kind": "e2e_teardown", "process_role": "server",
                    "measurement_note": "server exited before final cumulative sample",
                },
            )
        log_file.close()
        # Clean up the per-run tempdir (DB + logs). Without this, repeated
        # local / CI runs leak /tmp/phoenix-e2e-* directories.
        shutil.rmtree(tmpdir, ignore_errors=True)


# ----------------------- minimal client helpers -----------------------


def _default_model(base_url: str) -> str:
    response = httpx.get(f"{base_url}/api/models", timeout=10.0)
    response.raise_for_status()
    model = response.json().get("default")
    if not isinstance(model, str) or not model:
        raise AssertionError("/api/models did not advertise a default model")
    return model


def _new_conv(
    base_url: str,
    text: str,
    images: list[dict] | None = None,
    cwd: str | None = None,
) -> dict:
    payload = {
        "cwd": cwd if cwd is not None else str(ROOT),
        "model": _default_model(base_url),
        "text": text,
        "images": images or [],
        "message_id": str(uuid.uuid4()),
    }
    r = httpx.post(f"{base_url}/api/conversations/new", json=payload, timeout=10.0)
    r.raise_for_status()
    return r.json()["conversation"]


def _new_conv_in(base_url: str, cwd: str, text: str, images: list[dict] | None = None) -> dict:
    """Convenience wrapper for scenarios that need an isolated cwd."""
    return _new_conv(base_url, text, images=images, cwd=cwd)


def _cancel(base_url: str, conv_id: str) -> dict:
    r = httpx.post(f"{base_url}/api/conversations/{conv_id}/cancel", timeout=10.0)
    r.raise_for_status()
    return r.json()


def _get_conv(base_url: str, conv_id: str) -> dict:
    r = httpx.get(f"{base_url}/api/conversations/{conv_id}", timeout=10.0)
    r.raise_for_status()
    return r.json()


def _state_str(state) -> str:
    if isinstance(state, dict):
        return state.get("type", "unknown")
    return str(state)


def _init_has_completed_turn(data: dict) -> bool:
    conversation = data.get("conversation") or {}
    state = _state_str(conversation.get("state"))
    if state == "error":
        state_data = conversation.get("state_data") or {}
        raise RuntimeError(f"conversation error: {state_data.get('message')}")

    if data.get("presentation_mode") not in ("idle", "done"):
        return False

    messages = data.get("messages") or []
    latest_user_index = max(
        (index for index, message in enumerate(messages) if message.get("message_type") == "user"),
        default=-1,
    )
    return any(
        message.get("message_type") == "agent"
        for message in messages[latest_user_index + 1 :]
    )


def _terminal_event(event_type: str, raw_data: str) -> bool:
    if event_type == "ping":
        return False

    try:
        data = json.loads(raw_data) if raw_data else {}
    except json.JSONDecodeError as error:
        raise ValueError(
            f"malformed JSON in SSE event {event_type!r}: {raw_data[:200]!r}"
        ) from error

    if event_type == "init":
        return _init_has_completed_turn(data)
    if event_type == "state_change":
        presentation = data.get("presentation_mode")
        state = _state_str(data.get("state"))
        if state == "error":
            state_data = data.get("state_data") or {}
            raise RuntimeError(f"conversation error: {state_data.get('message')}")
        return presentation in ("idle", "done")
    if event_type == "agent_done":
        return True
    if event_type == "error":
        message = data.get("message") or raw_data or "(no data)"
        raise RuntimeError(f"sse error: {message}")
    return False


def _timeout_diagnostic(base_url: str, conv_id: str) -> str:
    try:
        response = httpx.get(
            f"{base_url}/api/conversations/{conv_id}",
            timeout=httpx.Timeout(3.0),
        )
        response.raise_for_status()
        snapshot = response.json()
        conversation = snapshot["conversation"]
        state = _state_str(conversation.get("state"))
        state_data = conversation.get("state_data")
        messages = snapshot.get("messages") or []
        message_types = [message.get("message_type") for message in messages]
        return (
            f"last state={state!r}, state_data={state_data!r}, "
            f"message_types={message_types!r}"
        )
    except Exception as error:
        return f"final state unavailable ({type(error).__name__}: {error})"


def _has_agent_after_message(messages: list[dict], message_id: str) -> bool:
    user_index = next(
        (
            index
            for index, message in enumerate(messages)
            if message.get("message_type") == "user"
            and message.get("message_id") == message_id
        ),
        None,
    )
    return user_index is not None and any(
        message.get("message_type") == "agent"
        for message in messages[user_index + 1 :]
    )


async def _snapshot_has_agent_after_message(
    client: httpx.AsyncClient,
    base_url: str,
    conv_id: str,
    message_id: str,
) -> bool:
    response = await client.get(f"{base_url}/api/conversations/{conv_id}")
    response.raise_for_status()
    snapshot = response.json()
    conversation = snapshot["conversation"]
    state = _state_str(conversation.get("state"))
    if state == "error":
        state_data = conversation.get("state_data") or {}
        raise RuntimeError(f"conversation error: {state_data.get('message')}")
    return state == "idle" and _has_agent_after_message(
        snapshot.get("messages") or [], message_id
    )


async def _new_conv_and_stream_async(
    base_url: str,
    text: str,
    timeout: float,
) -> str:
    conv_id = str(uuid.uuid4())
    message_id = str(uuid.uuid4())
    payload = {
        "conversation_id": conv_id,
        "cwd": str(ROOT),
        "model": _default_model(base_url),
        "text": text,
        "images": [],
        "message_id": message_id,
    }
    stream_url = f"{base_url}/api/conversations/{conv_id}/stream"
    create_url = f"{base_url}/api/conversations/new"
    transport_timeout = httpx.Timeout(connect=5.0, read=20.0, write=5.0, pool=5.0)

    async with httpx.AsyncClient(timeout=transport_timeout) as client:
        async with asyncio.timeout(timeout):
            create_task = asyncio.create_task(client.post(create_url, json=payload))
            try:
                while True:
                    try:
                        async with aconnect_sse(client, "GET", stream_url) as source:
                            source.response.raise_for_status()
                            events = source.aiter_sse()
                            init = await anext(events)
                            if init.event != "init":
                                raise RuntimeError(
                                    f"expected initial SSE event, got {init.event!r}"
                                )
                            if _terminal_event(
                                init.event, init.data
                            ) and await _snapshot_has_agent_after_message(
                                client, base_url, conv_id, message_id
                            ):
                                break

                            async for event in events:
                                if _terminal_event(
                                    event.event, event.data
                                ) and await _snapshot_has_agent_after_message(
                                    client, base_url, conv_id, message_id
                                ):
                                    break
                            else:
                                raise RuntimeError(
                                    "SSE stream closed before the first turn completed"
                                )
                            break
                    except httpx.HTTPStatusError as error:
                        if error.response.status_code != 404:
                            raise
                        if create_task.done():
                            response = await create_task
                            response.raise_for_status()
                        await asyncio.sleep(0)

                response = await create_task
                response.raise_for_status()
                return conv_id
            finally:
                if not create_task.done():
                    create_task.cancel()
                    with contextlib.suppress(asyncio.CancelledError):
                        await create_task


def _new_conv_and_stream(base_url: str, text: str, timeout: float) -> str:
    try:
        return asyncio.run(_new_conv_and_stream_async(base_url, text, timeout))
    except (TimeoutError, httpx.TimeoutException) as error:
        raise TimeoutError(
            f"first-turn SSE did not reach terminal in {timeout:g}s"
        ) from error


async def _send_chat_and_stream_async(
    base_url: str,
    conv_id: str,
    text: str,
    timeout: float,
) -> int:
    stream_url = f"{base_url}/api/conversations/{conv_id}/stream"
    chat_url = f"{base_url}/api/conversations/{conv_id}/chat"
    transport_timeout = httpx.Timeout(connect=5.0, read=20.0, write=5.0, pool=5.0)
    async with httpx.AsyncClient(timeout=transport_timeout) as client:
        baseline = await client.get(f"{base_url}/api/conversations/{conv_id}")
        baseline.raise_for_status()
        baseline_count = len(baseline.json().get("messages", []))
        async with asyncio.timeout(timeout):
            async with aconnect_sse(client, "GET", stream_url) as source:
                events = source.aiter_sse()
                init = await anext(events)
                if init.event != "init":
                    raise RuntimeError(f"expected initial SSE event, got {init.event!r}")
                # Validate the snapshot, but only a post-chat event can complete
                # the continuation barrier.
                _terminal_event(init.event, init.data)

                message_id = str(uuid.uuid4())
                response = await client.post(
                    chat_url,
                    json={"text": text, "images": [], "message_id": message_id},
                )
                response.raise_for_status()
                async for event in events:
                    if event.event != "ping":
                        _terminal_event(event.event, event.data)
                    snapshot = await client.get(f"{base_url}/api/conversations/{conv_id}")
                    snapshot.raise_for_status()
                    messages = snapshot.json().get("messages", [])
                    if len(messages) >= baseline_count + 2:
                        return baseline_count + 2
            raise RuntimeError("SSE stream closed before the continuation completed")


def _send_chat_and_stream(
    base_url: str,
    conv_id: str,
    text: str,
    timeout: float,
) -> int:
    try:
        return asyncio.run(_send_chat_and_stream_async(base_url, conv_id, text, timeout))
    except (TimeoutError, httpx.TimeoutException) as error:
        diagnostic = _timeout_diagnostic(base_url, conv_id)
        raise TimeoutError(
            f"continuation SSE did not reach terminal in {timeout:g}s ({diagnostic})"
        ) from error


def _assert_next_chat_is_accepted(base_url: str, conv_id: str) -> None:
    baseline_count = len(_get_conv(base_url, conv_id).get("messages") or [])
    message_id = str(uuid.uuid4())
    response = httpx.post(
        f"{base_url}/api/conversations/{conv_id}/chat",
        json={
            "text": "[[scenario:plain_text]] ownership release probe",
            "images": [],
            "message_id": message_id,
        },
        timeout=10.0,
    )
    assert response.status_code == 200, (
        "completed direct turn did not release the next acceptance: "
        f"status={response.status_code}, body={response.text[:500]!r}"
    )

    accepted = response.json()
    assert accepted.get("steering") is not True, (
        "ownership probe was queued as steering instead of accepted as a direct turn: "
        f"body={accepted!r}"
    )

    _poll_to_idle_with_messages(
        base_url,
        conv_id,
        lambda messages: len(messages) >= baseline_count + 2,
        "accepted ownership probe response",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )


def _poll_to_idle_with_messages(
    base_url: str,
    conv_id: str,
    predicate,
    label: str,
    timeout: float = 30.0,
) -> dict:
    start = time.monotonic()
    last: dict | None = None
    while time.monotonic() - start < timeout:
        data = _get_conv(base_url, conv_id)
        last = data
        state = _state_str(data["conversation"]["state"])
        if state == "error":
            sd = data["conversation"].get("state_data") or {}
            raise RuntimeError(f"conversation error: {sd.get('message')}")
        if state == "idle" and predicate(data.get("messages") or []):
            return data
        time.sleep(0.1)
    state = _state_str(last["conversation"]["state"]) if last else "unknown"
    raise TimeoutError(
        f"poll timeout waiting for idle transcript evidence: {label} (last state: {state})"
    )


def _agent_text(messages: list[dict]) -> str:
    """Concatenate all assistant-message text blocks.

    Wire shape (verified against the running server): agent messages have
    `message_type == "agent"` and `content` is a list of blocks with
    `{"type": "text", "text": "..."}` for text blocks.
    """
    parts: list[str] = []
    for m in messages:
        if m.get("message_type") != "agent":
            continue
        content = m.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                parts.append(block.get("text", ""))
    return "\n".join(parts)


def _has_tool_use(messages: list[dict], name: str) -> bool:
    for m in messages:
        if m.get("message_type") != "agent":
            continue
        content = m.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("name") == name:
                return True
    return False


def _count_tool_use(messages: list[dict], name: str) -> int:
    n = 0
    for m in messages:
        if m.get("message_type") != "agent":
            continue
        content = m.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("name") == name:
                n += 1
    return n


def _user_messages(messages: list[dict]) -> list[dict]:
    return [m for m in messages if m.get("message_type") == "user"]


def _user_message_images(message: dict) -> list[dict]:
    content = message.get("content")
    if isinstance(content, dict):
        return list(content.get("images") or [])
    return []


# ----------------------- scenarios -----------------------


def scenario_text_streaming(base_url: str) -> None:
    word_count = 8
    conv_id = _new_conv_and_stream(
        base_url,
        f"[[perf:{word_count}]] stream this turn",
        SCENARIO_TIMEOUT_SECONDS,
    )
    final = _get_conv(base_url, conv_id)
    agent_messages = [
        message
        for message in final["messages"]
        if message.get("message_type") == "agent"
    ]
    actual_word_count = len(_agent_text(agent_messages[-1:]).split())
    assert actual_word_count == word_count, (
        f"expected {word_count} persisted streamed words, got {actual_word_count}"
    )


def scenario_multi_tool(base_url: str) -> None:
    conv = _new_conv(base_url, "[[scenario:multi_tool]] go")
    final = _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: _count_tool_use(messages, "bash") == 2,
        "two bash tool uses",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )
    n_bash = _count_tool_use(final["messages"], "bash")
    assert n_bash == 2, f"expected 2 bash tool uses, got {n_bash}"
    state = _state_str(final["conversation"]["state"])
    assert state == "idle", f"final state not idle: {state}"


def scenario_think_tool(base_url: str) -> None:
    conv = _new_conv(base_url, "[[scenario:think]] explain")
    final = _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: _has_tool_use(messages, "think"),
        "think tool use",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )
    assert _has_tool_use(final["messages"], "think"), "expected a 'think' tool use in transcript"
    # Tool result must be a success — catches input-schema drift between the
    # mock's ToolUse payload and the real tool's expected fields.
    tool_msgs = [m for m in final["messages"] if m.get("message_type") == "tool"]
    assert tool_msgs, "expected at least one tool result message"
    for m in tool_msgs:
        content = m.get("content") or {}
        assert content.get("is_error") is False, (
            f"think tool result reported error: {content!r}"
        )


def scenario_continuation(base_url: str) -> None:
    conv = _new_conv(base_url, "[[scenario:plain_text]] one")
    _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: "analyzed the situation" in _agent_text(messages),
        "first-turn response",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )
    second_turn_message_count = _send_chat_and_stream(
        base_url,
        conv["id"],
        "[[scenario:markdown]] two",
        SCENARIO_TIMEOUT_SECONDS,
    )
    _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: len(messages) >= second_turn_message_count,
        "second-turn idle completion",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )
    final = _get_conv(base_url, conv["id"])
    users = _user_messages(final["messages"])
    assert len(users) >= 2, f"expected at least 2 user messages, got {len(users)}"
    text = _agent_text(final["messages"])
    assert "analyzed the situation" in text, "first turn's assistant text missing"
    assert "## Analysis" in text, "second turn's assistant text missing"
    _assert_next_chat_is_accepted(base_url, conv["id"])


def scenario_mid_stream_cancel(base_url: str) -> None:
    """Cancel during streaming; verify state reaches idle cleanly."""
    conv = _new_conv(base_url, "[[scenario:long]] start streaming")
    # Creation returns an instant provisioning shell; poll until the worker has
    # submitted the first turn before cancelling. Waiting on the SSE iterator can
    # block past the deadline when no event arrives during async provisioning.
    deadline = time.monotonic() + 10.0
    last_state = None
    while time.monotonic() < deadline:
        snap = _get_conv(base_url, conv["id"])
        last_state = _state_str(snap["conversation"]["state"])
        if last_state not in ("idle", "provisioning"):
            break
        time.sleep(0.1)
    else:
        raise AssertionError(f"conversation did not start before cancel deadline (last: {last_state})")
    resp = _cancel(base_url, conv["id"])
    assert not resp.get("no_op", False), "cancel was a no-op — conversation already idle before we cancelled"
    # State should converge to idle within a few seconds.
    deadline = time.monotonic() + 5.0
    last_state = None
    while time.monotonic() < deadline:
        snap = _get_conv(base_url, conv["id"])
        last_state = _state_str(snap["conversation"]["state"])
        if last_state == "idle":
            return
        time.sleep(0.1)
    raise AssertionError(f"after cancel, state did not become idle (last: {last_state})")


def scenario_image_roundtrip(base_url: str) -> None:
    image = {"media_type": "image/png", "data": TINY_PNG_B64}
    conv = _new_conv(base_url, "[[scenario:plain_text]] describe", images=[image])
    final = _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: "analyzed the situation" in _agent_text(messages),
        "image-turn response",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )
    users = _user_messages(final["messages"])
    assert users, "no user message in transcript"
    images = _user_message_images(users[0])
    assert images, "image attachment did not surface in user message content"
    assert images[0].get("media_type") == "image/png"
    assert images[0].get("data") == TINY_PNG_B64, "image bytes did not round-trip intact"


def scenario_list_models(base_url: str) -> None:
    r = httpx.get(f"{base_url}/api/models", timeout=5.0)
    r.raise_for_status()
    body = r.json()
    ids = {m.get("id") for m in body.get("models", [])}
    assert "mock" in ids, f"mock model missing from /api/models: {ids}"


def scenario_read_file(base_url: str) -> None:
    # Mock scenario: see [[scenario:read_file]] in mock.rs — reads Cargo.toml
    # at the conversation's cwd (which is the project root here).
    conv = _new_conv(base_url, "[[scenario:read_file]] inspect")
    final = _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: _has_tool_use(messages, "read_file"),
        "read_file tool use",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )
    assert _has_tool_use(final["messages"], "read_file"), "expected a 'read_file' tool use"
    tool_msgs = [m for m in final["messages"] if m.get("message_type") == "tool"]
    assert tool_msgs, "expected at least one tool result message"
    for m in tool_msgs:
        content = m.get("content") or {}
        assert content.get("is_error") is False, (
            f"read_file tool result reported error: {content!r}"
        )


def scenario_patch(base_url: str) -> None:
    # Mock scenario: see [[scenario:patch]] in mock.rs — emits a patch tool
    # call that overwrites e2e-mock-patch-out.txt in the conversation cwd.
    # Use an isolated tempdir as cwd so this scenario doesn't write into the
    # repo even if patch tool semantics ever change.
    work_dir = tempfile.mkdtemp(prefix="phoenix-e2e-patch-")
    try:
        conv = _new_conv_in(base_url, work_dir, "[[scenario:patch]] write")
        final = _poll_to_idle_with_messages(
            base_url,
            conv["id"],
            lambda messages: _has_tool_use(messages, "patch"),
            "patch tool use",
            timeout=SCENARIO_TIMEOUT_SECONDS,
        )
        assert _has_tool_use(final["messages"], "patch"), "expected a 'patch' tool use"
        tool_msgs = [m for m in final["messages"] if m.get("message_type") == "tool"]
        assert tool_msgs, "expected at least one tool result message"
        for m in tool_msgs:
            content = m.get("content") or {}
            assert content.get("is_error") is False, (
                f"patch tool result reported error: {content!r}"
            )
        out_path = Path(work_dir) / "e2e-mock-patch-out.txt"
        assert out_path.exists(), f"patch tool did not create {out_path}"
        body = out_path.read_text()
        assert body == "hello from mock patch scenario\n", f"unexpected file body: {body!r}"
    finally:
        shutil.rmtree(work_dir, ignore_errors=True)


def scenario_perf_stream(base_url: str) -> None:
    # Uses the [[perf:N]] marker (see mock.rs `parse_perf_words`) — emits
    # exactly N whitespace-separated deterministic words. Catches stream
    # finalization or persistence regressions that only manifest on longer
    # streams (most other scenarios finalize in <100 tokens).
    # 100 words remains longer than the longest ordinary text scenario
    # (62-word plain_text) and the dedicated 8-word streaming scenario. No
    # finalization threshold lives at a particular N; the regression class is
    # "more chunks than the short scenarios".
    n = 100
    conv = _new_conv(base_url, f"[[perf:{n}]] go")
    final = _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: bool(_agent_text(messages)),
        "persisted assistant response",
        timeout=SCENARIO_TIMEOUT_SECONDS,
    )
    text = _agent_text(final["messages"])
    word_count = len(text.split())
    assert word_count == n, f"expected {n} words from perf stream, got {word_count}"


SCENARIOS = [
    ("list_models", scenario_list_models),
    ("text_streaming", scenario_text_streaming),
    ("multi_tool", scenario_multi_tool),
    ("think_tool", scenario_think_tool),
    ("read_file", scenario_read_file),
    ("patch", scenario_patch),
    ("continuation", scenario_continuation),
    ("mid_stream_cancel", scenario_mid_stream_cancel),
    ("image_roundtrip", scenario_image_roundtrip),
    ("perf_stream", scenario_perf_stream),
]


class HarnessIsolationTests(unittest.TestCase):
    def test_server_env_isolates_home_and_removes_real_providers(self):
        with tempfile.TemporaryDirectory() as directory:
            tmpdir = Path(directory)
            parent = {
                "PATH": "/test/bin",
                "HOME": "/real/home",
                "USERPROFILE": "/real/profile",
                "XDG_CONFIG_HOME": "/real/config",
                "CODEX_HOME": "/real/codex",
                "PHOENIX_DATA_DIR": "/real/phoenix-data",
                "PHOENIX_LOG_STDOUT": "false",
                "PHOENIX_LOG_FILE": "/real/phoenix.log",
                "PHOENIX_TRACE_EXPORTER": "datadog",
                "DD_TRACE_ENABLED": "true",
                "DD_TRACE_AGENT_URL": "http://real-agent:8126",
                "OTEL_EXPORTER_OTLP_ENDPOINT": "http://real-collector:4318",
                "ANTHROPIC_API_KEY": "secret",
                "OPENAI_API_KEY": "secret",
                "LLM_API_KEY_HELPER": "secret-helper",
                "E2E_RUST_LOG": "debug",
            }
            env = _server_env(tmpdir, parent)

            self.assertEqual(env["PATH"], "/test/bin")
            self.assertEqual(env["HOME"], str(tmpdir / "home"))
            self.assertEqual(env["USERPROFILE"], str(tmpdir / "home"))
            self.assertEqual(env["XDG_CONFIG_HOME"], str(tmpdir / "config"))
            self.assertEqual(env["CODEX_HOME"], str(tmpdir / "codex"))
            self.assertEqual(env["PHOENIX_DATA_DIR"], str(tmpdir / "data"))
            self.assertEqual(env["PHOENIX_LOG_STDOUT"], "true")
            self.assertEqual(env["PHOENIX_TRACE_EXPORTER"], "none")
            self.assertEqual(env["RUST_LOG"], "debug")
            self.assertEqual(env["DEFAULT_MODEL"], "mock")
            self.assertNotIn("ANTHROPIC_API_KEY", env)
            self.assertNotIn("OPENAI_API_KEY", env)
            self.assertNotIn("LLM_API_KEY_HELPER", env)
            self.assertNotIn("PHOENIX_LOG_FILE", env)
            self.assertNotIn("DD_TRACE_ENABLED", env)
            self.assertNotIn("DD_TRACE_AGENT_URL", env)
            self.assertNotIn("OTEL_EXPORTER_OTLP_ENDPOINT", env)

    def test_literal_ping_is_accepted_without_json_parsing(self):
        self.assertFalse(_terminal_event("ping", "ping"))

    def test_agent_done_completes_turn_barrier(self):
        self.assertTrue(_terminal_event("agent_done", "{}"))

    def test_error_event_is_actionable(self):
        with self.assertRaisesRegex(RuntimeError, "sse error: broken stream"):
            _terminal_event("error", json.dumps({"message": "broken stream"}))

    def test_error_state_change_is_actionable(self):
        data = json.dumps(
            {
                "presentation_mode": "error",
                "state": {"type": "error"},
                "state_data": {"message": "mock failed"},
            }
        )
        with self.assertRaisesRegex(RuntimeError, "conversation error: mock failed"):
            _terminal_event("state_change", data)

    def test_done_state_change_is_detected(self):
        data = json.dumps({"presentation_mode": "done", "state": "terminal"})
        self.assertTrue(_terminal_event("state_change", data))

    def test_idle_state_change_completes_turn_barrier(self):
        data = json.dumps({"presentation_mode": "idle", "state": "idle"})
        self.assertTrue(_terminal_event("state_change", data))

    def test_idle_init_with_agent_after_latest_user_is_terminal(self):
        data = json.dumps(
            {
                "presentation_mode": "idle",
                "conversation": {"state": "idle"},
                "messages": [
                    {"message_type": "user"},
                    {"message_type": "agent"},
                ],
            }
        )
        self.assertTrue(_terminal_event("init", data))

    def test_idle_init_with_unanswered_latest_user_is_not_terminal(self):
        data = json.dumps(
            {
                "presentation_mode": "idle",
                "conversation": {"state": "idle"},
                "messages": [
                    {"message_type": "user"},
                    {"message_type": "agent"},
                    {"message_type": "user"},
                ],
            }
        )
        self.assertFalse(_terminal_event("init", data))

    def test_agent_response_is_tied_to_exact_user_message(self):
        messages = [
            {"message_type": "user", "message_id": "first"},
            {"message_type": "agent", "message_id": "agent-first"},
            {"message_type": "user", "message_id": "second"},
        ]
        self.assertTrue(_has_agent_after_message(messages, "first"))
        self.assertFalse(_has_agent_after_message(messages, "second"))
        self.assertFalse(_has_agent_after_message(messages, "missing"))

    def test_agent_response_after_exact_user_message_is_detected(self):
        messages = [
            {"message_type": "user", "message_id": "first"},
            {"message_type": "agent", "message_id": "agent-first"},
            {"message_type": "user", "message_id": "second"},
            {"message_type": "agent", "message_id": "agent-second"},
        ]
        self.assertTrue(_has_agent_after_message(messages, "second"))

    def test_malformed_typed_event_has_actionable_error(self):
        with self.assertRaisesRegex(ValueError, "malformed JSON.*state_change"):
            _terminal_event("state_change", "not-json")


class CpuProfilingTests(unittest.TestCase):
    def test_linux_process_cpu_includes_waited_children(self):
        # fields after comm begin at proc field 3; indexes 11..14 are
        # utime, stime, cutime, and cstime respectively.
        fields = ["S", *(["0"] * 10), "100", "25", "40", "10"]
        sample = _linux_proc_cpu_times(f"42 (phoenix worker) {' '.join(fields)}", 100)

        self.assertEqual((1.4, 0.35, 1.75), sample)

    def test_linux_process_cpu_rejects_malformed_stat(self):
        self.assertIsNone(_linux_proc_cpu_times("42 malformed", 100))
        self.assertIsNone(_linux_proc_cpu_times("42 (short) S 1", 100))

    def test_darwin_process_cpu_includes_waited_children(self):
        info = type("Rusage", (), {
            "ri_user_time": 1_000_000_000,
            "ri_system_time": 250_000_000,
            "ri_child_user_time": 400_000_000,
            "ri_child_system_time": 100_000_000,
        })()

        self.assertEqual((1.4, 0.35, 1.75), _darwin_rusage_cpu_times(info))

    @unittest.skipUnless(sys.platform == "darwin", "macOS proc_pid_rusage only")
    def test_darwin_process_cpu_samples_live_process(self):
        sample = _darwin_process_cpu_times(os.getpid())

        self.assertIsNotNone(sample)
        self.assertGreater(sample[2], 0)

    def test_cpu_window_record_has_required_fields(self):
        started_monotonic_ns = 1_000_000
        finished_monotonic_ns = 3_500_000
        record = _cpu_window_record(
            identity="e2e:scenario:demo:harness",
            started_wall_ns=123,
            started_monotonic_ns=started_monotonic_ns,
            finished_monotonic_ns=finished_monotonic_ns,
            start_cpu=(1.0, 2.0, 3.0),
            finish_cpu=(1.25, 2.5, 3.75),
            extra={"kind": "e2e_scenario", "process_role": "harness"},
        )

        self.assertEqual(1, record["schema_version"])
        self.assertEqual("windowed_process", record["provenance"])
        self.assertEqual("e2e:scenario:demo:harness", record["identity"])
        self.assertEqual(2.5, record["wall_ms"])
        self.assertEqual(250.0, record["user_cpu_ms"])
        self.assertEqual(500.0, record["system_cpu_ms"])
        self.assertEqual(750.0, record["total_cpu_ms"])
        self.assertEqual("harness", record["process_role"])

    def test_total_only_cpu_window_does_not_fabricate_components(self):
        record = _cpu_window_record(
            identity="e2e:server",
            started_wall_ns=123,
            started_monotonic_ns=1_000_000,
            finished_monotonic_ns=2_000_000,
            start_cpu=(None, None, 1.0),
            finish_cpu=(None, None, 1.5),
        )

        self.assertEqual("windowed_process_total_only", record["provenance"])
        self.assertIsNone(record["user_cpu_ms"])
        self.assertIsNone(record["system_cpu_ms"])
        self.assertEqual(500.0, record["total_cpu_ms"])

    @unittest.skipIf(resource is None, "resource module unavailable")
    def test_harness_cpu_excludes_waited_children(self):
        own = type("Rusage", (), {"ru_utime": 1.0, "ru_stime": 0.25})()
        children = type("Rusage", (), {"ru_utime": 4.0, "ru_stime": 2.0})()

        with mock.patch.object(resource, "getrusage", side_effect=[own, own, children]):
            harness = _harness_cpu_times()
            combined = _harness_with_waited_children_cpu_times()

        self.assertEqual((1.0, 0.25, 1.25), harness)
        self.assertEqual((5.0, 2.25, 7.25), combined)

    def test_write_cpu_window_appends_jsonl(self):
        with tempfile.TemporaryDirectory() as directory:
            profile_dir = Path(directory)
            _write_cpu_window(
                profile_dir,
                identity="e2e:startup:harness",
                started_wall_ns=100,
                started_monotonic_ns=0,
                finished_monotonic_ns=2_000_000,
                start_cpu=(0.0, 0.0, 0.0),
                finish_cpu=(0.1, 0.2, 0.3),
                extra={"kind": "e2e_startup", "process_role": "harness"},
            )
            lines = (profile_dir / "e2e-scenario-cpu.jsonl").read_text().splitlines()

        self.assertEqual(1, len(lines))
        record = json.loads(lines[0])
        self.assertEqual("e2e:startup:harness", record["identity"])
        self.assertEqual(300.0, record["total_cpu_ms"])


class StartupRetryTests(unittest.TestCase):
    def test_addr_in_use_detection_accepts_rust_and_os_messages(self):
        self.assertTrue(_is_addr_in_use('Error: Os { code: 48, kind: AddrInUse }'))
        self.assertTrue(_is_addr_in_use("bind failed: Address already in use"))
        self.assertFalse(_is_addr_in_use("database migration failed"))

    def test_retries_retryable_failures_and_returns_success(self):
        attempts: list[int] = []
        expected = object()

        def start_attempt(_env, _tmpdir, attempt):
            attempts.append(attempt)
            if attempt < STARTUP_ATTEMPTS:
                raise _StartupFailure("port taken", retryable=True)
            return expected

        actual = _start_server_with_retries({}, Path("/unused"), start_attempt)
        self.assertIs(actual, expected)
        self.assertEqual(attempts, [1, 2, 3])

    def test_non_retryable_failure_stops_immediately(self):
        attempts: list[int] = []

        def start_attempt(_env, _tmpdir, attempt):
            attempts.append(attempt)
            raise _StartupFailure("bad config")

        with self.assertRaisesRegex(_StartupFailure, "bad config"):
            _start_server_with_retries({}, Path("/unused"), start_attempt)
        self.assertEqual(attempts, [1])

    def test_retryable_failure_stops_at_attempt_bound(self):
        attempts: list[int] = []

        def start_attempt(_env, _tmpdir, attempt):
            attempts.append(attempt)
            raise _StartupFailure("port taken", retryable=True)

        with self.assertRaisesRegex(_StartupFailure, "port taken"):
            _start_server_with_retries({}, Path("/unused"), start_attempt)
        self.assertEqual(attempts, list(range(1, STARTUP_ATTEMPTS + 1)))


def _run_self_tests() -> int:
    suite = unittest.TestSuite(
        unittest.defaultTestLoader.loadTestsFromTestCase(case)
        for case in (HarnessIsolationTests, CpuProfilingTests, StartupRetryTests)
    )
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def main() -> int:
    if _run_self_tests() != 0:
        return 1
    _build_binary()
    failures: list[tuple[str, str]] = []
    log_text = ""
    with _server() as (base_url, log_path, server_pid, profile_dir):
        print(f"[e2e] server up at {base_url}", flush=True)
        for name, fn in SCENARIOS:
            started_wall_ns = time.time_ns()
            started_monotonic_ns = time.monotonic_ns()
            started_harness_cpu = (
                _harness_cpu_times() if profile_dir is not None else None
            )
            started_server_cpu = (
                _process_cpu_times(server_pid) if profile_dir is not None else None
            )
            scenario_status = "failed"
            try:
                fn(base_url)
                scenario_status = "passed"
                dt = (time.monotonic_ns() - started_monotonic_ns) / 1_000_000_000.0
                print(f"  ✓ {name:<22s} {dt:6.2f}s", flush=True)
            except Exception as e:
                dt = (time.monotonic_ns() - started_monotonic_ns) / 1_000_000_000.0
                detail = f"{type(e).__name__}: {e}"
                print(f"  ✗ {name:<22s} {dt:6.2f}s  {detail}", flush=True)
                failures.append((name, detail))
                print("[e2e] stopping after first failure to preserve root-cause signal", flush=True)
                break
            finally:
                finished_monotonic_ns = time.monotonic_ns()
                finished_harness_cpu = (
                    _harness_cpu_times() if profile_dir is not None else None
                )
                finished_server_cpu = (
                    _process_cpu_times(server_pid) if profile_dir is not None else None
                )
                _write_cpu_window(
                    profile_dir,
                    identity=f"e2e:scenario:{name}:harness",
                    started_wall_ns=started_wall_ns,
                    started_monotonic_ns=started_monotonic_ns,
                    finished_monotonic_ns=finished_monotonic_ns,
                    start_cpu=started_harness_cpu,
                    finish_cpu=finished_harness_cpu,
                    extra={"kind": "e2e_scenario", "process_role": "harness", "scenario": name, "status": scenario_status},
                )
                _write_cpu_window(
                    profile_dir,
                    identity=f"e2e:scenario:{name}:server",
                    started_wall_ns=started_wall_ns,
                    started_monotonic_ns=started_monotonic_ns,
                    finished_monotonic_ns=finished_monotonic_ns,
                    start_cpu=started_server_cpu,
                    finish_cpu=finished_server_cpu,
                    extra={"kind": "e2e_scenario", "process_role": "server", "scenario": name, "status": scenario_status},
                )
            
        # Read log inside the context — the tempdir may be cleaned up after.
        if log_path.exists():
            log_text = log_path.read_text()

    external_mcp_lines = [
        line
        for line in log_text.splitlines()
        if "MCP stderr:" in line or "Connecting to remote server:" in line
    ]
    if external_mcp_lines:
        print("\n[e2e] external MCP process output detected in hermetic server log:")
        for line in external_mcp_lines[:5]:
            print(f"  | {line}")
        failures.append(
            ("external-mcp-tripwire", f"{len(external_mcp_lines)} external MCP log line(s)")
        )

    # Tripwire: surface sqlx slow-statement WARNs so cross-lane I/O
    # contention can't silently bloat write latency (task 13042). Filter
    # out the one-time startup PRAGMA (WAL+synchronous setup) — that's an
    # admin call, not a steady-state regression signal.
    slow_lines = [
        l for l in log_text.splitlines()
        if "slow statement" in l and "PRAGMA " not in l
    ]
    if slow_lines:
        print(f"\n[e2e] {len(slow_lines)} slow-statement WARN(s) in server log:")
        for line in slow_lines[:5]:
            print(f"  | {line}")
        failures.append(
            ("slow-statement-tripwire", f"{len(slow_lines)} slow sqlx WARN(s)")
        )

    if failures:
        print(f"\n[e2e] server log tail:", flush=True)
        for line in log_text.splitlines()[-80:]:
            print(f"  | {line}")
        print(f"\n✗ {len(failures)} e2e check(s) failed")
        return 1
    print(
        f"\n✓ all {len(SCENARIOS)} e2e scenarios passed "
        "(no external MCP or slow-statement WARNs)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(_run_self_tests() if "--self-test" in sys.argv[1:] else main())
