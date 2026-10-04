#!/usr/bin/env python3
"""Private, opt-in conversation search benchmark helper.

This module deliberately uses only stdlib. It captures SQLite with the online
backup API and never opens the production database writable.
"""
from __future__ import annotations
import argparse
import hashlib
import math
import stat
try:
    import fcntl
except ImportError:
    fcntl = None
import json
import os
import platform
import re
import shutil
import signal
import sqlite3
import subprocess
import time
import uuid
from pathlib import Path

DEFAULT_ARTIFACTS = Path("conversation-search-benchmark")
# Every named slow call is retained; duplicate IDs differing only in case are
# rejected rather than silently collapsing two production observations. Known
# transcript ids let capture use the indexed conversation_id column rather than
# scanning every content blob in a multi-gigabyte database.
KNOWN_CALLS = [
    {"conversation_id": "9bc3b72d", "prefix": True, "call_id": "call_w7yaFJY51rJxTjog4DKeE2wo"},
    {"conversation_id": "ca640950-4b82-4499-a1af-9846827f29dc", "call_id": "call_AHuEHesog7J4anh2hSiia6kh"},
]
CALL_IDS = [item["call_id"] for item in KNOWN_CALLS]
RECOVERY_MESSAGE_LIMIT = 2_000
LABEL_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")


def _private(path: Path) -> None:
    """Create a private directory and reject symlinked artifact roots."""
    if path.is_symlink():
        raise SystemExit(f"refusing symlinked artifact directory: {path}")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(path, 0o700)


def _artifact_root(value: str) -> Path:
    raw = Path(value).expanduser().absolute()
    if raw.is_symlink():
        raise SystemExit("refusing symlinked artifact root")
    if (raw / ".capture-pending").exists(): raise SystemExit("incomplete prior capture; choose a new dedicated directory")
    manifest_path = raw / "capture-manifest.json"
    if manifest_path.is_file():
        try: manifest = json.loads(manifest_path.read_text())
        except (OSError, ValueError): raise SystemExit("invalid artifact ownership manifest")
        if manifest.get("kind") != "conversation-search-fixture":
            raise SystemExit("invalid artifact ownership manifest")
    if raw.exists() and any(raw.iterdir()) and not (raw / "capture-manifest.json").is_file():
        raise SystemExit("refusing nonempty unrecognized artifact root; choose a dedicated directory")
    return raw.resolve()


def _label(label: str) -> str:
    if not LABEL_RE.fullmatch(label):
        raise SystemExit("label must contain only letters, digits, '.', '_' or '-' and be at most 64 characters")
    return label


def _write_private(path: Path, text: str) -> None:
    if path.is_symlink():
        raise SystemExit(f"refusing symlinked private artifact: {path}")
    path.write_text(text)
    os.chmod(path, 0o600)


def _git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=Path(__file__).parents[1], text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def _ensure_ignored_artifacts(outdir: Path) -> None:
    """Refuse private benchmark output in a repository-visible directory."""
    repo = Path(__file__).parents[1].resolve()
    try:
        relative = outdir.relative_to(repo)
    except ValueError:
        parent = outdir
        while not parent.exists(): parent = parent.parent
        foreign = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=parent, capture_output=True, text=True)
        if foreign.returncode == 0: raise SystemExit("refusing artifacts inside another Git worktree")
        return
    if not relative.parts:
        raise SystemExit("refusing to write benchmark artifacts in the repository root")
    nearest=outdir
    while not nearest.exists(): nearest=nearest.parent
    owner=subprocess.run(["git","rev-parse","--show-toplevel"],cwd=nearest,capture_output=True,text=True)
    if owner.returncode==0 and Path(owner.stdout.strip()).resolve()!=repo.resolve(): raise SystemExit("nested Git artifact directory refused")
    root_check = subprocess.run(["git", "check-ignore", "--quiet", str(relative) + "/"], cwd=repo)
    if root_check.returncode: raise SystemExit("refusing unignored dedicated artifact root; ignore the entire directory")
    tracked = subprocess.check_output(["git", "ls-files", "--", str(relative)], cwd=repo, text=True)
    if tracked.strip():
        raise SystemExit("refusing tracked benchmark artifacts")

def _measurement_digest() -> str:
    root=Path(__file__).parents[1]
    rust=(root/"crates/phoenix-ide/src/coordinator_tools.rs").read_text()
    loop=rust.split("async fn production_conversation_search_benchmark()",1)[1].split("    #[test]",1)[0]
    setup=(root/"crates/phoenix-db/src/lib.rs").read_text().split("pub async fn open_read_only",1)[1].split("pub async fn open(",1)[0]
    digest=hashlib.sha256(Path(__file__).read_bytes()+loop.encode()+setup.encode())
    return digest.hexdigest()


def _build_configuration() -> dict:
    if "LIBSQLITE3_SYS_USE_PKG_CONFIG" in os.environ and os.environ["LIBSQLITE3_SYS_USE_PKG_CONFIG"] != "0": raise SystemExit("external SQLite linkage unsupported for this benchmark; use pinned bundled build")
    """Capture compiler, Cargo, target, profile, and feature inputs to the run."""
    repo = Path(__file__).parents[1]
    try:
        rustc = subprocess.check_output(["rustc", "-Vv"], cwd=repo, text=True).strip()
        cargo = subprocess.check_output(["cargo", "-V"], cwd=repo, text=True).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"unable to record release build configuration: {error}") from error
    forwarded = {
        key: value
        for key, value in os.environ.items()
        if re.fullmatch(r"(?:CC|CXX|CFLAGS|CXXFLAGS|AR|ARFLAGS)_[A-Za-z0-9_]+", key) or key.startswith("CARGO_PROFILE_") or re.fullmatch(r"CARGO_TARGET_[A-Z0-9_]+_(RUSTFLAGS|LINKER|RUNNER)", key) or key in {"CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_JOBS"}
        or key in {
            "RUSTFLAGS", "RUSTUP_TOOLCHAIN", "TARGET", "PROFILE", "RUSTC",
            "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CC", "CXX", "CFLAGS", "CXXFLAGS", "AR", "ARFLAGS", "HOST_CC", "HOST_CFLAGS", "LIBSQLITE3_FLAGS", "SQLITE_MAX_VARIABLE_NUMBER", "SQLITE_MAX_EXPR_DEPTH", "LIBSQLITE3_SYS_USE_PKG_CONFIG",
        }
    }
    host = next((line.split(":", 1)[1].strip() for line in rustc.splitlines() if line.startswith("host:")), None)
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    config_paths = [cargo_home / "config", cargo_home / "config.toml"]
    project = Path(__file__).parents[1].resolve()
    config_paths += [parent / ".cargo" / name for parent in [project, *project.parents] for name in ("config", "config.toml")]
    config_hashes = {("project/" + str(path.relative_to(project)) if path.is_relative_to(project) else str(path)): _hash(path) for path in config_paths if path.is_file()}
    return {
        "cargo_config_hashes": config_hashes,
        "release_profile": __import__("tomllib").loads((project / "Cargo.toml").read_text() if (project / "Cargo.toml").exists() else "").get("profile", {}).get("release", {}),
        "rustc_version_verbose": rustc,
        "cargo_version": cargo,
        "target": forwarded.get("CARGO_BUILD_TARGET") or forwarded.get("TARGET") or host,
        "profile": "release",
        "features": [],
        "environment": forwarded,
    }


def _ensure_clean_source() -> None:
    """Require the compiled benchmark source to be identified by a commit."""
    try:
        status = subprocess.check_output(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            cwd=Path(__file__).parents[1],
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"unable to verify a clean benchmark source tree: {error}") from error
    if status.strip():
        raise SystemExit("refusing to benchmark a dirty source tree; commit all changes first")


def observed_call_ids():
    """Allow the operator to add independently verified call IDs without code edits."""
    configured = os.environ.get("PHOENIX_SEARCH_CALL_IDS", "")
    values = CALL_IDS + [item.strip() for item in configured.split(",") if item.strip()]
    seen: set[str] = set()
    result = []
    for value in values:
        normalized = value.casefold()
        if normalized not in seen:
            seen.add(normalized)
            result.append(value)
    return result


def _normalize_query(query: str) -> str:
    """Normalize harmless query formatting differences for duplicate detection."""
    return " ".join(query.split()).casefold()


def _logical_database_size(conn: sqlite3.Connection) -> int:
    """Return the logical source size, including committed pages still in WAL."""
    page_count = conn.execute("PRAGMA page_count").fetchone()[0]
    page_size = conn.execute("PRAGMA page_size").fetchone()[0]
    if not isinstance(page_count, int) or not isinstance(page_size, int) or page_count < 0 or page_size <= 0:
        raise SystemExit("source reported invalid SQLite page_count/page_size")
    return page_count * page_size


def _ensure_backup_capacity(outdir: Path, remaining: int, page_size: int, free_reserve: int) -> None:
    if shutil.disk_usage(outdir).free < remaining * page_size + free_reserve:
        raise OSError("online backup would consume the configured free-space reserve")


def _remove_private(path: Path) -> None:
    if path.is_symlink():
        raise SystemExit(f"refusing symlinked private artifact: {path}")
    if path.exists():
        path.unlink()


def _write_atomic_private(path: Path, text: str) -> None:
    """Publish a private text artifact without exposing a partial JSON document."""
    if path.is_symlink():
        raise SystemExit(f"refusing symlinked private artifact: {path}")
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    _remove_private(temporary)
    try:
        _write_private(temporary, text)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)
    os.chmod(path, 0o600)




def _uri(path: Path, *, immutable: bool = False) -> str:
    return f"{path.resolve().as_uri()}?mode=ro" + ("&immutable=1" if immutable else "")

def _hash(path: Path) -> str:
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()

def _json_strings(value):
    """Yield query strings from one tool block, decoding serialized arguments."""
    if isinstance(value, dict):
        for key in ("query", "input"):
            candidate = value.get(key)
            if isinstance(candidate, str):
                yield candidate
            elif isinstance(candidate, (dict, list)):
                yield from _json_strings(candidate)
        arguments = value.get("arguments")
        if isinstance(arguments, (dict, list)):
            yield from _json_strings(arguments)
        elif isinstance(arguments, str):
            try:
                decoded = json.loads(arguments)
            except (TypeError, ValueError):
                decoded = None
            if isinstance(decoded, (dict, list)):
                yield from _json_strings(decoded)
        # Do not recurse through arbitrary sibling blocks: a message may contain
        # several tool calls and recovering a query from the wrong one is worse
        # than refusing to make a baseline.
    elif isinstance(value, list):
        for item in value:
            yield from _json_strings(item)



def _message_columns(conn: sqlite3.Connection) -> set[str]:
    try:
        return {row[1] for row in conn.execute("PRAGMA table_info(messages)")}
    except sqlite3.Error:
        return set()


def _recover_from_rows(rows, call_id: str, found: list[dict], seen_ids: set[str]) -> None:
    for conversation_id, message_id, content, display_data in rows:
        for raw in (content, display_data):
            try:
                parsed = json.loads(raw) if raw else None
            except (TypeError, ValueError):
                continue

            def matching_blocks(value):
                if isinstance(value, dict):
                    direct_ids = {
                        value.get(key)
                        for key in ("tool_use_id", "tool_call_id", "id")
                        if isinstance(value.get(key), str)
                    }
                    if call_id in direct_ids:
                        yield value
                    else:
                        for item in value.values():
                            yield from matching_blocks(item)
                elif isinstance(value, list):
                    for item in value:
                        yield from matching_blocks(item)

            for block in matching_blocks(parsed):
                query = next(iter(_json_strings(block)), None)
                if query and query.strip() and call_id.lower() not in seen_ids:
                    found.append({
                        "query": query,
                        "source_call_id": call_id,
                        "conversation_id": conversation_id,
                        "message_id": message_id,
                    })
                    seen_ids.add(call_id.lower())
                    return


def _recover_queries(conn: sqlite3.Connection) -> list[dict]:
    """Recover named calls using bounded indexed transcript lookups when known."""
    found: list[dict] = []
    seen_ids: set[str] = set()
    columns = _message_columns(conn)
    if not columns:
        return found

    if "conversation_id" in columns:
        # The first production id is intentionally a stable prefix; the second
        # is already a complete UUID. Both predicates remain indexable and are
        # bounded to a small transcript window.
        known_rows = False
        for item in KNOWN_CALLS:
            predicate = "conversation_id=?" if not item.get("prefix") else "conversation_id GLOB ?"
            value = item["conversation_id"] + "*" if item.get("prefix") else item["conversation_id"]
            if conn.execute(f"SELECT 1 FROM messages WHERE {predicate} LIMIT 1", (value,)).fetchone():
                known_rows = True
                break
        if known_rows:
            order = "created_at, message_id" if "created_at" in columns else "message_id"
            for item in KNOWN_CALLS:
                predicate = "conversation_id=?" if not item.get("prefix") else "conversation_id GLOB ?"
                value = item["conversation_id"] + "*" if item.get("prefix") else item["conversation_id"]
                rows = conn.execute(
                    f"SELECT conversation_id,message_id,content,display_data FROM messages "
                    f"WHERE {predicate} ORDER BY {order} LIMIT ?",
                    (value, RECOVERY_MESSAGE_LIMIT),
                ).fetchall()
                _recover_from_rows(rows, item["call_id"], found, seen_ids)

    # Synthetic fixtures and older schemas retain the content fallback. It also
    # allows an operator-supplied replacement call to complete partial recovery
    # when only one of the known transcripts is present, without manufacturing
    # production transcript ids.
    conversation = "conversation_id" if "conversation_id" in columns else "NULL"
    order = "created_at, message_id" if "created_at" in columns else "message_id"
    for call_id in _recover_fallback_call_ids(conn, columns):
        if call_id.casefold() in seen_ids:
            continue
        replacement_transcript = os.environ.get("PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT")
        if "conversation_id" in columns and "created_at" in columns and not replacement_transcript:
            raise SystemExit("replacement calls require PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT for bounded recovery")
        predicate = "conversation_id = ? AND (content LIKE ? OR display_data LIKE ?)" if replacement_transcript else "content LIKE ? OR display_data LIKE ?"
        params = (replacement_transcript, f"%{call_id}%", f"%{call_id}%", RECOVERY_MESSAGE_LIMIT) if replacement_transcript else (f"%{call_id}%", f"%{call_id}%", RECOVERY_MESSAGE_LIMIT)
        rows = conn.execute(
            f"SELECT {conversation},message_id,content,display_data FROM messages "
            f"WHERE {predicate} "
            f"ORDER BY {order} LIMIT ?",
            params,
        ).fetchall()
        _recover_from_rows(rows, call_id, found, seen_ids)
    return found

def _counts(conn):
    out = {}
    for table in ('conversations', 'messages', 'message_fts_rows'):
        try: out[table] = conn.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0]
        except sqlite3.Error: out[table] = None
    try: out['fts_rows'] = conn.execute('SELECT COUNT(*) FROM message_fts').fetchone()[0]
    except sqlite3.Error: out['fts_rows'] = None
    return out


def _schema_evidence(conn) -> tuple[str, list[dict] | None]:
    objects = conn.execute(
        "SELECT type, name, tbl_name, sql FROM sqlite_master "
        "WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name"
    ).fetchall()
    schema = [
        {"type": kind, "name": name, "table": table, "sql": sql}
        for kind, name, table, sql in objects
    ]
    schema_digest = hashlib.sha256(
        json.dumps(schema, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    ledger = None
    if any(name == "_migrations" for _, name, _, _ in objects):
        ledger = [
            {"version": version, "name": name}
            for version, name in conn.execute(
                "SELECT version, name FROM _migrations ORDER BY version"
            )
        ]
    return schema_digest, ledger


def _configured_call_ids() -> list[str]:
    return [item.strip() for item in os.environ.get("PHOENIX_SEARCH_CALL_IDS", "").split(",") if item.strip()]


def _has_table(conn, name: str) -> bool:
    return conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", (name,)
    ).fetchone() is not None


def _fixture_fingerprint(path: Path) -> dict:
    if any(Path(str(path) + suffix).exists() for suffix in ("-wal", "-shm", "-journal")):
        raise SystemExit("immutable fixture has SQLite sidecars; refusing changed state")
    try:
        stat = path.stat()
        return {
            "device": stat.st_dev,
            "inode": stat.st_ino,
            "size_bytes": stat.st_size,
            "mtime_ns": stat.st_mtime_ns,
            "sha256": _hash(path),
        }
    except OSError:
        return {"missing": True}


def _carry_run_metadata(
    path: Path,
    capture: dict,
    build_configuration: dict,
    expected_case_surface_set: list[list[str]],
    *,
    run_uuid: str,
    started_at_unix: float,
    completed_at_unix: float,
    measurement_digest: str,
) -> None:
    """Attach immutable setup and execution identity evidence without changing samples."""
    try:
        run = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit("child result cannot be decoded; not a successful benchmark") from error
    run["schema_digest"] = capture["schema_digest"]
    run["migration_ledger"] = capture["migration_ledger"]
    run["build_configuration"] = build_configuration
    run["expected_case_surface_set"] = expected_case_surface_set
    run["run_uuid"] = run_uuid
    run["measurement_digest"] = measurement_digest
    run["started_at_unix"] = started_at_unix
    run["completed_at_unix"] = completed_at_unix
    _write_atomic_private(path, json.dumps(run, indent=2) + "\n")


def _recover_fallback_call_ids(conn, columns: set[str]) -> list[str]:
    configured = _configured_call_ids()
    if configured:
        return configured
    # A production-shaped schema must never scan all content blobs by default.
    # Message-only databases are the small synthetic fixtures used by tests.
    return CALL_IDS if not _has_table(conn, "conversations") else []


def _expected_case_surface_set(scenarios: list[dict]) -> list[list[str]]:
    pairs = []
    for scenario in scenarios:
        surface_names = ["retriever"] if scenario.get("kind") == "retriever" else ["tool", "retriever"]
        pairs.extend([scenario["id"], surface] for surface in surface_names)
    return sorted(pairs)


def _selective_term(conn: sqlite3.Connection, query: str) -> str:
    candidates = [
        term for term in re.findall(r"[A-Za-z0-9]{6,}", query)
        if term.casefold() not in {"conversation", "search"}
    ]
    for term in candidates:
        try:
            rows = conn.execute(
                "SELECT rowid FROM message_fts WHERE message_fts MATCH ? LIMIT 1001", ('"' + term + '"',)
            ).fetchall()
        except sqlite3.Error:
            continue
        if 0 < len(rows) <= 1000:
            return term
    raise SystemExit("no observed query token has a verified nonzero FTS match count below 1000")


def _stop_process(process) -> None:
    """Stop and reap a detached process group on every exceptional exit."""
    try:
        if process.poll() is not None:
            return
        if os.name == "posix":
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                process.wait()
                return
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
        else:
            process.kill()
            process.wait()
    finally:
        # A mocked process may not expose poll; wait is still best effort in tests.
        if process.poll() is None:
            process.wait()


def snapshot(args) -> int:
    if fcntl is None: raise SystemExit("snapshot requires POSIX flock (macOS/Linux)")
    outdir = _artifact_root(args.artifacts)
    if outdir.exists() and any(outdir.iterdir()): raise SystemExit("snapshot requires a new empty dedicated directory")
    _ensure_ignored_artifacts(outdir)
    _private(outdir)
    fd = os.open(outdir / ".capture-lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
        os.close(fd)
        raise SystemExit("capture lock must be an owned unlinked regular file")
    os.fchmod(fd, 0o600)
    with os.fdopen(fd, "r+") as lock:
        try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError: raise SystemExit("capture already active for this artifact directory")
        return _snapshot_locked(args)


def _snapshot_locked(args) -> int:
    if type(getattr(args,"deadline",300)) not in (int,float) or not math.isfinite(getattr(args,"deadline",300)) or getattr(args,"deadline",300) <= 0: raise SystemExit("snapshot deadline must be finite positive")
    source = Path(args.source).expanduser().resolve()
    outdir = Path(args.artifacts).expanduser().absolute()
    _ensure_ignored_artifacts(outdir)
    requested = outdir / "captured.db"
    if (
        source == requested
        or source == outdir
        or source in outdir.parents
        or outdir in source.parents
    ):
        raise SystemExit("refusing to overwrite or capture from the artifact directory")
    if not source.exists(): raise SystemExit(f'source does not exist: {source}')
    _private(outdir)
    dest = requested
    pending = outdir / ".capture-pending"
    if pending.exists():
        raise SystemExit("incomplete prior capture; preserved files; choose a new dedicated artifact directory")
    if (outdir / "capture-manifest.json").exists(): raise SystemExit("existing capture manifest; use a new dedicated directory")
    if dest.exists(): raise SystemExit('fixture exists; capture into a new dedicated directory (replacement unsupported)')
    if source == (Path.home() / ".phoenix-ide" / "prod.db").resolve() or ((Path.home() / ".phoenix-ide" / "prod.db").exists() and os.path.samefile(source, Path.home() / ".phoenix-ide" / "prod.db")):
        raise SystemExit("live production source refused; supply a consistent offline snapshot")
    if not getattr(args, "offline_snapshot", False):
        raise SystemExit("explicit --offline-snapshot attestation required: source is a consistent standalone snapshot, not a live DB")
    if any(Path(str(source) + suffix).exists() for suffix in ("-wal", "-shm", "-journal")):
        raise SystemExit("offline snapshot must be standalone without WAL/SHM/journal; live source refused")
    source_conn = sqlite3.connect(_uri(source, immutable=True), uri=True, timeout=args.busy_timeout)
    try:
        logical_size = _logical_database_size(source_conn)
        page_size = source_conn.execute("PRAGMA page_size").fetchone()[0]
    finally:
        source_conn.close()
    # The backup needs one logical database copy, while the second copy is a
    # reserve for WAL growth, SQLite journals, and a failed retry.
    free_reserve = logical_size
    if shutil.disk_usage(outdir).free < logical_size + free_reserve:
        raise SystemExit('insufficient free space for logical snapshot and safety reserve')
    _write_atomic_private(pending, 'initial capture staged\n')
    tmp = outdir / f'.captured.db.{os.getpid()}.tmp'
    tmp.unlink(missing_ok=True)
    started = time.time()
    deadline_seconds = getattr(args, "deadline", 300.0)
    deadline = time.monotonic() + deadline_seconds
    last = None
    progress = {"pages": 0, "remaining": 0, "total": 0}
    for attempt in range(args.retries):
        if time.monotonic() >= deadline:
            break
        src = dst = None
        try:
            src = sqlite3.connect(_uri(source, immutable=True), uri=True, timeout=args.busy_timeout)
            dst = sqlite3.connect(tmp, timeout=args.busy_timeout)
            def on_progress(status, remaining, total):
                progress.update(pages=total - remaining, remaining=remaining, total=total)
                _ensure_backup_capacity(outdir, remaining, page_size, free_reserve)
                if time.monotonic() >= deadline:
                    raise TimeoutError("online backup deadline exceeded")
            with dst:
                src.backup(dst, pages=256, sleep=0.05, progress=on_progress)
            last = None
            break
        except (sqlite3.Error, OSError, TimeoutError) as error:
            last = error
            tmp.unlink(missing_ok=True)
            if time.monotonic() < deadline:
                time.sleep(min(2**attempt, 8, max(0, deadline - time.monotonic())))
        except BaseException:
            tmp.unlink(missing_ok=True)
            raise
        finally:
            if src is not None:
                src.close()
            if dst is not None:
                dst.close()
    if last or not tmp.exists():
        raise SystemExit(f"online backup failed before deadline ({deadline_seconds}s): {last}")
    # Integrity is checked while the fixture is still private and temporary.
    # Only a verified complete backup may become the published fixture.
    conn = None
    validated = False
    try:
        conn = sqlite3.connect(_uri(tmp, immutable=True), uri=True, timeout=args.busy_timeout)
        integrity = conn.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise SystemExit(f"snapshot integrity check failed: {integrity}")
        counts = _counts(conn)
        schema_digest, migration_ledger = _schema_evidence(conn)
        recovered_queries = _recover_queries(conn)
        validated = True
    finally:
        if conn is not None:
            conn.close()
        if not validated:
            tmp.unlink(missing_ok=True)
    try:
        manifest = {'kind':'conversation-search-fixture','source_path':str(source),'source_provenance':'operator-attested consistent offline snapshot',
          'captured_at_unix':started,'snapshot_path':str(dest.resolve()),'size_bytes':tmp.stat().st_size,
          'sha256':_hash(tmp),'integrity_check':integrity,'sqlite_version':sqlite3.sqlite_version,
          'logical_size_bytes':logical_size,'page_size_bytes':page_size,
          'counts':counts,'schema_digest':schema_digest,'migration_ledger':migration_ledger,
          'recovered_queries':recovered_queries,'backup_progress':progress,
          'backup_deadline_seconds':deadline_seconds}
        _write_atomic_private(outdir / ".capture-pending", "initial capture staged\n")
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    try:
        os.replace(tmp, dest)
        os.chmod(dest, 0o600)
        _write_atomic_private(outdir/'capture-manifest.json', json.dumps(manifest, indent=2)+'\n')
    except BaseException:
        dest.unlink(missing_ok=True)
        raise
    _remove_private(outdir / ".capture-pending")
    print(f'captured immutable fixture: {dest}\nsha256: {manifest["sha256"]}\ncounts: {manifest["counts"]}')
    print(f'next: ./dev.py conversation-search prepare --artifacts {outdir}')
    return 0

def prepare(args) -> int:
    outdir = _artifact_root(args.artifacts); db = outdir/'captured.db'; manifest = outdir/'capture-manifest.json'
    _ensure_ignored_artifacts(outdir)
    if not db.exists() or not manifest.exists(): raise SystemExit('capture-manifest.json and captured.db are required')
    capture = json.loads(manifest.read_text())
    if capture.get('kind') != 'conversation-search-fixture' or Path(capture.get('snapshot_path', '')).resolve() != db:
        raise SystemExit('capture manifest does not identify this captured.db')
    if capture.get('size_bytes') != db.stat().st_size or capture.get('sha256') != _hash(db):
        raise SystemExit('captured.db does not match capture manifest')
    if Path(capture.get('source_path', '')).resolve() == db:
        raise SystemExit('refusing to benchmark the capture source directly')
    conn=sqlite3.connect(_uri(db, immutable=True), uri=True); recovered=_recover_queries(conn)
    if not recovered: raise SystemExit('named production call query was not recovered; refusing to invent a baseline')
    if len(recovered) < 2:
        raise SystemExit(
            "fewer than two independently identified slow calls were recovered; "
            "set PHOENIX_SEARCH_CALL_IDS to verified IDs rather than duplicating a query"
        )
    normalized_queries = [_normalize_query(item["query"]) for item in recovered]
    if len(set(normalized_queries)) != len(normalized_queries):
        raise SystemExit("recovered slow calls contain duplicate normalized queries; provide distinct verified call IDs")
    exact=recovered[0]['query']; ids=[]
    row=conn.execute('SELECT conversation_id FROM messages WHERE message_id=?',(recovered[0]['message_id'],)).fetchone()
    if row: ids=[row[0]]
    broad_count = conn.execute("SELECT count(*) FROM message_fts WHERE message_fts MATCH 'conversation'").fetchone()[0]
    if broad_count < 1000:
        raise SystemExit("broad candidate has fewer than1000matches; choose a representative fixture")
    selective_query = _selective_term(conn, exact)
    nohit_query = "phoenixbenchmarknosuchterm9f3c2"
    if conn.execute("SELECT count(*) FROM message_fts WHERE message_fts MATCH ?",('"' + nohit_query + '"*',)).fetchone()[0] != 0:
        raise SystemExit("no-hit candidate exists in fixture; refusing unverified scenario")
    scenarios=[
      {'id':'observed-slow-exact','kind':'tool','query':exact,'source_call_id':recovered[0]['source_call_id'],'expected':'hit'},
      {'id':'observed-slow-other','kind':'tool','query':recovered[1]['query'],'source_call_id':recovered[1]['source_call_id'],'expected':'hit'},
      {'id':'broad-common','kind':'tool','query':'conversation','expected':'hit'},
      {'id':'selective-known-match','kind':'tool','query':selective_query,'expected':'hit'},
      {'id':'verified-no-hit','kind':'tool','query':nohit_query,'expected':'no_hit'},
      {'id':'scoped-existing-transcript','kind':'retriever','scope':'conversation','query':exact,'conversation_ids':ids,'expected':'hit'},
    ]
    conn.close()
    scenarios_path = outdir/'scenarios.json'
    if scenarios_path.exists() and not getattr(args, "force", False):
        raise SystemExit(f'frozen scenarios exist (use --force only to replace): {scenarios_path}')
    scenario_manifest = {
        'version': 1,
        'fixture_sha256': capture['sha256'],
        'expected_case_surface_set': _expected_case_surface_set(scenarios),
        'scenarios': scenarios,
    }
    _write_atomic_private(scenarios_path, json.dumps(scenario_manifest, indent=2)+'\n')
    print(f'wrote frozen scenarios: {scenarios_path}'); return 0

def run(args) -> int:
    outdir = _artifact_root(args.artifacts)
    _ensure_ignored_artifacts(outdir)
    result_dir = outdir / "runs"
    _private(result_dir)
    _label(args.label)
    reservation = result_dir / ".active-run.reserved"
    try:
        fd = os.open(reservation, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        raise SystemExit("run label already reserved; inspect the existing owner before removing reservation")
    os.write(fd, f"owner_pid={os.getpid()}\n".encode())
    os.close(fd)
    handlers = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGHUP)}
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt("benchmark interrupted; child group will be stopped")
    for sig in handlers: signal.signal(sig, interrupted)
    try:
        return _run_reserved(args)
    finally:
        for sig, previous in handlers.items(): signal.signal(sig, previous)
        reservation.unlink(missing_ok=True)


def _run_reserved(args) -> int:
    outdir=_artifact_root(args.artifacts); db=outdir/'captured.db'; scen=outdir/'scenarios.json'
    _ensure_ignored_artifacts(outdir)
    manifest=outdir/'capture-manifest.json'
    if not db.exists() or not scen.exists() or not manifest.exists():
        raise SystemExit('run requires captured.db, capture-manifest.json, and scenarios.json')
    capture=json.loads(manifest.read_text())
    if not _metadata_has_values(capture.get("schema_digest")) or "migration_ledger" not in capture:
        raise SystemExit("capture-manifest.json lacks schema digest or migration ledger evidence")
    if Path(capture.get('snapshot_path', '')).resolve() != db or capture.get('sha256') != _hash(db) or capture.get('size_bytes') != db.stat().st_size:
        raise SystemExit('captured.db does not match capture-manifest.json')
    if Path(capture.get('source_path', '')).resolve() == db:
        raise SystemExit('refusing to benchmark the capture source directly')
    scenarios=json.loads(scen.read_text())
    if scenarios.get('version') != 1 or not isinstance(scenarios.get('scenarios'), list):
        raise SystemExit('invalid scenarios manifest')
    if scenarios.get('fixture_sha256') != capture.get('sha256'):
        raise SystemExit('scenarios.json does not match capture-manifest.json fixture')
    expected_set = _expected_case_surface_set(scenarios['scenarios'])
    if scenarios.get('expected_case_surface_set') != expected_set:
        raise SystemExit('scenarios.json has an invalid expected case/surface set')
    result_dir=outdir/'runs'; _private(result_dir)
    label=_label(args.label)
    output=result_dir/f'{label}.json'
    if output.exists() and not args.force:
        raise SystemExit(f'result exists (use --force only to replace): {output}')
    if args.force:
        # A failed forced rerun must not leave a previous successful result
        # masquerading as the outcome. The Rust harness publishes to a private
        # temporary path; only a complete result is renamed into place below.
        _remove_private(output)
    fixture_before = _fixture_fingerprint(db)
    run_uuid = uuid.uuid4().hex
    measurement_digest = _measurement_digest()
    launched_commit = _git_commit()
    started_at_unix = time.time()
    output_tmp = result_dir / f'.{label}.json.{os.getpid()}.tmp'
    _remove_private(output_tmp)
    failure_output = result_dir / "failures" / f"{label}.{run_uuid}.json"
    _ensure_clean_source()
    build_configuration = _build_configuration()
    cmd=['cargo','test','-p','phoenix_ide','--release','production_conversation_search_benchmark','--lib','--','--ignored','--nocapture']
    env=dict(os.environ,
        PHOENIX_SEARCH_BENCH_DB=str(db), PHOENIX_SEARCH_BENCH_SCENARIOS=str(scen),
        PHOENIX_SEARCH_BENCH_CAPTURE_MANIFEST=str(manifest), PHOENIX_SEARCH_BENCH_OUT=str(output_tmp),
        PHOENIX_SEARCH_BENCH_SCHEMA_DIGEST=str(capture.get("schema_digest", "")),
        PHOENIX_SEARCH_BENCH_MIGRATION_LEDGER=json.dumps(capture.get("migration_ledger")),
        PHOENIX_SEARCH_BENCH_COMMIT=launched_commit, PHOENIX_SEARCH_BENCH_HOST=platform.node(),
        PHOENIX_SEARCH_BENCH_PLATFORM=platform.platform(), PHOENIX_SEARCH_BENCH_PROCESSOR=platform.processor() or "unknown",
        PHOENIX_SEARCH_BENCH_CPU_COUNT=str(os.cpu_count() or 1))
    process = subprocess.Popen(
        cmd,
        cwd=Path(__file__).parents[1],
        env=env,
        start_new_session=(os.name == "posix"),
    )
    try:
        process.wait(timeout=args.timeout)
    except subprocess.TimeoutExpired as error:
        _stop_process(process)
        failure = {
            "kind": "conversation-search-benchmark-failure",
            "run_label": label,
            "status": "outer_timeout",
            "timeout_seconds": args.timeout,
            "command": cmd,
            "recorded_at_unix": time.time(),
        }
        failures_dir = result_dir / "failures"
        _private(failures_dir)
        _remove_private(output_tmp)
        _write_atomic_private(failures_dir/f"{label}.{run_uuid}.json", json.dumps(failure, indent=2) + "\n")
        raise SystemExit(f'benchmark timed out after {args.timeout}s; process group was stopped') from error
    except BaseException:
        _stop_process(process)
        _remove_private(output_tmp)
        raise
    try:
        _ensure_clean_source()
        if _git_commit() != env["PHOENIX_SEARCH_BENCH_COMMIT"]: raise SystemExit("source changed during run")
    except BaseException:
        if output_tmp.exists():
            _private(failure_output.parent)
            os.replace(output_tmp, failure_output)
        raise
    completed_at_unix = time.time()
    fixture_after = _fixture_fingerprint(db)
    if fixture_before != fixture_after:
        failures_dir = result_dir / "failures"
        _private(failures_dir)
        if output_tmp.exists():
            os.replace(output_tmp, failure_output)
            os.chmod(failure_output, 0o600)
        _write_atomic_private(
            failures_dir / f"{label}.{run_uuid}.fixture-changed.json",
            json.dumps({
                "kind": "conversation-search-benchmark-failure",
                "run_label": label,
                "status": "fixture_changed_during_run",
                "before": fixture_before,
                "after": fixture_after,
            }, indent=2) + "\n",
        )
        raise SystemExit("benchmark fixture changed while the run was in progress; result rejected")
    if process.returncode:
        failures_dir = result_dir / "failures"
        _private(failures_dir)
        if output_tmp.exists():
            os.replace(output_tmp, failure_output)
            os.chmod(failure_output, 0o600)
        else:
            _write_atomic_private(failure_output, json.dumps({
                "kind": "conversation-search-benchmark-failure",
                "run_label": label, "status": "process_failure",
                "exit_status": process.returncode,
            }, indent=2) + "\n")
        raise SystemExit(f'benchmark failed with exit status {process.returncode}; failure evidence retained')
    if not output_tmp.exists():
        raise SystemExit('benchmark completed without publishing a result')
    _carry_run_metadata(
        output_tmp,
        capture,
        build_configuration,
        expected_set,
        run_uuid=run_uuid,
        started_at_unix=started_at_unix,
        completed_at_unix=completed_at_unix,
        measurement_digest=measurement_digest,
    )
    _ensure_clean_source()
    if _git_commit() != launched_commit or _measurement_digest() != measurement_digest:
        _private(failure_output.parent)
        os.replace(output_tmp, failure_output)
        raise SystemExit("source changed before publication; private raw evidence retained")
    os.replace(output_tmp, output)
    os.chmod(output, 0o600)
    print(output); return 0

def _median(values):
    values = sorted(values)
    if not values:
        return None
    middle = len(values) // 2
    return values[middle] if len(values) % 2 else (values[middle - 1] + values[middle]) / 2


def _iqr(values):
    values = sorted(values)
    if len(values) < 2:
        return None
    middle = len(values) // 2
    lower, upper = values[:middle], values[(len(values) + 1) // 2:]
    return _median(upper) - _median(lower)


def report(args) -> int:
    outdir = _artifact_root(args.artifacts)
    _ensure_ignored_artifacts(outdir)
    files = sorted((outdir / "runs").glob("*.json"))
    if not files:
        raise SystemExit("no run results")
    runs = [(path, json.loads(path.read_text())) for path in files]
    rows = []
    for path, run in runs:
        run_label = run.get("run_label") or path.stem
        for sample in run.get("samples", []):
            # The Rust harness emits one warmup invocation per case/surface.
            # Keep failed warmups visible, but never let successful warmups
            # influence the report's timings or counts.
            if sample.get("phase") == "warmup_discarded" and sample.get("ok"):
                continue
            rows.append((run_label, path.name, sample))
    by = {}
    for run_label, filename, sample in rows:
        by.setdefault((run_label, filename, sample["case_id"], sample.get("surface", "unknown"), sample.get("phase", "unknown")), []).append(sample)
    lines = ["# Conversation search benchmark report", "", f"raw files: {len(files)}", ""]
    for path, run in runs:
        run_label = run.get("run_label") or path.stem
        lines += [
            f"## Run {run_label} / {path.name}",
            f"fixture_sha256: {run.get('fixture_sha256', 'missing')}",
            f"scenario_digest: {run.get('scenario_digest', 'missing')}",
            "",
        ]
    for (run_label, filename, case, surface, phase), values in by.items():
        durations = [x["duration_ms"] for x in values if x.get("ok")]
        errors = [x.get("error", x.get("result")) for x in values if not x.get("ok")]
        lines += [f"## {run_label} / {filename} — {case} — {surface} — {phase}", f"samples: {len(values)} successful: {len(durations)}"]
        if durations:
            lines.append(
                f"median_ms: {_median(durations):.3f} min_ms: {min(durations):.3f} "
                f"max_ms: {max(durations):.3f} iqr_ms: {_iqr(durations) or 0:.3f}"
            )
        if errors:
            lines.append(f"errors: {errors!r}")
        lines.append("")
    _write_atomic_private(outdir / "report.md", "\n".join(lines))
    print(outdir / "report.md")
    return 0

def _metadata_has_values(value) -> bool:
    if isinstance(value, dict):
        return bool(value) and all(_metadata_has_values(item) for item in value.values())
    if isinstance(value, (list, tuple)):
        return bool(value) and all(_metadata_has_values(item) for item in value)
    if isinstance(value, str):
        return bool(value.strip())
    return value is not None


def _validate_run(run: dict, name: str) -> dict:
    required = {"fixture_sha256", "schema_digest", "migration_ledger", "scenario_digest", "profile", "warmup_runs", "measured_warm_runs", "commit", "environment", "sqlite_pragmas", "runtime", "explain_enabled", "build_configuration", "expected_case_surface_set", "case_policies", "measurement_regimes", "tool_oracle_regime", "fixture_validation", "measurement_digest", "run_uuid", "started_at_unix", "completed_at_unix", "samples"}
    missing = sorted(required - run.keys())
    if missing:
        raise SystemExit(f"refusing comparison: {name} is missing metadata: {', '.join(missing)}")
    for key in required - {"samples", "migration_ledger", "build_configuration", "expected_case_surface_set"}:
        if not _metadata_has_values(run[key]):
            raise SystemExit(f"refusing comparison: {name} has empty metadata: {key}")
    for field in ("started_at_unix", "completed_at_unix"):
        value=run[field]
        if type(value) not in (int,float) or not math.isfinite(value): raise SystemExit(f"refusing comparison: {name} invalid interval")
    if run["started_at_unix"] > run["completed_at_unix"]: raise SystemExit(f"refusing comparison: {name} reversed interval")
    shapes = {
        "environment": {"host","platform","processor","cpu_count"},
        "sqlite_pragmas": {"sqlite_version","journal_mode","synchronous","busy_timeout_ms","foreign_keys","query_only"},
        "runtime": {"worker_threads","measurement_clock"},
    }
    for key, shape in shapes.items():
        if not isinstance(run[key],dict) or set(run[key]) != shape: raise SystemExit(f"invalid execution metadata: {key}")
    env = run["environment"]; pragmas=run["sqlite_pragmas"]
    if any(not isinstance(env[k],str) or not env[k].strip() for k in ("host","platform","processor")) or not str(env["cpu_count"]).isdigit() or int(env["cpu_count"])<=0: raise SystemExit("invalid environment values")
    if any(type(pragmas[k]) is not int or pragmas[k]<0 for k in ("synchronous","busy_timeout_ms")) or any(type(pragmas[k]) is not bool for k in ("foreign_keys","query_only")) or pragmas["journal_mode"] not in ("delete","truncate","persist","memory","wal","off") or not isinstance(pragmas["sqlite_version"],str) or not pragmas["sqlite_version"]: raise SystemExit("invalid SQLite values")
    if run["runtime"]["measurement_clock"] != "monotonic" or type(run["runtime"]["worker_threads"]) is not int or run["runtime"]["worker_threads"] <= 0: raise SystemExit("invalid runtime")
    freshness = run["fixture_validation"]
    shape = {"transcript_count", "freshness_batch_size", "locator_orphans", "missing_physical_rows", "unlocated_physical_rows"}
    if not isinstance(freshness, dict) or set(freshness) != shape or any(type(v) is not int for v in freshness.values()) or freshness["transcript_count"] < 0 or freshness["freshness_batch_size"] <= 0 or any(freshness[k] != 0 for k in shape - {"transcript_count", "freshness_batch_size"}):
        raise SystemExit(f"refusing comparison: {name} invalid fixture freshness record")
    policies = run["case_policies"]
    try:
        policy_keys = [(p["case_id"], p["surface"]) for p in policies]
        if not (len(set(policy_keys)) == len(policy_keys) and set(policy_keys) == {tuple(pair) for pair in run["expected_case_surface_set"]}): raise ValueError("invalid policy")
        for p in policies:
            v=p["policy"]
            if not (set(v)=={"scope","visibility","grouping","match_mode","limit","lexical_expression"}): raise ValueError("invalid policy")
            if not (type(v["limit"]) is int and v["limit"]>0): raise ValueError("invalid policy")
            if not (isinstance(v["scope"],str) and v["scope"] and v["visibility"] in {"All", "UserTopLevel"} and v["grouping"] in {"None", "BestPerConversation"} and v["match_mode"] in {"ExactTerms", "FinalTokenPrefix"} and isinstance(v["lexical_expression"], str) and v["lexical_expression"]): raise ValueError("invalid policy")
            scope = v["scope"]
            if not (scope == "Global" or re.fullmatch(r'(?:Conversations|GlobalExcluding)\(\[.*\]\)', scope)): raise ValueError("invalid policy")
    except (ValueError,KeyError,TypeError): raise SystemExit(f"refusing comparison: {name} invalid case policies")
    build = run["build_configuration"]
    if not isinstance(build, dict) or any(
        key not in build or not isinstance(build[key], (str, list, dict))
        for key in ("rustc_version_verbose", "cargo_version", "target", "profile", "features", "environment", "cargo_config_hashes", "release_profile")
    ):
        raise SystemExit(f"refusing comparison: {name} has invalid build configuration")
    if any(not isinstance(build[field],str) or not build[field].strip() for field in ("rustc_version_verbose", "cargo_version", "target", "profile")): raise SystemExit("missing compiler identity")
    if not isinstance(run["samples"], list) or not run["samples"]:
        raise SystemExit(f"refusing comparison: {name} has no samples")
    expected_set = run["expected_case_surface_set"]
    if not isinstance(expected_set, list) or any(
        not isinstance(pair, list) or len(pair) != 2 or not all(isinstance(value, str) and value for value in pair)
        for pair in expected_set
    ) or len({tuple(pair) for pair in expected_set}) != len(expected_set):
        raise SystemExit(f"refusing comparison: {name} has invalid expected case/surface set")
    digests = {}
    phases = {}
    for sample in run["samples"]:
        key = (sample.get("case_id"), sample.get("surface"))
        count = sample.get("result_count")
        identity = sample.get("result_identity")
        if not isinstance(count, int) or count < 0 or not isinstance(identity, list) or len(identity) != count:
            raise SystemExit(f"refusing comparison: {name} missing count/order evidence")
        result=sample.get("result")
        if not isinstance(result,str) or hashlib.sha256(result.encode()).hexdigest()!=sample.get("result_digest"): raise SystemExit("raw result digest mismatch")
        digest = sample.get("result_digest")
        phase = sample.get("phase")
        if not key[0] or not key[1] or not digest or not phase:
            raise SystemExit(f"refusing comparison: {name} has incomplete sample output metadata")
        if sample.get("ok") is not True:
            raise SystemExit(f"refusing comparison: {name} has errors/timeouts for {key[0]} ({key[1]})")
        duration = sample.get("duration_ms")
        if type(duration) not in (int, float) or not math.isfinite(duration) or duration < 0:
            raise SystemExit(f"refusing comparison: {name} has invalid duration for {key[0]} ({key[1]})")
        evidence = (count, tuple(identity), digest)
        prior = digests.setdefault(key, evidence)
        if prior != evidence:
            raise SystemExit(f"refusing comparison: output mismatch for {key[0]} ({key[1]}) in {name}")
        phases.setdefault(key, {}).setdefault(phase, []).append(sample)
    expected_warm = run["measured_warm_runs"]
    if expected_warm != 10:
        raise SystemExit(f"refusing comparison: {name} must declare exactly 10 measured warm runs")
    for key, by_phase in phases.items():
        first_phases = [phase for phase in by_phase if phase.startswith("first_")]
        if len(first_phases) != 1 or set(by_phase) != {first_phases[0], "warmup_discarded", "warm"}:
            raise SystemExit(f"refusing comparison: {name} has incomplete measurement phases")
        for phase, count in [(first_phases[0], 1), ("warmup_discarded", run["warmup_runs"])]:
            if sorted(sample.get("iteration") for sample in by_phase[phase]) != list(range(count)):
                raise SystemExit(f"refusing comparison: {name} has incomplete {phase} iterations")
        warm = by_phase.get("warm", [])
        iterations = sorted(sample.get("iteration") for sample in warm)
        if len(warm) != 10 or iterations != list(range(10)):
            raise SystemExit(f"refusing comparison: {name} has incomplete warm iterations for {key[0]} ({key[1]})")
    return {"digests": digests, "phases": phases}


def compare(args) -> int:
    a = json.loads(Path(args.before).read_text())
    b = json.loads(Path(args.after).read_text())
    validated_a = _validate_run(a, "before")
    validated_b = _validate_run(b, "after")
    if a["environment"].get("host") == b["environment"].get("host") and max(a["started_at_unix"], b["started_at_unix"]) < min(a["completed_at_unix"], b["completed_at_unix"]):
        raise SystemExit("refusing comparison: same-host run intervals overlap")
    if a["run_uuid"] == b["run_uuid"]:
        raise SystemExit("refusing comparison: same execution identity")
    keys = ("fixture_sha256", "schema_digest", "migration_ledger", "scenario_digest", "profile", "warmup_runs", "measured_warm_runs", "environment", "sqlite_pragmas", "runtime", "explain_enabled", "build_configuration", "expected_case_surface_set", "case_policies", "tool_oracle_regime", "fixture_validation", "measurement_digest", "measurement_regimes")
    if any(a.get(key) != b.get(key) for key in keys):
        raise SystemExit("refusing comparison: fixture, scenarios, profile, or full measurement regime differ")
    if validated_a["phases"].keys() != validated_b["phases"].keys() or set(map(tuple, a["expected_case_surface_set"])) != set(map(tuple, validated_a["phases"])):
        raise SystemExit("refusing comparison: case/surface regimes differ from the frozen manifest")
    if set(map(tuple, b["expected_case_surface_set"])) != set(map(tuple, validated_b["phases"])):
        raise SystemExit("refusing comparison: case/surface regimes differ from the frozen manifest")
    if {key: set(phases) for key, phases in validated_a["phases"].items()} != {key: set(phases) for key, phases in validated_b["phases"].items()}:
        raise SystemExit("refusing comparison: measurement phase names differ")
    if validated_a["digests"] != validated_b["digests"]:
        digests_a, digests_b = validated_a["digests"], validated_b["digests"]
        raise SystemExit("refusing comparison: output mismatch (identity/digests differ between runs)")
    medians = []
    for run in (a, b):
        by = {}
        for sample in run["samples"]:
            if sample.get("phase") == "warm" and sample.get("ok"):
                by.setdefault((sample["case_id"], sample["surface"]), []).append(sample["duration_ms"])
        medians.append(by)
    print("comparable fixture/scenarios/profile/regimes; per-case warm medians (before -> after):")
    for key in sorted(set(medians[0]) | set(medians[1])):
        before = _median(medians[0].get(key, []))
        after = _median(medians[1].get(key, []))
        if before is None or after is None:
            raise SystemExit(f"refusing comparison: missing successful warm samples for {key[0]} ({key[1]})")
        print(f"  {key[0]} ({key[1]}): {before:.3f} -> {after:.3f} ms")
    return 0

def main():
    p=argparse.ArgumentParser(); sub=p.add_subparsers(dest='command',required=True)
    s=sub.add_parser("snapshot")
    s.add_argument("--source", required=True)
    s.add_argument("--offline-snapshot", action="store_true", help="attest source is a consistent standalone offline snapshot, never a live DB copy")
    s.add_argument("--artifacts", default=str(DEFAULT_ARTIFACTS))
    s.add_argument("--retries", type=int, default=5)
    s.add_argument("--busy-timeout", type=float, default=5.0)
    s.add_argument("--deadline", type=float, default=300.0)
    s.set_defaults(func=snapshot)
    for name in ('prepare','run','report'):
      x=sub.add_parser(name); x.add_argument('--artifacts',default=str(DEFAULT_ARTIFACTS)); x.set_defaults(func=globals()[name])
    sub.choices['prepare'].add_argument('--force',action='store_true')
    sub.choices['run'].add_argument('--label',default='suite-1')
    sub.choices['run'].add_argument('--timeout',type=float,default=1800.0)
    sub.choices['run'].add_argument('--force',action='store_true')
    c=sub.add_parser('compare'); c.add_argument('before'); c.add_argument('after'); c.set_defaults(func=compare)
    args = p.parse_args()
    return args.func(args)
if __name__=='__main__': main()
