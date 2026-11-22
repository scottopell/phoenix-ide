#!/usr/bin/env python3
"""Hosted qualification for large, populated SQLite backup verification."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import shutil
import sqlite3
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HELPER_PATH = ROOT / "scripts/launchd_deploy_helper.py"
DEFAULT_SOURCE_BYTES = 13_400_000_000
GENERATION_BUDGET_SECONDS = 1_800.0
BATCH_ROWS = 5_000


def load_helper():
    spec = importlib.util.spec_from_file_location("phoenix_launchd_deploy_helper", HELPER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load helper: {HELPER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _payload(sequence: int, kind: str, size: int) -> str | bytes:
    """Produce substantive deterministic payloads with changing row identity."""
    if kind == "text":
        prefix = f"record-{sequence:012d} indexed phoenix backup verification text "
        return (prefix + "representative searchable content " * ((size // 34) + 2))[:size]
    seed = hashlib.sha256(f"phoenix-backup-fixture-{sequence}".encode()).digest()
    return (seed * ((size + len(seed) - 1) // len(seed)))[:size]


def create_fixture(database: Path, *, minimum_bytes: int, rows_per_table: int | None,
                   text_bytes: int, blob_bytes: int, extra_columns: int = 0) -> dict:
    started = time.monotonic()
    deadline = started + GENERATION_BUDGET_SECONDS
    text_rows = blob_rows = 0
    with sqlite3.connect(database) as connection:
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("PRAGMA synchronous=NORMAL")
        connection.execute("PRAGMA page_size=4096")
        connection.executescript("""
            CREATE TABLE documents (
                body TEXT NOT NULL,
                payload BLOB NOT NULL
            );
            CREATE VIRTUAL TABLE documents_fts USING fts5(body);
            CREATE TABLE keyed_records (
                id INTEGER NOT NULL PRIMARY KEY,
                body TEXT NOT NULL,
                payload BLOB NOT NULL
            ) WITHOUT ROWID;
            CREATE INDEX documents_body_prefix ON documents(substr(body, 1, 32));
            CREATE INDEX keyed_body_prefix ON keyed_records(substr(body, 1, 32));
        """)
        for index in range(extra_columns):
            # Defaults become stored cells on INSERT, not virtual/generated columns.
            connection.execute(f"ALTER TABLE documents ADD COLUMN extra_{index} INTEGER DEFAULT {index}")
            connection.execute(f"ALTER TABLE keyed_records ADD COLUMN extra_{index} INTEGER DEFAULT {index}")
        while True:
            if time.monotonic() >= deadline:
                raise TimeoutError("fixture generation exceeded 30-minute budget")
            if rows_per_table is None:
                # Grow in bounded transactions until SQLite's real database file
                # crosses the qualification threshold; no sparse padding/freelist.
                limit = BATCH_ROWS
                text_batch = [(_payload(text_rows + i, "text", text_bytes),
                               _payload(text_rows + i, "blob", blob_bytes))
                              for i in range(limit)]
                connection.executemany("INSERT INTO documents(body,payload) VALUES (?,?)", text_batch)
                connection.executemany("INSERT INTO documents_fts(body) VALUES (?)",
                                       ((row[0],) for row in text_batch))
                text_rows += len(text_batch)
                keyed_batch = [(text_rows + blob_rows + i, _payload(blob_rows + i, "text", text_bytes),
                                _payload(blob_rows + i, "blob", blob_bytes)) for i in range(limit)]
                connection.executemany("INSERT INTO keyed_records(id,body,payload) VALUES (?,?,?)", keyed_batch)
                blob_rows += len(keyed_batch)
                connection.commit()
                connection.execute("PRAGMA wal_checkpoint(PASSIVE)")
                if database.stat().st_size >= minimum_bytes and min(text_rows, blob_rows) >= 1_000_000:
                    break
            else:
                remaining = rows_per_table - min(text_rows, blob_rows)
                if remaining <= 0:
                    break
                count = min(BATCH_ROWS, remaining)
                text_batch = [(_payload(text_rows + i, "text", text_bytes),
                               _payload(text_rows + i, "blob", blob_bytes)) for i in range(count)]
                connection.executemany("INSERT INTO documents(body,payload) VALUES (?,?)", text_batch)
                connection.executemany("INSERT INTO documents_fts(body) VALUES (?)",
                                       ((row[0],) for row in text_batch))
                text_rows += count
                keyed_batch = [(text_rows + blob_rows + i, _payload(blob_rows + i, "text", text_bytes),
                                _payload(blob_rows + i, "blob", blob_bytes)) for i in range(count)]
                connection.executemany("INSERT INTO keyed_records(id,body,payload) VALUES (?,?,?)", keyed_batch)
                blob_rows += count
                connection.commit()
                connection.execute("PRAGMA wal_checkpoint(PASSIVE)")
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        connection.execute("PRAGMA journal_mode=DELETE")
        connection.commit()
        page_count = connection.execute("PRAGMA page_count").fetchone()[0]
        page_size = connection.execute("PRAGMA page_size").fetchone()[0]
        fts_rows = connection.execute("SELECT count(*) FROM documents_fts").fetchone()[0]
        fts_shadows = {
            name: connection.execute(f'SELECT count(*) FROM "{name}"').fetchone()[0]
            for (name,) in connection.execute(
                "SELECT name FROM sqlite_schema WHERE type='table' AND name LIKE 'documents_fts_%'")
        }
    if not fts_shadows or not all(count > 0 for count in fts_shadows.values()):
        raise RuntimeError(f"FTS5 shadow tables were not populated: {fts_shadows}")
    with database.open("rb") as source:
        os.fsync(source.fileno())
    source_bytes = database.stat().st_size
    if source_bytes < minimum_bytes:
        raise RuntimeError(f"populated SQLite file is only {source_bytes} bytes; requires {minimum_bytes}")
    return {
        "extra_stored_columns_per_table": extra_columns,
        "documents_rows": text_rows,
        "keyed_records_rows": blob_rows,
        "fts5_rows": fts_rows,
        "fts5_shadow_rows": fts_shadows,
        "text_bytes_per_row": text_bytes,
        "blob_bytes_per_row": blob_bytes,
        "sqlite_page_count": page_count,
        "sqlite_page_size": page_size,
        "source_file_bytes": source_bytes,
        "generation_elapsed_seconds": round(time.monotonic() - started, 3),
    }


def run_verified_backup(helper, database: Path, work_dir: Path, diagnostics: dict,
                        *, clock=time.monotonic) -> tuple[Path, dict, float, float, float]:
    """Verify under the writer guard; lock acquisition is outside the verification budget."""
    envelope_started = clock()
    with helper.sqlite_writer_guard(str(database)):
        guarded_at = clock()
        deadline = guarded_at + helper.BACKUP_VERIFICATION_TIMEOUT_SECS
        backup = helper.create_verified_database_backup(
            str(database), work_dir, timeout=deadline - clock(), diagnostics=diagnostics)
        proof = helper._backup_proof(backup, deadline, diagnostics)
        verification_elapsed = clock() - guarded_at
    guard_acquisition_elapsed = guarded_at - envelope_started
    envelope_elapsed = clock() - envelope_started
    if verification_elapsed > helper.BACKUP_VERIFICATION_TIMEOUT_SECS:
        raise RuntimeError(f"backup, verification, proof, and fsync exceeded "
                           f"{helper.BACKUP_VERIFICATION_TIMEOUT_SECS}s: {verification_elapsed:.3f}s")
    return backup, proof, guard_acquisition_elapsed, verification_elapsed, envelope_elapsed


def run(args: argparse.Namespace) -> dict:
    helper = load_helper()
    backup_budget_seconds = helper.BACKUP_VERIFICATION_TIMEOUT_SECS
    target_bytes = args.minimum_source_bytes
    # Ensure enough space for source, verified copy, plus modest SQLite/temp headroom.
    free = shutil.disk_usage(args.work_dir).free
    required_free = target_bytes * 2 + 1_000_000_000
    if free < required_free:
        raise RuntimeError(f"insufficient free disk before generation: {free} bytes free; "
                           f"need at least {required_free} for source + backup + headroom")

    database = args.work_dir / "populated-source.sqlite"
    composition = create_fixture(database, minimum_bytes=target_bytes,
                                 rows_per_table=args.rows_per_table,
                                 text_bytes=args.text_bytes, blob_bytes=args.blob_bytes, extra_columns=args.extra_columns)
    args.fixture = composition
    args.generation_seconds = composition["generation_elapsed_seconds"]
    source_bytes = database.stat().st_size
    free_before_backup = shutil.disk_usage(args.work_dir).free
    if free_before_backup < source_bytes + 1_000_000_000:
        raise RuntimeError(f"insufficient free disk for backup: {free_before_backup} bytes free")

    diagnostics = {}
    outcome = "failure"
    try:
        (backup, proof, guard_acquisition_elapsed, verification_elapsed,
         envelope_elapsed) = run_verified_backup(helper, database, args.work_dir, diagnostics)
        outcome = "success"
    finally:
        args.diagnostics = diagnostics
        args.verification_elapsed_seconds = locals().get("verification_elapsed")
        helper._emit_verifier_diagnostics(diagnostics, outcome)
    if proof["backup_bytes"] != source_bytes:
        raise RuntimeError(f"backup size {proof['backup_bytes']} differs from source {source_bytes}")
    if source_bytes < target_bytes:
        raise RuntimeError("source database fell below required populated file size")

    return {
        "qualification": "large populated SQLite backup verification",
        "commit": os.environ.get("GITHUB_SHA") or _git_commit(),
        "runner": {"os": platform.platform(), "machine": platform.machine(),
                   "python": sys.version, "sqlite": sqlite3.sqlite_version},
        "capacity": {"free_before_generation": free,
                     "required_before_generation": required_free,
                     "free_before_backup": free_before_backup},
        "fixture": {**composition, "minimum_source_bytes": target_bytes,
                    "database_file_bytes": source_bytes,
                    "actual_populated_sqlite_file": True,
                    "payloads": "deterministic repeated TEXT/BLOB content; indexed FTS5 text"},
        "verification": {"elapsed_seconds": round(verification_elapsed, 3),
                         "guard_acquisition_seconds": round(guard_acquisition_elapsed, 3),
                         "envelope_seconds": round(envelope_elapsed, 3),
                         "budget_seconds": backup_budget_seconds,
                         "diagnostics": diagnostics,
                         "proof_bytes": proof["backup_bytes"],
                         "proof_sha256": proof["backup_sha256"]},
    }


def _git_commit() -> str:
    import subprocess
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True,
                        help="dedicated disposable runner directory for source and backup")
    parser.add_argument("--result", type=Path, default=Path("backup-verification-result.json"))
    parser.add_argument("--minimum-source-bytes", type=int, default=DEFAULT_SOURCE_BYTES,
                        help="fixture threshold (small values are intended only for local smoke tests)")
    parser.add_argument("--rows-per-table", type=int,
                        help="fixed row count per table; only for local smoke tests")
    parser.add_argument("--extra-columns", type=int, choices=(0, 24), default=0)
    parser.add_argument("--text-bytes", type=int, default=1_100)
    parser.add_argument("--blob-bytes", type=int, default=1_100)
    args = parser.parse_args()
    if args.minimum_source_bytes <= 0 or args.text_bytes <= 0 or args.blob_bytes <= 0:
        parser.error("byte and payload sizes must be positive")
    if args.rows_per_table is not None and args.rows_per_table <= 0:
        parser.error("--rows-per-table must be positive")
    args.work_dir.mkdir(parents=True, exist_ok=True)
    try:
        result = run(args)
    except Exception as exc:
        failure = {
            "qualification": "large populated SQLite backup verification",
            "status": "failed",
            "diagnostics": getattr(args, "diagnostics", None),
            "parameters": {key: getattr(args, key) for key in ("extra_columns", "text_bytes", "blob_bytes", "rows_per_table", "minimum_source_bytes")},
            "fixture": getattr(args, "fixture", None),
            "generation_seconds": getattr(args, "generation_seconds", None),
            "verification_elapsed_seconds": getattr(args, "verification_elapsed_seconds", None),
            "free_disk_bytes": shutil.disk_usage(args.work_dir).free,
            "error": f"{type(exc).__name__}: {exc}",
            "commit": os.environ.get("GITHUB_SHA") or _git_commit(),
            "runner": {"os": platform.platform(), "machine": platform.machine(),
                       "python": sys.version, "sqlite": sqlite3.sqlite_version},
        }
        args.result.parent.mkdir(parents=True, exist_ok=True)
        args.result.write_text(json.dumps(failure, indent=2) + "\n")
        print(f"backup qualification failed: {exc}", file=sys.stderr)
        return 1
    args.result.parent.mkdir(parents=True, exist_ok=True)
    args.result.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
