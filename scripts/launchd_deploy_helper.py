#!/usr/bin/env python3
"""Independent launchd activation with verified backup and bounded pre-migration recovery."""
from __future__ import annotations

import argparse
import contextlib
import dataclasses
import datetime as dt
import fcntl
import hashlib
import json
import os
import plistlib
import re
import shutil
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Callable

HANDOFF_PROTOCOL_VERSION = 3
DEPLOY_HELPER_LABEL_PREFIX = "com.phoenix-ide.deploy"
DEFAULT_TRANSITION_TIMEOUT_SECS = 30.0
DEFAULT_HEALTH_TIMEOUT_SECS = 120.0
MIGRATION_TIMEOUT_SECS = 600
FULL_GIT_SHA_RE = re.compile(r"[0-9a-f]{40}")
HASH_RE = re.compile(r"[0-9a-f]{64}")
TERMINAL_STATES = {"committed", "precondition_failed", "failed"}


class ActivationError(RuntimeError):
    pass


class ConcurrentDeploy(ActivationError):
    pass


@dataclasses.dataclass(frozen=True)
class Identity:
    version: str
    git_sha: str


@dataclasses.dataclass(frozen=True)
class Manifest:
    manifest_version: int
    transaction_id: str
    source_kind: str
    source_commit: str
    expected: Identity
    candidate_binary: str
    candidate_binary_sha256: str
    candidate_plist: str
    candidate_plist_sha256: str
    target_binary: str
    target_plist: str
    label: str
    helper_label: str
    uid: int
    health_url: str
    health_insecure_tls: bool
    active_path: str
    status_path: str
    deployed_sha_path: str
    lock_path: str
    claim_lock_path: str
    created_at: str
    release_tag: str | None = None
    release_commit: str | None = None
    transition_timeout_secs: float = DEFAULT_TRANSITION_TIMEOUT_SECS
    health_timeout_secs: float = DEFAULT_HEALTH_TIMEOUT_SECS

    @classmethod
    def load(cls, path: Path) -> Manifest:
        try:
            if path.is_symlink() or not path.is_file():
                raise ActivationError("manifest must be a regular file")
            raw = json.loads(path.read_text())
            if not isinstance(raw, dict) or raw.get("manifest_version") != HANDOFF_PROTOCOL_VERSION:
                raise ActivationError("unsupported deployment manifest; use its retained helper for diagnosis")
            raw["expected"] = Identity(**raw["expected"])
            manifest = cls(**raw)
            validate_manifest_identities(manifest)
            return manifest
        except (OSError, TypeError, KeyError, ValueError) as exc:
            raise ActivationError(f"invalid manifest: {exc}") from exc


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def fsync_dir(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_write(path: Path, data: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            os.fchmod(stream.fileno(), mode)
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        fsync_dir(path.parent)
    finally:
        Path(temporary).unlink(missing_ok=True)


def prepare_atomic_install(source: Path, target: Path, mode: int) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix=f".{target.name}.", dir=target.parent)
    try:
        with os.fdopen(fd, "wb") as outgoing, source.open("rb") as incoming:
            os.fchmod(outgoing.fileno(), mode)
            shutil.copyfileobj(incoming, outgoing)
            outgoing.flush()
            os.fsync(outgoing.fileno())
        return Path(temporary)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def prepare_plist_publication(source: Path, target: Path) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix=f".{target.name}.publish-", dir=target.parent)
    os.close(fd)
    prepared = Path(temporary)
    prepared.unlink()
    try:
        os.link(source, prepared)
        prepared.chmod(0o600)
        fsync_dir(target.parent)
        return prepared
    except BaseException:
        prepared.unlink(missing_ok=True)
        raise


def commit_atomic_install(prepared: Path, target: Path) -> None:
    os.replace(prepared, target)
    fsync_dir(target.parent)


def verify_staged(path: str, expected_hash: str, description: str) -> Path:
    candidate = Path(path)
    if candidate.is_symlink() or not candidate.is_file():
        raise ActivationError(f"{description} must be a regular file")
    if sha256(candidate) != expected_hash:
        raise ActivationError(f"{description} checksum mismatch")
    return candidate


def validate_manifest_identities(manifest: Manifest) -> None:
    if type(manifest.manifest_version) is not int or manifest.manifest_version != HANDOFF_PROTOCOL_VERSION:
        raise ActivationError("unsupported deployment manifest")
    if not isinstance(manifest.expected, Identity):
        raise ActivationError("expected runtime identity is missing")
    if (not isinstance(manifest.expected.version, str) or not manifest.expected.version
            or not isinstance(manifest.source_commit, str)
            or FULL_GIT_SHA_RE.fullmatch(manifest.source_commit) is None
            or manifest.expected.git_sha != manifest.source_commit):
        raise ActivationError("candidate identity must match the complete source commit")
    if manifest.source_kind not in {"local_head", "published_release"}:
        raise ActivationError("only ordinary local or published candidates are supported")
    if manifest.source_kind == "published_release":
        if (not isinstance(manifest.release_tag, str) or not manifest.release_tag
                or manifest.release_commit != manifest.source_commit):
            raise ActivationError("release identity does not match selected source")
    elif manifest.release_tag is not None or manifest.release_commit is not None:
        raise ActivationError("local candidate cannot carry release identity")
    if (not isinstance(manifest.transaction_id, str)
            or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", manifest.transaction_id) is None
            or manifest.helper_label != f"{DEPLOY_HELPER_LABEL_PREFIX}.{manifest.transaction_id}"
            or not isinstance(manifest.label, str) or not manifest.label
            or manifest.label == manifest.helper_label
            or type(manifest.uid) is not int or manifest.uid != os.getuid()):
        raise ActivationError("invalid launchd invocation identity")
    for field in ("candidate_binary", "candidate_plist", "target_binary", "target_plist",
                  "active_path", "status_path", "deployed_sha_path", "lock_path", "claim_lock_path"):
        value = getattr(manifest, field)
        if not isinstance(value, str) or not Path(value).is_absolute():
            raise ActivationError(f"{field} must be an absolute path")
    paths = [Path(getattr(manifest, field)).resolve(strict=False) for field in (
        "candidate_binary", "candidate_plist", "target_binary", "target_plist", "active_path",
        "status_path", "deployed_sha_path", "lock_path", "claim_lock_path")]
    if len(paths) != len(set(paths)):
        raise ActivationError("deployment paths must be distinct")
    for field in ("candidate_binary_sha256", "candidate_plist_sha256"):
        if not isinstance(getattr(manifest, field), str) or HASH_RE.fullmatch(getattr(manifest, field)) is None:
            raise ActivationError(f"{field} must be a SHA-256 digest")
    if not isinstance(manifest.health_url, str):
        raise ActivationError("health endpoint must be a URL")
    endpoint = urllib.parse.urlsplit(manifest.health_url)
    if (endpoint.scheme not in {"http", "https"} or not endpoint.hostname
            or endpoint.username is not None or endpoint.password is not None
            or endpoint.path != "/api/version" or endpoint.query or endpoint.fragment
            or type(manifest.health_insecure_tls) is not bool):
        raise ActivationError("health endpoint must be credential-free /api/version")
    if any(type(value) not in (int, float) or not 0 < value <= 600 for value in (
        manifest.transition_timeout_secs, manifest.health_timeout_secs)):
        raise ActivationError("invalid launchd or health timeout")


def validate_pre_disruption_handoff(manifest: Manifest) -> dict[str, str]:
    validate_manifest_identities(manifest)
    binary = verify_staged(manifest.candidate_binary, manifest.candidate_binary_sha256, "candidate binary")
    plist = verify_staged(manifest.candidate_plist, manifest.candidate_plist_sha256, "candidate plist")
    try:
        with plist.open("rb") as stream:
            config = plistlib.load(stream)
        if (config.get("Label") != manifest.label
                or config.get("ProgramArguments") != [manifest.target_binary]):
            raise ActivationError("candidate plist does not bind the target binary and label")
        environment = config.get("EnvironmentVariables", {})
        if not isinstance(environment, dict) or any(
            not isinstance(key, str) or not isinstance(value, str) for key, value in environment.items()
        ):
            raise ActivationError("candidate environment must contain strings")
        if not environment.get("PHOENIX_DB_PATH") or not Path(environment["PHOENIX_DB_PATH"]).is_absolute():
            raise ActivationError("candidate plist must select an absolute database path")
        result = subprocess.run([str(binary), "--build-identity"], check=True, capture_output=True,
                                text=True, timeout=10)
        if json.loads(result.stdout) != dataclasses.asdict(manifest.expected):
            raise ActivationError("candidate embedded identity mismatch")
    except (OSError, ValueError, TypeError, subprocess.SubprocessError) as exc:
        raise ActivationError(f"candidate validation failed: {type(exc).__name__}") from exc
    return environment


def write_status(manifest: Manifest, state: str, *, failure: str | None = None) -> None:
    payload = {
        "manifest_version": HANDOFF_PROTOCOL_VERSION,
        "transaction_id": manifest.transaction_id,
        "state": state,
        "source_kind": manifest.source_kind,
        "source_commit": manifest.source_commit,
        "release_tag": manifest.release_tag,
        "release_commit": manifest.release_commit,
        "expected_version": manifest.expected.version,
        "expected_git_sha": manifest.expected.git_sha,
        "created_at": manifest.created_at,
        "updated_at": utc_now(),
        "failure": failure,
    }
    atomic_write(Path(manifest.status_path), (json.dumps(payload, indent=2) + "\n").encode())


def service_absence_confirmed(output: str, label: str, uid: int) -> bool:
    return re.search(r'^\s*Could not find service "' + re.escape(label)
                     + r'" in domain (?:gui/' + str(uid) + r'|for user gui: ' + str(uid)
                     + r')\s*$', output, re.MULTILINE) is not None


class Launchctl:
    def __init__(self, manifest: Manifest, run: Callable = subprocess.run):
        self.manifest = manifest
        self.run = run
        self.domain = f"gui/{manifest.uid}"
        self.target = f"{self.domain}/{manifest.label}"

    def inspect(self) -> tuple[str, int | None]:
        result = self.run(["launchctl", "print", self.target], capture_output=True, text=True, timeout=10)
        if result.returncode != 0:
            if service_absence_confirmed(result.stdout + "\n" + result.stderr, self.manifest.label, self.manifest.uid):
                return "not_loaded", None
            raise ActivationError("launchctl could not establish target state")
        state, pid = "unknown", None
        for raw in result.stdout.splitlines():
            line = raw.strip()
            if line.startswith("state = "):
                state = line.partition(" = ")[2]
            elif line.startswith("pid = "):
                try:
                    pid = int(line.partition(" = ")[2])
                except ValueError:
                    pass
        return state, pid

    def wait(self, predicate: Callable, deadline: float, description: str) -> tuple[str, int | None]:
        while True:
            observed = self.inspect()
            if predicate(*observed):
                return observed
            if time.monotonic() >= deadline:
                raise ActivationError(f"timed out waiting for launchd {description}; state={observed[0]} pid={observed[1]}")
            time.sleep(0.1)

    def stop(self, *, expected_pid: int | None = None) -> int | None:
        state, old_pid = self.inspect()
        if state != "not_loaded":
            if expected_pid is not None and old_pid != expected_pid:
                raise ActivationError(f"recovery target changed before bootout: {state} pid={old_pid}")
            result = self.run(["launchctl", "bootout", self.target], capture_output=True, text=True, timeout=10)
            if result.returncode != 0:
                raise ActivationError(f"launchctl bootout failed with exit {result.returncode}")
            self.wait(lambda state, pid: state == "not_loaded" and pid is None,
                      time.monotonic() + self.manifest.transition_timeout_secs, "teardown")
        return old_pid

    def start(self, old_pid: int | None, *, plist_path: str | None = None) -> int:
        result = self.run(["launchctl", "bootstrap", self.domain, plist_path or self.manifest.candidate_plist],
                          capture_output=True, text=True, timeout=10)
        if result.returncode != 0:
            raise ActivationError(f"launchctl bootstrap failed with exit {result.returncode}")
        _, pid = self.wait(lambda state, pid: state in {"running", "active"}
                           and pid is not None and pid != old_pid,
                           time.monotonic() + self.manifest.transition_timeout_secs, "new process")
        assert pid is not None
        return pid


def fetch_identity(url: str, timeout: float = 2, insecure_tls: bool = False) -> Identity:
    context = ssl._create_unverified_context() if insecure_tls else None
    with urllib.request.urlopen(url, timeout=timeout, context=context) as response:
        body = json.load(response)
    return Identity(version=body["version"], git_sha=body["git_sha"])


def wait_for_identity(manifest: Manifest, expected: Identity | None = None) -> None:
    expected = expected or manifest.expected
    deadline = time.monotonic() + manifest.health_timeout_secs
    observed = "not responding"
    while time.monotonic() < deadline:
        try:
            actual = fetch_identity(manifest.health_url, insecure_tls=manifest.health_insecure_tls)
            observed = f"version={actual.version} git_sha={actual.git_sha}"
            if actual == expected:
                return
        except (OSError, ValueError, KeyError, TypeError) as exc:
            observed = type(exc).__name__
        time.sleep(0.2)
    raise ActivationError(f"exact candidate health verification failed: {observed}")


def release_claim(manifest: Manifest) -> bool:
    with Path(manifest.claim_lock_path).open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        active = Path(manifest.active_path)
        if active.is_symlink() or not active.is_file() or active.read_text().strip() != manifest.transaction_id:
            return False
        active.unlink()
        fsync_dir(active.parent)
        return True


def warn_committed(message: str) -> None:
    try:
        print(message, file=sys.stderr, flush=True)
    except (OSError, ValueError):
        pass


def _database_size_bytes(database_path: str) -> int:
    import sqlite3

    path = Path(database_path)
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise ActivationError("configured production database must be an existing regular file")
    try:
        with sqlite3.connect(f"{path.absolute().as_uri()}?mode=ro", uri=True, timeout=5) as database:
            page_count = database.execute("PRAGMA page_count").fetchone()[0]
            page_size = database.execute("PRAGMA page_size").fetchone()[0]
    except (sqlite3.Error, OSError, TypeError) as exc:
        raise ActivationError(f"cannot inspect production database capacity: {exc}") from exc
    if type(page_count) is not int or type(page_size) is not int or page_count < 0 or page_size <= 0:
        raise ActivationError("production database returned invalid SQLite capacity")
    return page_count * page_size


@contextlib.contextmanager
def _diagnostic_stage(diagnostics: dict | None, name: str):
    if diagnostics is None:
        yield
        return
    started = time.monotonic()
    diagnostics["current_stage"] = name
    if "deadline" in diagnostics:
        diagnostics["stage_budget_ms"] = round(max(0, diagnostics["deadline"] - started) * 1000, 3)
    try:
        yield
    finally:
        timings = diagnostics.setdefault("stage_ms", {})
        timings[name] = round(timings.get(name, 0.0) + (time.monotonic() - started) * 1000, 3)


def _emit_verifier_diagnostics(diagnostics: dict, outcome: str) -> None:
    """Emit one bounded, content-free verifier summary without affecting the operation."""
    try:
        record = {"event": "sqlite_backup_verifier", "outcome": outcome,
                  **{key: diagnostics.get(key) for key in (
                      "current_stage", "stage_budget_ms", "stage_ms", "table_ordinal",
                      "rows_compared", "cells_compared", "table_rows_compared",
                      "table_cells_compared", "table_elapsed_ms", "tables_completed")}}
        sys.stderr.write(json.dumps(record, separators=(",", ":")) + "\n")
        sys.stderr.flush()
    except Exception:
        pass


def _verify_logical_equivalence(source, backup, deadline: float, diagnostics: dict | None = None) -> None:
    """Compare SQLite metadata and persisted rows without constructing SQL dumps."""
    if diagnostics is not None:
        diagnostics["deadline"] = deadline
        diagnostics.setdefault("rows_compared", 0)
        diagnostics.setdefault("cells_compared", 0)
        diagnostics.setdefault("tables_completed", 0)
    import sqlite3
    import struct

    def check_deadline() -> None:
        if time.monotonic() >= deadline:
            raise ActivationError("SQLite logical verification deadline exceeded")

    def text(value) -> str:
        return value.decode("utf-8") if isinstance(value, bytes) else value

    def quote(identifier: str) -> str:
        return '"' + identifier.replace('"', '""') + '"'

    def pragma_rows(database, statement: str):
        check_deadline()
        rows = database.execute(statement).fetchall()
        check_deadline()
        return rows

    def metadata(database):
        check_deadline()
        db_list = pragma_rows(database, "PRAGMA database_list")
        schemas = [text(row[1]) for row in db_list if text(row[1]) != "temp"]
        info = []
        for schema in schemas:
            qschema = quote(schema)
            schema_rows = pragma_rows(
                database,
                f"SELECT type,name,tbl_name,rootpage,sql FROM {qschema}.sqlite_schema ORDER BY type,name,tbl_name",
            )
            table_list = pragma_rows(database, f"PRAGMA {qschema}.table_list")
            table_entries = []
            for row in table_list:
                if text(row[0]) != schema:
                    continue
                name, kind = text(row[1]), text(row[2])
                xinfo = pragma_rows(database, f"PRAGMA {qschema}.table_xinfo({quote(name)})")
                table_entries.append((name, kind, tuple(xinfo)))
            settings = (pragma_rows(database, f"PRAGMA {qschema}.user_version")[0][0],
                        pragma_rows(database, f"PRAGMA {qschema}.application_id")[0][0],
                        text(pragma_rows(database, f"PRAGMA {qschema}.encoding")[0][0]))
            info.append((schema, tuple(schema_rows), tuple(row for row in table_list if text(row[0]) == schema),
                         tuple(table_entries), settings))
        return tuple(info)

    def module_for(sql):
        sql = text(sql or b"")
        length = len(sql)
        position = 0

        def token():
            nonlocal position
            while position < length:
                if sql[position].isspace():
                    position += 1
                elif sql.startswith("--", position):
                    end = sql.find("\n", position + 2)
                    position = length if end < 0 else end + 1
                elif sql.startswith("/*", position):
                    end = sql.find("*/", position + 2)
                    if end < 0:
                        return None
                    position = end + 2
                else:
                    break
            if position >= length:
                return None

            char = sql[position]
            if char in ("'", '"', '`', '['):
                closing = ']' if char == '[' else char
                position += 1
                value = []
                while position < length:
                    current = sql[position]
                    position += 1
                    if current == closing:
                        if closing != ']' and position < length and sql[position] == closing:
                            value.append(closing)
                            position += 1
                            continue
                        return ("identifier", "".join(value))
                    value.append(current)
                return None
            if char == '.':
                position += 1
                return ("dot", char)
            start = position
            while position < length and not sql[position].isspace() and sql[position] not in ".();'\"`[]":
                if sql.startswith("--", position) or sql.startswith("/*", position):
                    break
                position += 1
            if start == position:
                return None
            return ("word", sql[start:position])

        def take_word(expected):
            item = token()
            if item is None or item[0] != "word" or item[1].upper() != expected:
                return False
            return True

        if not (take_word("CREATE") and take_word("VIRTUAL") and take_word("TABLE")):
            return None
        next_item = token()
        if next_item is not None and next_item[0] == "word" and next_item[1].upper() == "IF":
            if not (take_word("NOT") and take_word("EXISTS")):
                return None
            next_item = token()
        if next_item is None or next_item[0] not in ("word", "identifier"):
            return None
        after_name = position
        separator = token()
        if separator == ("dot", "."):
            table = token()
            if table is None or table[0] not in ("word", "identifier"):
                return None
        else:
            position = after_name
        if not take_word("USING"):
            return None
        module = token()
        if module is None or module[0] not in ("word", "identifier"):
            return None
        return module[1].lower()

    def typed_value(storage, value):
        storage = text(storage)
        if storage == "null":
            return (storage, None)
        if storage == "integer":
            return (storage, int(value))
        if storage == "real":
            return (storage, struct.pack(">d", float(value)))
        if storage in ("text", "blob"):
            if not isinstance(value, bytes):
                raise ActivationError("SQLite returned a non-byte TEXT/BLOB value during verification")
            return (storage, value)
        raise ActivationError(f"unsupported SQLite storage class during verification: {storage!r}")

    def compare_table(source, backup, schema: str, name: str, kind: str, xinfo) -> None:
        check_deadline()
        if kind == "virtual":
            return  # FTS5's persisted shadow tables carry its stored contents.
        if kind not in ("table", "shadow"):
            return
        columns = [(text(row[1]), text(row[2]), int(row[5])) for row in xinfo if int(row[6]) not in (1, 2)]
        names = [column[0] for column in columns]
        list_row = next(row for row in pragma_rows(source, f"PRAGMA {quote(schema)}.table_list")
                        if text(row[1]) == name and text(row[0]) == schema)
        without_rowid = bool(list_row[4])
        if without_rowid:
            indexes = pragma_rows(source, f"PRAGMA {quote(schema)}.index_list({quote(name)})")
            pk_name = next((text(row[1]) for row in indexes if text(row[3]) == "pk"), None)
            if pk_name is None:
                raise ActivationError(f"SQLite WITHOUT ROWID table ordinal {(diagnostics or {}).get('table_ordinal', 0)} has no primary key")
            keys = [row for row in pragma_rows(source, f"PRAGMA {quote(schema)}.index_xinfo({quote(pk_name)})")
                    if row[5]]
            if not keys or any(row[1] < 0 or row[2] is None or row[4] is None for row in keys):
                raise ActivationError(f"cannot identify SQLite primary-key ordering for table ordinal {(diagnostics or {}).get('table_ordinal', 0)}")
            order = ",".join(f"{quote(text(row[2]))} COLLATE {quote(text(row[4]))}"
                             + (" DESC" if row[3] else " ASC") for row in keys)
            identity = []
        else:
            aliases = {"rowid", "_rowid_", "oid"} - {text(column[1]).lower() for column in xinfo}
            if not aliases:
                raise ActivationError(f"cannot verify rowid identity for table ordinal {(diagnostics or {}).get('table_ordinal', 0)}")
            rowid = next(alias for alias in ("rowid", "_rowid_", "oid") if alias in aliases)
            order = quote(rowid)
            identity = [quote(rowid)]
        selected = identity + [part for column in names for part in (f"typeof({quote(column)})", quote(column))]
        table = f"{quote(schema)}.{quote(name)}"
        query = f"SELECT {','.join(selected)} FROM {table} ORDER BY {order}"
        check_deadline()
        left, right = source.execute(query), backup.execute(query)
        while True:
            check_deadline()
            a, b = left.fetchone(), right.fetchone()
            check_deadline()
            if a is None or b is None:
                if a != b:
                    raise ActivationError(f"SQLite backup row count differs in table ordinal {(diagnostics or {}).get('table_ordinal', 0)}")
                break
            offset = len(identity)
            if identity and a[0] != b[0]:
                raise ActivationError(f"SQLite backup row identity differs in table ordinal {(diagnostics or {}).get('table_ordinal', 0)}")
            for index in range(offset, len(a), 2):
                if diagnostics is not None:
                    diagnostics["cells_compared"] += 1
                    diagnostics["table_cells_compared"] += 1
                if typed_value(a[index], a[index + 1]) != typed_value(b[index], b[index + 1]):
                    raise ActivationError(f"SQLite backup row differs in table ordinal {(diagnostics or {}).get('table_ordinal', 0)}")
            if diagnostics is not None:
                diagnostics["rows_compared"] += 1
                diagnostics["table_rows_compared"] += 1

    old_factories = source.text_factory, backup.text_factory
    try:
        source.text_factory = backup.text_factory = bytes
        handler = lambda: 1 if time.monotonic() >= deadline else 0
        source.set_progress_handler(handler, 1000)
        backup.set_progress_handler(handler, 1000)
        with _diagnostic_stage(diagnostics, "metadata"):
            check_deadline()
            source_metadata = metadata(source)
            backup_metadata = metadata(backup)
            if source_metadata != backup_metadata:
                raise ActivationError("SQLite backup schema or database metadata differs from the stopped source")
        with _diagnostic_stage(diagnostics, "table_comparisons"):
            table_ordinal = 0
            for schema, _schema_rows, _table_list, tables, _settings in source_metadata:
                for name, kind, xinfo in tables:
                    if kind == "virtual":
                        table_ordinal += 1
                        if diagnostics is not None:
                            diagnostics["table_ordinal"] = table_ordinal
                            diagnostics["table_rows_compared"] = 0
                            diagnostics["table_cells_compared"] = 0
                            diagnostics["table_elapsed_ms"] = None
                        sql = next((row[4] for row in _schema_rows if text(row[0]) == "table" and text(row[1]) == name), None)
                        if module_for(sql) != "fts5":
                            raise ActivationError(f"cannot verify unsupported SQLite virtual table module at table ordinal {table_ordinal}")
                        continue
                    table_ordinal += 1
                    if diagnostics is not None:
                        diagnostics["table_ordinal"] = table_ordinal
                    table_started = time.monotonic()
                    if diagnostics is not None:
                        diagnostics["table_rows_compared"] = 0
                        diagnostics["table_cells_compared"] = 0
                    try:
                        compare_table(source, backup, schema, name, kind, xinfo)
                    finally:
                        if diagnostics is not None:
                            diagnostics["table_elapsed_ms"] = round((time.monotonic() - table_started) * 1000, 3)
                    if diagnostics is not None:
                        diagnostics["tables_completed"] = diagnostics.get("tables_completed", 0) + 1
        check_deadline()
    except sqlite3.Error as exc:
        if time.monotonic() >= deadline:
            raise ActivationError("SQLite logical verification deadline exceeded") from None
        raise ActivationError("SQLite logical verification database operation failed") from None
    finally:
        source.text_factory, backup.text_factory = old_factories




@contextlib.contextmanager
def sqlite_writer_guard(database_path: str, *, timeout: float = 5):
    import sqlite3

    # A separate read/write connection reserves the writer slot while the native
    # backup reads through its own read-only connection.
    with contextlib.closing(sqlite3.connect(f"{Path(database_path).absolute().as_uri()}?mode=rw", uri=True,
                                            timeout=timeout)) as guard:
        guard.execute(f"PRAGMA busy_timeout={max(1, int(timeout * 1000))}")
        guard.execute("BEGIN IMMEDIATE")
        try:
            yield
        finally:
            guard.rollback()


def create_verified_database_backup(database_path: str, directory: Path, *, timeout: float = 600,
                                    diagnostics: dict | None = None) -> Path:
    import sqlite3

    source_path = Path(database_path)
    deadline = time.monotonic() + timeout
    if diagnostics is not None:
        diagnostics["deadline"] = deadline
    fd, name = tempfile.mkstemp(prefix="database-backup-", suffix=".sqlite", dir=directory)
    os.close(fd)
    backup_path = Path(name)

    def remaining() -> float:
        left = deadline - time.monotonic()
        if left <= 0:
            raise ActivationError("SQLite backup and verification deadline exceeded")
        return left

    def progress(_status: int, _remaining_pages: int, _total_pages: int) -> None:
        remaining()

    try:
        os.chmod(backup_path, 0o600)
        with contextlib.closing(sqlite3.connect(f"{source_path.absolute().as_uri()}?mode=ro", uri=True, timeout=remaining())) as source:
            source.execute(f"PRAGMA busy_timeout={max(1, min(100, int(remaining() * 1000)))}")
            source.set_progress_handler(lambda: 1 if time.monotonic() >= deadline else 0, 1000)
            with contextlib.closing(sqlite3.connect(backup_path, timeout=remaining())) as backup:
                backup.execute(f"PRAGMA busy_timeout={max(1, min(100, int(remaining() * 1000)))}")
                backup.set_progress_handler(lambda: 1 if time.monotonic() >= deadline else 0, 1000)
                with _diagnostic_stage(diagnostics, "native_backup"):
                    source.backup(backup, pages=128, progress=progress, sleep=0.05)
                remaining()
                with _diagnostic_stage(diagnostics, "integrity_check"):
                    check = backup.execute("PRAGMA integrity_check").fetchone()
                if time.monotonic() >= deadline:
                    raise ActivationError("SQLite integrity verification deadline exceeded")
                if check != ("ok",):
                    raise ActivationError("SQLite backup integrity check failed")
                _verify_logical_equivalence(source, backup, deadline, diagnostics)
        remaining()
        with _diagnostic_stage(diagnostics, "fsync"):
            fd = os.open(backup_path, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
            fsync_dir(directory)
        return backup_path
    except BaseException as exc:
        backup_path.unlink(missing_ok=True)
        if isinstance(exc, sqlite3.Error) and time.monotonic() >= deadline:
            raise ActivationError("SQLite backup and verification deadline exceeded") from None
        if isinstance(exc, sqlite3.Error):
            raise ActivationError("SQLite backup database operation failed") from None
        raise


def _first_install_capacity(database_path: str) -> int:
    path = Path(database_path)
    if not path.is_absolute() or os.path.lexists(path):
        raise ActivationError("first-install database must still be absent")
    if not path.parent.is_dir():
        raise ActivationError("first-install database parent must be an existing directory")
    reserve = 16 * 1024 * 1024
    available = shutil.disk_usage(path.parent).free
    if available < reserve:
        raise ActivationError(
            f"insufficient space for first-install database and deploy overhead: need {reserve} bytes, have {available}"
        )
    return reserve


def _preflight_backup_capacity(database_path: str, directory: Path) -> int:
    database_bytes = _database_size_bytes(database_path)
    # Native backup may need one full DB copy; 16 MiB covers bounded status,
    # service, journal, and filesystem metadata overhead without a size floor.
    required = database_bytes + 16 * 1024 * 1024
    available = shutil.disk_usage(directory).free
    if available < required:
        raise ActivationError(
            f"insufficient space for verified SQLite backup and deploy overhead: need {required} bytes, have {available}"
        )
    return required


def write_proof(manifest: Manifest, **fields) -> None:
    path = Path(manifest.status_path).with_suffix(".proof.json")
    previous = {}
    if path.is_file() and not path.is_symlink():
        try:
            loaded = json.loads(path.read_text())
            if loaded.get("transaction_id") == manifest.transaction_id:
                previous = loaded
        except (OSError, ValueError, AttributeError):
            pass
    previous.update({"transaction_id": manifest.transaction_id, "updated_at": utc_now(), **fields})
    atomic_write(path, (json.dumps(previous, indent=2) + "\n").encode())


def capture_predecessor(manifest: Manifest, launchctl: "Launchctl") -> dict | None:
    try:
        state, pid = launchctl.inspect()
        if state not in {"running", "active"} or type(pid) is not int or pid <= 1:
            return None
        identity = fetch_identity(manifest.health_url, insecure_tls=manifest.health_insecure_tls)
        if launchctl.inspect() != (state, pid):
            return None
        plist = Path(manifest.target_plist)
        binary = Path(manifest.target_binary)
        if (plist.is_symlink() or not plist.is_file() or binary.is_symlink() or not binary.is_file()):
            return None
        with plist.open("rb") as stream:
            config = plistlib.load(stream)
        if config.get("Label") != manifest.label or config.get("ProgramArguments") != [str(binary)]:
            return None
        embedded = subprocess.run([str(binary), "--build-identity"], capture_output=True, text=True, timeout=10)
        if embedded.returncode != 0 or json.loads(embedded.stdout) != dataclasses.asdict(identity):
            return None
        if launchctl.inspect() != (state, pid):
            return None
        return {"pid": pid, "identity": dataclasses.asdict(identity),
                "binary_sha256": sha256(binary), "plist_sha256": sha256(plist)}
    except Exception:
        return None


def predecessor_unchanged(manifest: Manifest, captured: dict) -> bool:
    try:
        return (sha256(Path(manifest.target_binary)) == captured["binary_sha256"]
                and sha256(Path(manifest.candidate_plist).parent / "previous.plist") == captured["plist_sha256"])
    except (OSError, KeyError, TypeError):
        return False


def stop_captured_predecessor(launchctl: "Launchctl", observed: tuple[str, int | None]) -> bool:
    state, pid = launchctl.inspect()
    if (state, pid) != observed:
        raise ActivationError(f"service changed before stop: captured={observed}, observed={(state, pid)}")
    if state == "not_loaded":
        return False
    result = launchctl.run(["launchctl", "bootout", launchctl.target], capture_output=True, text=True, timeout=10)
    if result.returncode != 0:
        raise ActivationError(f"launchctl bootout failed with exit {result.returncode}")
    launchctl.wait(lambda state, current: state == "not_loaded" and current is None,
                   time.monotonic() + launchctl.manifest.transition_timeout_secs, "teardown")
    return pid is not None and state in {"running", "active"}


def _confirm_pid_gone(pid: int | None) -> bool:
    if pid is None:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    except PermissionError:
        return False
    return False


def restore_predecessor(manifest: Manifest, launchctl: "Launchctl", captured: dict, previous_plist: Path) -> int:
    state, pid = launchctl.inspect()
    if state != "not_loaded" or pid is not None:
        raise ActivationError(f"predecessor recovery uncertain: target is {state} pid={pid}")
    if not predecessor_unchanged(manifest, captured):
        raise ActivationError("predecessor recovery uncertain: captured binary or plist identity changed")
    identity = Identity(**captured["identity"])
    recovered_pid = None
    try:
        atomic_write(Path(manifest.target_plist), previous_plist.read_bytes(), 0o600)
        recovered_pid = launchctl.start(captured["pid"], plist_path=manifest.target_plist)
        wait_for_identity(manifest, identity)
        observed_state, observed_pid = launchctl.inspect()
        if observed_state not in {"running", "active"} or observed_pid != recovered_pid:
            raise ActivationError("recovered PID changed during verification")
        write_proof(manifest, recovery_outcome="predecessor_recovered_verified",
                    predecessor_verified=True, recovered_pid=recovered_pid)
        previous_plist.unlink()
        fsync_dir(previous_plist.parent)
        return recovered_pid
    except Exception as exc:
        teardown_error = None
        try:
            state, pid = launchctl.inspect()
            if state != "not_loaded":
                if recovered_pid is None or pid != recovered_pid or pid == captured["pid"]:
                    raise ActivationError(f"recovery target changed before teardown: {state} pid={pid}")
                launchctl.stop(expected_pid=recovered_pid)
            stopped_state, stopped_pid = launchctl.inspect()
            if stopped_state != "not_loaded" or stopped_pid is not None:
                raise ActivationError(f"recovery process remains loaded as {stopped_state} pid={stopped_pid}")
        except Exception as stop_exc:
            teardown_error = str(stop_exc)
        quarantine_error = None
        try:
            canonical = Path(manifest.target_plist)
            if os.path.lexists(canonical):
                if not previous_plist.exists():
                    os.replace(canonical, previous_plist)
                    fsync_dir(canonical.parent)
                    fsync_dir(previous_plist.parent)
                else:
                    canonical.unlink()
                    fsync_dir(canonical.parent)
        except Exception as quarantine_exc:
            quarantine_error = str(quarantine_exc)
        details = "; ".join(filter(None, (f"predecessor recovery uncertain: {exc}",
                                        f"teardown uncertain: {teardown_error}" if teardown_error else None,
                                        f"requarantine uncertain: {quarantine_error}" if quarantine_error else None)))
        raise ActivationError(details) from exc


def _backup_proof(path: Path, deadline: float, diagnostics: dict | None = None) -> dict:
    digest = hashlib.sha256()
    with _diagnostic_stage(diagnostics, "proof_hash"):
        with path.open("rb") as stream:
            while True:
                if time.monotonic() >= deadline:
                    raise ActivationError("SQLite backup and verification deadline exceeded during proof hashing")
                block = stream.read(1024 * 1024)
                if not block:
                    break
                digest.update(block)
        if time.monotonic() >= deadline:
            raise ActivationError("SQLite backup and verification deadline exceeded during proof hashing")
    return {"backup_path": str(path), "backup_bytes": path.stat().st_size,
            "backup_sha256": digest.hexdigest()}


def _reserve_before_boundary(database_path: str, directory: Path) -> int:
    # Backup bytes are already durable and retained; only the residual bounded
    # operational reserve remains to be checked at the irreversible boundary.
    database_bytes = _database_size_bytes(database_path)
    available = shutil.disk_usage(directory).free
    reserve = 16 * 1024 * 1024 + min(database_bytes, 64 * 1024 * 1024)
    if available < reserve:
        raise ActivationError(
            f"insufficient residual deploy space before migration boundary: need {reserve} bytes, have {available}"
        )
    return database_bytes + reserve


def activate(manifest: Manifest) -> str:
    validate_manifest_identities(manifest)
    with Path(manifest.lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another host operation is running") from exc
        active = Path(manifest.active_path)
        if active.is_symlink() or not active.is_file() or active.read_text().strip() != manifest.transaction_id:
            raise ConcurrentDeploy("deployment claim does not belong to this transaction")
        status = json.loads(Path(manifest.status_path).read_text())
        if status.get("transaction_id") != manifest.transaction_id or status.get("state") != "prepared":
            raise ActivationError("activation requires a fresh prepared transaction; inspect retained diagnostics manually")
        prepared: list[Path] = []
        disrupted = False
        publication_attempted = False
        boundary_attempted = False
        stop_confirmed = False
        captured_stop = False
        previous_plist: Path | None = None
        old_pid: int | None = None
        predecessor: dict | None = None
        predecessor_recovered = False
        backup: Path | None = None
        phase = "preparing"
        launchctl = Launchctl(manifest)
        try:
            environment = validate_pre_disruption_handoff(manifest)
            database_path = environment["PHOENIX_DB_PATH"]
            backup_directory = Path(manifest.candidate_binary).parent
            source_absent = not os.path.lexists(database_path)
            if source_absent:
                _first_install_capacity(database_path)
            else:
                _preflight_backup_capacity(database_path, backup_directory)
            predecessor = capture_predecessor(manifest, launchctl)
            observed_service = launchctl.inspect()
            if source_absent and (predecessor is not None or observed_service != ("not_loaded", None)
                                  or os.path.lexists(manifest.target_binary)
                                  or os.path.lexists(manifest.target_plist)):
                raise ActivationError("missing production database with an existing predecessor or installation")
            if predecessor is not None and (observed_service[0] not in {"running", "active"}
                                            or observed_service[1] != predecessor["pid"]):
                raise ActivationError("captured predecessor changed before stop")
            write_proof(manifest, phase="preparing", predecessor=predecessor,
                        recovery_eligible=predecessor is not None, boundary="not_crossed",
                        database_source="absent" if source_absent else "existing")
            previous_plist = Path(manifest.candidate_plist).parent / "previous.plist"
            if os.path.lexists(previous_plist):
                raise ActivationError("previous plist already exists; activation cannot be replayed")
            staged_binary = prepare_atomic_install(Path(manifest.candidate_binary), Path(manifest.target_binary), 0o755)
            prepared.append(staged_binary)
            staged_plist = prepare_plist_publication(Path(manifest.candidate_plist), Path(manifest.target_plist))
            prepared.append(staged_plist)
            phase = "stopping"
            write_status(manifest, phase)
            disrupted = True
            canonical = Path(manifest.target_plist)
            if os.path.lexists(canonical):
                os.replace(canonical, previous_plist)
                fsync_dir(canonical.parent)
                fsync_dir(previous_plist.parent)
            captured_stop = stop_captured_predecessor(launchctl, observed_service)
            stop_confirmed = True
            old_pid = predecessor["pid"] if predecessor else None
            if predecessor is not None:
                captured_stop = (captured_stop and _confirm_pid_gone(predecessor["pid"])
                                 and predecessor_unchanged(manifest, predecessor))
            write_proof(manifest, phase="stopped", stop_confirmed=True,
                        captured_pid=predecessor["pid"] if predecessor else None,
                        captured_pid_gone=(captured_stop if predecessor else False))
            if source_absent:
                _first_install_capacity(database_path)
                backup_proof = None
                write_proof(manifest, phase="source_absent", backup=None)
                phase = "migration_started"
                _first_install_capacity(database_path)
                boundary_attempted = True
                write_proof(manifest, phase=phase, boundary="attempted", backup=None)
                write_status(manifest, phase)
            else:
                _preflight_backup_capacity(database_path, backup_directory)
                with sqlite_writer_guard(database_path):
                    backup_deadline = time.monotonic() + 600
                    verifier_diagnostics = {}
                    try:
                        backup = create_verified_database_backup(
                            database_path, backup_directory, timeout=backup_deadline - time.monotonic(),
                            diagnostics=verifier_diagnostics,
                        )
                        backup_proof = _backup_proof(backup, backup_deadline, verifier_diagnostics)
                    except BaseException as verifier_error:
                        _emit_verifier_diagnostics(verifier_diagnostics, type(verifier_error).__name__)
                        raise
                    _emit_verifier_diagnostics(verifier_diagnostics, "verified")
                    write_proof(manifest, phase="backup_verified", backup=backup_proof)
                    print(f"{manifest.transaction_id}: verified SQLite backup retained at {backup}", flush=True)
                    phase = "migration_started"
                    _reserve_before_boundary(database_path, backup_directory)
                    boundary_attempted = True
                    write_proof(manifest, phase=phase, boundary="attempted", backup=backup_proof)
                    write_status(manifest, phase)
            commit_atomic_install(staged_binary, Path(manifest.target_binary))
            print(f"{manifest.transaction_id}: running candidate migrations (timeout {MIGRATION_TIMEOUT_SECS}s)", flush=True)
            migration = subprocess.run([manifest.candidate_binary, "--migrate-only"],
                                       env={**os.environ, **environment}, stdin=subprocess.DEVNULL,
                                       timeout=MIGRATION_TIMEOUT_SECS, check=False)
            if migration.returncode != 0:
                raise ActivationError(f"candidate migration exited {migration.returncode}; inspect activation.log")
            phase = "starting"
            write_status(manifest, phase)
            candidate_pid = launchctl.start(old_pid, plist_path=manifest.candidate_plist)
            phase = "verifying"
            write_status(manifest, phase)
            wait_for_identity(manifest)
            state, observed_pid = launchctl.inspect()
            if state not in {"running", "active"} or observed_pid != candidate_pid:
                raise ActivationError("candidate PID changed during identity verification")
            phase = "publishing"
            write_status(manifest, phase)
            publication_attempted = True
            commit_atomic_install(staged_plist, canonical)
            atomic_write(Path(manifest.deployed_sha_path), (manifest.source_commit + "\n").encode())
            write_proof(manifest, phase="candidate_verified", boundary="crossed", outcome="candidate_verified",
                        candidate_pid=candidate_pid, backup=backup_proof)
            write_status(manifest, "committed")
        except Exception as exc:
            stop_failure = None
            if disrupted and boundary_attempted:
                try:
                    launchctl.stop()
                except Exception as stop_exc:
                    stop_failure = str(stop_exc)
                if publication_attempted and os.path.lexists(manifest.target_plist):
                    try:
                        Path(manifest.target_plist).unlink()
                        fsync_dir(Path(manifest.target_plist).parent)
                    except OSError as unlink_exc:
                        stop_failure = "; ".join(filter(None, (stop_failure, f"candidate plist removal failed: {unlink_exc}")))
            elif disrupted and not boundary_attempted and not source_absent:
                try:
                    if (not stop_confirmed or not captured_stop or predecessor is None
                            or previous_plist is None or not previous_plist.is_file()):
                        raise ActivationError("predecessor recovery is uncertain: captured identity/stop proof unavailable")
                    restore_predecessor(manifest, launchctl, predecessor, previous_plist)
                    predecessor_recovered = True
                except Exception as recovery_exc:
                    stop_failure = "; ".join(filter(None, (
                        stop_failure, f"predecessor recovery uncertainty: {recovery_exc}"
                    )))
                    try:
                        write_proof(manifest, recovery_outcome="uncertain", recovery_uncertainty=str(recovery_exc),
                                    quarantine_path=str(previous_plist) if previous_plist else None)
                    except OSError as proof_exc:
                        stop_failure += f"; recovery proof persistence failed: {proof_exc}"
            state = "failed" if disrupted else "precondition_failed"
            failure = f"{phase}: {exc}"
            if predecessor_recovered:
                failure += "; unchanged predecessor restored and exact identity verified"
            if stop_failure:
                failure += f"; stopping failed: {stop_failure}"
            try:
                write_proof(manifest, phase=phase, boundary="attempted" if boundary_attempted else "not_crossed",
                            outcome="failed", failure=failure)
            except OSError as proof_exc:
                failure += f"; failure proof persistence failed: {proof_exc}"
            status_durable = False
            try:
                write_status(manifest, state, failure=failure)
                status_durable = True
            except OSError as status_exc:
                failure += f"; terminal status persistence failed: {status_exc}"
            if status_durable and (not disrupted or predecessor_recovered):
                try:
                    release_claim(manifest)
                except OSError as release_exc:
                    failure += f"; claim release failed: {release_exc}"
            try:
                print(f"{manifest.transaction_id}: {failure}", file=sys.stderr, flush=True)
            except (OSError, ValueError):
                pass
            if disrupted and not predecessor_recovered:
                try:
                    if stop_failure and "predecessor recovery uncertainty" in stop_failure:
                        print(f"Predecessor recovery is uncertain; quarantined plist retained at {previous_plist}. "
                              "Do not publish or start it until the uncertainty is resolved.", file=sys.stderr, flush=True)
                    else:
                        print("Production requires manual recovery. The candidate may have changed the database; "
                              "do not start an older binary without restoring its matching database. "
                              "Inspect activation.log and the retained transaction.", file=sys.stderr, flush=True)
                except (OSError, ValueError):
                    pass
            return state
        finally:
            for path in prepared:
                try:
                    path.unlink(missing_ok=True)
                except OSError as cleanup_exc:
                    try:
                        print(f"{manifest.transaction_id}: staged artifact cleanup failed: {cleanup_exc}",
                              file=sys.stderr, flush=True)
                    except (OSError, ValueError):
                        pass
        claim_released = False
        try:
            claim_released = release_claim(manifest)
        except Exception as exc:
            warn_committed(f"Committed deployment could not release its claim: {exc}; inspect ownership manually")
        else:
            if not claim_released:
                warn_committed("Committed deployment claim no longer matches; inspect ownership manually")
        if claim_released:
            candidate = Path(manifest.candidate_binary)
            for path in (candidate, candidate.parent / "previous-phoenix-ide"):
                try:
                    path.unlink(missing_ok=True)
                except OSError as exc:
                    warn_committed(f"Committed deployment left temporary binary {path}: {exc}")
        try:
            print(f"{manifest.transaction_id}: committed", flush=True)
        except (OSError, ValueError):
            pass
        return "committed"


def request_helper_bootout(uid: int, helper_label: str) -> None:
    subprocess.Popen(["launchctl", "bootout", f"gui/{uid}/{helper_label}"],
                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     start_new_session=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", default="activate", choices=["activate"])
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--helper-label")
    parser.add_argument("--uid", type=int)
    args = parser.parse_args()
    if args.protocol_version:
        print(HANDOFF_PROTOCOL_VERSION)
        return 0
    if args.manifest is None or args.helper_label is None or args.uid is None:
        parser.error("activation requires --manifest, --helper-label and --uid")
    verified = False
    try:
        manifest = Manifest.load(args.manifest)
        if manifest.helper_label != args.helper_label or manifest.uid != args.uid:
            raise ActivationError("helper invocation does not match the immutable manifest")
        verified = True
        return 0 if activate(manifest) == "committed" else 1
    except ConcurrentDeploy as exc:
        verified = False
        print(f"activation helper refused: {exc}", file=sys.stderr, flush=True)
        return 1
    except Exception as exc:
        print(f"activation helper refused/failed: {exc}", file=sys.stderr, flush=True)
        return 1
    finally:
        if verified:
            request_helper_bootout(args.uid, args.helper_label)


if __name__ == "__main__":
    raise SystemExit(main())
