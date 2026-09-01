#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx>=0.27,<1", "httpx-sse>=0.4,<1"]
# ///
"""Deterministic black-box ProductConversation journeys against an isolated Phoenix."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

import httpx
from httpx_sse import connect_sse

ROOT = Path(__file__).resolve().parents[2]
_RUN_SPEC = importlib.util.spec_from_file_location("phoenix_e2e_run", Path(__file__).with_name("run.py"))
assert _RUN_SPEC and _RUN_SPEC.loader
run = importlib.util.module_from_spec(_RUN_SPEC)
_RUN_SPEC.loader.exec_module(run)

TIMEOUT = 60.0


def request(base_url: str, method: str, path: str, *, body: dict | None = None, expected: int = 200) -> dict:
    response = httpx.request(method, f"{base_url}{path}", json=body, timeout=30.0)
    if response.status_code != expected:
        raise AssertionError(f"{method} {path}: expected {expected}, got {response.status_code}: {response.text}")
    return response.json() if response.content else {}


def poll(label: str, read, predicate, timeout: float = TIMEOUT):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        last = read()
        if predicate(last):
            return last
        time.sleep(0.1)
    raise AssertionError(f"timed out waiting for {label}; last={last!r}")


class RetryableCreationScheduled(RuntimeError):
    pass


def create_product(base_url: str, cwd: Path, objective: str) -> dict:
    request_id = str(uuid.uuid4())
    body = {
        "request_id": request_id,
        "cwd": str(cwd),
        "objective": objective,
        "model": "mock",
        "effort": None,
        "llm_language": "phoenix-native",
        "images": [],
    }
    deadline = time.monotonic() + TIMEOUT
    while True:
        response = httpx.post(f"{base_url}/api/product-conversations/new", json=body, timeout=30.0)
        if response.status_code == 200:
            result = response.json()
            break
        payload = response.json()
        if payload.get("error_type") != "product_creation_retry_scheduled":
            raise AssertionError(f"POST /api/product-conversations/new: {response.status_code}: {response.text}")
        if time.monotonic() >= deadline:
            raise RetryableCreationScheduled(response.text)
        time.sleep(0.1)
    assert result["canonical_route"] == f"/product-conversations/{result['product_conversation_id']}"
    result["request_id"] = request_id
    return result


def product_snapshot(base_url: str, product_id: str) -> dict:
    return request(base_url, "GET", f"/api/product-conversations/{product_id}?message_limit=200")


def product_rows(base_url: str) -> list[dict]:
    payload = request(base_url, "GET", "/api/product-conversations")
    return payload if isinstance(payload, list) else payload.get("product_conversations", payload.get("rows", []))


def conversation(base_url: str, transcript_id: str) -> dict:
    return request(base_url, "GET", f"/api/conversations/{transcript_id}")


def connect_transcript(base_url: str, transcript_id: str) -> None:
    with httpx.Client(timeout=httpx.Timeout(connect=5.0, read=5.0, write=5.0, pool=5.0)) as client:
        with connect_sse(client, "GET", f"{base_url}/api/conversations/{transcript_id}/stream") as source:
            source.response.raise_for_status()
            next(source.iter_sse())


def wait_for_objective(base_url: str, transcript_id: str, objective: str) -> dict:
    def has_first_objective(payload: dict) -> bool:
        user_messages = [
            message for message in payload.get("messages", [])
            if message.get("message_type") == "user"
        ]
        return bool(user_messages) and user_messages[0].get("content", {}).get("text") == objective
    return poll("initial objective persistence", lambda: conversation(base_url, transcript_id), has_first_objective)


def assert_one_row(rows: list[dict], product_id: str) -> dict:
    assert len(rows) == 1, f"expected exactly one global aggregate row, got {rows!r}"
    row = rows[0]
    assert row.get("product_conversation_id") == product_id, row
    return row


def scenario_create_objective_one_row_reload(base_url: str, cwd: Path) -> dict:
    objective = "[[scenario:plain_text]] PRODUCT-JOURNEY-INITIAL-OBJECTIVE"
    created = create_product(base_url, cwd, objective)
    transcript_id = created["transcript_row_id"]
    connect_transcript(base_url, transcript_id)
    wait_for_objective(base_url, transcript_id, objective)
    snapshot = poll(
        "open product snapshot",
        lambda: product_snapshot(base_url, created["product_conversation_id"]),
        lambda value: value.get("writable_transcript_row_id") == transcript_id,
    )
    assert snapshot["ordinary_lifecycle"] == "open"
    assert snapshot["latest_transcript_row_id"] == transcript_id
    assert_one_row(product_rows(base_url), created["product_conversation_id"])

    reloaded = product_snapshot(base_url, created["canonical_route"].rsplit("/", 1)[-1])
    assert reloaded["product_conversation_id"] == created["product_conversation_id"]
    assert reloaded["writable_transcript_row_id"] == transcript_id
    assert_one_row(product_rows(base_url), created["product_conversation_id"])
    return created


def scenario_busy_stop_work(base_url: str, cwd: Path) -> None:
    created = create_product(base_url, cwd, "[[stall:1,30000]] PRODUCT-JOURNEY-BUSY")
    transcript_id = created["transcript_row_id"]
    connect_transcript(base_url, transcript_id)
    poll(
        "busy transcript",
        lambda: conversation(base_url, transcript_id),
        lambda value: value.get("presentation_mode") not in ("idle", "done"),
    )
    response = httpx.post(f"{base_url}/api/conversations/{transcript_id}/abandon-task", timeout=30.0)
    assert response.status_code == 409, response.text
    payload = response.json()
    assert payload.get("error_type") == "close_stop_work_confirmation_required", payload
    snapshot = product_snapshot(base_url, created["product_conversation_id"])
    close = snapshot.get("close") or {}
    attempt_id = close.get("attempt_id")
    assert attempt_id, snapshot
    request(base_url, "POST", f"/api/conversations/{transcript_id}/close/confirm-stop-work", body={"attempt_id": attempt_id})
    history = poll(
        "busy Close to History",
        lambda: product_snapshot(base_url, created["product_conversation_id"]),
        lambda value: value.get("ordinary_lifecycle") == "history",
    )
    assert history["writable_transcript_row_id"] is None
    assert_one_row(product_rows(base_url), created["product_conversation_id"])


def init_git_repo(path: Path) -> None:
    subprocess.run(["git", "init", "-q", "-b", "main", str(path)], check=True)
    subprocess.run(["git", "-C", str(path), "config", "user.name", "Phoenix QA"], check=True)
    subprocess.run(["git", "-C", str(path), "config", "user.email", "qa@example.invalid"], check=True)
    (path / "tracked.txt").write_text("baseline\n")
    subprocess.run(["git", "-C", str(path), "add", "tracked.txt"], check=True)
    subprocess.run(["git", "-C", str(path), "commit", "-qm", "baseline"], check=True)


def scenario_clean_close_history(base_url: str, cwd: Path) -> None:
    created = scenario_create_objective_one_row_reload(base_url, cwd)
    transcript_id = created["transcript_row_id"]
    poll("idle before Close", lambda: conversation(base_url, transcript_id), run._init_has_completed_turn)
    request(base_url, "POST", f"/api/conversations/{transcript_id}/abandon-task")
    history = poll(
        "clean Close to History",
        lambda: product_snapshot(base_url, created["product_conversation_id"]),
        lambda value: value.get("ordinary_lifecycle") == "history",
    )
    assert history["writable_transcript_row_id"] is None
    assert_one_row(product_rows(base_url), created["product_conversation_id"])


def scenario_dirty_exact_loss(base_url: str, cwd: Path) -> None:
    created = scenario_create_objective_one_row_reload(base_url, cwd)
    transcript_id = created["transcript_row_id"]
    poll("idle before dirty Close", lambda: conversation(base_url, transcript_id), run._init_has_completed_turn)
    snapshot = product_snapshot(base_url, created["product_conversation_id"])
    worktree = Path(snapshot["work_identity"]["worktree_path"])
    exact_path = worktree / "qa-exact-loss.txt"
    exact_path.write_text("uncommitted ProductConversation QA loss\n")
    response = httpx.post(f"{base_url}/api/conversations/{transcript_id}/abandon-task", timeout=30.0)
    assert response.status_code == 409, response.text
    assert response.json().get("error_type") == "close_loss_confirmation_required", response.text
    close = product_snapshot(base_url, created["product_conversation_id"])["close"]
    losses = close.get("losses") or (close.get("inspection") or {}).get("losses")
    expected_identity = "git_path_bytes_hex_v1:" + "qa-exact-loss.txt".encode().hex()
    identities = {loss.get("identity") for loss in losses}
    assert identities == {expected_identity}, losses
    confirmation = close["confirmation_snapshot"]
    request(base_url, "POST", f"/api/conversations/{transcript_id}/close/confirm-loss-retirement", body={
        "attempt_id": close["attempt_id"],
        "inspection_generation": confirmation["generation"],
        "inspection_fingerprint": confirmation["fingerprint"],
    })
    history = poll(
        "dirty Close to History",
        lambda: product_snapshot(base_url, created["product_conversation_id"]),
        lambda value: value.get("ordinary_lifecycle") == "history",
    )
    assert history["writable_transcript_row_id"] is None
    assert_one_row(product_rows(base_url), created["product_conversation_id"])


def scenario_merged_creation_recovery() -> None:
    result = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "phoenix_ide",
            "--lib",
            "runtime::creation_worker::product_creation_delivery_replay_tests::explicit_retry_after_queue_full_reuses_published_identities_without_duplicate_aggregate",
            "--",
            "--exact",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    output = result.stdout + result.stderr
    if "1 passed" not in output:
        raise AssertionError(f"merged creation-recovery selector did not run exactly one test:\n{output}")


def scenario_merged_context_continuation(base_url: str, _cwd: Path) -> None:
    run.scenario_product_conversation_context_continuation(base_url)


def is_retryable_creation_response(error: Exception) -> bool:
    if isinstance(error, RetryableCreationScheduled):
        return True
    if not isinstance(error, httpx.HTTPStatusError):
        return False
    try:
        return error.response.json().get("error_type") == "product_creation_retry_scheduled"
    except ValueError:
        return False


def run_journeys() -> None:
    run._build_binary()
    recovery_started = time.monotonic()
    scenario_merged_creation_recovery()
    print(f"✓ creation-failure/retry {time.monotonic() - recovery_started:.2f}s", flush=True)
    scenarios = [
        ("create/objective/one-row/reload", scenario_create_objective_one_row_reload),
        ("multi-transcript/handoff/latest-writable", scenario_merged_context_continuation),
        ("clean-close/history", scenario_clean_close_history),
        ("busy/stop-work-confirm", scenario_busy_stop_work),
        ("dirty/exact-loss", scenario_dirty_exact_loss),
    ]
    for name, scenario in scenarios:
        for attempt in range(1, 4):
            try:
                with tempfile.TemporaryDirectory(prefix="product-conversation-journey-") as directory:
                    root = Path(directory)
                    repo = root / "repo"
                    repo.mkdir()
                    init_git_repo(repo)
                    with run._server() as (base_url, _log_path, _pid, _profile_dir):
                        started = time.monotonic()
                        scenario(base_url, repo)
                        print(f"✓ {name} {time.monotonic() - started:.2f}s", flush=True)
                break
            except (httpx.HTTPStatusError, RetryableCreationScheduled) as error:
                if attempt == 3 or not is_retryable_creation_response(error):
                    raise
                print(f"[qa] {name}: retrying in a fresh isolated instance ({attempt}/3)", flush=True)


if __name__ == "__main__":
    run_journeys()
