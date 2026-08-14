#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["httpx>=0.27", "httpx-sse>=0.4"]
# ///
"""Paired offline database-restore acceptance for GitRepository Foundation."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import queue
import re
import shutil
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


@dataclass(frozen=True)
class Census:
    revision: str
    digest: str
    shadow_reference_count: int
    project_authority_path_count: int


def child_env(**phoenix: str) -> dict[str, str]:
    inherited = (
        "PATH",
        "HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "WINDIR",
        "ComSpec",
        "PATHEXT",
        "USERPROFILE",
    )
    env = {key: os.environ[key] for key in inherited if os.environ.get(key)}
    env.update({key: value for key, value in phoenix.items() if value is not None})
    return env


def run(
    *args: str,
    cwd: Path = ROOT,
    env: dict[str, str] | None = None,
    timeout: float = 900.0,
) -> str:
    command = " ".join(args)
    print(f"+ [{cwd}] {command}", flush=True)
    try:
        completed = subprocess.run(
            args,
            cwd=cwd,
            env=env or child_env(),
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
    except subprocess.CalledProcessError as error:
        raise RuntimeError(
            f"command failed ({error.returncode}): {command}\n"
            f"stdout:\n{error.stdout}\nstderr:\n{error.stderr}"
        ) from error
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(
            f"command exceeded {timeout}s liveness bound: {command}\n"
            f"stdout:\n{error.stdout}\nstderr:\n{error.stderr}"
        ) from error
    return completed.stdout.strip()


def assert_clean_checkout(cwd: Path = ROOT) -> None:
    status = run("git", "status", "--porcelain=v1", "--untracked-files=all", cwd=cwd)
    if status:
        raise RuntimeError(f"checkout must be exactly clean; git status returned: {status!r}")


def run_current_test_filter(test_filter: str) -> None:
    output = run(
        "cargo",
        "test",
        "--offline",
        "-p",
        "phoenix-db",
        test_filter,
    )
    passed = sum(
        int(match.group(1))
        for match in re.finditer(r"test result: ok\. (\d+) passed;", output)
    )
    if passed == 0:
        raise RuntimeError(f"current test filter {test_filter!r} executed zero tests")


def canonical_json_digest(value: Any) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def census_sources() -> list[str]:
    rust = ROOT.glob("crates/**/*.rs")
    python = [ROOT / "dev.py", ROOT / "phoenix-client.py", *ROOT.glob("scripts/**/*.py")]
    paths = [*rust, *python]
    return sorted(
        path.relative_to(ROOT).as_posix()
        for path in paths
        if path.is_file()
        and "/target/" not in path.as_posix()
        and "/generated/" not in path.as_posix()
        and "__pycache__" not in path.as_posix()
    )


def census_observed(
    inventory: dict[str, Any], injected: dict[str, str] | None = None
) -> dict[str, Any]:
    sources = census_sources()
    injected = injected or {}

    def occurrences(symbol: str) -> dict[str, int]:
        return {
            path: count
            for path in sources
            if (count := ((ROOT / path).read_text() + injected.get(path, "")).count(symbol))
        }

    return {
        "shadow_table_occurrences": {
            table: occurrences(table) for table in inventory["shadow_tables"]
        },
        "shadow_caller_occurrences": {
            symbol: occurrences(symbol) for symbol in inventory["shadow_caller_symbols"]
        },
        "project_authority_occurrences": {
            path["name"]: occurrences(path["pattern"])
            for path in inventory["project_authority_paths"]
        },
    }


def census_expected(inventory: dict[str, Any]) -> dict[str, Any]:
    return {
        "shadow_table_occurrences": inventory["shadow_table_occurrences"],
        "shadow_caller_occurrences": inventory["shadow_caller_occurrences"],
        "project_authority_occurrences": {
            path["name"]: path["occurrences"] for path in inventory["project_authority_paths"]
        },
    }


def validate_census_aggregate_counts(
    inventory: dict[str, Any], observed: dict[str, Any]
) -> tuple[int, int]:
    shadow_count = sum(
        sum(files.values())
        for category in ("shadow_table_occurrences", "shadow_caller_occurrences")
        for files in observed[category].values()
    )
    project_count = sum(
        sum(files.values()) for files in observed["project_authority_occurrences"].values()
    )
    if inventory.get("shadow_reference_count") != shadow_count:
        raise RuntimeError("stored shadow_reference_count disagrees with the exact inventory")
    if inventory.get("project_authority_path_count") != project_count:
        raise RuntimeError("stored project_authority_path_count disagrees with the exact inventory")
    return shadow_count, project_count


def census() -> Census:
    inventory = json.loads((ROOT / "tests/e2e/git_repository_r1_census.json").read_text())
    observed = census_observed(inventory)
    if observed != census_expected(inventory):
        raise RuntimeError("authority census drift")
    shadow_count, project_count = validate_census_aggregate_counts(inventory, observed)
    digest = canonical_json_digest({"inventory": inventory, "observed": observed})
    return Census(inventory["revision"], digest, shadow_count, project_count)


def census_self_test() -> None:
    inventory = json.loads((ROOT / "tests/e2e/git_repository_r1_census.json").read_text())
    expected = census_expected(inventory)
    rust_file = next(path for path in census_sources() if path.endswith(".rs"))
    python_file = "dev.py"
    categories = (
        ("shadow_table_occurrences", inventory["shadow_tables"]),
        ("shadow_caller_occurrences", inventory["shadow_caller_symbols"]),
        (
            "project_authority_occurrences",
            [path["pattern"] for path in inventory["project_authority_paths"]],
        ),
    )
    for category, entries in categories:
        for token in entries:
            for injected_file in (rust_file, python_file):
                observed = census_observed(inventory, {injected_file: token})
                if observed[category] == expected[category]:
                    raise RuntimeError(
                        f"census self-test missed {category} token {token!r} in {injected_file}"
                    )
    observed = census_observed(inventory)
    for count_field in ("shadow_reference_count", "project_authority_path_count"):
        contradictory = dict(inventory)
        contradictory[count_field] += 1
        try:
            validate_census_aggregate_counts(contradictory, observed)
        except RuntimeError:
            continue
        raise RuntimeError(f"census accepted contradictory {count_field}")
    print("census self-test passed")


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


def stop(process: subprocess.Popen[str], lines: queue.Queue[object], retained: list[str]) -> str:
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
            raise RuntimeError("historical server reader did not reach EOF")
        try:
            value = lines.get(timeout=remaining)
        except queue.Empty:
            continue
        if value is EOF:
            break
    logs = "".join(retained)
    if process.returncode not in (0, -signal.SIGTERM):
        raise RuntimeError(
            f"historical server stopped with unexpected exit {process.returncode}; logs:\n{logs}"
        )
    return logs


def start_old(
    binary: Path, db_path: Path, root: Path, label: str, run_logs: list[str]
) -> tuple[subprocess.Popen[str], str, queue.Queue[object], list[str]]:
    home, config, data = root / "home", root / "config", root / "data"
    for directory in (home, config, data):
        directory.mkdir(exist_ok=True)
    for attempt in range(3):
        port = free_port()
        env = child_env(
            HOME=str(home),
            USERPROFILE=str(home),
            XDG_CONFIG_HOME=str(config),
            PHOENIX_DATA_DIR=str(data),
            PHOENIX_DB_PATH=str(db_path),
            PHOENIX_PORT=str(port),
            PHOENIX_BIND_ADDR="127.0.0.1",
            PHOENIX_ENABLE_MOCK_MODEL="1",
            DEFAULT_MODEL="mock",
            PHOENIX_LOG_STDOUT="true",
            PHOENIX_LOG_FILE=str(root / f"historical-{label}-{attempt}.log"),
            PHOENIX_TRACE_EXPORTER="none",
            RUST_LOG="info",
        )
        process = subprocess.Popen(
            [str(binary)],
            cwd=root,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
            start_new_session=True,
        )
        assert process.stdout is not None
        lines: queue.Queue[object] = queue.Queue()
        retained: list[str] = []
        threading.Thread(
            target=line_reader, args=(process.stdout, lines, retained), daemon=True
        ).start()
        deadline = time.monotonic() + OUTER_TIMEOUT
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                stop(process, lines, retained)
                raise RuntimeError(f"historical server did not emit {STARTUP_EVENT!r}")
            try:
                event = lines.get(timeout=remaining)
            except queue.Empty:
                continue
            if event is EOF:
                code = process.wait(timeout=1)
                logs = "".join(retained)
                run_logs.extend(retained)
                bind_race = "address already in use" in logs.lower()
                if bind_race and attempt < 2:
                    break
                raise RuntimeError(
                    f"historical server exited before startup ({code}); logs:\n{logs}"
                )
            assert isinstance(event, str)
            if STARTUP_EVENT in event:
                return process, f"http://127.0.0.1:{port}", lines, retained
    raise RuntimeError("historical server exhausted bounded bind retries")


async def create_seeded_empty(base_url: str, canonical_path: str) -> None:
    payload = {
        "conversation_id": CONVERSATION_ID,
        "cwd": canonical_path,
        "model": "mock",
        "text": "",
        "message_id": MESSAGE_ID,
        "images": [],
        "files": [],
        "mode": "direct",
        "base_branch": None,
        "seed_parent_id": None,
        "checkout_ref": None,
        "seed_label": "paired-offline-restore",
    }
    async with httpx.AsyncClient(timeout=httpx.Timeout(OUTER_TIMEOUT)) as client:
        response = await client.post(f"{base_url}/api/conversations/new", json=payload)
        response.raise_for_status()
        await wait_for_idle(client, base_url)


async def wait_for_idle(client: httpx.AsyncClient, base_url: str) -> None:
    async with aconnect_sse(
        client, "GET", f"{base_url}/api/conversations/{CONVERSATION_ID}/stream"
    ) as source:
        async for event in source.aiter_sse():
            if event.event not in {"init", "state_change"}:
                continue
            value = json.loads(event.data)
            state = (
                value.get("conversation", {}).get("state")
                if event.event == "init"
                else value.get("state")
            )
            kind = state.get("type", state) if isinstance(state, dict) else state
            if str(kind).lower() == "idle":
                return
    raise RuntimeError("SSE ended before Idle")


async def stream_existing_idle(base_url: str) -> None:
    async with httpx.AsyncClient(timeout=httpx.Timeout(OUTER_TIMEOUT)) as client:
        await wait_for_idle(client, base_url)


def quote_identifier(value: str) -> str:
    return '"' + value.replace('"', '""') + '"'


def database_snapshot(db_path: Path) -> dict[str, Any]:
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        definitions = connection.execute(
            "SELECT type, name, tbl_name, sql FROM sqlite_master "
            "WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name"
        ).fetchall()
        tables = [
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_master "
                "WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
            )
        ]
        rows: dict[str, Any] = {}
        for table in tables:
            quoted_table = quote_identifier(table)
            columns = [
                row[1]
                for row in connection.execute(f"PRAGMA table_info({quoted_table})")
            ]
            encoded = ", ".join(
                f"typeof({quote_identifier(column)}), quote({quote_identifier(column)})"
                for column in columns
            )
            order = ", ".join(
                f"quote({quote_identifier(column)})" for column in columns
            )
            rows[table] = connection.execute(
                f"SELECT {encoded} FROM {quoted_table} ORDER BY {order}"
            ).fetchall()
        return {"definitions": definitions, "rows": rows}
    finally:
        connection.close()


def offline_backup(source: Path, backup: Path) -> None:
    with sqlite3.connect(source) as source_connection, sqlite3.connect(backup) as backup_connection:
        source_connection.backup(backup_connection)


def registered_worktree(tree: Path) -> bool:
    marker = f"worktree {tree.resolve()}"
    return marker in run("git", "worktree", "list", "--porcelain").splitlines()


@contextmanager
def historical_worktree(work: Path) -> Iterator[Path]:
    tree = work / "historical"
    body_error: BaseException | None = None
    try:
        run("git", "worktree", "add", "--detach", str(tree), HISTORICAL_SHA)
        yield tree
    except BaseException as error:
        body_error = error
    finally:
        cleanup_error: BaseException | None = None
        try:
            if registered_worktree(tree):
                run("git", "worktree", "remove", "--force", str(tree))
            run("git", "worktree", "prune")
            if registered_worktree(tree):
                raise RuntimeError("historical worktree remains registered")
        except BaseException as error:
            cleanup_error = error
        if body_error and cleanup_error:
            raise BaseExceptionGroup(
                "historical worktree body and cleanup failed", [body_error, cleanup_error]
            )
        if cleanup_error:
            raise cleanup_error
        if body_error:
            raise body_error


def publish_artifact(path: Path, artifact: dict[str, Any]) -> None:
    encoded = (json.dumps(artifact, sort_keys=True, indent=2) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        with temporary.open("xb") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> None:
    if os.environ.get("PHOENIX_R1_COMPAT_CENSUS_SELF_TEST") == "1":
        census_self_test()
        print(census())
        return

    assert_clean_checkout()
    candidate_sha = run("git", "rev-parse", "HEAD")
    source_census = census()
    output = ROOT / "target/git_repository_r1_compat.artifact.json"
    failure_log = ROOT / "target/git_repository_r1_compat.failure.log"
    output.unlink(missing_ok=True)
    failure_log.unlink(missing_ok=True)
    run_logs: list[str] = []

    with tempfile.TemporaryDirectory(prefix="phoenix-r1-restore-") as temporary:
        work = Path(temporary)
        old_target = work / "historical-target"
        source_db = work / "historical-source.db"
        backup_db = work / "historical-backup.db"
        restored_db = work / "historical-restored.db"
        try:
            with historical_worktree(work) as old_tree:
                assert_clean_checkout(old_tree)
                ignored_index = old_tree / "ui/dist/index.html"
                ignored_index.parent.mkdir(parents=True, exist_ok=True)
                ignored_index.write_text("<!doctype html><title>restore placeholder</title>")
                run(
                    "cargo",
                    "build",
                    "--offline",
                    "--locked",
                    "-p",
                    "phoenix_ide",
                    cwd=old_tree,
                    env=child_env(CARGO_TARGET_DIR=str(old_target)),
                )
                assert_clean_checkout(old_tree)
                if run("git", "rev-parse", "HEAD", cwd=old_tree) != HISTORICAL_SHA:
                    raise RuntimeError("historical build revision drift")
                old_binary = old_target / "debug/phoenix_ide"

                repo = work / "canonical-repository"
                repo.mkdir()
                run("git", "init", cwd=repo)
                run("git", "config", "user.email", "restore@example.test", cwd=repo)
                run("git", "config", "user.name", "Restore", cwd=repo)
                (repo / "README.md").write_text("paired restore\n")
                run("git", "add", "README.md", cwd=repo)
                run("git", "commit", "-m", "initial", cwd=repo)

                process, base_url, lines, retained = start_old(
                    old_binary, source_db, work, "source", run_logs
                )
                try:
                    version = httpx.get(
                        f"{base_url}/api/version", timeout=OUTER_TIMEOUT
                    ).json()["git_sha"]
                    if version != HISTORICAL_SHA[:12]:
                        raise RuntimeError("historical source binary identity mismatch")
                    asyncio.run(
                        asyncio.wait_for(
                            create_seeded_empty(base_url, str(repo.resolve())), OUTER_TIMEOUT
                        )
                    )
                finally:
                    run_logs.extend(retained)
                    stop(process, lines, retained)

                source_snapshot = database_snapshot(source_db)
                offline_backup(source_db, backup_db)
                backup_snapshot = database_snapshot(backup_db)
                shutil.copy2(backup_db, restored_db)
                restored_before = database_snapshot(restored_db)
                if not (source_snapshot == backup_snapshot == restored_before):
                    raise RuntimeError("offline backup/restore changed historical schema or rows")

                process, base_url, lines, retained = start_old(
                    old_binary, restored_db, work, "restored", run_logs
                )
                try:
                    version = httpx.get(
                        f"{base_url}/api/version", timeout=OUTER_TIMEOUT
                    ).json()["git_sha"]
                    if version != HISTORICAL_SHA[:12]:
                        raise RuntimeError("restored binary identity mismatch")
                    asyncio.run(
                        asyncio.wait_for(stream_existing_idle(base_url), OUTER_TIMEOUT)
                    )
                finally:
                    run_logs.extend(retained)
                    stop(process, lines, retained)

                restored_after = database_snapshot(restored_db)
                if restored_after != restored_before:
                    raise RuntimeError("matching old binary mutated restored DB during read journey")

                run_current_test_filter("migration_065")
                run_current_test_filter("git_repository_reconciliation")

                source_digest = canonical_json_digest(source_snapshot)
                artifact = {
                    "authority_census": {
                        "conclusion": "passed",
                        "candidate_sha": candidate_sha,
                        "revision": source_census.revision,
                        "content_digest": source_census.digest,
                        "shadow_reference_count": source_census.shadow_reference_count,
                        "project_authority_path_count": source_census.project_authority_path_count,
                    },
                    "historical_sha": HISTORICAL_SHA,
                    "historical_runtime_identity": HISTORICAL_SHA[:12],
                    "source_digest": source_digest,
                    "backup_digest": canonical_json_digest(backup_snapshot),
                    "restored_before_digest": canonical_json_digest(restored_before),
                    "restored_after_digest": canonical_json_digest(restored_after),
                    "paired_offline_restore": "passed",
                }
                if len(
                    {
                        artifact["source_digest"],
                        artifact["backup_digest"],
                        artifact["restored_before_digest"],
                        artifact["restored_after_digest"],
                    }
                ) != 1:
                    raise RuntimeError("paired restore digest equality failed")
                publish_artifact(output, artifact)
                print(json.dumps(artifact, sort_keys=True))
        except BaseException:
            output.unlink(missing_ok=True)
            failure_log.parent.mkdir(parents=True, exist_ok=True)
            failure_log.write_text(
                f"run_head={candidate_sha}\n"
                + ("".join(run_logs) or "failure occurred before server logs\n")
            )
            raise
        else:
            failure_log.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
