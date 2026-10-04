#!/usr/bin/env python3
"""Transactional launchd activation helper.

This file is copied into each staged transaction and bootstrapped as its own
one-shot LaunchAgent. It intentionally uses only the Python standard library
and paths captured in the immutable manifest.
"""
from __future__ import annotations

import argparse
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
import urllib.request
import urllib.parse
import sqlite3
import stat
from pathlib import Path
from typing import Callable, Optional
from contextlib import closing

HANDOFF_PROTOCOL_VERSION = 1
FULL_GIT_SHA_RE = re.compile(r"[0-9a-f]{40}")
LEGACY_GIT_SHA_RE = re.compile(r"[0-9a-f]{12}")
VERSION_RE = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}")

TERMINAL_STATES = {
    "committed",
    "precondition_failed",
    "activation_failed_rolled_back",
    "activation_failed_rollback_failed",
    "rejected_concurrent",
}


@dataclasses.dataclass(frozen=True)
class Identity:
    version: str
    git_sha: str


@dataclasses.dataclass(frozen=True)
class PairedDatabaseUpgrade:
    """The sole structural representation of the feature-scoped DB snapshot."""
    database_path: str
    backup_path: str
    proof_path: str
    controller_source_commit: str
    controller_helper_sha256: str
    controller_helper_path: str


@dataclasses.dataclass(frozen=True)
class PairedDatabaseUpgrade:
    """The sole structural representation of the feature-scoped DB snapshot."""
    database_path: str
    backup_path: str
    proof_path: str
    controller_source_commit: str
    controller_helper_sha256: str
    controller_helper_path: str


@dataclasses.dataclass(frozen=True)
class DatabaseCapacityReservation:
    backup_path: Path
    restore_path: Path
    capacity_bytes: int


@dataclasses.dataclass(frozen=True)
class Manifest:
    manifest_version: int
    transaction_id: str
    source_kind: str
    source_commit: str
    release_tag: Optional[str]
    release_commit: Optional[str]
    expected: Identity
    previous: Optional[Identity]
    previous_deployed_sha: Optional[str]
    candidate_binary: str
    candidate_binary_sha256: str
    candidate_plist: str
    candidate_plist_sha256: str
    rollback_binary: Optional[str]
    rollback_binary_sha256: Optional[str]
    rollback_plist: Optional[str]
    rollback_plist_sha256: Optional[str]
    target_binary: str
    target_plist: str
    label: str
    helper_label: str
    uid: int
    health_url: str
    health_insecure_tls: bool
    previous_health_url: Optional[str]
    previous_health_insecure_tls: Optional[bool]
    previous_health_json: Optional[bool]
    active_path: str
    status_path: str
    deployed_sha_path: str
    lock_path: str
    claim_lock_path: str
    created_at: str
    transition_timeout_secs: float = 30.0
    health_timeout_secs: float = 120.0
    # None is intentional for pre-feature runtime-only manifests.
    paired_database_upgrade: Optional[PairedDatabaseUpgrade] = None

    @classmethod
    def load(cls, path: Path) -> "Manifest":
        raw = json.loads(path.read_text())
        if raw.get("manifest_version") != HANDOFF_PROTOCOL_VERSION:
            raise ActivationError(
                f"unsupported handoff protocol {raw.get('manifest_version')!r}; expected {HANDOFF_PROTOCOL_VERSION}"
            )
        raw["expected"] = Identity(**raw["expected"])
        raw["previous"] = Identity(**raw["previous"]) if raw.get("previous") else None
        paired = raw.get("paired_database_upgrade")
        raw["paired_database_upgrade"] = PairedDatabaseUpgrade(**paired) if paired else None
        # Runtime-only manifests written before the paired feature omit the field.
        raw.pop("database_mode", None)
        for legacy_key in ("database_path", "database_backup_path", "database_backup_sha256", "database_backup_verified", "database_proof_path", "controller_source_commit", "controller_helper_sha256", "controller_helper_path"):
            raw.pop(legacy_key, None)
        return cls(**raw)


class ActivationError(RuntimeError):
    pass


class ConcurrentDeploy(ActivationError):
    pass


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fsync_dir(path: Path) -> None:
    """Flush directory metadata after an atomic rename."""
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_write(path: Path, data: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        fsync_dir(path.parent)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def atomic_install(staged: Path, target: Path, mode: int) -> None:
    """Install without an unlink gap; staged and target must share a filesystem."""
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.parent / f".{target.name}.install-{os.getpid()}"
    shutil.copy2(staged, temporary)
    temporary.chmod(mode)
    with temporary.open("rb") as stream:
        os.fsync(stream.fileno())
    os.replace(temporary, target)
    fsync_dir(target.parent)


def prepare_atomic_install(staged: Path, target: Path, mode: int) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{target.name}.install-", dir=target.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as destination:
            fd = -1
            with staged.open("rb") as source:
                shutil.copyfileobj(source, destination)
            destination.flush()
            os.fsync(destination.fileno())
        return Path(temporary)
    except BaseException:
        if fd >= 0:
            os.close(fd)
        Path(temporary).unlink(missing_ok=True)
        raise


def commit_atomic_install(prepared: Path, target: Path) -> None:
    os.replace(prepared, target)
    fsync_dir(target.parent)


def write_status(manifest: Manifest, state: str, *, failure: Optional[str] = None, rollback_failure: Optional[str] = None) -> None:
    status = {
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
        "rollback_failure": rollback_failure,
    }
    atomic_write(Path(manifest.status_path), (json.dumps(status, sort_keys=True, indent=2) + "\n").encode())


def verify_staged(path: Optional[str], expected_hash: Optional[str], description: str) -> Path:
    if not path or not expected_hash:
        raise ActivationError(f"missing {description}")
    candidate = Path(path)
    try:
        stat = candidate.lstat()
    except OSError as exc:
        raise ActivationError(f"{description} is unavailable") from exc
    if not candidate.is_file() or not __import__("stat").S_ISREG(stat.st_mode) or candidate.is_symlink():
        raise ActivationError(f"{description} is not a regular non-symlink file")
    actual = sha256(candidate)
    if actual != expected_hash:
        raise ActivationError(f"{description} checksum mismatch")
    return candidate


def database_paths(manifest: Manifest) -> tuple[Path, ...]:
    if manifest.paired_database_upgrade is None:
        raise ActivationError("paired database upgrade is not configured")
    database = Path(manifest.paired_database_upgrade.database_path)
    return tuple(database.parent / name for name in (database.name, database.name + "-wal", database.name + "-shm"))


def _regular_nosymlink(path: Path, description: str, *, required: bool = True) -> None:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        if required:
            raise ActivationError(f"{description} is unavailable")
        return
    except OSError as exc:
        raise ActivationError(f"{description} is unavailable") from exc
    if not stat.S_ISREG(mode) or path.is_symlink():
        raise ActivationError(f"{description} is not a regular non-symlink file")


def _private_transaction_dir(path: Path) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        path.mkdir(parents=True, mode=0o700)
        mode = path.lstat().st_mode
    except OSError as exc:
        raise ActivationError("paired transaction directory is unavailable") from exc
    if not stat.S_ISDIR(mode) or path.is_symlink() or (mode & 0o777) != 0o700:
        raise ActivationError("paired transaction directory must be a private 0700 directory")
    return path


def _paired_path_context(manifest: Manifest) -> tuple[Path, Path, Path, Path]:
    paired = manifest.paired_database_upgrade
    if paired is None:
        raise ActivationError("paired database upgrade is not configured")
    database = Path(paired.database_path)
    backup = Path(paired.backup_path)
    proof = Path(paired.proof_path)
    for path, description in ((database, "paired database"), (backup, "database backup"), (proof, "database proof")):
        if not path.is_absolute() or path.is_symlink():
            raise ActivationError(f"{description} path must be absolute and non-symlink")
    if backup.parent != proof.parent:
        raise ActivationError("database backup and proof must share one transaction directory")
    resolved = {path.resolve(strict=False) for path in (database, backup, proof)}
    if len(resolved) != 3:
        raise ActivationError("database backup and proof must not alias the database or each other")
    return database, backup, proof, _private_transaction_dir(backup.parent)


def _restore_capacity_path(manifest: Manifest, database: Optional[Path] = None) -> Path:
    if database is None:
        database, _backup, _proof, _transaction = _paired_path_context(manifest)
    transaction_id = manifest.transaction_id
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", transaction_id):
        raise ActivationError("paired transaction id cannot name a restore reservation")
    path = database.parent / f".{database.name}.restore-{transaction_id}"
    if path.is_symlink() or path.resolve(strict=False) == database.resolve(strict=False):
        raise ActivationError("restore reservation path is unsafe")
    return path


def assert_database_exclusive(manifest: Manifest) -> None:
    """Prove that no process still owns the database or SQLite sidecars."""
    database, _backup, _proof, _transaction = _paired_path_context(manifest)
    if not database.exists():
        raise ActivationError("paired database does not exist")
    _regular_nosymlink(database, "paired database")
    paths = []
    for path in database_paths(manifest):
        if path.exists() or path.is_symlink():
            _regular_nosymlink(path, "SQLite database sidecar")
            paths.append(path)
    try:
        result = subprocess.run(
            ["lsof", "-nP", "-t", "--", *map(str, paths)],
            capture_output=True, text=True, timeout=10, check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise ActivationError("could not prove database exclusivity") from exc
    if result.returncode not in (0, 1) or result.stderr.strip():
        raise ActivationError("could not prove database exclusivity")
    owners = {line.strip() for line in (result.stdout or "").splitlines() if line.strip()}
    if owners:
        raise ActivationError("database remains open by another process")


def _readonly_connection(path: Path) -> sqlite3.Connection:
    if not path.is_absolute() or path.is_symlink():
        raise ActivationError("SQLite path must be an absolute non-symlink file")
    uri = "file:" + urllib.parse.quote(str(path), safe="/") + "?mode=ro"
    try:
        return sqlite3.connect(uri, uri=True, timeout=2)
    except sqlite3.Error as exc:
        raise ActivationError("could not open SQLite database read-only") from exc


def validate_database(path: Path) -> None:
    from contextlib import closing
    try:
        with closing(_readonly_connection(path)) as connection:
            result = connection.execute("PRAGMA integrity_check").fetchone()
    except sqlite3.Error as exc:
        raise ActivationError(f"SQLite integrity check failed: {exc}") from exc
    if result != ("ok",):
        raise ActivationError(f"SQLite integrity check returned {result!r}")


def validate_legacy_database(path: Path) -> None:
    """Read-only gate for the one supported 69 -> ProductConversation upgrade."""
    from contextlib import closing
    try:
        with closing(_readonly_connection(path)) as connection:
            ledger = connection.execute(
                "SELECT COALESCE(MAX(version), -1) FROM _migrations"
            ).fetchone()
            if ledger is None or ledger[0] < 0 or ledger[0] > 69:
                raise ActivationError("paired upgrade requires an existing migration ledger at version <= 69")
            product_tables = connection.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name IN ('product_conversations', 'product_conversation_members') LIMIT 1"
            ).fetchone()
            if product_tables is not None:
                raise ActivationError("paired upgrade requires a legacy database without ProductConversation tables")
    except sqlite3.Error as exc:
        raise ActivationError("paired legacy database preflight failed") from exc


def _plist_database_path(path: Path) -> Optional[str]:
    try:
        with path.open("rb") as stream:
            plist = plistlib.load(stream)
        value = plist.get("EnvironmentVariables", {}).get("PHOENIX_DB_PATH")
        return str(value) if value is not None else None
    except (OSError, plistlib.InvalidFileException, ValueError) as exc:
        raise ActivationError("launchd plist is unreadable") from exc


def _source_capacity_bytes(database: Path) -> int:
    """Reserve the live DB, WAL, and a page-sized margin before stopping it."""
    try:
        database_size = database.stat().st_size
        wal_size = database.with_name(database.name + "-wal").stat().st_size if database.with_name(database.name + "-wal").exists() else 0
        with closing(_readonly_connection(database)) as connection:
            page_size = int(connection.execute("PRAGMA page_size").fetchone()[0])
    except (OSError, sqlite3.Error, TypeError, ValueError) as exc:
        raise ActivationError("could not measure paired database capacity") from exc
    margin = max(page_size, 4096) * 2
    return database_size + wal_size + margin


def _allocate_private_sqlite(path: Path, size: int) -> None:
    if path.exists() or path.is_symlink():
        raise ActivationError("database capacity reservation already exists")
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(fd)
        with closing(sqlite3.connect(path)) as connection:
            connection.execute("PRAGMA journal_mode=DELETE")
            connection.execute("CREATE TABLE capacity_seed (value INTEGER)")
            connection.commit()
        with path.open("r+b") as stream:
            stream.seek(0, os.SEEK_END)
            remaining = max(0, size - stream.tell())
            zeros = b"\0" * (1024 * 1024)
            while remaining:
                written = stream.write(zeros[: min(remaining, len(zeros))])
                if written <= 0:
                    raise OSError("capacity reservation made no progress")
                remaining -= written
            stream.flush()
            os.fsync(stream.fileno())
        if path.stat().st_blocks * 512 < size:
            raise OSError("filesystem did not allocate reserved capacity")
    except (OSError, sqlite3.Error) as exc:
        path.unlink(missing_ok=True)
        raise ActivationError("could not reserve paired database capacity") from exc


def reserve_database_capacity(manifest: Manifest) -> DatabaseCapacityReservation:
    """Hold backup and restore space while the service is stopped."""
    if manifest.paired_database_upgrade is None:
        raise ActivationError("paired database upgrade is not configured")
    database, backup, proof, transaction_dir = _paired_path_context(manifest)
    _regular_nosymlink(database, "paired database")
    _private_transaction_dir(transaction_dir)
    restore = _restore_capacity_path(manifest, database)
    for path, description in ((restore, "restore reservation"),):
        if not path.is_absolute() or path.is_symlink():
            raise ActivationError(f"{description} path must be absolute and non-symlink")
    capacity = _source_capacity_bytes(database)
    # Never replace an existing protected snapshot: recovery must retain it.
    if backup.exists() or backup.is_symlink() or proof.exists() or proof.is_symlink():
        raise ActivationError("paired recovery snapshot is already present")
    _allocate_private_sqlite(backup, capacity)
    try:
        _allocate_private_sqlite(restore, capacity)
    except BaseException:
        backup.unlink(missing_ok=True)
        raise
    return DatabaseCapacityReservation(backup, restore, capacity)


def _reservation_still_sufficient(manifest: Manifest, reservation: DatabaseCapacityReservation) -> None:
    database, _backup, _proof, _transaction = _paired_path_context(manifest)
    required = _source_capacity_bytes(database)
    for path in (reservation.backup_path, reservation.restore_path):
        if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o777 != 0o600:
            raise ActivationError("paired database capacity reservation is unavailable")
        if path.stat().st_size < required or path.stat().st_blocks * 512 < required:
            raise ActivationError("paired database grew beyond its reserved capacity")


def create_database_backup(manifest: Manifest, reservation: Optional[DatabaseCapacityReservation] = None) -> None:
    """Take a SQLite backup API snapshot into the held destination."""
    if manifest.paired_database_upgrade is None:
        return
    assert_database_exclusive(manifest)
    source, backup, proof_path, transaction_dir = _paired_path_context(manifest)
    _regular_nosymlink(source, "paired database")
    _private_transaction_dir(transaction_dir)
    if reservation is None:
        reservation = reserve_database_capacity(manifest)
    if reservation.backup_path != backup:
        raise ActivationError("database backup reservation does not match manifest")
    try:
        source_uri = "file:" + urllib.parse.quote(str(source), safe="/") + "?mode=ro"
        with closing(sqlite3.connect(source_uri, uri=True, timeout=2)) as source_db, closing(sqlite3.connect(backup)) as backup_db:
            backup_db.execute("PRAGMA journal_mode=DELETE")
            source_db.backup(backup_db)
            backup_db.commit()
        for suffix in ("-wal", "-shm"):
            backup.with_name(backup.name + suffix).unlink(missing_ok=True)
        with backup.open("rb") as stream:
            os.fsync(stream.fileno())
        fsync_dir(transaction_dir)
        _regular_nosymlink(backup, "database backup")
        validate_database(backup)
        observed = sha256(backup)
        atomic_write(proof_path, (json.dumps({
            "transaction_id": manifest.transaction_id,
            "source_commit": manifest.source_commit,
            "database": str(source),
            "sha256": observed,
            "candidate_binary_sha256": manifest.candidate_binary_sha256,
            "candidate_plist_sha256": manifest.candidate_plist_sha256,
            "previous_binary_sha256": manifest.rollback_binary_sha256,
            "previous_plist_sha256": manifest.rollback_plist_sha256,
            "controller_source_commit": manifest.paired_database_upgrade.controller_source_commit,
            "controller_helper_sha256": manifest.paired_database_upgrade.controller_helper_sha256,
        }, sort_keys=True) + "\n").encode(), 0o600)
    except (sqlite3.Error, OSError, ValueError) as exc:
        raise ActivationError(f"database snapshot failed: {exc}") from exc


def restore_database(manifest: Manifest) -> None:
    if manifest.paired_database_upgrade is None:
        return
    assert_database_exclusive(manifest)
    database, backup_path, proof_path, transaction_dir = _paired_path_context(manifest)
    _private_transaction_dir(transaction_dir)
    _regular_nosymlink(proof_path, "database proof")
    if proof_path.stat().st_mode & 0o777 != 0o600:
        raise ActivationError("database proof must be private")
    try:
        proof = json.loads(proof_path.read_text())
        expected_hash = proof["sha256"]
        if not isinstance(expected_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_hash):
            raise ActivationError("paired database snapshot proof has an invalid checksum")
        expected = {
            "transaction_id": manifest.transaction_id,
            "source_commit": manifest.source_commit,
            "database": manifest.paired_database_upgrade.database_path,
            "candidate_binary_sha256": manifest.candidate_binary_sha256,
            "candidate_plist_sha256": manifest.candidate_plist_sha256,
            "previous_binary_sha256": manifest.rollback_binary_sha256,
            "previous_plist_sha256": manifest.rollback_plist_sha256,
            "controller_source_commit": manifest.paired_database_upgrade.controller_source_commit,
            "controller_helper_sha256": manifest.paired_database_upgrade.controller_helper_sha256,
        }
        if any(proof.get(key) != value for key, value in expected.items()):
            raise ActivationError("paired database snapshot proof context does not match manifest")
    except ActivationError:
        raise
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
        raise ActivationError("paired database snapshot proof is unavailable") from exc
    backup = verify_staged(str(backup_path), expected_hash, "database backup")
    if backup.stat().st_mode & 0o777 != 0o600:
        raise ActivationError("database backup must be private")
    validate_database(backup)
    _regular_nosymlink(database, "paired database")
    reserved = _restore_capacity_path(manifest, database)
    if not reserved.exists():
        _allocate_private_sqlite(reserved, backup.stat().st_size)
    _regular_nosymlink(reserved, "database restore capacity")
    if reserved.stat().st_mode & 0o777 != 0o600 or reserved.stat().st_blocks * 512 < backup.stat().st_size:
        raise ActivationError("database restore capacity is unverified")
    with backup.open("rb") as source, reserved.open("r+b") as target:
        shutil.copyfileobj(source, target)
        target.truncate(source.tell())
        target.flush()
        os.fsync(target.fileno())
    validate_database(reserved)
    if sha256(reserved) != proof["sha256"]:
        raise ActivationError("prepared database restore checksum mismatch")
    for sidecar in database_paths(manifest)[1:]:
        if sidecar.is_symlink():
            raise ActivationError("SQLite sidecar must not be a symlink")
        sidecar.unlink(missing_ok=True)
    os.replace(reserved, database)
    fsync_dir(database.parent)
    validate_database(database)


class Launchctl:
    def __init__(self, manifest: Manifest, run: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run):
        self.manifest = manifest
        self.run = run
        self.domain = f"gui/{manifest.uid}"
        self.target = f"{self.domain}/{manifest.label}"
        self.disruption_started = False

    def inspect(self) -> tuple[str, Optional[int]]:
        result = self.run(["launchctl", "print", self.target], capture_output=True, text=True)
        output = result.stdout + "\n" + result.stderr
        absent = re.search(r'^\s*Could not find service "' + re.escape(self.manifest.label) + r'" in domain (?:gui/' + str(self.manifest.uid) + r'|for user gui: ' + str(self.manifest.uid) + r')\s*$', output, re.MULTILINE)
        if absent is not None:
            return "not_loaded", None
        if result.returncode != 0:
            raise ActivationError(f"launchctl print failed with exit {result.returncode}; service absence is unconfirmed")
        state = "unknown"
        pid = None
        for raw in result.stdout.splitlines():
            line = raw.strip()
            if line.startswith("state = "):
                state = line.split(" = ", 1)[1]
            elif line.startswith("pid = "):
                try:
                    pid = int(line.split(" = ", 1)[1])
                except ValueError:
                    pass
        return state, pid

    def wait(self, predicate: Callable[[str, Optional[int]], bool], deadline: float, description: str) -> tuple[str, Optional[int]]:
        while True:
            observed = self.inspect()
            if predicate(*observed):
                return observed
            if time.monotonic() >= deadline:
                raise ActivationError(f"timed out waiting for launchd {description}; state={observed[0]} pid={observed[1]}")
            time.sleep(0.1)

    def stop(self) -> Optional[int]:
        _state, old_pid = self.inspect()
        if _state == "not_loaded":
            return old_pid
        result = self.run(["launchctl", "bootout", self.domain, self.manifest.target_plist], capture_output=True, text=True)
        if result.returncode != 0:
            raise ActivationError(f"launchctl bootout failed with exit {result.returncode}")
        self.disruption_started = True
        self.wait(lambda state, pid: state == "not_loaded" and pid is None, time.monotonic() + self.manifest.transition_timeout_secs, "teardown")
        return old_pid

    def start(self, old_pid: Optional[int]) -> int:
        result = self.run(["launchctl", "bootstrap", self.domain, self.manifest.target_plist], capture_output=True, text=True)
        if result.returncode != 0:
            raise ActivationError(f"launchctl bootstrap failed with exit {result.returncode}")
        _state, pid = self.wait(
            lambda state, pid: state in {"running", "active"} and pid is not None and pid != old_pid,
            time.monotonic() + self.manifest.transition_timeout_secs,
            "running with a new PID",
        )
        assert pid is not None
        return pid


def parse_legacy_version_body(body: str) -> str:
    value = body.strip()
    prefix = "phoenix-ide "
    if value.startswith(prefix):
        value = value[len(prefix):].strip()
    if not value:
        raise ActivationError("legacy health response has no version")
    return value


def fetch_identity(
    url: str,
    timeout: float = 2.0,
    insecure_tls: bool = False,
    expected_git_sha: Optional[str] = None,
) -> Identity:
    context = ssl._create_unverified_context() if insecure_tls else None
    with urllib.request.urlopen(url, timeout=timeout, context=context) as response:
        if expected_git_sha is not None:
            version = parse_legacy_version_body(response.read().decode())
            return Identity(version=version, git_sha=expected_git_sha)
        body = json.load(response)
    try:
        return Identity(version=str(body["version"]), git_sha=str(body["git_sha"]))
    except (KeyError, TypeError) as exc:
        raise ActivationError("health response has no version identity") from exc


def wait_for_identity(
    manifest: Manifest,
    expected: Identity,
    *,
    health_url: Optional[str] = None,
    health_insecure_tls: Optional[bool] = None,
    health_json: bool = True,
    monotonic: Optional[Callable[[], float]] = None,
    sleep: Optional[Callable[[float], None]] = None,
    fetch: Optional[Callable[..., Identity]] = None,
) -> None:
    monotonic = monotonic or time.monotonic
    sleep = sleep or time.sleep
    fetch = fetch or fetch_identity
    started_at = monotonic()
    deadline = started_at + manifest.health_timeout_secs
    url = health_url or manifest.health_url
    insecure_tls = manifest.health_insecure_tls if health_insecure_tls is None else health_insecure_tls
    last = "not responding"
    while monotonic() < deadline:
        try:
            actual = fetch(
                url,
                insecure_tls=insecure_tls,
                expected_git_sha=None if health_json else expected.git_sha,
            )
            last = f"version={actual.version} git_sha={actual.git_sha}"
            if actual == expected:
                return
        except Exception as exc:
            last = f"{type(exc).__name__}: {exc}"
        sleep(0.2)
    elapsed = monotonic() - started_at
    raise ActivationError(
        "exact health verification failed after "
        f"{elapsed:.1f}s (budget={manifest.health_timeout_secs:.1f}s, deadline={deadline:.3f} monotonic): "
        f"expected version={expected.version} git_sha={expected.git_sha}; observed {last}"
    )


def restore_deployed_sha(manifest: Manifest) -> None:
    deployed_sha = Path(manifest.deployed_sha_path)
    if manifest.previous_deployed_sha is None:
        deployed_sha.unlink(missing_ok=True)
        if deployed_sha.parent.exists():
            fsync_dir(deployed_sha.parent)
    else:
        atomic_write(deployed_sha, (manifest.previous_deployed_sha + "\n").encode(), 0o600)


def restore(
    manifest: Manifest,
    launchctl: Launchctl,
    prepared_rollback: Optional[tuple[Path, Path]] = None,
) -> None:
    # Candidate stop is always attempted before any paired database restoration.
    try:
        launchctl.stop()
        if manifest.paired_database_upgrade is not None:
            state, pid = launchctl.inspect()
            if state != "not_loaded" or pid is not None:
                raise ActivationError("candidate remains loaded after stop")
    except ActivationError:
        if manifest.paired_database_upgrade is not None:
            raise
    if manifest.paired_database_upgrade is not None:
        restore_database(manifest)
    if manifest.previous is None:
        if manifest.rollback_binary is not None or manifest.rollback_plist is not None:
            raise ActivationError("first-install rollback inputs are inconsistent")
        Path(manifest.target_binary).unlink(missing_ok=True)
        Path(manifest.target_plist).unlink(missing_ok=True)
        fsync_dir(Path(manifest.target_binary).parent)
        fsync_dir(Path(manifest.target_plist).parent)
        state, pid = launchctl.inspect()
        if state != "not_loaded" or pid is not None:
            raise ActivationError("failed first-install candidate remains loaded")
        restore_deployed_sha(manifest)
        return
    if prepared_rollback is None:
        rollback_binary = verify_staged(manifest.rollback_binary, manifest.rollback_binary_sha256, "rollback binary")
        rollback_plist = verify_staged(manifest.rollback_plist, manifest.rollback_plist_sha256, "rollback plist")
        atomic_install(rollback_binary, Path(manifest.target_binary), 0o755)
        atomic_install(rollback_plist, Path(manifest.target_plist), 0o600)
    else:
        prepared_binary, prepared_plist = prepared_rollback
        commit_atomic_install(prepared_binary, Path(manifest.target_binary))
        commit_atomic_install(prepared_plist, Path(manifest.target_plist))
    old_pid = launchctl.inspect()[1]
    launchctl.start(old_pid)
    if (
        manifest.previous_health_url is None
        or manifest.previous_health_insecure_tls is None
        or manifest.previous_health_json is None
    ):
        raise ActivationError("previous endpoint is unavailable")
    wait_for_identity(
        manifest,
        manifest.previous,
        health_url=manifest.previous_health_url,
        health_insecure_tls=manifest.previous_health_insecure_tls,
        health_json=manifest.previous_health_json,
    )
    restore_deployed_sha(manifest)


def release_claim(manifest: Manifest) -> bool:
    claim = Path(manifest.active_path)
    claim_lock = Path(manifest.claim_lock_path)
    claim_lock.parent.mkdir(parents=True, exist_ok=True)
    with claim_lock.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            if claim.read_text().strip() != manifest.transaction_id:
                return False
            claim.unlink()
            return True
        except FileNotFoundError:
            return False


def request_helper_bootout(uid: int, helper_label: str) -> None:
    subprocess.Popen(
        ["launchctl", "bootout", f"gui/{uid}/{helper_label}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
    )


def validate_manifest_mode(manifest: Manifest) -> None:
    paired = manifest.paired_database_upgrade
    if manifest.source_kind == "prepared_artifact" and paired is None:
        raise ActivationError("prepared artifact requires paired database mode")
    if paired is not None:
        if manifest.source_kind != "prepared_artifact":
            raise ActivationError("paired database mode requires a prepared artifact candidate")
        if not FULL_GIT_SHA_RE.fullmatch(paired.controller_source_commit):
            raise ActivationError("paired mode requires exact controller source binding")
        if not paired.controller_helper_sha256 or not paired.controller_helper_path:
            raise ActivationError("paired mode requires helper equivalence binding")
        for raw_path in (paired.database_path, paired.backup_path, paired.proof_path, paired.controller_helper_path):
            if not Path(raw_path).is_absolute() or Path(raw_path).is_symlink():
                raise ActivationError("paired mode paths must be absolute non-symlinks")
        helper = Path(paired.controller_helper_path)
        running_helper = Path(__file__).resolve()
        if helper.resolve() != running_helper:
            raise ActivationError("paired mode helper path is not the running helper")
        if not helper.is_file() or sha256(helper) != paired.controller_helper_sha256:
            raise ActivationError("controller helper bytes changed")
        _database, _backup, _proof, _transaction_dir = _paired_path_context(manifest)
        if _transaction_dir != helper.parent:
            raise ActivationError("paired snapshot must belong to the helper transaction")
        if manifest.previous is None or manifest.rollback_binary_sha256 is None or manifest.rollback_plist_sha256 is None:
            raise ActivationError("paired mode requires a predecessor")
        database = str(Path(paired.database_path))
        if _plist_database_path(Path(manifest.candidate_plist)) != database or _plist_database_path(Path(manifest.rollback_plist or "")) != database:
            raise ActivationError("paired database path differs between candidate, predecessor, and manifest")




def validate_manifest_identities(manifest: Manifest) -> None:
    if (
        not VERSION_RE.fullmatch(manifest.expected.version)
        or not FULL_GIT_SHA_RE.fullmatch(manifest.expected.git_sha)
        or manifest.expected.git_sha != manifest.source_commit
    ):
        raise ActivationError("candidate runtime identity must be the exact full source commit")
    if manifest.release_commit is not None and not FULL_GIT_SHA_RE.fullmatch(manifest.release_commit):
        raise ActivationError("release commit must be a full lowercase git SHA")
    if manifest.previous_deployed_sha is not None and not FULL_GIT_SHA_RE.fullmatch(manifest.previous_deployed_sha):
        raise ActivationError("previous deployed SHA must be a full lowercase git SHA")
    if manifest.previous is not None and (
        not VERSION_RE.fullmatch(manifest.previous.version)
        or not (
            LEGACY_GIT_SHA_RE.fullmatch(manifest.previous.git_sha)
            or FULL_GIT_SHA_RE.fullmatch(manifest.previous.git_sha)
        )
    ):
        raise ActivationError("previous runtime identity is malformed")


def activate(manifest: Manifest) -> str:
    lock_path = Path(manifest.lock_path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is already activating") from exc

        prepared_installs: list[Path] = []
        prepared_rollback: Optional[tuple[Path, Path]] = None
        capacity_reservation: Optional[DatabaseCapacityReservation] = None
        try:
            validate_manifest_identities(manifest)
            validate_manifest_mode(manifest)
            candidate_binary = verify_staged(manifest.candidate_binary, manifest.candidate_binary_sha256, "candidate binary")
            candidate_plist = verify_staged(manifest.candidate_plist, manifest.candidate_plist_sha256, "candidate plist")
            with candidate_plist.open("rb") as stream:
                plistlib.load(stream)
            prepared_candidate_binary = prepare_atomic_install(
                candidate_binary, Path(manifest.target_binary), 0o755
            )
            prepared_installs.append(prepared_candidate_binary)
            prepared_candidate_plist = prepare_atomic_install(
                candidate_plist, Path(manifest.target_plist), 0o600
            )
            prepared_installs.append(prepared_candidate_plist)
            prepared_candidate = (prepared_candidate_binary, prepared_candidate_plist)
            if manifest.previous is not None:
                rollback_binary = verify_staged(manifest.rollback_binary, manifest.rollback_binary_sha256, "rollback binary")
                rollback_plist = verify_staged(manifest.rollback_plist, manifest.rollback_plist_sha256, "rollback plist")
                with rollback_plist.open("rb") as stream:
                    plistlib.load(stream)
                prepared_rollback_binary = prepare_atomic_install(
                    rollback_binary, Path(manifest.target_binary), 0o755
                )
                prepared_installs.append(prepared_rollback_binary)
                prepared_rollback_plist = prepare_atomic_install(
                    rollback_plist, Path(manifest.target_plist), 0o600
                )
                prepared_installs.append(prepared_rollback_plist)
                prepared_rollback = (prepared_rollback_binary, prepared_rollback_plist)
            elif any((
                manifest.rollback_binary,
                manifest.rollback_binary_sha256,
                manifest.rollback_plist,
                manifest.rollback_plist_sha256,
            )):
                raise ActivationError("first-install rollback inputs are inconsistent")
            if manifest.paired_database_upgrade is not None:
                validate_legacy_database(Path(manifest.paired_database_upgrade.database_path))
                capacity_reservation = reserve_database_capacity(manifest)
        except Exception as exc:
            for prepared in prepared_installs:
                prepared.unlink(missing_ok=True)
            write_status(manifest, "precondition_failed", failure=str(exc))
            raise

        launchctl = Launchctl(manifest)
        write_status(manifest, "activating")
        disrupted = False
        try:
            if manifest.paired_database_upgrade is not None:
                # Production owns the database until launchd stops it; only the
                # read-only legacy shape gate is safe before disruption.
                validate_legacy_database(Path(manifest.paired_database_upgrade.database_path))
            old_pid = launchctl.stop()
            disrupted = True
            if manifest.paired_database_upgrade is not None:
                # Exclusivity is meaningful only after launchd confirms teardown.
                assert_database_exclusive(manifest)
                validate_legacy_database(Path(manifest.paired_database_upgrade.database_path))
                assert capacity_reservation is not None
                _reservation_still_sufficient(manifest, capacity_reservation)
                create_database_backup(manifest, capacity_reservation)
            commit_atomic_install(prepared_candidate[0], Path(manifest.target_binary))
            commit_atomic_install(prepared_candidate[1], Path(manifest.target_plist))
            launchctl.start(old_pid)
            wait_for_identity(manifest, manifest.expected)
            atomic_write(Path(manifest.deployed_sha_path), (manifest.source_commit + "\n").encode(), 0o600)
            write_status(manifest, "committed")
            return "committed"
        except Exception as activation_exc:
            failure = str(activation_exc)
            disrupted = disrupted or launchctl.disruption_started
            if not disrupted:
                write_status(manifest, "precondition_failed", failure=failure)
                raise
            try:
                restore(manifest, launchctl, prepared_rollback)
                write_status(manifest, "activation_failed_rolled_back", failure=failure)
                return "activation_failed_rolled_back"
            except Exception as rollback_exc:
                rollback_failure = str(rollback_exc)
                if manifest.paired_database_upgrade is not None:
                    try:
                        launchctl.stop()
                        state, pid = launchctl.inspect()
                        if state != "not_loaded" or pid is not None:
                            raise ActivationError("failed paired recovery teardown is unconfirmed")
                    except Exception as teardown_exc:
                        rollback_failure += f"; recovery teardown failed: {teardown_exc}"
                write_status(manifest, "activation_failed_rollback_failed", failure=failure, rollback_failure=rollback_failure)
                return "activation_failed_rollback_failed"
        finally:
            for prepared in prepared_installs:
                prepared.unlink(missing_ok=True)


def recover_paired(manifest: Manifest) -> str:
    with Path(manifest.lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment operation owns recovery") from exc
        validate_manifest_identities(manifest)
        validate_manifest_mode(manifest)
        if manifest.paired_database_upgrade is None:
            raise ActivationError("paired recovery requires a paired transaction")
        claim = Path(manifest.active_path)
        status = json.loads(Path(manifest.status_path).read_text())
        if claim.read_text().strip() != manifest.transaction_id or status.get("transaction_id") != manifest.transaction_id or status.get("state") != "activation_failed_rollback_failed":
            raise ActivationError("paired recovery must own the retained failed transaction")
        launchctl = Launchctl(manifest)
        try:
            restore(manifest, launchctl, None)
            write_status(manifest, "activation_failed_rolled_back", failure=status.get("failure"))
            return "activation_failed_rolled_back"
        except Exception as exc:
            failure = str(exc)
            try:
                launchctl.stop()
                state, pid = launchctl.inspect()
                if state != "not_loaded" or pid is not None:
                    raise ActivationError("paired recovery teardown is unconfirmed")
            except Exception as teardown:
                failure += f"; recovery teardown failed: {teardown}"
            write_status(manifest, "activation_failed_rollback_failed", failure=status.get("failure"), rollback_failure=failure)
            return "activation_failed_rollback_failed"


def status_is_durable_terminal(manifest: Manifest) -> bool:
    try:
        status = json.loads(Path(manifest.status_path).read_text())
        return (
            status.get("transaction_id") == manifest.transaction_id
            and status.get("state") in TERMINAL_STATES
            and not (
                manifest.paired_database_upgrade is not None
                and status.get("state") == "activation_failed_rollback_failed"
            )
        )
    except (OSError, json.JSONDecodeError):
        return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", nargs="?", choices=["activate", "recover-paired"])
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--helper-label")
    parser.add_argument("--uid", type=int)
    args = parser.parse_args()
    if args.protocol_version:
        print(HANDOFF_PROTOCOL_VERSION)
        return 0
    if args.manifest is None or args.helper_label is None or args.uid is None:
        parser.error("activation requires --manifest, --helper-label, and --uid")
    manifest = None
    try:
        manifest = Manifest.load(args.manifest)
        if manifest.helper_label != args.helper_label or manifest.uid != args.uid:
            raise ActivationError("helper identity does not match the immutable manifest")
        state = recover_paired(manifest) if args.command == "recover-paired" else activate(manifest)
        if state in TERMINAL_STATES and status_is_durable_terminal(manifest):
            release_claim(manifest)
        print(state, flush=True)
        return 0 if state == "committed" or (args.command == "recover-paired" and state == "activation_failed_rolled_back") else 1
    except ConcurrentDeploy as exc:
        if manifest is not None:
            try:
                state = "activation_failed_rollback_failed" if args.command == "recover-paired" else "rejected_concurrent"
                write_status(manifest, state, failure=str(exc))
            finally:
                if status_is_durable_terminal(manifest):
                    release_claim(manifest)
        print(f"activation helper failed: {exc}", file=sys.stderr)
        return 1
    except Exception as exc:
        if manifest is not None and status_is_durable_terminal(manifest):
            release_claim(manifest)
        print(f"activation helper failed: {exc}", file=sys.stderr)
        return 1
    finally:
        request_helper_bootout(args.uid, args.helper_label)


if __name__ == "__main__":
    raise SystemExit(main())
