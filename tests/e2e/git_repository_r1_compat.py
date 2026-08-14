#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.13"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""Historical-R1-to-candidate GitRepository compatibility acceptance.

Run from a clean candidate commit:
    uv run tests/e2e/git_repository_r1_compat.py

The report is acceptance output only. It is never loaded by Phoenix.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import queue
import signal
import sqlite3
import subprocess
import tempfile
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx
from httpx_sse import aconnect_sse

ROOT = Path(__file__).resolve().parents[2]
HISTORICAL_SHA = "799ea4d63c3d451f3f47859fa21df46fe3072923"
CONVERSATION_ID = "11111111-1111-4111-8111-111111111111"
MESSAGE_ID = "22222222-2222-4222-8222-222222222222"
PROJECT_ID = "33333333-3333-4333-8333-333333333333"
STARTUP_EVENT = "Phoenix IDE server listening"
OUTER_TIMEOUT = 120.0


@dataclass(frozen=True)
class Census:
    revision: str
    digest: str
    shadow_reference_count: int
    project_authority_path_count: int


def run(*args: str, cwd: Path = ROOT, env: dict[str, str] | None = None) -> str:
    completed = subprocess.run(
        args, cwd=cwd, env=env, check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    return completed.stdout.strip()


def clean_head() -> str:
    if run("git", "status", "--porcelain=v1", "--untracked-files=all"):
        raise RuntimeError("candidate checkout is dirty; commit every change before running R1 compatibility acceptance")
    return run("git", "rev-parse", "HEAD")


def census() -> Census:
    inventory_path = ROOT / "tests/e2e/git_repository_r1_census.json"
    raw = inventory_path.read_bytes()
    inventory = json.loads(raw)
    all_sources = sorted(
        str(path.relative_to(ROOT))
        for path in ROOT.glob("crates/**/*.rs")
        if "target" not in path.parts
    )
    tables = inventory["shadow_tables"]
    actual_shadow_files = sorted(
        path for path in all_sources if any(table in (ROOT / path).read_text() for table in tables)
    )
    expected_shadow_files = sorted(inventory["shadow_reference_files"])
    if actual_shadow_files != expected_shadow_files:
        raise RuntimeError(f"unreviewed shadow-table reference drift: expected {expected_shadow_files}, got {actual_shadow_files}")
    if set(inventory["shadow_production_roles"]) != set(expected_shadow_files):
        raise RuntimeError("shadow-table inventory must classify every allowlisted production file")
    reconcile = (ROOT / "crates/phoenix-db/src/git_repository_reconciliation.rs").read_text()
    db_lib = (ROOT / "crates/phoenix-db/src/lib.rs").read_text()
    if "catch_up_dormant_git_repositories" not in reconcile or "test_only_mint" not in reconcile:
        raise RuntimeError("dormant reconciliation classification no longer proves test-only permit minting")
    if db_lib.count("git_repository_reconciliation::catch_up_dormant_git_repositories(self, permit)") != 1:
        raise RuntimeError("normal production caller added for dormant GitRepository reconciliation")
    actual_project_paths = sorted(
        path for path in all_sources if "find_or_create_project(" in (ROOT / path).read_text()
    )
    expected_project_paths = sorted(inventory["project_authority_paths"])
    if actual_project_paths != expected_project_paths:
        raise RuntimeError(f"Project authority inventory drift: expected {expected_project_paths}, got {actual_project_paths}")
    digest = hashlib.sha256(raw + b"\0" + "\n".join(actual_shadow_files + actual_project_paths).encode()).hexdigest()
    return Census(inventory["revision"], digest, len(actual_shadow_files), len(actual_project_paths))


def free_port() -> int:
    import socket
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def line_reader(stream: Any, lines: queue.Queue[str]) -> None:
    for line in iter(stream.readline, ""):
        lines.put(line)
    stream.close()


def start_old(binary: Path, db_path: Path, root: Path) -> tuple[subprocess.Popen[str], str, queue.Queue[str]]:
    port = free_port()
    home = root / "home"
    config = root / "config"
    data = root / "data"
    for directory in (home, config, data):
        directory.mkdir()
    env = os.environ | {
        "HOME": str(home), "USERPROFILE": str(home), "XDG_CONFIG_HOME": str(config),
        "PHOENIX_DATA_DIR": str(data), "PHOENIX_DB_PATH": str(db_path), "PHOENIX_PORT": str(port),
        "PHOENIX_BIND_ADDR": "127.0.0.1", "PHOENIX_ENABLE_MOCK_MODEL": "1", "DEFAULT_MODEL": "mock",
        "PHOENIX_LOG_STDOUT": "true", "PHOENIX_TRACE_EXPORTER": "none", "RUST_LOG": "info",
    }
    for key in tuple(env):
        if key.startswith(("DD_", "OTEL_")):
            env.pop(key)
    for key in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "PHOENIX_PASSWORD", "PHOENIX_TLS", "PHOENIX_TLS_CERT_PATH", "PHOENIX_TLS_KEY_PATH"):
        env.pop(key, None)
    process = subprocess.Popen([str(binary)], cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    assert process.stdout is not None
    lines: queue.Queue[str] = queue.Queue()
    threading.Thread(target=line_reader, args=(process.stdout, lines), daemon=True).start()
    async def await_startup() -> None:
        while True:
            if process.poll() is not None:
                raise RuntimeError(f"historical server exited before startup: {process.returncode}")
            line = await asyncio.to_thread(lines.get)
            if STARTUP_EVENT in line:
                return
    asyncio.run(asyncio.wait_for(await_startup(), OUTER_TIMEOUT))
    return process, f"http://127.0.0.1:{port}", lines


def stop(process: subprocess.Popen[str]) -> None:
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
    process.wait(timeout=OUTER_TIMEOUT)


def table_counts(db_path: Path) -> dict[str, int]:
    tables = ["git_repositories", "work_scope_git_repositories", "git_repository_locator_observations", "git_repository_default_branch_observations"]
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        return {table: int(connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]) for table in tables}
    finally:
        connection.close()


def old_database_proof(db_path: Path, canonical_path: str, before_shadow: dict[str, int]) -> dict[str, Any]:
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        row = connection.execute("SELECT id FROM projects WHERE canonical_path = ?", (canonical_path,)).fetchall()
        if row != [(PROJECT_ID,)]:
            raise RuntimeError(f"historical project proof failed: {row!r}")
        conversation = connection.execute("SELECT project_id, state_kind FROM conversations WHERE id = ?", (CONVERSATION_ID,)).fetchone()
        if conversation != (PROJECT_ID, "idle"):
            raise RuntimeError(f"historical conversation must be Idle and attached to canonical Project: {conversation!r}")
        job = connection.execute("SELECT status FROM conversation_creation_jobs WHERE conversation_id = ?", (CONVERSATION_ID,)).fetchone()
        if job != ("ready",):
            raise RuntimeError(f"historical creation job must finalize ready: {job!r}")
        migrations = connection.execute("SELECT version, name FROM schema_migrations ORDER BY version").fetchall()
        if not any(name == "create_git_repository_shadow_tables" for _, name in migrations):
            raise RuntimeError("candidate additive shadow migration was removed")
    finally:
        connection.close()
    after_shadow = table_counts(db_path)
    if after_shadow != before_shadow:
        raise RuntimeError(f"historical server wrote shadow tables: before={before_shadow}, after={after_shadow}")
    return {"projects_at_canonical_path": 1, "conversation_project_matches": True, "state": "Idle", "creation_job": "ready", "shadow_before": before_shadow, "shadow_after_old": after_shadow}


async def create_seeded_empty(base_url: str, canonical_path: str) -> None:
    payload = {"conversation_id": CONVERSATION_ID, "cwd": canonical_path, "model": "mock", "text": "", "images": [], "files": [], "message_id": MESSAGE_ID, "mode": "direct", "seed_parent_id": "44444444-4444-4444-8444-444444444444", "seed_label": "R1 compatibility seed"}
    async with httpx.AsyncClient(timeout=httpx.Timeout(OUTER_TIMEOUT)) as client:
        async with aconnect_sse(client, "GET", f"{base_url}/api/conversations/{CONVERSATION_ID}/stream") as source:
            response = await client.post(f"{base_url}/api/conversations/new", json=payload)
            response.raise_for_status()
            async for event in source.aiter_sse():
                if event.event != "init":
                    continue
                value = json.loads(event.data)
                state = value.get("conversation", {}).get("state")
                state_kind = state.get("type", state) if isinstance(state, dict) else state
                if str(state_kind).lower() == "idle":
                    return
                raise RuntimeError(f"seeded empty SSE init did not prove Idle: {state!r}")
    raise RuntimeError("SSE ended before an Idle init")


def main() -> None:
    candidate_sha = clean_head()
    source_census = census()
    with tempfile.TemporaryDirectory(prefix="phoenix-r1-compat-") as temporary:
        work = Path(temporary)
        old_tree = work / "historical"
        old_target = work / "historical-target"
        db_path = work / "candidate-expanded.db"
        old_binary = old_target / "debug" / "phoenix_ide"
        artifact = work / "finalizer.json"
        run("git", "worktree", "add", "--detach", str(old_tree), HISTORICAL_SHA)
        try:
            (old_tree / "ui/dist").mkdir(parents=True, exist_ok=True)
            (old_tree / "ui/dist/index.html").write_text("<!doctype html><title>compatibility placeholder</title>")
            run("git", "add", "-f", "ui/dist/index.html", cwd=old_tree)
            build_env = os.environ | {"CARGO_TARGET_DIR": str(old_target)}
            run("cargo", "build", "--offline", "--locked", "-p", "phoenix_ide", cwd=old_tree, env=build_env)
            old_version = run("git", "rev-parse", "HEAD", cwd=old_tree)
            old_dirty = run("git", "status", "--porcelain=v1", "--untracked-files=all", cwd=old_tree)
            if old_version != HISTORICAL_SHA or old_dirty != "A  ui/dist/index.html":
                raise RuntimeError(f"historical build identity changed beyond its placeholder: head={old_version}, status={old_dirty!r}")
            candidate_binary = ROOT / "target/debug/phoenix_ide"
            run("cargo", "build", "--offline", "--locked", "-p", "phoenix_ide")
            migrate_env = os.environ | {"PHOENIX_DB_PATH": str(db_path)}
            run(str(candidate_binary), "--migrate-only", env=migrate_env)
            repo = work / "canonical-repository"
            repo.mkdir()
            run("git", "init", cwd=repo)
            run("git", "config", "user.email", "compat@example.test", cwd=repo)
            run("git", "config", "user.name", "Compatibility", cwd=repo)
            (repo / "README.md").write_text("compatibility\n")
            run("git", "add", "README.md", cwd=repo)
            run("git", "commit", "-m", "initial", cwd=repo)
            before_shadow = table_counts(db_path)
            process, base_url, _ = start_old(old_binary, db_path, work)
            try:
                version = httpx.get(f"{base_url}/api/version", timeout=OUTER_TIMEOUT).json()["git_sha"]
                expected_version = f"{HISTORICAL_SHA[:12]}-dirty"
                if version != expected_version:
                    raise RuntimeError(f"historical /api/version must identify the frozen SHA plus only the placeholder dirtiness: {version!r}")
                asyncio.run(asyncio.wait_for(create_seeded_empty(base_url, str(repo.resolve())), OUTER_TIMEOUT))
            finally:
                stop(process)
            proof = old_database_proof(db_path, str(repo.resolve()), before_shadow)
            finalizer_env = os.environ | {
                "PHOENIX_R1_COMPAT_DB_PATH": str(db_path), "PHOENIX_R1_COMPAT_CANONICAL_PATH": str(repo.resolve()),
                "PHOENIX_R1_COMPAT_PROJECT_ID": PROJECT_ID, "PHOENIX_R1_COMPAT_CONVERSATION_ID": CONVERSATION_ID,
                "PHOENIX_R1_COMPAT_FINALIZER_ARTIFACT": str(artifact), "PHOENIX_R1_COMPAT_HISTORICAL_SHA": HISTORICAL_SHA,
                "PHOENIX_R1_COMPAT_CENSUS_REVISION": source_census.revision, "PHOENIX_R1_COMPAT_CENSUS_DIGEST": source_census.digest,
                "PHOENIX_R1_COMPAT_SHADOW_REFERENCE_COUNT": str(source_census.shadow_reference_count),
                "PHOENIX_R1_COMPAT_PROJECT_AUTHORITY_PATH_COUNT": str(source_census.project_authority_path_count),
            }
            run("cargo", "test", "--offline", "-p", "phoenix-db", "finalizes_historical_r1_compatibility_handoff", "--", "--ignored", "--exact", env=finalizer_env)
            finalizer = json.loads(artifact.read_text())
            if finalizer["candidate_sha"] != candidate_sha or not finalizer["complete_eligibility"]:
                raise RuntimeError(f"candidate finalizer did not produce complete exact evidence: {finalizer}")
            if table_counts(db_path)["git_repositories"] != 1:
                raise RuntimeError("candidate catch-up did not retain the additive GitRepository schema")
            report = {"candidate_sha": candidate_sha, "candidate_schema_digest": finalizer["candidate_schema_digest"], "historical_sha": HISTORICAL_SHA, "historical_version_endpoint": version, "historical_placeholder_only_dirty": True, "database_proof": proof, "census_revision": source_census.revision, "census_content_digest": source_census.digest, "rollback_posture": finalizer["rollback_posture"], "complete_eligibility": finalizer["complete_eligibility"], "integrity_digest": finalizer["integrity_digest"]}
            output = ROOT / "tests/e2e/git_repository_r1_compat.artifact.json"
            output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
            print(json.dumps(report, indent=2, sort_keys=True))
        finally:
            run("git", "worktree", "remove", "--force", str(old_tree))


if __name__ == "__main__":
    main()
