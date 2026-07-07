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
   `_new_conv`, `_send_chat`, `_cancel`, `_get_conv`, `_drive`,
   `_agent_text`, `_has_tool_use`, `_count_tool_use`, `_user_messages`,
   `_user_message_images`. Raise `AssertionError` (with a useful
   message) on failure.

4. Register it in the `SCENARIOS` list near the bottom of this file.
   The list is ordered: faster scenarios first so the slowest get the
   tail of the wall clock if you're iterating locally.
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from contextlib import contextmanager
from pathlib import Path

import httpx
from httpx_sse import connect_sse

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target" / "debug" / "phoenix_ide"
STARTUP_ATTEMPTS = 3
STARTUP_TIMEOUT_SECONDS = 30.0

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
        cwd=ROOT,
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


@contextmanager
def _server():
    tmpdir = Path(tempfile.mkdtemp(prefix="phoenix-e2e-"))
    db_path = tmpdir / "phoenix.db"
    env = os.environ.copy()
    # Strip every channel that could register a non-mock provider so the
    # registry contains exactly one model and behavior is reproducible.
    for k in (
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "LLM_API_KEY_HELPER",
        "OPENAI_USE_CODEX_AUTH",
        "PHOENIX_PASSWORD",
        "PHOENIX_TLS",
        "PHOENIX_TLS_CERT_PATH",
        "PHOENIX_TLS_KEY_PATH",
    ):
        env.pop(k, None)
    env.update(
        {
            "PHOENIX_ENABLE_MOCK_MODEL": "1",
            "DEFAULT_MODEL": "mock",
            "PHOENIX_DB_PATH": str(db_path),
            # Bind loopback: the harness talks to the server over 127.0.0.1, so a
            # loopback bind is correct and satisfies the binary's fail-closed
            # guard (no password, no insecure-bind override needed).
            "PHOENIX_BIND_ADDR": "127.0.0.1",
            # Quiet logs unless a test fails (we capture stderr and print on
            # failure). RUST_LOG=warn drops the per-request access log too,
            # which would otherwise spam the harness output.
            "RUST_LOG": os.environ.get("E2E_RUST_LOG", "warn"),
        },
    )

    try:
        proc, base_url, log_path, log_file = _start_server_with_retries(env, tmpdir)
    except Exception:
        shutil.rmtree(tmpdir, ignore_errors=True)
        raise

    try:
        yield base_url, log_path
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
        log_file.close()
        # Clean up the per-run tempdir (DB + logs). Without this, repeated
        # local / CI runs leak /tmp/phoenix-e2e-* directories.
        shutil.rmtree(tmpdir, ignore_errors=True)


# ----------------------- minimal client helpers -----------------------


def _new_conv(
    base_url: str,
    text: str,
    images: list[dict] | None = None,
    cwd: str | None = None,
) -> dict:
    payload = {
        "cwd": cwd if cwd is not None else str(ROOT),
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


def _send_chat(base_url: str, conv_id: str, text: str) -> None:
    r = httpx.post(
        f"{base_url}/api/conversations/{conv_id}/chat",
        json={"text": text, "images": [], "message_id": str(uuid.uuid4())},
        timeout=10.0,
    )
    r.raise_for_status()


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


def _stream_to_terminal(base_url: str, conv_id: str, timeout: float = 30.0) -> None:
    """Drive the SSE stream until the conversation reaches a terminal state.

    Returns nothing — callers should refetch via _get_conv for the authoritative
    final message list. The stream is purely a synchronization barrier here
    because message events fire incrementally during agent runs and would
    double-count if naively accumulated.

    A watchdog thread closes the client at `timeout` seconds. iter_sse() does
    not yield items for keepalive pings, so a per-event deadline check is not
    sufficient — if termination signaling is broken (renamed event labels,
    missing state_change), pings keep the read socket alive indefinitely.
    Closing the client from a watchdog is the only timing-robust escape hatch.
    """
    import threading
    url = f"{base_url}/api/conversations/{conv_id}/stream"
    # Read timeout exceeds the server's 15s SSE keepalive so we don't fight
    # pings during legitimate long-tool gaps; the watchdog is the actual
    # deadline.
    sse_timeout = httpx.Timeout(connect=5.0, read=20.0, write=5.0, pool=5.0)
    client = httpx.Client(timeout=sse_timeout)
    watchdog = threading.Timer(timeout, client.close)
    watchdog.daemon = True
    watchdog.start()
    try:
        with connect_sse(client, "GET", url) as src:
            for event in src.iter_sse():
                data = json.loads(event.data) if event.data else {}
                if event.event == "state_change":
                    display = data.get("display_state")
                    state = _state_str(data.get("state"))
                    if state == "error":
                        sd = data.get("state_data") or {}
                        raise RuntimeError(f"conversation error: {sd.get('message')}")
                    if display == "terminal":
                        return
                elif event.event == "agent_done":
                    return
                elif event.event == "error":
                    # Include raw event.data so an empty/malformed error
                    # payload doesn't degrade to "sse error: None".
                    msg = data.get("message") or event.data or "(no data)"
                    raise RuntimeError(f"sse error: {msg}")
    except Exception as e:
        # Watchdog-triggered close manifests as a transport error or runtime
        # error from the SSE library. Map to a clean TimeoutError if the
        # deadline has actually elapsed, otherwise re-raise.
        if not watchdog.is_alive():
            raise TimeoutError(
                f"SSE did not reach terminal in {timeout}s ({type(e).__name__})"
            ) from e
        raise
    finally:
        watchdog.cancel()
        client.close()


def _poll_to_idle(base_url: str, conv_id: str, timeout: float = 30.0) -> None:
    """Poll GET until the conversation is idle. Same barrier role as SSE."""
    start = time.monotonic()
    while time.monotonic() - start < timeout:
        data = _get_conv(base_url, conv_id)
        state = _state_str(data["conversation"]["state"])
        if state == "idle":
            return
        if state == "error":
            sd = data["conversation"].get("state_data") or {}
            raise RuntimeError(f"conversation error: {sd.get('message')}")
        time.sleep(0.1)
    raise TimeoutError(f"poll timeout after {timeout}s")


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


def _drive(base_url: str, conv_id: str, timeout: float = 30.0, use_polling: bool = False) -> dict:
    """Wait for the conversation to settle, then return the authoritative
    state via GET (not the in-flight stream snapshot)."""
    if use_polling:
        _poll_to_idle(base_url, conv_id, timeout)
    else:
        _stream_to_terminal(base_url, conv_id, timeout)
    return _get_conv(base_url, conv_id)


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
    conv = _new_conv(base_url, "[[scenario:plain_text]] hello")
    final = _drive(base_url, conv["id"], timeout=15.0)
    text = _agent_text(final["messages"])
    assert "analyzed the situation" in text, f"unexpected assistant text: {text[:200]!r}"


def scenario_multi_tool(base_url: str) -> None:
    conv = _new_conv(base_url, "[[scenario:multi_tool]] go")
    final = _drive(base_url, conv["id"], timeout=30.0)
    n_bash = _count_tool_use(final["messages"], "bash")
    assert n_bash == 2, f"expected 2 bash tool uses, got {n_bash}"
    state = _state_str(final["conversation"]["state"])
    assert state == "idle", f"final state not idle: {state}"


def scenario_think_tool(base_url: str) -> None:
    conv = _new_conv(base_url, "[[scenario:think]] explain")
    final = _drive(base_url, conv["id"], timeout=15.0)
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
    _drive(base_url, conv["id"], timeout=15.0)
    _send_chat(base_url, conv["id"], "[[scenario:markdown]] two")
    final = _drive(base_url, conv["id"], timeout=15.0)
    users = _user_messages(final["messages"])
    assert len(users) >= 2, f"expected at least 2 user messages, got {len(users)}"
    text = _agent_text(final["messages"])
    assert "analyzed the situation" in text, "first turn's assistant text missing"
    assert "## Analysis" in text, "second turn's assistant text missing"


def scenario_mid_stream_cancel(base_url: str) -> None:
    """Cancel during streaming; verify state reaches idle cleanly."""
    conv = _new_conv(base_url, "[[scenario:long]] start streaming")
    # Creation returns an instant provisioning shell; wait until the first turn
    # has actually left idle/provisioning before cancelling.
    url = f"{base_url}/api/conversations/{conv['id']}/stream"
    with httpx.Client(timeout=httpx.Timeout(connect=5.0, read=10.0, write=5.0, pool=5.0)) as client:
        with connect_sse(client, "GET", url) as src:
            deadline = time.monotonic() + 10.0
            for event in src.iter_sse():
                if time.monotonic() > deadline:
                    raise AssertionError("conversation did not start before cancel deadline")
                if event.event != "state_change":
                    continue
                data = json.loads(event.data) if event.data else {}
                state = _state_str(data.get("state"))
                if state not in ("idle", "provisioning"):
                    break
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
    final = _drive(base_url, conv["id"], timeout=15.0)
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
    #
    # Drives via polling (use_polling=True) rather than SSE so the
    # _poll_to_idle barrier — GET state converging to "idle" — stays
    # exercised. Every other scenario uses the SSE barrier; this is the one
    # place the poll path runs end-to-end.
    conv = _new_conv(base_url, "[[scenario:read_file]] inspect")
    final = _poll_to_idle_with_messages(
        base_url,
        conv["id"],
        lambda messages: _has_tool_use(messages, "read_file"),
        "read_file tool use",
        timeout=15.0,
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
        final = _drive(base_url, conv["id"], timeout=15.0)
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
        import shutil
        shutil.rmtree(work_dir, ignore_errors=True)


def scenario_perf_stream(base_url: str) -> None:
    # Uses the [[perf:N]] marker (see mock.rs `parse_perf_words`) — emits
    # exactly N whitespace-separated deterministic words. Catches stream
    # finalization or persistence regressions that only manifest on longer
    # streams (most other scenarios finalize in <100 tokens).
    # 200 words is ~3x the longest text scenario (~70-word plain_text) and
    # produces several hundred chunks — enough to exercise the same long-stream
    # finalization/persistence path a larger count would, without paying for
    # word-count the regression class doesn't depend on (no threshold lives at
    # any particular N; the bug class is "more chunks than the short scenarios").
    n = 200
    conv = _new_conv(base_url, f"[[perf:{n}]] go")
    final = _drive(base_url, conv["id"], timeout=30.0)
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
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(StartupRetryTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def main() -> int:
    if _run_self_tests() != 0:
        return 1
    _build_binary()
    failures: list[tuple[str, str]] = []
    log_text = ""
    with _server() as (base_url, log_path):
        print(f"[e2e] server up at {base_url}", flush=True)
        for name, fn in SCENARIOS:
            t0 = time.monotonic()
            try:
                fn(base_url)
                dt = time.monotonic() - t0
                print(f"  ✓ {name:<22s} {dt:6.2f}s", flush=True)
            except Exception as e:
                dt = time.monotonic() - t0
                detail = f"{type(e).__name__}: {e}"
                print(f"  ✗ {name:<22s} {dt:6.2f}s  {detail}", flush=True)
                failures.append((name, detail))
        # Read log inside the context — the tempdir may be cleaned up after.
        if log_path.exists():
            log_text = log_path.read_text()

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
    print(f"\n✓ all {len(SCENARIOS)} e2e scenarios passed (no slow-statement WARNs)")
    return 0


if __name__ == "__main__":
    sys.exit(_run_self_tests() if "--self-test" in sys.argv[1:] else main())
