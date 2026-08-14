#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.13"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""Explicit, test-only Historical-R1-to-candidate GitRepository acceptance."""

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
import time
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterator

import httpx
from httpx_sse import aconnect_sse

ROOT = Path(__file__).resolve().parents[2]
HISTORICAL_SHA = "799ea4d63c3d451f3f47859fa21df46fe3072923"
CONVERSATION_ID = "11111111-1111-4111-8111-111111111111"
MESSAGE_ID = "22222222-2222-4222-8222-222222222222"
STARTUP_EVENT = "Phoenix IDE server listening"
OUTER_TIMEOUT = 120.0
EOF = object()
SHADOW_TABLES = ("git_repositories", "work_scope_git_repositories", "git_repository_locator_observations", "git_repository_default_branch_observations")
SOURCE_TABLES = ("projects", "conversations", "work_scopes", "conversation_work_scope_attachments", "conversation_creation_jobs")


@dataclass(frozen=True)
class Census:
    revision: str
    digest: str
    shadow_reference_count: int
    project_authority_path_count: int


@dataclass(frozen=True)
class Preparation:
    initial_catchup: dict[str, int]
    replay_catchup: dict[str, int]


def child_env(**phoenix: str) -> dict[str, str]:
    """The acceptance child contract: no ambient credentials/tracing/config leaks."""
    inherited = ("PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "WINDIR", "ComSpec", "PATHEXT", "USERPROFILE")
    env = {key: os.environ[key] for key in inherited if os.environ.get(key)}
    env.update({key: value for key, value in phoenix.items() if value is not None})
    return env


def run(*args: str, cwd: Path = ROOT, env: dict[str, str] | None = None, timeout: float = 900.0) -> str:
    command = " ".join(args)
    print(f"+ [{cwd}] {command}", flush=True)
    try:
        completed = subprocess.run(args, cwd=cwd, env=env or child_env(), check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"command failed ({error.returncode}): {command}\nstdout:\n{error.stdout}\nstderr:\n{error.stderr}") from error
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"command exceeded {timeout}s liveness bound: {command}\nstdout:\n{error.stdout}\nstderr:\n{error.stderr}") from error
    return completed.stdout.strip()


def clean_head() -> str:
    if run("git", "status", "--porcelain=v1", "--untracked-files=all"):
        raise RuntimeError("candidate checkout is dirty; commit every change before R1 compatibility acceptance")
    return run("git", "rev-parse", "HEAD")


def canonical_json_digest(value: Any) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def length_framed_digest(parts: list[tuple[str, str]]) -> str:
    digest = hashlib.sha256()
    for name, value in parts:
        encoded_name, encoded_value = name.encode(), value.encode()
        digest.update(len(encoded_name).to_bytes(8, "big")); digest.update(encoded_name)
        digest.update(len(encoded_value).to_bytes(8, "big")); digest.update(encoded_value)
    return digest.hexdigest()


CATCHUP_KEYS = (
    "inserted_git_repositories", "deleted_git_repositories",
    "inserted_work_scope_attachments", "replaced_work_scope_attachments",
    "deleted_work_scope_attachments", "deleted_locator_observations",
    "deleted_default_branch_observations",
)
SHADOW_COUNT_KEYS = (
    "git_repositories", "work_scope_git_repositories",
    "git_repository_locator_observations", "git_repository_default_branch_observations",
)


def exact_catchup(value: Any, name: str) -> dict[str, int]:
    if not isinstance(value, dict) or set(value) != set(CATCHUP_KEYS):
        raise RuntimeError(f"{name} must contain exactly the catch-up stat keys")
    if any(type(value[key]) is not int or value[key] < 0 for key in CATCHUP_KEYS):
        raise RuntimeError(f"{name} has invalid catch-up statistics")
    return {key: value[key] for key in CATCHUP_KEYS}


def read_preparation(path: Path) -> Preparation:
    value = json.loads(path.read_text())
    if set(value) != {"initial_catchup", "replay_catchup"}:
        raise RuntimeError("preparation artifact has unexpected fields")
    preparation = Preparation(
        exact_catchup(value["initial_catchup"], "preparation initial catchup"),
        exact_catchup(value["replay_catchup"], "preparation replay catchup"),
    )
    if any(preparation.replay_catchup.values()):
        raise RuntimeError("preparation replay catch-up was not exactly zero")
    return preparation


def canonical_shadow_counts(value: Any) -> dict[str, int]:
    if not isinstance(value, dict) or set(value) != set(SHADOW_COUNT_KEYS):
        raise RuntimeError("shadow row counts must have exactly the canonical shadow table keys")
    if any(type(value[key]) is not int or value[key] < 0 for key in SHADOW_COUNT_KEYS):
        raise RuntimeError("shadow row counts are invalid")
    return {key: value[key] for key in SHADOW_COUNT_KEYS}


def verify_finalizer(finalizer: dict[str, Any], candidate_sha: str) -> None:
    if finalizer.get("candidate_sha") != candidate_sha or finalizer.get("eligibility") != "passed":
        raise RuntimeError(f"finalizer did not produce exact eligible evidence: {finalizer}")
    finalizer["preparation_initial_catchup"] = exact_catchup(finalizer.get("preparation_initial_catchup"), "preparation initial catchup")
    finalizer["preparation_replay_catchup"] = exact_catchup(finalizer.get("preparation_replay_catchup"), "preparation replay catchup")
    finalizer["final_catchup"] = exact_catchup(finalizer.get("final_catchup"), "final catchup")
    finalizer["final_replay"] = exact_catchup(finalizer.get("final_replay"), "final replay")
    finalizer["shadow_row_counts"] = canonical_shadow_counts(finalizer.get("shadow_row_counts"))
    if any(finalizer["preparation_replay_catchup"].values()) or any(finalizer["final_catchup"].values()) or any(finalizer["final_replay"].values()):
        raise RuntimeError("artifact catch-up/replay proof was not exactly zero")
    members = finalizer.get("integrity_members")
    if not isinstance(members, list) or any(not isinstance(item, list) or len(item) != 2 or not all(isinstance(part, str) for part in item) for item in members):
        raise RuntimeError("integrity members must be string pairs")
    member_map = dict(members)
    if len(member_map) != len(members):
        raise RuntimeError("integrity members contain duplicate keys")
    expected = {
        key: str(finalizer[key]) for key in (
            "candidate_sha", "candidate_package_version", "candidate_schema_digest", "target_database_digest", "readiness_root", "run_nonce",
            "census_revision", "census_content_digest", "shadow_reference_count", "project_authority_path_count",
            "historical_sha", "old_source_digest_before", "old_source_digest_after", "shadow_digest_before_old",
            "shadow_digest_after_old", "rollback_posture", "eligibility",
        )
    }
    for prefix, stats in (("preparation_initial_catchup", finalizer["preparation_initial_catchup"]), ("preparation_replay_catchup", finalizer["preparation_replay_catchup"]), ("final_catchup", finalizer["final_catchup"]), ("final_replay", finalizer["final_replay"])):
        expected.update({f"{prefix}.{key}": str(value) for key, value in stats.items()})
    expected.update({f"shadow_row_counts.{key}": str(value) for key, value in finalizer["shadow_row_counts"].items()})
    if set(member_map) != set(expected):
        raise RuntimeError(f"integrity member keys are not exhaustive: expected={set(expected)!r}, got={set(member_map)!r}")
    if member_map != expected:
        raise RuntimeError("top-level artifact values do not match their integrity members")
    if finalizer.get("integrity_digest") != length_framed_digest([(key, value) for key, value in members]):
        raise RuntimeError("Python integrity verification rejected Rust artifact")


def census() -> Census:
    inventory = json.loads((ROOT / "tests/e2e/git_repository_r1_census.json").read_text())
    sources = sorted(path.relative_to(ROOT).as_posix() for path in ROOT.glob("crates/**/*.rs") if "/tests/" not in path.as_posix() and not path.name.endswith("_test.rs"))
    def occurrences(symbol: str) -> dict[str, int]:
        return {path: (ROOT / path).read_text().count(symbol) for path in sources if (ROOT / path).read_text().count(symbol)}
    actual_tables = {table: occurrences(table) for table in inventory["shadow_tables"]}
    actual_callers = {symbol: occurrences(symbol) for symbol in inventory["shadow_caller_symbols"]}
    actual_projects = {symbol: occurrences(symbol) for symbol in inventory["project_authority_symbols"]}
    observed = {"shadow_table_occurrences": actual_tables, "shadow_caller_occurrences": actual_callers, "project_authority_occurrences": actual_projects}
    expected = {key: inventory[key] for key in observed}
    if observed != expected:
        raise RuntimeError(f"authority census drift; expected={expected!r}, observed={observed!r}")
    digest = canonical_json_digest({"inventory": inventory, "observed": observed})
    return Census(inventory["revision"], digest, sum(sum(files.values()) for files in actual_tables.values()), sum(sum(files.values()) for files in actual_projects.values()))


def free_port() -> int:
    import socket
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def line_reader(stream: Any, lines: queue.Queue[object], retained: list[str]) -> None:
    try:
        for line in iter(stream.readline, ""):
            retained.append(line)
            lines.put(line)
    finally:
        stream.close()
        lines.put(EOF)


def preserved_logs(lines: list[str], failure_path: Path) -> str:
    text = "".join(lines)
    failure_path.parent.mkdir(parents=True, exist_ok=True)
    failure_path.write_text(text)
    return text


def stop(process: subprocess.Popen[str], lines: queue.Queue[object], retained: list[str]) -> None:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=10)
    deadline = time.monotonic() + 10
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError("historical server reader did not reach EOF after process reap")
        try:
            value = lines.get(timeout=remaining)
        except queue.Empty:
            continue
        if value is EOF:
            break
    logs = preserved_logs(retained, ROOT / "target/git_repository_r1_compat.failure.log")
    if process.returncode not in (0, -signal.SIGTERM):
        raise RuntimeError(f"historical server stopped with unexpected exit {process.returncode}; logs:\n{logs}")


def start_old(binary: Path, db_path: Path, root: Path, label: str) -> tuple[subprocess.Popen[str], str, queue.Queue[object], list[str]]:
    port = free_port()  # Server API does not accept a pre-bound listener; diagnose bind races below.
    home, config, data = root / "home", root / "config", root / "data"
    for directory in (home, config, data): directory.mkdir(exist_ok=True)
    env = child_env(HOME=str(home), USERPROFILE=str(home), XDG_CONFIG_HOME=str(config), PHOENIX_DATA_DIR=str(data), PHOENIX_DB_PATH=str(db_path), PHOENIX_PORT=str(port), PHOENIX_BIND_ADDR="127.0.0.1", PHOENIX_ENABLE_MOCK_MODEL="1", DEFAULT_MODEL="mock", PHOENIX_LOG_STDOUT="true", PHOENIX_LOG_FILE=str(root / f"historical-{label}.log"), PHOENIX_TRACE_EXPORTER="none", RUST_LOG="info")
    process = subprocess.Popen([str(binary)], cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1, start_new_session=True)
    assert process.stdout is not None
    lines: queue.Queue[object] = queue.Queue(); retained: list[str] = []
    threading.Thread(target=line_reader, args=(process.stdout, lines, retained), daemon=True).start()
    deadline = time.monotonic() + OUTER_TIMEOUT
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            stop(process, lines, retained)
            logs = preserved_logs(retained, ROOT / "target/git_repository_r1_compat.failure.log")
            raise RuntimeError(f"historical server did not emit {STARTUP_EVENT!r}; bind race or startup failure on port {port}; logs:\n{logs}")
        try: event = lines.get(timeout=remaining)
        except queue.Empty: continue
        if event is EOF:
            code = process.wait(timeout=1)
            logs = preserved_logs(retained, ROOT / "target/git_repository_r1_compat.failure.log")
            raise RuntimeError(f"historical server exited before startup ({code}); requested port={port}; logs:\n{logs}")
        assert isinstance(event, str)
        if STARTUP_EVENT in event:
            return process, f"http://127.0.0.1:{port}", lines, retained


def snapshot(db_path: Path, tables: tuple[str, ...]) -> dict[str, Any]:
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        output: dict[str, Any] = {}
        for table in tables:
            ddl = connection.execute("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name = ?", (table,)).fetchone()
            definitions = connection.execute("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE tbl_name = ? OR name = ? ORDER BY type, name", (table, table)).fetchall()
            if ddl is None: raise RuntimeError(f"expected table missing from read-only snapshot: {table}")
            columns = [row[1] for row in connection.execute(f'PRAGMA table_info("{table}")')]
            encoded = ", ".join(f"typeof(\"{column}\"), quote(\"{column}\")" for column in columns)
            rows = connection.execute(f'SELECT {encoded} FROM "{table}" ORDER BY ' + ", ".join(f'quote("{column}")' for column in columns)).fetchall()
            output[table] = {"schema": [list(definition) for definition in definitions], "columns": columns, "rows": [list(row) for row in rows]}
        return output
    finally: connection.close()


def proof_digest(proof: dict[str, Any]) -> str: return canonical_json_digest(proof)


async def stream_existing_idle(base_url: str) -> None:
    async with httpx.AsyncClient(timeout=httpx.Timeout(OUTER_TIMEOUT)) as client:
        async with aconnect_sse(client, "GET", f"{base_url}/api/conversations/{CONVERSATION_ID}/stream") as source:
            async for event in source.aiter_sse():
                if event.event not in {"init", "state_change"}:
                    continue
                value = json.loads(event.data)
                state = value.get("conversation", {}).get("state") if event.event == "init" else value.get("state")
                kind = state.get("type", state) if isinstance(state, dict) else state
                if str(kind).lower() == "idle":
                    return
    raise RuntimeError("historical rollback SSE ended before it operated on existing Idle conversation")


async def create_seeded_empty(base_url: str, canonical_path: str) -> None:
    payload = {"conversation_id": CONVERSATION_ID, "cwd": canonical_path, "model": "mock", "text": "", "message_id": MESSAGE_ID, "images": [], "files": [], "mode": "direct", "base_branch": None, "seed_parent_id": None, "checkout_ref": None, "seed_label": "r1-old-binary-compat"}
    async with httpx.AsyncClient(timeout=httpx.Timeout(OUTER_TIMEOUT)) as client:
        response = await client.post(f"{base_url}/api/conversations/new", json=payload); response.raise_for_status()
        async with aconnect_sse(client, "GET", f"{base_url}/api/conversations/{CONVERSATION_ID}/stream") as source:
            async for event in source.aiter_sse():
                if event.event in {"init", "state_change"}:
                    value = json.loads(event.data); state = value.get("conversation", {}).get("state") if event.event == "init" else value.get("state")
                    kind = state.get("type", state) if isinstance(state, dict) else state
                    if str(kind).lower() == "idle": return
    raise RuntimeError("SSE ended before init/state_change proved Idle")


@contextmanager
def historical_worktree(work: Path) -> Iterator[Path]:
    tree = work / "historical"; added = False
    try:
        run("git", "worktree", "add", "--detach", str(tree), HISTORICAL_SHA); added = True
        yield tree
    finally:
        if added or tree.exists():
            try: run("git", "worktree", "remove", "--force", str(tree))
            except Exception as error: print(f"warning: worktree remove failed during cleanup: {error}")
        try: run("git", "worktree", "prune")
        except Exception as error: print(f"warning: worktree prune failed during cleanup: {error}")


def main() -> None:
    candidate_sha = clean_head(); source_census = census()
    failure_log = ROOT / "target/git_repository_r1_compat.failure.log"
    with tempfile.TemporaryDirectory(prefix="phoenix-r1-compat-") as temporary:
        work = Path(temporary); old_target = work / "historical-target"; db_path = work / "candidate-expanded.db"; artifact = work / "finalizer.json"; preparation = work / "preparation.json"
        try:
            with historical_worktree(work) as old_tree:
                (old_tree / "ui/dist").mkdir(parents=True, exist_ok=True); (old_tree / "ui/dist/index.html").write_text("<!doctype html><title>compatibility placeholder</title>")
                run("git", "add", "-f", "ui/dist/index.html", cwd=old_tree)
                run("cargo", "build", "--offline", "--locked", "-p", "phoenix_ide", cwd=old_tree, env=child_env(CARGO_TARGET_DIR=str(old_target)))
                old_binary = old_target / "debug/phoenix_ide"
                if run("git", "rev-parse", "HEAD", cwd=old_tree) != HISTORICAL_SHA: raise RuntimeError("historical build revision drift")
                candidate_target = ROOT / "target"; candidate_binary = candidate_target / "debug/phoenix_ide"
                run("cargo", "build", "--offline", "--locked", "-p", "phoenix_ide", env=child_env(CARGO_TARGET_DIR=str(candidate_target)))
                if not candidate_binary.is_file(): raise RuntimeError("candidate exact binary missing from ROOT/target")
                # Candidate is deliberately executed only through migrate-only; it must not expose an API endpoint.
                run(str(candidate_binary), "--migrate-only", env=child_env(PHOENIX_DB_PATH=str(db_path)))
                repo = work / "canonical-repository"; repo.mkdir(); run("git", "init", cwd=repo); run("git", "config", "user.email", "compat@example.test", cwd=repo); run("git", "config", "user.name", "Compatibility", cwd=repo)
                (repo / "README.md").write_text("compatibility\n"); run("git", "add", "README.md", cwd=repo); run("git", "commit", "-m", "initial", cwd=repo)
                shadow_before_old = snapshot(db_path, SHADOW_TABLES)
                process, base_url, lines, retained = start_old(old_binary, db_path, work, "seed")
                try:
                    version = httpx.get(f"{base_url}/api/version", timeout=OUTER_TIMEOUT).json()["git_sha"]
                    if version != f"{HISTORICAL_SHA[:12]}-dirty": raise RuntimeError(f"historical version identity mismatch: {version!r}")
                    asyncio.run(asyncio.wait_for(create_seeded_empty(base_url, str(repo.resolve())), OUTER_TIMEOUT))
                finally: stop(process, lines, retained)
                source_before_candidate = snapshot(db_path, SOURCE_TABLES); shadow_after_old = snapshot(db_path, SHADOW_TABLES)
                if shadow_after_old != shadow_before_old: raise RuntimeError("historical binary wrote additive shadow schema")
                env = child_env(CARGO_TARGET_DIR=str(candidate_target), PHOENIX_R1_COMPAT_DB_PATH=str(db_path), PHOENIX_R1_COMPAT_CANONICAL_PATH=str(repo.resolve()), PHOENIX_R1_COMPAT_FINALIZER_ARTIFACT=str(artifact), PHOENIX_R1_COMPAT_PREPARATION_ARTIFACT=str(preparation), PHOENIX_R1_COMPAT_HISTORICAL_SHA=HISTORICAL_SHA, PHOENIX_R1_COMPAT_CENSUS_REVISION=source_census.revision, PHOENIX_R1_COMPAT_CENSUS_DIGEST=source_census.digest, PHOENIX_R1_COMPAT_SHADOW_REFERENCE_COUNT=str(source_census.shadow_reference_count), PHOENIX_R1_COMPAT_PROJECT_AUTHORITY_PATH_COUNT=str(source_census.project_authority_path_count), PHOENIX_R1_COMPAT_OLD_SOURCE_DIGEST=proof_digest(source_before_candidate), PHOENIX_R1_COMPAT_OLD_SHADOW_DIGEST=proof_digest(shadow_after_old))
                # First candidate pass only prepares typed catch-up proof before exercising the old binary again.
                run("cargo", "test", "--offline", "-p", "phoenix-db", "git_repository_reconciliation::tests::finalizes_historical_r1_compatibility_handoff", "--", "--ignored", "--exact", env=env | {"PHOENIX_R1_COMPAT_PHASE": "prepare"})
                prepared = read_preparation(preparation)
                expected_attachments = len(source_before_candidate["conversation_work_scope_attachments"]["rows"])
                if prepared.initial_catchup["inserted_git_repositories"] != 1 or prepared.initial_catchup["inserted_work_scope_attachments"] != expected_attachments:
                    raise RuntimeError(f"prepare catch-up did not derive exact repository/attachment equality: {prepared}")
                source_before_rollback, shadow_before_rollback = snapshot(db_path, SOURCE_TABLES), snapshot(db_path, SHADOW_TABLES)
                process, base_url, lines, retained = start_old(old_binary, db_path, work, "rollback")
                try:
                    version = httpx.get(f"{base_url}/api/version", timeout=OUTER_TIMEOUT).json()["git_sha"]
                    if version != f"{HISTORICAL_SHA[:12]}-dirty": raise RuntimeError("rollback binary identity drift")
                    asyncio.run(asyncio.wait_for(stream_existing_idle(base_url), OUTER_TIMEOUT))
                finally: stop(process, lines, retained)
                source_after_rollback, shadow_after_rollback = snapshot(db_path, SOURCE_TABLES), snapshot(db_path, SHADOW_TABLES)
                if source_after_rollback != source_before_rollback: raise RuntimeError("historical rollback binary mutated legacy source rows")
                if shadow_after_rollback != shadow_before_rollback: raise RuntimeError("historical rollback binary mutated additive shadows")
                # Final candidate pass mints the artifact only after the binary rollback exercise.
                final_env = env | {"PHOENIX_R1_COMPAT_PHASE": "finalize", "PHOENIX_R1_COMPAT_ROLLBACK_SOURCE_DIGEST": proof_digest(source_after_rollback), "PHOENIX_R1_COMPAT_ROLLBACK_SHADOW_DIGEST": proof_digest(shadow_after_rollback)}
                run("cargo", "test", "--offline", "-p", "phoenix-db", "git_repository_reconciliation::tests::finalizes_historical_r1_compatibility_handoff", "--", "--ignored", "--exact", env=final_env)
                if snapshot(db_path, SOURCE_TABLES) != source_before_candidate: raise RuntimeError("candidate catch-up mutated complete legacy source tables")
                finalizer = json.loads(artifact.read_text())
                verify_finalizer(finalizer, candidate_sha)
                output = ROOT / "target/git_repository_r1_compat.artifact.json"; output.write_text(json.dumps(finalizer, indent=2, sort_keys=True) + "\n")
                print(json.dumps(finalizer, indent=2, sort_keys=True))
        except BaseException:
            # Every started server drains into retained; preserve the shared diagnostic location on failure.
            if not failure_log.exists(): failure_log.write_text("R1 compatibility failed before a server emitted logs\n")
            raise


if __name__ == "__main__": main()
