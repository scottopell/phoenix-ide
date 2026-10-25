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
import math
import os
import plistlib
import re
import secrets
import shutil
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request
import urllib.parse
import stat
from pathlib import Path
from typing import Callable, Optional, get_args, get_type_hints
from contextlib import closing

HANDOFF_PROTOCOL_VERSION = 1
FULL_GIT_SHA_RE = re.compile(r"[0-9a-f]{40}")
LEGACY_GIT_SHA_RE = re.compile(r"[0-9a-f]{12}")
VERSION_RE = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}")
CURRENT_SCHEMA_BASELINE_VERSION = 123
CURRENT_SCHEMA_REQUIRED_MIGRATIONS = {
    126: "retire_wake_feature_preserving_history",
    127: "coordinator_owned_work_scope_home",
    128: "product_creation_provisioning_stage",
}
# REQ-LDD-028: exact additive candidate registry; historical protected pins are independent.
ORDINARY_SOURCE129_TARGET131_REGISTRY_SHA256 = "c3911ab5dcf05dc4c19472486ae463179948d692370976ed3c81ee6948445744"
ORDINARY_SOURCE129_TARGET131_MIGRATIONS = (
    (129, "single_conversation_name_and_route_projection"),
    (130, "continuation_handoff_candidates"),
    (131, "request_capacity_diagnostics"),
)
MINIMUM_FREE_AFTER_RESERVATION_BYTES = 20 * 1024 * 1024 * 1024
CURRENT_SCHEMA_GROWTH_MARGIN_MIN_BYTES = 1024 * 1024 * 1024
CURRENT_SCHEMA_GROWTH_MARGIN_MAX_BYTES = 8 * 1024 * 1024 * 1024
CURRENT_SCHEMA_GROWTH_MARGIN_DIVISOR = 10
CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES = 2 * 1024 * 1024
MAX_DATABASE_PROOF_BYTES = 1024 * 1024
CAPACITY_EVIDENCE_BUDGET_BYTES = 4096
PROJECTED_MIGRATION_TIMESTAMP = "9999-12-31T23:59:59.999999+00:00"
DEPLOY_HELPER_LABEL_PREFIX = "com.phoenix-ide.deploy"
DEFAULT_TRANSITION_TIMEOUT_SECS = 30.0
DEFAULT_HEALTH_TIMEOUT_SECS = 120.0

TERMINAL_STATES = {
    "committed",
    "precondition_failed",
    "activation_failed_rolled_back",
    "activation_failed_rollback_failed",
    "activation_failed_database_committed",
    "ordinary_activation_failed_rollback_failed",
    "ordinary_activation_failed_forward_only",
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
class CurrentSchemaUpgrade:
    database_path: str
    backup_path: str
    proof_path: str
    controller_source_commit: str
    controller_helper_sha256: str
    controller_helper_path: str
    migration_registry_sha256: str
    migration_registry_path: str


PREPARED_SOURCE114_COMMIT = "23badb8be4bc8d05a7e1c49c5367ceb14ebccd92"
PREPARED_SOURCE114_REGISTRY_SHA256 = "a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6"
PREPARED_SOURCE128_COMMIT = "d5513ee5d9453f66575a0d9161491fb49bcb0b67"
PREPARED_SOURCE128_VERSION = "0.13.0"
PREPARED_SOURCE128_REGISTRY_SHA256 = "c54f9884b1b86aea19ca214cf059724f456c899a219006b59f1c6a16cca6caa5"
PREPARED_SOURCE128_TARGET = (129, "single_conversation_name_and_route_projection")
PREPARED_SCHEMA129_REGISTRY_SHA256 = PREPARED_SOURCE128_REGISTRY_SHA256


def _protected_129(manifest: Manifest) -> bool:
    return isinstance(manifest.current_schema_upgrade, (PreparedSource128Upgrade, PreparedSchema129Refresh))


def _schema129_refresh(manifest: Manifest) -> bool:
    return isinstance(manifest.current_schema_upgrade, PreparedSchema129Refresh)


def _post_refresh_marker(manifest: Manifest) -> bool:
    status = read_status(manifest)
    if status.get("transaction_id") != manifest.transaction_id:
        raise ActivationError("refresh checkpoint belongs to another transaction")
    return status.get("state") in {"migration_started", "activation_failed_database_committed", "committed"}


def _refresh_start_proof(manifest: Manifest) -> bool:
    current = manifest.current_schema_upgrade
    assert isinstance(current, PreparedSchema129Refresh)
    path = Path(current.proof_path)
    if not path.exists() and not path.is_symlink():
        return False
    return "candidate_start_attempted_at" in _snapshot_proof(manifest, require_rehearsal=False)


SOURCE128_QUARANTINE_ATTEMPTS = 8
SOURCE128_PROOF_BASE_KEYS = {
    "transaction_id", "source_commit", "database", "sha256", "candidate_binary_sha256",
    "candidate_plist_sha256", "previous_binary_sha256", "previous_plist_sha256",
    "controller_source_commit", "controller_helper_sha256", "database_mode",
    "migration_registry_sha256", "prepared_source_qualification", "source_ledger",
    "target_ledger", "source_logical_digest", "backup_logical_digest",
}
SOURCE128_REHEARSAL_KEYS = {
    "rehearsal_ledger", "rehearsal_backup_sha256", "rehearsal_logical_digest", "rehearsed_at",
}
SOURCE128_MIGRATED_KEYS = {"migration_committed_at", "migrated_ledger", "migrated_logical_digest"}
SOURCE128_START_KEYS = {"candidate_start_attempted_at"}


def _source128_proof_pairs(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ActivationError("duplicate source128 database proof key")
        result[key] = value
    return result


def _source128_proof_shape(proof: dict) -> None:
    base = SOURCE128_PROOF_BASE_KEYS
    rehearsed = base | SOURCE128_REHEARSAL_KEYS
    migrated = rehearsed | SOURCE128_MIGRATED_KEYS
    if set(proof) not in (base, rehearsed, migrated, migrated | SOURCE128_START_KEYS, rehearsed | SOURCE128_START_KEYS):
        raise ActivationError("source128 requires exact retained proof keys")
    if proof["backup_logical_digest"] != proof["source_logical_digest"]:
        raise ActivationError("source128 snapshot digests differ")
    if "candidate_start_attempted_at" in proof and (not isinstance(proof["candidate_start_attempted_at"], str) or not proof["candidate_start_attempted_at"]):
        raise ActivationError("source128 candidate start checkpoint is invalid")


def checkpoint_source128_start(manifest: Manifest) -> None:
    proof = _snapshot_proof(manifest)
    expected = SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS
    if not _schema129_refresh(manifest):
        expected |= SOURCE128_MIGRATED_KEYS
    if set(proof) != expected:
        raise ActivationError("protected candidate start requires exact pre-start proof")
    if _schema129_refresh(manifest) and not _post_refresh_marker(manifest):
        raise ActivationError("refresh start requires durable no-old-fallback marker")
    current = manifest.current_schema_upgrade
    assert isinstance(current, (PreparedSource128Upgrade, PreparedSchema129Refresh))
    path = Path(current.proof_path)
    proof["candidate_start_attempted_at"] = utc_now()
    encoded = _encode_database_proof(proof)
    _validate_database_proof_lifecycle_budget(
        b"0" * path.stat().st_size, encoded,
        max(os.statvfs(path.parent).f_frsize, 4096),
    )
    atomic_write(path, encoded, 0o600)


@dataclasses.dataclass(frozen=True)
class PreparedSource128Upgrade(CurrentSchemaUpgrade):
    prepared_source_baseline: int
    candidate_registry_expected_sha256: str


@dataclasses.dataclass(frozen=True)
class PreparedSchema129Refresh(CurrentSchemaUpgrade):
    prepared_source_baseline: int
    candidate_registry_expected_sha256: str
    receipt_path: str
    receipt_sha256: str


CURRENT_REPAIR_COMMIT = "237100c79d9ce672867b284167a099fa0b7a2bcf"
CURRENT_REPAIR_VERSION = "0.13.0-rc.4"
CURRENT_REPAIR_REGISTRY_SHA256 = PREPARED_SOURCE114_REGISTRY_SHA256
CURRENT_REPAIR_DIRECTORY = "current-schema-repair"
CURRENT_REPAIR_RUNTIME_FENCE_SHA256 = hashlib.sha256(b"").hexdigest()
CURRENT_REPAIR_SERVICE_LABEL = "com.phoenix-ide.server"
CURRENT_REPAIR_PLIST_EVIDENCE_MAX_BYTES = 1024 * 1024
CURRENT_REPAIR_QUARANTINE_MAX_ENTRIES = 128
CURRENT_REPAIR_QUARANTINE_MAX_TOTAL_BYTES = 32 * 1024 * 1024
CURRENT_REPAIR_QUARANTINE_ATTEMPTS = 8
CURRENT_REPAIR_QUARANTINE_HEADROOM_ENTRIES = 2 + 3 * CURRENT_REPAIR_QUARANTINE_ATTEMPTS
CURRENT_REPAIR_QUARANTINE_HEADROOM_BYTES = (
    CURRENT_REPAIR_QUARANTINE_HEADROOM_ENTRIES * CURRENT_REPAIR_PLIST_EVIDENCE_MAX_BYTES
)


@dataclasses.dataclass(frozen=True)
class CurrentRepairFileToken:
    device: int
    inode: int
    mode: int
    size: int
    modified_ns: int
    changed_ns: int


@dataclasses.dataclass(frozen=True)
class CurrentRepairRuntimeFence:
    target_binary: Path
    target_plist: Path


def current_repair_runtime_fence(parent: Manifest) -> CurrentRepairRuntimeFence:
    return CurrentRepairRuntimeFence(Path(parent.target_binary), Path(parent.target_plist))


def validate_current_repair_runtime_fence_source(parent: Manifest) -> CurrentRepairRuntimeFence:
    fence = current_repair_runtime_fence(parent)
    _regular_nosymlink(fence.target_binary, "installed original candidate")
    target_digest = sha256(fence.target_binary)
    if target_digest not in {parent.candidate_binary_sha256, CURRENT_REPAIR_RUNTIME_FENCE_SHA256}:
        raise ActivationError("installed runtime is neither the exact original candidate nor the stopped repair fence")
    if os.path.lexists(fence.target_plist):
        _regular_nosymlink(fence.target_plist, "published original plist")
        verify_staged(str(fence.target_plist), parent.candidate_plist_sha256, "published original plist")
    return fence


def install_current_repair_runtime_fence(parent: Manifest) -> None:
    fence = validate_current_repair_runtime_fence_source(parent)
    require_launchd_absence(parent.label, parent.uid)
    try:
        if sha256(fence.target_binary) != CURRENT_REPAIR_RUNTIME_FENCE_SHA256:
            atomic_write(fence.target_binary, b"", 0o600)
        _regular_nosymlink(fence.target_binary, "stopped repair runtime fence")
        if sha256(fence.target_binary) != CURRENT_REPAIR_RUNTIME_FENCE_SHA256 or fence.target_binary.stat().st_mode & 0o111:
            raise ActivationError("stopped repair runtime fence is not durable")
        if os.path.lexists(fence.target_plist):
            fence.target_plist.unlink()
            fsync_dir(fence.target_plist.parent)
        require_launchd_absence(parent.label, parent.uid)
    except BaseException as exc:
        try:
            Launchctl(parent).stop()
            require_launchd_absence(parent.label, parent.uid)
        except Exception as teardown_exc:
            raise ActivationError(f"stopped repair runtime fence failed and teardown is unconfirmed: {teardown_exc}") from exc
        raise


@dataclasses.dataclass(frozen=True)
class CurrentSchemaRepairManifest:
    protocol_version: int
    parent_manifest_path: str
    parent_manifest_sha256: str
    parent_status_sha256: str
    parent_proof_sha256: str
    source_commit: str
    version: str
    controller_source_commit: str
    helper_sha256: str
    candidate_sha256: str
    registry_sha256: str
    plist_sha256: str
    receipt_sha256: str
    helper_label: str

    @classmethod
    def load(cls, path: Path) -> CurrentSchemaRepairManifest:
        try:
            repair = _manifest_record(cls, _repair_json(path))
            if repair.protocol_version != 1:
                raise ValueError("unsupported repair protocol")
            if (repair.source_commit, repair.version, repair.registry_sha256) != (
                CURRENT_REPAIR_COMMIT, CURRENT_REPAIR_VERSION, CURRENT_REPAIR_REGISTRY_SHA256
            ):
                raise ValueError("unqualified repair candidate")
            if not FULL_GIT_SHA_RE.fullmatch(repair.controller_source_commit) or repair.controller_source_commit == repair.source_commit:
                raise ValueError("repair requires a separate exact controller")
            for field in dataclasses.fields(cls):
                if field.name.endswith("sha256") and not re.fullmatch(r"[0-9a-f]{64}", getattr(repair, field.name)):
                    raise ValueError("invalid repair evidence digest")
            parent = Path(repair.parent_manifest_path)
            if not parent.is_absolute() or parent != parent.resolve() or parent.name != "manifest.json":
                raise ValueError("invalid repair parent path")
            if path != parent.parent / CURRENT_REPAIR_DIRECTORY / "manifest.json":
                raise ValueError("repair must use the single deterministic child")
            _private_transaction_dir(parent.parent)
            _private_transaction_dir(path.parent)
            return repair
        except (OSError, TypeError, ValueError, KeyError) as exc:
            raise ActivationError(f"invalid sealed current-schema repair: {exc}") from exc


@dataclasses.dataclass(frozen=True)
class CurrentSchemaRepairStatus:
    protocol_version: int
    manifest_sha256: str
    state: str
    resume_from: Optional[str]
    proof_sha256: Optional[str]
    finalization_pending: bool
    failure: Optional[str]

    @classmethod
    def load(cls, path: Path, manifest_hash: str) -> CurrentSchemaRepairStatus:
        try:
            status = _manifest_record(cls, _repair_json(path))
            phases = {"prepared", "reconciliation_started", "target_verified", "start_attempted", "committed"}
            failed = status.state in {"failed_stopped", "stop_unconfirmed"}
            phase = status.resume_from if failed else status.state
            if (status.protocol_version != 1 or status.manifest_sha256 != manifest_hash
                    or not re.fullmatch(r"[0-9a-f]{64}", manifest_hash) or phase not in phases):
                raise ValueError("repair status identity/state mismatch")
            if (not failed and status.resume_from is not None) or failed != (status.failure is not None):
                raise ValueError("repair failure checkpoint mismatch")
            verified = phase in {"target_verified", "start_attempted", "committed"}
            if verified != (status.proof_sha256 is not None):
                raise ValueError("repair proof/state mismatch")
            if status.proof_sha256 is not None and not re.fullmatch(r"[0-9a-f]{64}", status.proof_sha256):
                raise ValueError("repair proof digest malformed")
            if status.finalization_pending and (status.state != "committed" or failed):
                raise ValueError("repair finalization/state mismatch")
            return status
        except (OSError, TypeError, ValueError, KeyError) as exc:
            raise ActivationError(f"invalid current-schema repair status: {exc}") from exc


@dataclasses.dataclass(frozen=True)
class PreparedSource114Upgrade(CurrentSchemaUpgrade):
    prepared_source_baseline: int
    candidate_registry_expected_sha256: str


@dataclasses.dataclass(frozen=True)
class ValidatedLaunchdHandoff:
    candidate_binary: Path
    candidate_plist: Path
    rollback: Optional[tuple[Path, Path]]


@dataclasses.dataclass(frozen=True)
class OrdinaryForwardMigration:
    database_path: str
    helper_sha256: str
    helper_path: str
    migration_registry_sha256: str
    migration_registry_path: str


@dataclasses.dataclass(frozen=True)
class DatabaseCapacityPlan:
    reservation_bytes: int
    ceiling_bytes: int


@dataclasses.dataclass(frozen=True)
class DatabaseCapacityReservation:
    backup_path: Path
    restore_path: Path
    capacity_bytes: int
    ceiling_bytes: Optional[int] = None
    observed_source_bytes: Optional[int] = None
    measured_at: Optional[str] = None


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
    transition_timeout_secs: float = DEFAULT_TRANSITION_TIMEOUT_SECS
    health_timeout_secs: float = DEFAULT_HEALTH_TIMEOUT_SECS
    # None is intentional for pre-feature runtime-only manifests.
    paired_database_upgrade: Optional[PairedDatabaseUpgrade] = None
    current_schema_upgrade: Optional[CurrentSchemaUpgrade | PreparedSource114Upgrade | PreparedSource128Upgrade | PreparedSchema129Refresh] = None
    ordinary_forward_migration: Optional[OrdinaryForwardMigration] = None

    @classmethod
    def load(cls, path: Path) -> "Manifest":
        try:
            raw = json.loads(path.read_text())
            if not isinstance(raw, dict):
                raise TypeError("manifest must be an object")
            if raw.get("manifest_version") != HANDOFF_PROTOCOL_VERSION:
                raise ActivationError(
                    f"unsupported handoff protocol {raw.get('manifest_version')!r}; expected {HANDOFF_PROTOCOL_VERSION}"
                )
            raw["expected"] = _manifest_record(Identity, raw["expected"])
            previous = raw.get("previous")
            raw["previous"] = _manifest_record(Identity, previous) if previous is not None else None
            for key, record_type in (
                ("paired_database_upgrade", PairedDatabaseUpgrade),
                ("current_schema_upgrade", CurrentSchemaUpgrade),
                ("ordinary_forward_migration", OrdinaryForwardMigration),
            ):
                value = raw.get(key)
                if value is not None:
                    if key == "current_schema_upgrade" and isinstance(value, dict):
                        if value.get("prepared_source_baseline") == 129 and type(value.get("prepared_source_baseline")) is int:
                            record_type = PreparedSchema129Refresh
                        elif value.get("prepared_source_baseline") == 128 and type(value.get("prepared_source_baseline")) is int:
                            record_type = PreparedSource128Upgrade
                        elif "prepared_source_baseline" in value or "candidate_registry_expected_sha256" in value:
                            record_type = PreparedSource114Upgrade
                    value = _manifest_record(record_type, value)
                    if any(isinstance(item, str) and not item.strip() for item in dataclasses.asdict(value).values()):
                        raise ValueError(f"{key} string fields must be nonempty")
                raw[key] = value
            # Runtime-only manifests written before the paired feature omit the field.
            raw.pop("database_mode", None)
            for legacy_key in ("database_path", "database_backup_path", "database_backup_sha256", "database_backup_verified", "database_proof_path", "controller_source_commit", "controller_helper_sha256", "controller_helper_path"):
                raw.pop(legacy_key, None)
            return _manifest_record(cls, raw)
        except (TypeError, KeyError, ValueError) as exc:
            raise ActivationError(f"invalid manifest: {exc}") from exc


def _manifest_record(record_type: type, raw: object):
    if not isinstance(raw, dict):
        raise TypeError(f"{record_type.__name__} must be an object")
    record = record_type(**raw)
    for name, field_type in get_type_hints(record_type, globalns=globals()).items():
        value = getattr(record, name)
        choices = get_args(field_type) or (field_type,)
        if not any(
            (type(value) in (int, float) if choice is float else type(value) is choice)
            for choice in choices
        ):
            raise TypeError(f"{record_type.__name__}.{name} has an invalid type")
    return record


class ActivationError(RuntimeError):
    pass


class ConcurrentDeploy(ActivationError):
    pass


class CurrentSchemaPreconditionFailure(ActivationError):
    pass


class RecoveryModeMismatch(ActivationError):
    pass

class CleanupPreconditionRejected(ActivationError):
    pass


class DatabaseCapacityCleanupFailure(ActivationError):
    pass


class VerifiedPredecessorFinalizationError(ActivationError):
    pass


def read_status(manifest: Manifest) -> dict:
    try:
        value = json.loads(Path(manifest.status_path).read_text())
    except (OSError, json.JSONDecodeError):
        return {}
    return value if isinstance(value, dict) else {}


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


def prepare_plist_publication(staged: Path, target: Path) -> Path:
    fd, name = tempfile.mkstemp(prefix=f".{target.name}.publish-", dir=target.parent)
    os.close(fd)
    prepared = Path(name)
    prepared.unlink()
    try:
        os.link(staged, prepared)
        fsync_dir(target.parent)
        return prepared
    except BaseException:
        prepared.unlink(missing_ok=True)
        raise


def commit_atomic_install(prepared: Path, target: Path) -> None:
    os.replace(prepared, target)
    fsync_dir(target.parent)


def write_status(
    manifest: Manifest,
    state: str,
    *,
    failure: Optional[str] = None,
    rollback_failure: Optional[str] = None,
    cleanup_failure: Optional[str] = None,
    precondition_cleanup_completed: bool = False,
    committed_diagnostic: Optional[str] = None,
    finalization_pending: bool = False,
    recovery_mode: Optional[str] = None,
) -> None:
    try:
        prior = read_status(manifest)
    except (OSError, json.JSONDecodeError):
        prior = {}
    if isinstance(prior, dict) and prior.get("transaction_id") == manifest.transaction_id and prior.get("recovery_mode") is not None:
        recovery_mode = recovery_mode or prior.get("recovery_mode")
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
        "cleanup_failure": cleanup_failure,
        "precondition_cleanup_completed": precondition_cleanup_completed,
        "committed_diagnostic": committed_diagnostic,
        "finalization_pending": finalization_pending,
        "recovery_mode": recovery_mode,
    }
    status["prepared_mode"] = manifest_prepared_mode(manifest)
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
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
        raise ActivationError("prepared database upgrade is not configured")
    database = Path(upgrade.database_path)
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


def prepared_database_upgrade(
    manifest: Manifest,
) -> PairedDatabaseUpgrade | CurrentSchemaUpgrade | None:
    modes = tuple(
        mode
        for mode in (manifest.paired_database_upgrade, manifest.current_schema_upgrade)
        if mode is not None
    )
    if len(modes) > 1:
        raise ActivationError("prepared database upgrade modes are mutually exclusive")
    return modes[0] if modes else None


def _private_transaction_dir(path: Path, *, create: bool = True) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        if not create:
            raise ActivationError("paired transaction directory is unavailable")
        path.mkdir(parents=True, mode=0o700)
        mode = path.lstat().st_mode
    except OSError as exc:
        raise ActivationError("paired transaction directory is unavailable") from exc
    if not stat.S_ISDIR(mode) or path.is_symlink() or (mode & 0o777) != 0o700:
        raise ActivationError("paired transaction directory must be a private 0700 directory")
    return path


def _paired_path_context(
    manifest: Manifest,
    *,
    prepare_transaction: bool = True,
) -> tuple[Path, Path, Path, Path]:
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
        raise ActivationError("prepared database upgrade is not configured")
    database = Path(upgrade.database_path)
    backup = Path(upgrade.backup_path)
    proof = Path(upgrade.proof_path)
    for path, description in ((database, "paired database"), (backup, "database backup"), (proof, "database proof")):
        if not path.is_absolute() or path.is_symlink():
            raise ActivationError(f"{description} path must be absolute and non-symlink")
    if backup.parent != proof.parent:
        raise ActivationError("database backup and proof must share one transaction directory")
    resolved = {path.resolve(strict=False) for path in (database, backup, proof)}
    if len(resolved) != 3:
        raise ActivationError("database backup and proof must not alias the database or each other")
    return database, backup, proof, _private_transaction_dir(
        backup.parent,
        create=prepare_transaction,
    )


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
    import sqlite3
    if not path.is_absolute() or path.is_symlink():
        raise ActivationError("SQLite path must be an absolute non-symlink file")
    uri = "file:" + urllib.parse.quote(str(path), safe="/") + "?mode=ro"
    try:
        return sqlite3.connect(uri, uri=True, timeout=2)
    except sqlite3.Error as exc:
        raise ActivationError("could not open SQLite database read-only") from exc


def validate_database(path: Path) -> None:
    import sqlite3
    from contextlib import closing
    try:
        with closing(_readonly_connection(path)) as connection:
            result = connection.execute("PRAGMA integrity_check").fetchone()
            foreign_keys = connection.execute("PRAGMA foreign_key_check").fetchall()
    except sqlite3.Error as exc:
        raise ActivationError(f"SQLite integrity check failed: {exc}") from exc
    if result != ("ok",):
        raise ActivationError(f"SQLite integrity check returned {result!r}")
    if foreign_keys:
        raise ActivationError("SQLite foreign key check failed")


def migration_registry(
    path: Path, *, qualified_current_target: bool = True, qualified_source128: bool = False
) -> tuple[tuple[int, str], ...]:
    try:
        source = path.read_text()
    except (OSError, UnicodeError) as exc:
        raise ActivationError("candidate migration registry is unreadable") from exc
    match = re.search(
        r"const MIGRATIONS: &\[Migration\] = &\[(.*?)\n\];",
        source,
        re.DOTALL,
    )
    if match is None:
        raise ActivationError("candidate migration registry is invalid")
    entries = match.group(1).strip()
    block_pattern = re.compile(
        r'\s*Migration\s*\{((?:"(?:\\.|[^"\\])*"|[^{}"])*?)\}\s*(?:,|$)',
        re.DOTALL,
    )
    field_pattern = re.compile(
        r'\s*(?:version\s*:\s*(?P<version>[0-9]+)'
        r'|name\s*:\s*"(?P<name>[^"\\]+)"'
        r'|sql\s*:\s*(?P<sql>[A-Za-z_][A-Za-z0-9_]*'
        r'|"(?:\\.|[^"\\])*"'
        r'|include_str!\(\s*"(?:\\.|[^"\\])*"\s*\)))'
        r'\s*(?:,|$)',
        re.DOTALL,
    )
    parsed = []
    position = 0
    while position < len(entries):
        block = block_pattern.match(entries, position)
        if block is None:
            raise ActivationError("candidate migration registry is invalid")
        body = block.group(1).strip()
        fields = {}
        field_position = 0
        while field_position < len(body):
            field = field_pattern.match(body, field_position)
            if field is None or field.lastgroup in fields:
                raise ActivationError("candidate migration registry is invalid")
            fields[field.lastgroup] = field.group(field.lastgroup)
            field_position = field.end()
        if set(fields) != {"version", "name", "sql"}:
            raise ActivationError("candidate migration registry is invalid")
        parsed.append((int(fields["version"]), fields["name"]))
        position = block.end()
    migrations = tuple(parsed)
    versions = tuple(version for version, _name in migrations)
    if not migrations or versions != tuple(sorted(set(versions))):
        raise ActivationError("candidate migration registry is invalid")
    if qualified_current_target:
        registry = dict(migrations)
        if any(
            registry.get(version) != name
            for version, name in CURRENT_SCHEMA_REQUIRED_MIGRATIONS.items()
        ):
            raise ActivationError(
                "candidate migration registry lacks required irreversible migrations"
            )
        expected_target = (
            PREPARED_SOURCE128_TARGET if qualified_source128
            else (128, CURRENT_SCHEMA_REQUIRED_MIGRATIONS[128])
        )
        if migrations[-1][0] > expected_target[0]:
            raise ActivationError("candidate migration registry is newer than the qualified target")
        if migrations[-1] != expected_target:
            raise ActivationError("candidate migration registry differs from the qualified target")
    return migrations


def database_ledger(path: Path) -> tuple[tuple[int, str], ...]:
    import sqlite3
    from contextlib import closing
    try:
        with closing(_readonly_connection(path)) as connection:
            rows = connection.execute(
                "SELECT version, name FROM _migrations ORDER BY version"
            ).fetchall()
    except sqlite3.Error as exc:
        raise ActivationError("SQLite migration ledger check failed") from exc
    if any(not isinstance(version, int) or not isinstance(name, str) for version, name in rows):
        raise ActivationError("SQLite migration ledger is malformed")
    return tuple((version, name) for version, name in rows)


def _validate_product_conversation_schema(connection) -> None:
    required_tables = {
        "conversations",
        "product_conversations",
        "product_conversation_sources",
    }
    tables = {
        str(row[0])
        for row in connection.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN (?, ?, ?)",
            tuple(sorted(required_tables)),
        )
    }
    missing_tables = sorted(required_tables - tables)
    if missing_tables:
        raise ActivationError(
            "current-schema migration requires ProductConversation schema: missing table(s) "
            + ", ".join(missing_tables)
        )

    conversation_columns = {
        str(row[1]): (str(row[2]).upper(), int(row[3]), int(row[5]))
        for row in connection.execute("PRAGMA table_info(conversations)")
    }
    if conversation_columns.get("product_conversation_id") != ("TEXT", 0, 0):
        raise ActivationError(
            "current-schema migration requires ProductConversation schema: "
            "conversations.product_conversation_id must be nullable TEXT"
        )
    conversation_foreign_keys = {
        (str(row[2]), str(row[3]), str(row[4]), str(row[6]).upper())
        for row in connection.execute("PRAGMA foreign_key_list(conversations)")
    }
    if (
        "product_conversations",
        "product_conversation_id",
        "id",
        "CASCADE",
    ) not in conversation_foreign_keys:
        raise ActivationError(
            "current-schema migration requires ProductConversation schema: "
            "conversations.product_conversation_id foreign key is missing"
        )

    source_columns = {
        str(row[1]): (str(row[2]).upper(), int(row[3]), int(row[5]))
        for row in connection.execute("PRAGMA table_info(product_conversation_sources)")
    }
    required_source_columns = {
        "target_product_conversation_id": ("TEXT", 0, 1),
        "source_product_conversation_id": ("TEXT", 1, 0),
        "source_conversation_id": ("TEXT", 1, 0),
        "relation_kind": ("TEXT", 1, 0),
        "relation_key": ("TEXT", 1, 0),
        "created_at_us": ("INTEGER", 1, 0),
    }
    if any(
        source_columns.get(name) != shape
        for name, shape in required_source_columns.items()
    ):
        raise ActivationError(
            "current-schema migration requires ProductConversation schema: "
            "product_conversation_sources columns do not match migration 70"
        )
    source_foreign_keys = {
        (str(row[2]), str(row[3]), str(row[4]), str(row[6]).upper())
        for row in connection.execute(
            "PRAGMA foreign_key_list(product_conversation_sources)"
        )
    }
    required_source_foreign_keys = {
        (
            "product_conversations",
            "target_product_conversation_id",
            "id",
            "CASCADE",
        ),
    }
    if not required_source_foreign_keys.issubset(source_foreign_keys):
        raise ActivationError(
            "current-schema migration requires ProductConversation schema: "
            "product_conversation_sources foreign keys do not match migration 70"
        )


def validate_current_schema_database(
    path: Path,
    registry_path: Path,
    *,
    migrated: bool = False,
    source_kind: str = "prepared_artifact",
    manifest: Optional[Manifest] = None,
) -> tuple[tuple[int, str], ...]:
    import sqlite3
    from contextlib import closing

    qualified114 = manifest is not None and isinstance(manifest.current_schema_upgrade, PreparedSource114Upgrade)
    qualified128 = manifest is not None and _protected_129(manifest)
    if qualified114 or qualified128:
        validate_prepared_source_qualification(manifest)
        if source_kind != manifest.source_kind or registry_path != Path(manifest.current_schema_upgrade.migration_registry_path):
            raise ActivationError("qualified prepared registry or source kind differs from manifest")
    validate_database(path)
    registry = migration_registry(registry_path, qualified_source128=qualified128)
    ledger = database_ledger(path)
    target = tuple(registry)
    baseline_version = {
        "prepared_artifact": CURRENT_SCHEMA_BASELINE_VERSION,
        "published_release": 114,
    }.get(source_kind)
    if qualified114:
        baseline_version = 114
    elif qualified128:
        baseline_version = 129 if _schema129_refresh(manifest) else 128
    if baseline_version is None:
        raise ActivationError("current-schema source kind is not qualified")
    if not any(version == baseline_version for version, _name in registry):
        raise ActivationError("candidate migration registry is missing the qualified source baseline version")
    baseline = tuple(item for item in registry if item[0] <= baseline_version)
    if migrated:
        admitted = (target,)
    elif source_kind == "published_release" or qualified128:
        admitted = (baseline,)
    else:
        admitted = (baseline, target)
    if not ledger or ledger not in admitted:
        qualification = "candidate target" if migrated else "qualified source"
        raise ActivationError(
            f"current-schema migration ledger differs from {qualification}"
        )
    try:
        with closing(_readonly_connection(path)) as connection:
            _validate_product_conversation_schema(connection)
    except sqlite3.Error as exc:
        raise ActivationError("current-schema ProductConversation schema check failed") from exc
    return ledger


def database_logical_digest(path: Path) -> str:
    import sqlite3
    from contextlib import closing

    try:
        with closing(_readonly_connection(path)) as connection:
            schema = connection.execute(
                "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"
            ).fetchall()
            digest = hashlib.sha256()
            digest.update(json.dumps(schema, separators=(",", ":")).encode())
            for kind, name, _table, _sql in schema:
                if kind != "table":
                    continue
                quoted = '"' + name.replace('"', '""') + '"'
                row_hashes = []
                for row in connection.execute(f"SELECT * FROM {quoted}"):
                    encoded = [
                        ("blob", value.hex())
                        if isinstance(value, bytes)
                        else (type(value).__name__, value)
                        for value in row
                    ]
                    row_hashes.append(
                        hashlib.sha256(
                            json.dumps(
                                encoded, separators=(",", ":"), ensure_ascii=True
                            ).encode()
                        ).digest()
                    )
                digest.update(name.encode())
                digest.update(len(row_hashes).to_bytes(8, "big"))
                for row_hash in sorted(row_hashes):
                    digest.update(row_hash)
            return digest.hexdigest()
    except sqlite3.Error as exc:
        raise ActivationError("could not digest SQLite database") from exc


def ordinary_forward_target(ordinary: OrdinaryForwardMigration, registry: Path) -> tuple[tuple[int, str], ...]:
    if ordinary.migration_registry_sha256 == ORDINARY_SOURCE129_TARGET131_REGISTRY_SHA256:
        target = migration_registry(registry, qualified_current_target=False)
        if target[-3:] != ORDINARY_SOURCE129_TARGET131_MIGRATIONS:
            raise ActivationError("ordinary candidate registry differs from qualified target131")
        return target
    return migration_registry(registry)


def ordinary_forward_policy_applies(manifest: Manifest) -> bool:
    ordinary = manifest.ordinary_forward_migration
    if ordinary is None:
        return False
    database = Path(ordinary.database_path)
    ledger = database_ledger(database)
    registry = verify_staged(
        ordinary.migration_registry_path,
        ordinary.migration_registry_sha256,
        "ordinary candidate migration registry",
    )
    target = migration_registry(registry, qualified_current_target=False)
    if ledger == target:
        return False
    if ordinary.migration_registry_sha256 == ORDINARY_SOURCE129_TARGET131_REGISTRY_SHA256:
        target = ordinary_forward_target(ordinary, registry)
        source = tuple(item for item in target if item[0] <= 129)
        if ledger != source:
            raise ActivationError("ordinary pending migration interval is not qualified; expected exact source129 ledger")
        validate_database(database)
        return True
    if not ledger or ledger[-1][0] != CURRENT_SCHEMA_BASELINE_VERSION:
        raise ActivationError("ordinary pending migration interval is not qualified; use an explicit protected upgrade")
    validate_current_schema_database(database, registry)
    return True


def validate_legacy_database(path: Path) -> None:
    import sqlite3
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
    import sqlite3
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
    import sqlite3
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
        fsync_dir(path.parent)
    except (OSError, sqlite3.Error) as exc:
        path.unlink(missing_ok=True)
        raise ActivationError("could not reserve paired database capacity") from exc


def _capacity_pool_key(path: Path) -> tuple[str, object]:
    device = path.stat().st_dev
    if sys.platform != "darwin":
        return ("device", device)
    mount_point = path if path.is_dir() else path.parent
    mount_point = mount_point.resolve()
    while not os.path.ismount(mount_point):
        parent = mount_point.parent
        if parent == mount_point:
            raise ActivationError(f"could not identify mount point for {path}")
        mount_point = parent
    result = subprocess.run(
        ["/usr/sbin/diskutil", "info", "-plist", str(mount_point)],
        capture_output=True,
    )
    if result.returncode != 0:
        raise ActivationError(f"could not identify capacity pool for {path}")
    try:
        info = plistlib.loads(result.stdout)
    except (plistlib.InvalidFileException, ValueError) as exc:
        raise ActivationError(f"could not parse capacity pool for {path}") from exc
    container = info.get("APFSContainerReference")
    if isinstance(container, str) and container:
        return ("apfs_container", container)
    return ("device", device)


def _current_schema_live_growth_margin(observed: int) -> int:
    proportional = (
        observed + CURRENT_SCHEMA_GROWTH_MARGIN_DIVISOR - 1
    ) // CURRENT_SCHEMA_GROWTH_MARGIN_DIVISOR
    return min(
        CURRENT_SCHEMA_GROWTH_MARGIN_MAX_BYTES,
        max(CURRENT_SCHEMA_GROWTH_MARGIN_MIN_BYTES, proportional),
    )


def _current_schema_reservation_plan(
    manifest: Manifest,
    database: Path,
    backup: Path,
    restore: Path,
    observed: int,
) -> DatabaseCapacityPlan:
    reservation_parents = (backup.parent, restore.parent)
    reservations_per_pool: dict[tuple[str, object], int] = {}
    parent_by_pool: dict[tuple[str, object], Path] = {}
    paths_by_pool: dict[tuple[str, object], list[Path]] = {}
    for parent in reservation_parents:
        pool = _capacity_pool_key(parent)
        reservations_per_pool[pool] = reservations_per_pool.get(pool, 0) + 1
        parent_by_pool[pool] = parent
        paths_by_pool.setdefault(pool, []).append(parent)
    affected_paths = (
        database.parent,
        Path(manifest.target_binary).parent,
        Path(manifest.target_plist).parent,
    )
    for path in affected_paths:
        paths_by_pool.setdefault(_capacity_pool_key(path), []).append(path)

    ceilings = []
    allocation_units = []
    database_pool = _capacity_pool_key(database)
    evidence_pool = _capacity_pool_key(backup.parent)
    for pool, count in reservations_per_pool.items():
        parent = parent_by_pool[pool]
        free = min(shutil.disk_usage(path).free for path in paths_by_pool[pool])
        allocation_unit = max(os.statvfs(parent).f_frsize, 4096)
        allocation_units.append(allocation_unit)
        evidence_blocks = (
            CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES if pool == evidence_pool else 0
        )
        holds_live_growth = 1 if pool == database_pool else 0
        available = (
            free
            - MINIMUM_FREE_AFTER_RESERVATION_BYTES
            - evidence_blocks
        )
        divisor = count + holds_live_growth
        per_reservation = max(0, available // divisor)
        ceilings.append(per_reservation - (per_reservation % allocation_unit))

    for pool, paths in paths_by_pool.items():
        if (
            pool not in reservations_per_pool
            and min(shutil.disk_usage(path).free for path in paths)
            < MINIMUM_FREE_AFTER_RESERVATION_BYTES
        ):
            raise ActivationError(
                "current-schema migration requires 20 GiB free after backup and restore reservations"
            )

    common_unit = math.lcm(*allocation_units)
    ceiling = min(ceilings)
    ceiling -= ceiling % common_unit
    if ceiling < observed:
        raise ActivationError(
            "current-schema migration requires 20 GiB free after backup and restore reservations"
        )
    desired = observed + _current_schema_live_growth_margin(observed)
    desired += -desired % common_unit
    return DatabaseCapacityPlan(
        reservation_bytes=min(desired, ceiling),
        ceiling_bytes=ceiling,
    )


def _database_proof_bytes(
    manifest: Manifest,
    source: Path,
    *,
    backup_sha256: str,
    source_logical_digest: str,
    backup_logical_digest: str,
) -> bytes:
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
        raise ActivationError("database proof requires a prepared database transaction")
    payload = {
        "transaction_id": manifest.transaction_id,
        "source_commit": manifest.source_commit,
        "database": str(source),
        "sha256": backup_sha256,
        "candidate_binary_sha256": manifest.candidate_binary_sha256,
        "candidate_plist_sha256": manifest.candidate_plist_sha256,
        "previous_binary_sha256": manifest.rollback_binary_sha256,
        "previous_plist_sha256": manifest.rollback_plist_sha256,
        "controller_source_commit": upgrade.controller_source_commit,
        "controller_helper_sha256": upgrade.controller_helper_sha256,
        "database_mode": (
            "current_schema"
            if manifest.current_schema_upgrade is not None
            else "legacy_paired"
        ),
    }
    if manifest.current_schema_upgrade is not None:
        registry_path = Path(manifest.current_schema_upgrade.migration_registry_path)
        payload.update(
            migration_registry_sha256=(
                manifest.current_schema_upgrade.migration_registry_sha256
            ),
            source_ledger=validate_current_schema_database(source, registry_path, source_kind=manifest.source_kind, manifest=manifest),
            target_ledger=current_schema_target_registry(manifest, registry_path),
            source_logical_digest=source_logical_digest,
            backup_logical_digest=backup_logical_digest,
        )
    if isinstance(manifest.current_schema_upgrade, (PreparedSource114Upgrade, PreparedSource128Upgrade, PreparedSchema129Refresh)):
        payload["prepared_source_qualification"] = prepared_source_qualification(manifest)
    return _encode_database_proof(payload)


def _encode_database_proof(payload: dict) -> bytes:
    encoded = (json.dumps(payload, sort_keys=True) + "\n").encode()
    if len(encoded) > MAX_DATABASE_PROOF_BYTES:
        raise ActivationError(
            f"database proof exceeds {MAX_DATABASE_PROOF_BYTES} byte capacity bound"
        )
    return encoded


def _project_migrated_database_proof(proof_bytes: bytes) -> bytes:
    payload = json.loads(proof_bytes)
    qualification = payload.get("prepared_source_qualification")
    if isinstance(qualification, dict) and qualification.get("prepared_source_baseline") in (128, 129):
        payload["rehearsal_ledger"] = payload["target_ledger"]
        payload["rehearsal_backup_sha256"] = "0" * 64
        payload["rehearsal_logical_digest"] = "0" * 64
        payload["rehearsed_at"] = PROJECTED_MIGRATION_TIMESTAMP
    if not isinstance(qualification, dict) or qualification.get("prepared_source_baseline") != 129:
        payload["migration_committed_at"] = PROJECTED_MIGRATION_TIMESTAMP
        payload["migrated_ledger"] = payload["target_ledger"]
        payload["migrated_logical_digest"] = "0" * 64
    if isinstance(qualification, dict) and qualification.get("prepared_source_baseline") in (128, 129):
        payload["candidate_start_attempted_at"] = PROJECTED_MIGRATION_TIMESTAMP
    return _encode_database_proof(payload)


def _allocated_bytes(size: int, allocation_unit: int) -> int:
    return size + (-size % allocation_unit)


def _validate_database_proof_lifecycle_budget(
    initial_proof: bytes,
    migrated_proof: bytes,
    allocation_unit: int,
) -> None:
    capacity_evidence = _allocated_bytes(
        CAPACITY_EVIDENCE_BUDGET_BYTES, allocation_unit
    )
    required = max(
        _allocated_bytes(len(initial_proof), allocation_unit)
        + _allocated_bytes(len(migrated_proof), allocation_unit)
        + capacity_evidence,
        2 * capacity_evidence,
    )
    if required > CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES:
        raise ActivationError(
            "database proof lifecycle exceeds durable-evidence capacity bound"
        )


def _capacity_observation_path(reservation: DatabaseCapacityReservation) -> Path:
    return reservation.backup_path.parent / "capacity.json"


def _resident_evidence_bytes(
    manifest: Manifest,
    reservation: DatabaseCapacityReservation,
) -> int:
    allocation_unit = max(os.statvfs(reservation.backup_path.parent).f_frsize, 4096)
    proof_path = Path(manifest.current_schema_upgrade.proof_path)
    resident = 0
    for path in (_capacity_observation_path(reservation), proof_path):
        if not path.exists():
            continue
        stat = path.stat()
        blocks = getattr(stat, "st_blocks", 0)
        resident += (
            blocks * 512
            if isinstance(blocks, int) and blocks > 0
            else _allocated_bytes(stat.st_size, allocation_unit)
        )
    if resident > CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES:
        raise ActivationError("durable evidence exceeds its reserved capacity")
    return resident


def _remaining_evidence_allowance(
    manifest: Manifest,
    reservation: DatabaseCapacityReservation,
) -> int:
    return CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES - _resident_evidence_bytes(
        manifest, reservation
    )


def _require_current_schema_free_space(
    manifest: Manifest,
    reservation: DatabaseCapacityReservation,
) -> None:
    directories = {
        Path(manifest.current_schema_upgrade.database_path).parent,
        reservation.backup_path.parent,
        reservation.restore_path.parent,
        Path(manifest.target_binary).parent,
        Path(manifest.target_plist).parent,
    }
    free_by_pool: dict[tuple[str, str], int] = {}
    for directory in directories:
        pool = _capacity_pool_key(directory)
        free = shutil.disk_usage(directory).free
        free_by_pool[pool] = min(free_by_pool.get(pool, free), free)
    if any(
        free < MINIMUM_FREE_AFTER_RESERVATION_BYTES
        for free in free_by_pool.values()
    ):
        raise ActivationError(
            "current-schema capacity no longer preserves the 20 GiB safety floor"
        )
    remaining = _remaining_evidence_allowance(manifest, reservation)
    evidence_pool = _capacity_pool_key(reservation.backup_path.parent)
    if free_by_pool[evidence_pool] < MINIMUM_FREE_AFTER_RESERVATION_BYTES + remaining:
        raise ActivationError(
            "current-schema capacity no longer preserves the 20 GiB floor "
            "plus its remaining durable-evidence allowance"
        )


def _ledger_sha256(ledger: Sequence[tuple[int, str]]) -> str:
    encoded = json.dumps(
        [list(row) for row in ledger], separators=(",", ":")
    ).encode()
    return hashlib.sha256(encoded).hexdigest()


def _source128_capacity_source(manifest: Manifest, reservation: DatabaseCapacityReservation) -> dict:
    path = _capacity_observation_path(reservation)
    _regular_nosymlink(path, "source128 pre-stop capacity evidence")
    try:
        evidence = json.loads(path.read_text(), object_pairs_hook=_source128_proof_pairs)
    except (OSError, ValueError, TypeError) as exc:
        raise ActivationError("source128 pre-stop capacity evidence is unreadable") from exc
    expected_ledger = current_schema_target_registry(manifest, _current_schema_registry(manifest))
    if not _schema129_refresh(manifest):
        expected_ledger = expected_ledger[:-1]
    if (not isinstance(evidence, dict) or evidence.get("transaction_id") != manifest.transaction_id
            or evidence.get("pre_stop_source_ledger_sha256") != _ledger_sha256(expected_ledger)
            or not isinstance(evidence.get("pre_stop_logical_digest"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", evidence["pre_stop_logical_digest"])):
        raise ActivationError("source128 pre-stop ledger or logical digest evidence is invalid")
    return {
        "pre_stop_source_ledger_sha256": evidence["pre_stop_source_ledger_sha256"],
        "pre_stop_logical_digest": evidence["pre_stop_logical_digest"],
    }


def _write_capacity_observation(
    manifest: Manifest,
    reservation: DatabaseCapacityReservation,
    *,
    rechecked_source_bytes: Optional[int] = None,
    rechecked_at: Optional[str] = None,
) -> None:
    source128 = _protected_129(manifest)
    source_evidence = {}
    if source128:
        if rechecked_source_bytes is None:
            source = Path(manifest.current_schema_upgrade.database_path)
            ledger = validate_current_schema_database(
                source, _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest,
            )
            source_evidence = {
                "pre_stop_source_ledger_sha256": _ledger_sha256(ledger),
                "pre_stop_logical_digest": database_logical_digest(source),
            }
        else:
            source_evidence = _source128_capacity_source(manifest, reservation)
    payload = {
        **source_evidence,
        "transaction_id": manifest.transaction_id,
        "observed_source_bytes": reservation.observed_source_bytes,
        "measured_at": reservation.measured_at,
        "reserved_capacity_bytes": reservation.capacity_bytes,
        "reservation_ceiling_bytes": reservation.ceiling_bytes,
        "rechecked_source_bytes": rechecked_source_bytes,
        "rechecked_at": rechecked_at,
    }
    encoded = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode()
    if len(encoded) > CAPACITY_EVIDENCE_BUDGET_BYTES:
        raise ActivationError("capacity observation exceeds durable-evidence capacity bound")
    atomic_write(_capacity_observation_path(reservation), encoded)


def _remove_capacity_files(*paths: Path) -> None:
    failures = []
    parents = set()
    for path in paths:
        parents.add(path.parent)
        try:
            if path.is_symlink():
                raise OSError("refusing symlinked capacity evidence")
            path.unlink(missing_ok=True)
        except OSError as exc:
            failures.append(f"{path}: {exc}")
    for parent in parents:
        try:
            fsync_dir(parent)
        except OSError as exc:
            failures.append(f"{parent}: {exc}")
    if failures:
        raise DatabaseCapacityCleanupFailure("; ".join(failures))


def _atomic_write_temporary_paths(evidence: Path) -> tuple[Path, ...]:
    prefix = f".{evidence.name}."
    try:
        with os.scandir(evidence.parent) as entries:
            return tuple(
                evidence.parent / entry.name
                for entry in entries
                if entry.name.startswith(prefix)
            )
    except OSError as exc:
        raise DatabaseCapacityCleanupFailure(
            f"{evidence.parent}: could not enumerate atomic evidence temporaries: {exc}"
        ) from exc


def _owned_capacity_cleanup_paths(
    manifest: Manifest,
    reservation: DatabaseCapacityReservation,
) -> tuple[Path, ...]:
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
        raise ActivationError("capacity cleanup requires a prepared database mode")
    proof = Path(upgrade.proof_path)
    capacity = _capacity_observation_path(reservation)
    paths = {
        reservation.backup_path,
        reservation.restore_path,
        capacity,
    }
    for evidence in (capacity, proof):
        paths.update(_atomic_write_temporary_paths(evidence))
    for allocation in (reservation.backup_path, reservation.restore_path):
        paths.update(
            allocation.with_name(allocation.name + suffix)
            for suffix in ("-journal", "-wal", "-shm")
        )
    return tuple(sorted(paths, key=str))


def reserve_database_capacity(manifest: Manifest) -> DatabaseCapacityReservation:
    """Hold backup and restore space while the service is stopped."""
    if prepared_database_upgrade(manifest) is None:
        raise ActivationError("prepared database upgrade is not configured")
    database, backup, proof, transaction_dir = _paired_path_context(manifest)
    _regular_nosymlink(database, "paired database")
    _private_transaction_dir(transaction_dir)
    restore = _restore_capacity_path(manifest, database)
    for path, description in ((restore, "restore reservation"),):
        if not path.is_absolute() or path.is_symlink():
            raise ActivationError(f"{description} path must be absolute and non-symlink")
    observed = _source_capacity_bytes(database)
    measured_at = utc_now()
    projected_proof = None
    if manifest.current_schema_upgrade is not None:
        projected_proof = _database_proof_bytes(
            manifest,
            database,
            backup_sha256="0" * 64,
            source_logical_digest="0" * 64,
            backup_logical_digest="0" * 64,
        )
    plan = (
        _current_schema_reservation_plan(
            manifest, database, backup, restore, observed
        )
        if manifest.current_schema_upgrade is not None
        else DatabaseCapacityPlan(observed, observed)
    )
    if projected_proof is not None:
        allocation_unit = max(os.statvfs(backup.parent).f_frsize, 4096)
        _validate_database_proof_lifecycle_budget(
            projected_proof,
            _project_migrated_database_proof(projected_proof),
            allocation_unit,
        )
    # Never replace an existing protected snapshot: recovery must retain it.
    if backup.exists() or backup.is_symlink() or proof.exists() or proof.is_symlink():
        raise ActivationError("paired recovery snapshot is already present")
    reservation = DatabaseCapacityReservation(
        backup,
        restore,
        plan.reservation_bytes,
        ceiling_bytes=plan.ceiling_bytes,
        observed_source_bytes=observed,
        measured_at=measured_at,
    )
    try:
        _allocate_private_sqlite(backup, plan.reservation_bytes)
        _allocate_private_sqlite(restore, plan.reservation_bytes)
        if manifest.current_schema_upgrade is not None:
            affected_directories = (
                backup.parent,
                restore.parent,
                Path(manifest.target_binary).parent,
                Path(manifest.target_plist).parent,
            )
            reservation_filesystems = {
                path.stat().st_dev: path for path in affected_directories
            }
            if any(
                shutil.disk_usage(path).free < MINIMUM_FREE_AFTER_RESERVATION_BYTES
                for path in reservation_filesystems.values()
            ):
                raise ActivationError(
                    "current-schema migration requires 20 GiB free after backup and restore reservations"
                )
    except BaseException as exc:
        try:
            _remove_capacity_files(*_owned_capacity_cleanup_paths(manifest, reservation))
        except DatabaseCapacityCleanupFailure as cleanup_exc:
            raise cleanup_exc from exc
        raise
    try:
        if manifest.current_schema_upgrade is not None:
            _require_current_schema_free_space(manifest, reservation)
        _write_capacity_observation(manifest, reservation)
    except BaseException as exc:
        try:
            _remove_capacity_files(*_owned_capacity_cleanup_paths(manifest, reservation))
        except DatabaseCapacityCleanupFailure as cleanup_exc:
            raise cleanup_exc from exc
        raise ActivationError("could not persist paired capacity observation") from exc
    return reservation


def _reservation_still_sufficient(manifest: Manifest, reservation: DatabaseCapacityReservation) -> None:
    database, _backup, _proof, _transaction = _paired_path_context(manifest)
    required = _source_capacity_bytes(database)
    rechecked_at = utc_now()
    if manifest.current_schema_upgrade is not None:
        _require_current_schema_free_space(manifest, reservation)
    _write_capacity_observation(
        manifest,
        reservation,
        rechecked_source_bytes=required,
        rechecked_at=rechecked_at,
    )
    if manifest.current_schema_upgrade is not None:
        _require_current_schema_free_space(manifest, reservation)
    for path in (reservation.backup_path, reservation.restore_path):
        if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o777 != 0o600:
            raise ActivationError("paired database capacity reservation is unavailable")
        if _protected_129(manifest) and path == reservation.backup_path and Path(manifest.current_schema_upgrade.proof_path).exists():
            _snapshot_proof(manifest, require_rehearsal=False)
            continue
        if path.stat().st_size < required or path.stat().st_blocks * 512 < required:
            raise ActivationError("paired database grew beyond its reserved capacity")
    if _protected_129(manifest):
        restore = reservation.restore_path
        if restore.stat().st_size < reservation.capacity_bytes or restore.stat().st_blocks * 512 < reservation.capacity_bytes:
            raise ActivationError("physical restore reservation is not fully held")
        _require_current_schema_free_space(manifest, reservation)


def create_database_backup(manifest: Manifest, reservation: Optional[DatabaseCapacityReservation] = None) -> None:
    import sqlite3
    """Take a SQLite backup API snapshot into the held destination."""
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
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
        if (
            manifest.current_schema_upgrade is not None
            and database_logical_digest(source) != database_logical_digest(backup)
        ):
            raise ActivationError("current-schema snapshot contents differ from stopped source")
        proof_bytes = _database_proof_bytes(
            manifest,
            source,
            backup_sha256=observed,
            source_logical_digest=(
                database_logical_digest(source)
                if manifest.current_schema_upgrade is not None
                else ""
            ),
            backup_logical_digest=(
                database_logical_digest(backup)
                if manifest.current_schema_upgrade is not None
                else ""
            ),
        )
        if manifest.current_schema_upgrade is not None:
            _require_current_schema_free_space(manifest, reservation)
        atomic_write(proof_path, proof_bytes, 0o600)
    except (sqlite3.Error, OSError, ValueError) as exc:
        raise ActivationError(f"database snapshot failed: {exc}") from exc


def restore_database(manifest: Manifest) -> None:
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
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
            "database": upgrade.database_path,
            "candidate_binary_sha256": manifest.candidate_binary_sha256,
            "candidate_plist_sha256": manifest.candidate_plist_sha256,
            "previous_binary_sha256": manifest.rollback_binary_sha256,
            "previous_plist_sha256": manifest.rollback_plist_sha256,
            "controller_source_commit": upgrade.controller_source_commit,
            "controller_helper_sha256": upgrade.controller_helper_sha256,
            "database_mode": "current_schema" if manifest.current_schema_upgrade is not None else "legacy_paired",
            **({
                "migration_registry_sha256": manifest.current_schema_upgrade.migration_registry_sha256,
                "prepared_source_qualification": prepared_source_qualification(manifest),
            } if manifest.current_schema_upgrade is not None else {}),
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
    if manifest.current_schema_upgrade is not None:
        registry_path = Path(manifest.current_schema_upgrade.migration_registry_path)
        validate_current_schema_database(backup, registry_path, source_kind=manifest.source_kind, manifest=manifest)
        if database_logical_digest(backup) != proof.get("source_logical_digest"):
            raise ActivationError("current-schema backup contents differ from snapshot proof")
        backup_ledger = validate_current_schema_database(backup, registry_path, source_kind=manifest.source_kind, manifest=manifest)
        if [list(item) for item in backup_ledger] != proof.get("source_ledger"):
            raise ActivationError("current-schema backup ledger differs from snapshot proof")
        if [list(item) for item in current_schema_target_registry(manifest, registry_path)] != proof.get("target_ledger"):
            raise ActivationError("current-schema target ledger differs from snapshot proof")
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
    if manifest.current_schema_upgrade is not None:
        validate_current_schema_database(
            database, Path(manifest.current_schema_upgrade.migration_registry_path), source_kind=manifest.source_kind, manifest=manifest
        )
        if database_logical_digest(database) != proof.get("source_logical_digest"):
            raise ActivationError("restored current-schema database contents differ from proof")


def service_absence_confirmed(output: str, label: str, uid: int) -> bool:
    return re.search(r'^\s*Could not find service "' + re.escape(label) + r'" in domain (?:gui/' + str(uid) + r'|for user gui: ' + str(uid) + r')\s*$', output, re.MULTILINE) is not None


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
        if service_absence_confirmed(output, self.manifest.label, self.manifest.uid):
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
        result = self.run(["launchctl", "bootout", self.target], capture_output=True, text=True)
        if result.returncode != 0:
            raise ActivationError(f"launchctl bootout failed with exit {result.returncode}")
        self.disruption_started = True
        self.wait(lambda state, pid: state == "not_loaded" and pid is None, time.monotonic() + self.manifest.transition_timeout_secs, "teardown")
        return old_pid

    def start(self, old_pid: Optional[int], *, plist_path: Optional[str] = None) -> int:
        bootstrap_path = plist_path or self.manifest.target_plist
        result = self.run(["launchctl", "bootstrap", self.domain, bootstrap_path], capture_output=True, text=True)
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


def verify_source128_unchanged_without_snapshot(manifest: Manifest) -> None:
    current = manifest.current_schema_upgrade
    if not _protected_129(manifest):
        return
    source = Path(current.database_path)
    reservation = DatabaseCapacityReservation(
        Path(current.backup_path), _restore_capacity_path(manifest, source), 0,
    )
    evidence = _source128_capacity_source(manifest, reservation)
    ledger = validate_current_schema_database(
        source, _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest,
    )
    if (_ledger_sha256(ledger) != evidence["pre_stop_source_ledger_sha256"]
            or database_logical_digest(source) != evidence["pre_stop_logical_digest"]):
        raise ActivationError("source128 live source differs from pre-stop capacity evidence")


def restore(
    manifest: Manifest,
    launchctl: Launchctl,
    prepared_rollback: Optional[tuple[Path, Path]] = None,
    *,
    database_snapshot: bool = True,
    candidate_binary_mutated: bool = True,
    rollback_plist: Optional[Path] = None,
) -> None:
    upgrade = prepared_database_upgrade(manifest)
    if _schema129_refresh(manifest) and (_post_refresh_marker(manifest) or _refresh_start_proof(manifest)):
        raise ActivationError("post-marker refresh cannot restore predecessor or replay backup")
    try:
        launchctl.stop()
        if upgrade is not None:
            state, pid = launchctl.inspect()
            if state != "not_loaded" or pid is not None:
                raise ActivationError("candidate remains loaded after stop")
    except ActivationError:
        if upgrade is not None:
            raise
    if upgrade is not None:
        quarantined = quarantine_paired_plist(manifest)
        if rollback_plist == Path(manifest.target_plist):
            rollback_plist = quarantined
    if upgrade is not None and database_snapshot:
        assert_database_exclusive(manifest)
        if (
            manifest.current_schema_upgrade is not None
            and not current_schema_database_is_uncommitted(manifest)
        ):
            raise ActivationError(
                "current-schema database changed before exclusive snapshot replay"
            )
        if not _protected_129(manifest):
            restore_database(manifest)
    elif upgrade is not None:
        assert_database_exclusive(manifest)
        if manifest.paired_database_upgrade is not None:
            validate_legacy_database(Path(manifest.paired_database_upgrade.database_path))
        else:
            validate_current_schema_database(
                Path(upgrade.database_path), _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest
            )
        verify_source128_unchanged_without_snapshot(manifest)
        if sha256(Path(manifest.target_binary)) != manifest.rollback_binary_sha256:
            raise ActivationError("unchanged predecessor binary proof failed")
        captured = captured_paired_plist(manifest)
        if captured is None:
            raise ActivationError("unchanged predecessor configuration proof failed")
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

    if candidate_binary_mutated:
        if prepared_rollback is None:
            rollback_binary = verify_staged(manifest.rollback_binary, manifest.rollback_binary_sha256, "rollback binary")
            atomic_install(rollback_binary, Path(manifest.target_binary), 0o755)
        else:
            prepared_binary, _prepared_plist = prepared_rollback
            commit_atomic_install(prepared_binary, Path(manifest.target_binary))

    if upgrade is not None:
        rollback_plist = rollback_plist or captured_paired_plist(manifest)
        if rollback_plist is None:
            raise ActivationError("captured predecessor configuration is unverified")
        verify_staged(str(rollback_plist), manifest.rollback_plist_sha256, "captured rollback plist")
        rollback_plist = verify_staged(manifest.rollback_plist, manifest.rollback_plist_sha256, "rollback plist")
        publication = (
            prepared_rollback[1] if prepared_rollback is not None
            else prepare_plist_publication(rollback_plist, Path(manifest.target_plist))
        )
        try:
            existing = json.loads(Path(manifest.status_path).read_text())
        except (OSError, json.JSONDecodeError):
            existing = {}
        recovery_mode = "snapshot_restored" if database_snapshot and not _protected_129(manifest) else "unchanged_predecessor"
        write_status(manifest, "activation_failed_rollback_failed", failure=existing.get("failure"), rollback_failure=existing.get("rollback_failure"), recovery_mode=recovery_mode)
        old_pid = launchctl.inspect()[1]
        launchctl.start(old_pid, plist_path=str(rollback_plist))
    else:
        if prepared_rollback is None:
            rollback_plist = verify_staged(manifest.rollback_plist, manifest.rollback_plist_sha256, "rollback plist")
            atomic_install(rollback_plist, Path(manifest.target_plist), 0o600)
        else:
            _prepared_binary, prepared_plist = prepared_rollback
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
    try:
        restore_deployed_sha(manifest)
        if upgrade is not None:
            assert rollback_plist is not None
            commit_atomic_install(publication, Path(manifest.target_plist))
    except Exception as exc:
        if upgrade is not None:
            raise VerifiedPredecessorFinalizationError(str(exc)) from exc
        raise


def write_verified_predecessor_status(manifest: Manifest, **diagnostics) -> None:
    try:
        write_status(manifest, "activation_failed_rolled_back", **diagnostics)
    except Exception as exc:
        if prepared_database_upgrade(manifest) is not None:
            raise VerifiedPredecessorFinalizationError(str(exc)) from exc
        raise


def finalize_verified_predecessor(manifest: Manifest, failure: str | None) -> None:
    if prepared_database_upgrade(manifest) is not None:
        try:
            checkpoint = json.loads(Path(manifest.status_path).read_text())
            if not isinstance(checkpoint, dict) or checkpoint.get("transaction_id") != manifest.transaction_id or checkpoint.get("recovery_mode") not in {"unchanged_predecessor", "snapshot_restored"}:
                raise ActivationError("verified predecessor recovery checkpoint is unavailable")
            source128 = _protected_129(manifest)
            if source128 and Path(manifest.current_schema_upgrade.proof_path).exists():
                _snapshot_proof(manifest, require_rehearsal=False)
                release_capacity_reservation(
                    manifest, DatabaseCapacityReservation(
                        Path(manifest.current_schema_upgrade.backup_path),
                        _restore_capacity_path(manifest), 0,
                    ),
                )
            elif checkpoint["recovery_mode"] == "unchanged_predecessor":
                release_unsnapshotted_capacity(manifest)
        except Exception as exc:
            raise VerifiedPredecessorFinalizationError(f"verified predecessor capacity finalization failed: {exc}") from exc
    write_verified_predecessor_status(manifest, failure=failure)


def release_durable_terminal_claim(
    manifest: Manifest, *, required_state: Optional[str] = None
) -> bool:
    if not status_is_durable_terminal(manifest):
        return False
    status = read_status(manifest)
    if required_state is not None and status.get("state") != required_state:
        return False
    fsync_dir(Path(manifest.status_path).parent)
    return release_claim(manifest)


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
            try:
                fsync_dir(claim.parent)
            except OSError:
                try:
                    fd = os.open(claim, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                except FileExistsError:
                    pass
                else:
                    try:
                        os.write(fd, f"{manifest.transaction_id}\n".encode())
                        os.fsync(fd)
                    finally:
                        os.close(fd)
                    fsync_dir(claim.parent)
                raise
            return True
        except FileNotFoundError:
            return False


def request_helper_bootout(uid: int, helper_label: str) -> None:
    if not helper_label.startswith(f"{DEPLOY_HELPER_LABEL_PREFIX}."):
        raise ActivationError("refusing to boot out an unverified helper label")
    subprocess.Popen(
        ["launchctl", "bootout", f"gui/{uid}/{helper_label}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
    )


def manifest_prepared_mode(manifest: Manifest) -> Optional[str]:
    if manifest.current_schema_upgrade is not None:
        return "current_schema"
    if manifest.paired_database_upgrade is not None:
        return "paired"
    return None


def validate_status_manifest_mode(manifest: Manifest, status: dict) -> None:
    if (
        status.get("transaction_id") != manifest.transaction_id
        or status.get("source_kind") != manifest.source_kind
        or status.get("prepared_mode") != manifest_prepared_mode(manifest)
    ):
        raise RecoveryModeMismatch("status source kind or prepared mode does not match retained manifest")


def prepared_source_qualification(manifest: Manifest) -> Optional[dict]:
    current = manifest.current_schema_upgrade
    if isinstance(current, PreparedSource114Upgrade):
        return {
            "prepared_source_baseline": current.prepared_source_baseline,
            "candidate_registry_expected_sha256": current.candidate_registry_expected_sha256,
        }
    if isinstance(current, (PreparedSource128Upgrade, PreparedSchema129Refresh)):
        return {
            "prepared_source_baseline": current.prepared_source_baseline,
            "candidate_registry_expected_sha256": current.candidate_registry_expected_sha256,
            **({"receipt_sha256": current.receipt_sha256} if isinstance(current, PreparedSchema129Refresh) else {}),
        }
    return None


def validate_prepared_source128(manifest: Manifest) -> None:
    current = manifest.current_schema_upgrade
    if not isinstance(current, PreparedSource128Upgrade):
        return
    if (
        manifest.source_kind != "prepared_artifact"
        or manifest.source_commit != PREPARED_SOURCE128_COMMIT
        or manifest.expected != Identity(PREPARED_SOURCE128_VERSION, PREPARED_SOURCE128_COMMIT)
        or manifest.release_tag is not None or manifest.release_commit is not None
        or manifest.paired_database_upgrade is not None or manifest.ordinary_forward_migration is not None
        or not FULL_GIT_SHA_RE.fullmatch(current.controller_source_commit)
        or current.controller_source_commit == PREPARED_SOURCE128_COMMIT
        or type(current.prepared_source_baseline) is not int or current.prepared_source_baseline != 128
        or current.candidate_registry_expected_sha256 != PREPARED_SOURCE128_REGISTRY_SHA256
        or current.migration_registry_sha256 != PREPARED_SOURCE128_REGISTRY_SHA256
    ):
        raise ActivationError("invalid exact prepared source128 qualification")
    registry = verify_staged(current.migration_registry_path, PREPARED_SOURCE128_REGISTRY_SHA256, "prepared source128 candidate registry")
    migration_registry(registry, qualified_source128=True)


def validate_prepared_schema129_refresh(manifest: Manifest) -> None:
    current = manifest.current_schema_upgrade
    if not isinstance(current, PreparedSchema129Refresh):
        return
    if (
        manifest.source_kind != "prepared_artifact"
        or manifest.paired_database_upgrade is not None
        or manifest.ordinary_forward_migration is not None
        or manifest.release_tag is not None or manifest.release_commit is not None
        or not FULL_GIT_SHA_RE.fullmatch(manifest.source_commit)
        or manifest.expected.git_sha != manifest.source_commit
        or not VERSION_RE.fullmatch(manifest.expected.version)
        or manifest.previous is None or manifest.previous.git_sha == manifest.source_commit
        or not FULL_GIT_SHA_RE.fullmatch(current.controller_source_commit)
        or current.controller_source_commit == manifest.source_commit
        or type(current.prepared_source_baseline) is not int or current.prepared_source_baseline != 129
        or current.candidate_registry_expected_sha256 != PREPARED_SCHEMA129_REGISTRY_SHA256
        or current.migration_registry_sha256 != PREPARED_SCHEMA129_REGISTRY_SHA256
        or not re.fullmatch(r"[0-9a-f]{64}", current.receipt_sha256)
    ):
        raise ActivationError("invalid exact prepared schema129 refresh qualification")
    registry = verify_staged(current.migration_registry_path, PREPARED_SCHEMA129_REGISTRY_SHA256, "refresh candidate registry")
    if migration_registry(registry, qualified_source128=True)[-1] != PREPARED_SOURCE128_TARGET:
        raise ActivationError("refresh registry target differs from exact migration129")
    receipt_path = Path(current.receipt_path)
    if receipt_path != Path(current.proof_path).parent / "receipt.json":
        raise ActivationError("refresh receipt must be retained at the private transaction root")
    _private_transaction_dir(receipt_path.parent)
    _repair_private(receipt_path)
    verify_staged(str(receipt_path), current.receipt_sha256, "retained refresh receipt")
    receipt = _repair_json(receipt_path)
    checks = receipt.get("checks")
    target = receipt.get("target")
    digests = receipt.get("sha256")
    expected_checks = {
        "developer_id_signature": "verified", "hardened_runtime": "verified",
        "notarization": "accepted", "stapled_ticket": "validated",
        "gatekeeper": "accepted", "embedded_helper_bytes": "identical",
    }
    submission = checks.get("notarization_submission_id") if isinstance(checks, dict) else None
    binary_name = f"phoenix_ide-{target}-prepared-{manifest.source_commit[:12]}"
    if (
        type(receipt.get("schema")) is not int or receipt["schema"] != 1
        or receipt.get("operation") != "prepare-main"
        or receipt.get("commit") != manifest.source_commit
        or receipt.get("version") != manifest.expected.version
        or not isinstance(target, str) or target not in {"aarch64-apple-darwin", "x86_64-apple-darwin"}
        or not isinstance(checks, dict)
        or any(checks.get(key) != value for key, value in expected_checks.items())
        or not isinstance(submission, str)
        or not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", submission)
        or not isinstance(digests, dict)
        or digests.get(binary_name) != manifest.candidate_binary_sha256
    ):
        raise ActivationError("retained refresh receipt identity/signature/checksum mismatch")
    verify_staged(manifest.candidate_binary, manifest.candidate_binary_sha256, "receipt-bound refresh candidate")


def validate_prepared_source_qualification(manifest: Manifest) -> None:
    validate_prepared_source114(manifest)
    validate_prepared_source128(manifest)
    validate_prepared_schema129_refresh(manifest)


def current_schema_target_registry(manifest: Manifest, path: Path) -> tuple[tuple[int, str], ...]:
    return migration_registry(path, qualified_source128=_protected_129(manifest))


def validate_prepared_source114(manifest: Manifest) -> None:
    current = manifest.current_schema_upgrade
    if not isinstance(current, PreparedSource114Upgrade):
        return
    if (
        manifest.source_kind != "prepared_artifact"
        or manifest.source_commit != PREPARED_SOURCE114_COMMIT
        or manifest.expected.git_sha != PREPARED_SOURCE114_COMMIT
        or manifest.release_tag is not None or manifest.release_commit is not None
        or manifest.paired_database_upgrade is not None or manifest.ordinary_forward_migration is not None
        or not FULL_GIT_SHA_RE.fullmatch(current.controller_source_commit)
        or current.controller_source_commit == PREPARED_SOURCE114_COMMIT
        or type(current.prepared_source_baseline) is not int or current.prepared_source_baseline != 114
        or current.candidate_registry_expected_sha256 != PREPARED_SOURCE114_REGISTRY_SHA256
        or current.migration_registry_sha256 != PREPARED_SOURCE114_REGISTRY_SHA256
    ):
        raise ActivationError("invalid exact prepared source114 qualification")
    registry = verify_staged(current.migration_registry_path, PREPARED_SOURCE114_REGISTRY_SHA256, "prepared source114 candidate registry")
    migration_registry(registry)


def validate_manifest_mode(
    manifest: Manifest,
    *,
    prepare_transaction: bool = True,
) -> None:
    validate_prepared_source_qualification(manifest)
    upgrade = prepared_database_upgrade(manifest)
    if manifest.source_kind not in {
        "prepared_artifact",
        "local_head",
        "published_release",
    }:
        raise ActivationError("manifest source kind is unsupported")
    if manifest.source_kind == "prepared_artifact" and upgrade is None:
        raise ActivationError("prepared artifact requires an explicit database upgrade mode")
    ordinary = manifest.ordinary_forward_migration
    if ordinary is not None:
        if (
            upgrade is not None
            or manifest.source_kind not in {"local_head", "published_release"}
        ):
            raise ActivationError("ordinary forward policy cannot be combined with prepared mode")
        for raw_path in (
            ordinary.database_path,
            ordinary.migration_registry_path,
            ordinary.helper_path,
        ):
            if not Path(raw_path).is_absolute() or Path(raw_path).is_symlink():
                raise ActivationError("ordinary forward policy paths must be absolute non-symlinks")
        helper = verify_staged(
            ordinary.helper_path,
            ordinary.helper_sha256,
            "ordinary retained helper",
        )
        registry = verify_staged(
            ordinary.migration_registry_path,
            ordinary.migration_registry_sha256,
            "ordinary candidate migration registry",
        )
        if registry.parent != Path(manifest.candidate_binary).parent or helper.parent != registry.parent:
            raise ActivationError("ordinary forward evidence must belong to the helper transaction")
        migration_registry(registry, qualified_current_target=False)
        if _plist_database_path(Path(manifest.candidate_plist)) != ordinary.database_path:
            raise ActivationError("ordinary migration database path differs from candidate configuration")
    if upgrade is None:
        return
    if manifest.source_kind != "prepared_artifact":
        if manifest.current_schema_upgrade is None or manifest.source_kind != "published_release":
            raise ActivationError("prepared database mode requires a prepared artifact candidate")
        if (
            not manifest.release_tag
            or not FULL_GIT_SHA_RE.fullmatch(manifest.source_commit)
            or manifest.release_commit != manifest.source_commit
            or manifest.expected.git_sha != manifest.source_commit
            or upgrade.controller_source_commit == manifest.source_commit
        ):
            raise ActivationError("protected release requires exact release candidate source binding")
    if not FULL_GIT_SHA_RE.fullmatch(upgrade.controller_source_commit):
        raise ActivationError("prepared mode requires exact controller source binding")
    if not upgrade.controller_helper_sha256 or not upgrade.controller_helper_path:
        raise ActivationError("prepared mode requires helper equivalence binding")
    mode_paths = [
        upgrade.database_path,
        upgrade.backup_path,
        upgrade.proof_path,
        upgrade.controller_helper_path,
    ]
    if manifest.current_schema_upgrade is not None:
        mode_paths.append(manifest.current_schema_upgrade.migration_registry_path)
    for raw_path in mode_paths:
        if not Path(raw_path).is_absolute() or Path(raw_path).is_symlink():
            raise ActivationError("prepared mode paths must be absolute non-symlinks")
    helper = Path(upgrade.controller_helper_path)
    running_helper = Path(__file__).resolve()
    if helper.resolve() != running_helper:
        raise ActivationError("prepared mode helper path is not the running helper")
    if not helper.is_file() or sha256(helper) != upgrade.controller_helper_sha256:
        raise ActivationError("controller helper bytes changed")
    _database, _backup, _proof, transaction_dir = _paired_path_context(
        manifest,
        prepare_transaction=prepare_transaction,
    )
    if transaction_dir != helper.parent:
        raise ActivationError("prepared snapshot must belong to the helper transaction")
    if manifest.previous is None or manifest.rollback_binary_sha256 is None or manifest.rollback_plist_sha256 is None:
        raise ActivationError("prepared mode requires a predecessor")
    database = str(Path(upgrade.database_path))
    if _plist_database_path(Path(manifest.candidate_plist)) != database or _plist_database_path(Path(manifest.rollback_plist or "")) != database:
        raise ActivationError("database path differs between candidate, predecessor, and manifest")
    if manifest.current_schema_upgrade is not None:
        registry = verify_staged(
            manifest.current_schema_upgrade.migration_registry_path,
            manifest.current_schema_upgrade.migration_registry_sha256,
            "candidate migration registry",
        )
        if registry.parent != transaction_dir:
            raise ActivationError("migration registry must belong to the helper transaction")
        current_schema_target_registry(manifest, registry)


def _current_schema_registry(manifest: Manifest) -> Path:
    validate_prepared_source_qualification(manifest)
    current = manifest.current_schema_upgrade
    if current is None:
        raise ActivationError("current-schema upgrade is not configured")
    return verify_staged(
        current.migration_registry_path,
        current.migration_registry_sha256,
        "candidate migration registry",
    )


def _snapshot_proof(manifest: Manifest, *, require_rehearsal: bool = True) -> dict:
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
        raise ActivationError("prepared database upgrade is not configured")
    path = Path(upgrade.proof_path)
    _regular_nosymlink(path, "database proof")
    if path.stat().st_mode & 0o777 != 0o600:
        raise ActivationError("database proof must be private")
    try:
        proof = json.loads(
            path.read_text(),
            object_pairs_hook=_source128_proof_pairs if _protected_129(manifest) else None,
        )
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        raise ActivationError("database snapshot proof is unreadable") from exc
    if _protected_129(manifest):
        if not isinstance(proof, dict):
            raise ActivationError("protected database proof is not an object")
        _source128_proof_shape(proof)
        if _schema129_refresh(manifest) and set(proof) not in (
            SOURCE128_PROOF_BASE_KEYS,
            SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS,
            SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS | SOURCE128_START_KEYS,
        ):
            raise ActivationError("refresh proof cannot contain migration commit fields")
        if not _schema129_refresh(manifest) and set(proof) == SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS | SOURCE128_START_KEYS:
            raise ActivationError("source128 start requires migration commit fields")
    expected = {
        "transaction_id": manifest.transaction_id,
        "source_commit": manifest.source_commit,
        "database": upgrade.database_path,
        "candidate_binary_sha256": manifest.candidate_binary_sha256,
        "candidate_plist_sha256": manifest.candidate_plist_sha256,
        "previous_binary_sha256": manifest.rollback_binary_sha256,
        "previous_plist_sha256": manifest.rollback_plist_sha256,
        "controller_source_commit": upgrade.controller_source_commit,
        "controller_helper_sha256": upgrade.controller_helper_sha256,
        "database_mode": "current_schema" if manifest.current_schema_upgrade is not None else "legacy_paired",
    }
    if manifest.current_schema_upgrade is not None:
        registry_path = _current_schema_registry(manifest)
        registry = current_schema_target_registry(manifest, registry_path)
        expected["migration_registry_sha256"] = manifest.current_schema_upgrade.migration_registry_sha256
        expected["prepared_source_qualification"] = prepared_source_qualification(manifest)
        expected["target_ledger"] = [list(item) for item in registry]
    if not isinstance(proof, dict) or any(proof.get(key) != value for key, value in expected.items()):
        raise ActivationError("database snapshot proof context mismatch")
    backup = Path(upgrade.backup_path)
    _regular_nosymlink(backup, "database backup")
    if sha256(backup) != proof.get("sha256"):
        raise ActivationError("database backup checksum mismatch")
    if manifest.current_schema_upgrade is not None:
        source_ledger = validate_current_schema_database(backup, registry_path, source_kind=manifest.source_kind, manifest=manifest)
        if proof.get("source_ledger") != [list(item) for item in source_ledger]:
            raise ActivationError("database snapshot proof context mismatch")
        if database_logical_digest(backup) != proof.get("source_logical_digest"):
            raise ActivationError("database backup contents differ from snapshot proof")
        if _protected_129(manifest) and require_rehearsal:
            if (
                proof.get("rehearsal_ledger") != expected["target_ledger"]
                or proof.get("rehearsal_backup_sha256") != proof.get("sha256")
                or not isinstance(proof.get("rehearsed_at"), str)
                or not isinstance(proof.get("rehearsal_logical_digest"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", proof["rehearsal_logical_digest"])
            ):
                raise ActivationError("exact source128 rehearsal proof is unavailable")
    return proof


def current_schema_database_is_uncommitted(manifest: Manifest) -> bool:
    current = manifest.current_schema_upgrade
    if current is None:
        raise ActivationError("current-schema upgrade is not configured")
    proof = _snapshot_proof(manifest, require_rehearsal=False)
    database = Path(current.database_path)
    try:
        ledger = validate_current_schema_database(database, _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest)
        return (
            [list(item) for item in ledger] == proof.get("source_ledger")
            and database_logical_digest(database) == proof.get("source_logical_digest")
        )
    except ActivationError:
        return False


def run_current_schema_migration(
    manifest: Manifest,
    reservation: DatabaseCapacityReservation,
) -> None:
    current = manifest.current_schema_upgrade
    if current is None:
        return
    if isinstance(current, PreparedSchema129Refresh):
        raise ActivationError("live schema129 refresh must never invoke migrate-only")
    database = Path(current.database_path)
    registry = _current_schema_registry(manifest)
    proof = _snapshot_proof(manifest)
    if not current_schema_database_is_uncommitted(manifest):
        raise ActivationError("current-schema database changed before offline migration")
    assert_database_exclusive(manifest)
    invoke_current_schema_candidate(manifest, database, Path(manifest.target_binary))
    assert_database_exclusive(manifest)
    migrated_ledger = validate_current_schema_database(database, registry, source_kind=manifest.source_kind, manifest=manifest, migrated=True)
    migrated_size = _source_capacity_bytes(database)
    if migrated_size > reservation.capacity_bytes:
        raise ActivationError(
            "migrated database grew beyond its reserved capacity after commit"
        )
    _require_current_schema_free_space(manifest, reservation)
    proof["migration_committed_at"] = utc_now()
    proof["migrated_ledger"] = migrated_ledger
    proof["migrated_logical_digest"] = database_logical_digest(database)
    proof_path = Path(current.proof_path)
    encoded = _encode_database_proof(proof)
    allocation_unit = max(os.statvfs(proof_path.parent).f_frsize, 4096)
    _validate_database_proof_lifecycle_budget(
        b"0" * proof_path.stat().st_size,
        encoded,
        allocation_unit,
    )
    _require_current_schema_free_space(manifest, reservation)
    atomic_write(proof_path, encoded, 0o600)


def rehearse_prepared_source128(manifest: Manifest, reservation: DatabaseCapacityReservation) -> None:
    import sqlite3

    current = manifest.current_schema_upgrade
    if not _protected_129(manifest):
        raise ActivationError("exact protected rehearsal requires typed qualification")
    assert_database_exclusive(manifest)
    proof = _snapshot_proof(manifest, require_rehearsal=False)
    if "rehearsal_ledger" in proof or "migration_committed_at" in proof:
        raise ActivationError("exact source128 rehearsal has already been attempted")
    backup = Path(current.backup_path)
    restore = reservation.restore_path
    if restore != _restore_capacity_path(manifest, Path(current.database_path)) or restore.is_symlink():
        raise ActivationError("rehearsal slot does not belong to this transaction")
    _reservation_still_sufficient(manifest, reservation)
    try:
        with closing(_readonly_connection(backup)) as source, closing(sqlite3.connect(restore)) as copy:
            source.backup(copy)
        validate_current_schema_database(restore, _current_schema_registry(manifest), manifest=manifest)
        if database_logical_digest(restore) != proof["source_logical_digest"]:
            raise ActivationError("rehearsal copy differs from retained backup")
        candidate = verify_staged(manifest.candidate_binary, manifest.candidate_binary_sha256, "rehearsal candidate binary")
        invoke_current_schema_candidate(manifest, restore, candidate)
        assert_database_exclusive(manifest)
        ledger = validate_current_schema_database(
            restore, _current_schema_registry(manifest), manifest=manifest, migrated=True,
        )
        if _source_capacity_bytes(restore) > reservation.capacity_bytes:
            raise ActivationError("rehearsal grew beyond held restore capacity")
        _require_current_schema_free_space(manifest, reservation)
        if sha256(backup) != proof["sha256"]:
            raise ActivationError("retained backup changed during rehearsal")
        rehearsal_logical_digest = database_logical_digest(restore)
        if (_schema129_refresh(manifest)
                and rehearsal_logical_digest != proof["source_logical_digest"]):
            raise ActivationError("schema129 refresh rehearsal changed logical contents")
        proof.update(
            rehearsal_ledger=ledger,
            rehearsal_backup_sha256=proof["sha256"],
            rehearsal_logical_digest=rehearsal_logical_digest,
            rehearsed_at=utc_now(),
        )
        proof_path = Path(current.proof_path)
        encoded = _encode_database_proof(proof)
        _validate_database_proof_lifecycle_budget(
            b"0" * proof_path.stat().st_size,
            _project_migrated_database_proof(encoded),
            max(os.statvfs(proof_path.parent).f_frsize, 4096),
        )
        _require_current_schema_free_space(manifest, reservation)
        atomic_write(proof_path, encoded, 0o600)
    except sqlite3.Error as exc:
        raise ActivationError("native SQLite rehearsal failed") from exc
    finally:
        for suffix in ("-journal", "-wal", "-shm"):
            sidecar = restore.with_name(restore.name + suffix)
            if sidecar.is_symlink():
                raise ActivationError("rehearsal SQLite sidecar is a symlink")
            sidecar.unlink(missing_ok=True)
        _regular_nosymlink(restore, "rehearsal restore reservation")
        restore.unlink()
        fsync_dir(restore.parent)
        _allocate_private_sqlite(restore, reservation.capacity_bytes)
    _reservation_still_sufficient(manifest, reservation)
    _snapshot_proof(manifest)
    if database_logical_digest(Path(current.database_path)) != proof["source_logical_digest"]:
        raise ActivationError("live source changed during rehearsal")
    assert_database_exclusive(manifest)


def invoke_current_schema_candidate(manifest: Manifest, database: Path, binary: Path) -> None:
    current = manifest.current_schema_upgrade
    if current is None:
        raise ActivationError("current-schema upgrade is not configured")
    try:
        with Path(manifest.candidate_plist).open("rb") as stream:
            plist = plistlib.load(stream)
        plist_environment = plist.get("EnvironmentVariables", {})
    except (OSError, plistlib.InvalidFileException, ValueError) as exc:
        raise ActivationError("candidate launchd environment is unreadable") from exc
    if not isinstance(plist_environment, dict):
        raise ActivationError("candidate launchd environment is malformed")
    environment = os.environ.copy()
    environment.update({str(key): str(value) for key, value in plist_environment.items()})
    if environment.get("PHOENIX_DB_PATH") != current.database_path:
        raise ActivationError("candidate migration database path differs from manifest")
    environment["PHOENIX_DB_PATH"] = str(database)
    try:
        result = subprocess.run(
            [str(binary), "--migrate-only"],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=7200,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise ActivationError("candidate offline migration could not complete") from exc
    if result.returncode != 0:
        raise ActivationError(
            f"candidate offline migration failed with exit {result.returncode}"
        )


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


def _load_staged_plist(path: Path, description: str) -> dict:
    try:
        with path.open("rb") as stream:
            plist = plistlib.load(stream)
    except (OSError, plistlib.InvalidFileException, ValueError) as exc:
        raise ActivationError(f"{description} is unreadable") from exc
    if not isinstance(plist, dict):
        raise ActivationError(f"{description} is not a dictionary")
    return plist


def _validate_staged_plist_binding(
    plist: dict,
    *,
    manifest_label: str,
    target_binary: Path,
    description: str,
) -> None:
    if not manifest_label.strip() or plist.get("Label") != manifest_label:
        raise ActivationError(f"{description} label does not match the immutable manifest")
    if "Program" in plist or plist.get("ProgramArguments") != [str(target_binary)]:
        raise ActivationError(f"{description} program does not match the immutable target")


def expected_helper_label(manifest: Manifest) -> str:
    return f"{DEPLOY_HELPER_LABEL_PREFIX}.{manifest.transaction_id}"


def validate_pre_disruption_handoff(manifest: Manifest) -> ValidatedLaunchdHandoff:
    """Validate retained installer evidence without preparing or invoking mutation."""
    exact_prepared_source114 = isinstance(
        manifest.current_schema_upgrade, PreparedSource114Upgrade
    )
    exact_prepared_source128 = _protected_129(manifest)
    if exact_prepared_source114 or exact_prepared_source128:
        qualification = "source114" if exact_prepared_source114 else "schema129" if _schema129_refresh(manifest) else "source128"
        if manifest.transition_timeout_secs != DEFAULT_TRANSITION_TIMEOUT_SECS:
            raise ActivationError(
                f"exact prepared {qualification} transition_timeout_secs must be {DEFAULT_TRANSITION_TIMEOUT_SECS}"
            )
        if manifest.health_timeout_secs != DEFAULT_HEALTH_TIMEOUT_SECS:
            raise ActivationError(
                f"exact prepared {qualification} health_timeout_secs must be {DEFAULT_HEALTH_TIMEOUT_SECS}"
            )
    validate_manifest_identities(manifest)
    validate_manifest_mode(manifest, prepare_transaction=False)
    if manifest.helper_label != expected_helper_label(manifest):
        raise ActivationError("immutable manifest has an invalid helper label")
    candidate_binary = verify_staged(
        manifest.candidate_binary,
        manifest.candidate_binary_sha256,
        "candidate binary",
    )
    candidate_plist = verify_staged(
        manifest.candidate_plist,
        manifest.candidate_plist_sha256,
        "candidate plist",
    )
    candidate_plist_contents = _load_staged_plist(candidate_plist, "candidate plist")
    if exact_prepared_source114 or exact_prepared_source128:
        _validate_staged_plist_binding(
            candidate_plist_contents,
            manifest_label=manifest.label,
            target_binary=Path(manifest.target_binary),
            description="candidate plist",
        )
    rollback: Optional[tuple[Path, Path]] = None
    if manifest.previous is not None:
        rollback_binary = verify_staged(
            manifest.rollback_binary,
            manifest.rollback_binary_sha256,
            "rollback binary",
        )
        rollback_plist = verify_staged(
            manifest.rollback_plist,
            manifest.rollback_plist_sha256,
            "rollback plist",
        )
        rollback_plist_contents = _load_staged_plist(rollback_plist, "rollback plist")
        if exact_prepared_source114 or exact_prepared_source128:
            _validate_staged_plist_binding(
                rollback_plist_contents,
                manifest_label=manifest.label,
                target_binary=Path(manifest.target_binary),
                description="rollback plist",
            )
        rollback = (rollback_binary, rollback_plist)
    elif any(
        (
            manifest.rollback_binary,
            manifest.rollback_binary_sha256,
            manifest.rollback_plist,
            manifest.rollback_plist_sha256,
        )
    ):
        raise ActivationError("first-install rollback inputs are inconsistent")
    return ValidatedLaunchdHandoff(
        candidate_binary=candidate_binary,
        candidate_plist=candidate_plist,
        rollback=rollback,
    )


def require_fresh_prepared_activation(manifest: Manifest) -> None:
    if manifest.current_schema_upgrade is None:
        return
    status = read_status(manifest)
    if (
        status.get("transaction_id") == manifest.transaction_id
        and status.get("state") == "prepared"
        and not status.get("finalization_pending", False)
    ):
        return
    raise ActivationError(
        "prepared activation requires the matching fresh prepared transaction"
    )


def activate(manifest: Manifest) -> str:
    lock_path = Path(manifest.lock_path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is already activating") from exc

        refuse_selected_current_repair(manifest)
        require_fresh_prepared_activation(manifest)

        prepared_installs: list[Path] = []
        prepared_rollback: Optional[tuple[Path, Path]] = None
        capacity_reservation: Optional[DatabaseCapacityReservation] = None
        ordinary_forward_only = False
        try:
            validated = validate_pre_disruption_handoff(manifest)
            ordinary_forward_only = ordinary_forward_policy_applies(manifest)
            if manifest.paired_database_upgrade is not None:
                validate_legacy_database(Path(manifest.paired_database_upgrade.database_path))
            elif manifest.current_schema_upgrade is not None:
                validate_current_schema_database(
                    Path(manifest.current_schema_upgrade.database_path),
                    _current_schema_registry(manifest),
                    source_kind=manifest.source_kind,
                    manifest=manifest,
                )
            candidate_binary = validated.candidate_binary
            candidate_plist = validated.candidate_plist
            prepared_candidate_binary = prepare_atomic_install(
                candidate_binary, Path(manifest.target_binary), 0o755
            )
            prepared_installs.append(prepared_candidate_binary)
            prepared_candidate_plist = (
                prepare_plist_publication(candidate_plist, Path(manifest.target_plist))
                if prepared_database_upgrade(manifest) is not None
                else prepare_atomic_install(candidate_plist, Path(manifest.target_plist), 0o600)
            )
            prepared_installs.append(prepared_candidate_plist)
            prepared_candidate = (prepared_candidate_binary, prepared_candidate_plist)
            if validated.rollback is not None:
                rollback_binary, rollback_plist = validated.rollback
                prepared_rollback_binary = prepare_atomic_install(
                    rollback_binary, Path(manifest.target_binary), 0o755
                )
                prepared_installs.append(prepared_rollback_binary)
                prepared_rollback_plist = (
                    prepare_plist_publication(rollback_plist, Path(manifest.target_plist))
                    if prepared_database_upgrade(manifest) is not None
                    else prepare_atomic_install(rollback_plist, Path(manifest.target_plist), 0o600)
                )
                prepared_installs.append(prepared_rollback_plist)
                prepared_rollback = (prepared_rollback_binary, prepared_rollback_plist)
            upgrade = prepared_database_upgrade(manifest)
            if upgrade is not None:
                capacity_reservation = reserve_database_capacity(manifest)
        except Exception as exc:
            for prepared in prepared_installs:
                prepared.unlink(missing_ok=True)
            cleanup_pending = isinstance(exc, DatabaseCapacityCleanupFailure)
            if cleanup_pending:
                write_status(
                    manifest,
                    "precondition_cleanup_pending",
                    failure=str(exc.__cause__ or exc),
                    cleanup_failure=str(exc),
                )
            else:
                write_status(manifest, "precondition_failed", failure=str(exc))
            if manifest.current_schema_upgrade is not None and not cleanup_pending:
                raise CurrentSchemaPreconditionFailure(str(exc)) from exc
            raise

        launchctl = Launchctl(manifest)
        write_status(manifest, "activating")
        disrupted = False
        quarantined_previous_plist: Optional[Path] = None
        candidate_binary_mutated = False
        database_snapshot = False
        committed_durable = False
        current_schema_committed = False
        current_schema_migration_started = False
        source128_post_marker = False
        refresh = _schema129_refresh(manifest)
        ordinary_candidate_started = False
        try:
            upgrade = prepared_database_upgrade(manifest)
            if manifest.paired_database_upgrade is not None:
                validate_legacy_database(Path(manifest.paired_database_upgrade.database_path))
            elif manifest.current_schema_upgrade is not None:
                validate_current_schema_database(
                    Path(manifest.current_schema_upgrade.database_path),
                    _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest,
                )
            exact_source128 = _protected_129(manifest)
            if exact_source128:
                disrupted = True
                quarantined_previous_plist = quarantine_paired_plist(manifest)
                if quarantined_previous_plist is None:
                    raise ActivationError("prepared predecessor plist is unavailable")
                if sha256(quarantined_previous_plist) != manifest.rollback_plist_sha256:
                    raise ActivationError("captured predecessor plist checksum mismatch")
            old_pid = launchctl.stop()
            disrupted = True
            if exact_source128:
                quarantine_source128_target(manifest)
                state, pid = launchctl.inspect()
                if state != "not_loaded" or pid is not None:
                    raise ActivationError("source128 predecessor absence is unconfirmed")
                quarantine_source128_target(manifest)
            if upgrade is not None:
                if not exact_source128:
                    quarantined_previous_plist = quarantine_paired_plist(manifest)
                    if quarantined_previous_plist is None:
                        raise ActivationError("prepared predecessor plist is unavailable")
                    if sha256(quarantined_previous_plist) != manifest.rollback_plist_sha256:
                        raise ActivationError("captured predecessor plist checksum mismatch")
                assert_database_exclusive(manifest)
                if manifest.paired_database_upgrade is not None:
                    validate_legacy_database(Path(manifest.paired_database_upgrade.database_path))
                else:
                    validate_current_schema_database(
                        Path(manifest.current_schema_upgrade.database_path),
                        _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest,
                    )
                assert capacity_reservation is not None
                _reservation_still_sufficient(manifest, capacity_reservation)
                create_database_backup(manifest, capacity_reservation)
                database_snapshot = True
                if exact_source128:
                    rehearse_prepared_source128(manifest, capacity_reservation)
            candidate_binary_mutated = True
            commit_atomic_install(prepared_candidate[0], Path(manifest.target_binary))
            if manifest.current_schema_upgrade is not None and not refresh:
                if exact_source128:
                    source128_post_marker = True
                write_status(
                    manifest,
                    "migration_started",
                    finalization_pending=True,
                    committed_diagnostic="offline candidate migration may have committed writes; predecessor rollback is forbidden",
                )
                current_schema_migration_started = True
                run_current_schema_migration(manifest, capacity_reservation)
                current_schema_committed = True
                committed_durable = True
                write_status(
                    manifest,
                    "committed",
                    finalization_pending=True,
                    committed_diagnostic="irreversible database migration committed; candidate start and publication pending",
                )
            if upgrade is None:
                commit_atomic_install(prepared_candidate[1], Path(manifest.target_plist))
                if ordinary_forward_only:
                    write_status(
                        manifest,
                        "candidate_may_migrate",
                        finalization_pending=True,
                        committed_diagnostic="candidate startup may commit irreversible forward migration or initialization",
                    )
                    ordinary_candidate_started = True
                launchctl.start(old_pid)
            else:
                if manifest.current_schema_upgrade is not None:
                    assert_database_exclusive(manifest)
                if exact_source128:
                    if os.path.lexists(manifest.target_plist):
                        raise ActivationError("persistent target plist must be absent before private start")
                    if refresh:
                        write_status(
                            manifest, "migration_started", finalization_pending=True,
                            committed_diagnostic="private candidate start may write; predecessor fallback forbidden",
                        )
                        source128_post_marker = True
                    checkpoint_source128_start(manifest)
                launchctl.start(old_pid, plist_path=manifest.candidate_plist)
            if exact_source128:
                require_loaded_plist(manifest, launchctl, candidate_plist)
            wait_for_identity(manifest, manifest.expected)
            if exact_source128:
                atomic_write(
                    Path(manifest.deployed_sha_path),
                    (manifest.source_commit + "\n").encode(),
                    0o600,
                )
                write_status(
                    manifest,
                    "committed",
                    finalization_pending=True,
                    committed_diagnostic=(
                        "exact candidate verified; persistent publication and capacity cleanup pending"
                    ),
                )
                publish_source128_plist(
                    prepared_candidate[1], Path(manifest.target_plist)
                )
                assert capacity_reservation is not None
                release_source128_capacity_reservation(
                    manifest, capacity_reservation
                )
                write_status(manifest, "committed")
                if not release_durable_terminal_claim(manifest, required_state="committed"):
                    raise ActivationError("source128 activation claim release was unconfirmed")
                return "committed"

            atomic_write(Path(manifest.deployed_sha_path), (manifest.source_commit + "\n").encode(), 0o600)
            prepared_commit = upgrade is not None
            write_status(
                manifest, "committed", finalization_pending=prepared_commit,
                committed_diagnostic="prepared publication/cleanup pending; login/reboot persistence unconfirmed" if prepared_commit else None,
            )
            committed_durable = True
            diagnostics = []
            if capacity_reservation is not None:
                try:
                    release_capacity_reservation(manifest, capacity_reservation)
                except Exception as capacity_exc:
                    diagnostics.append(f"restore reservation cleanup failed: {capacity_exc}")
            if prepared_commit:
                try:
                    commit_atomic_install(prepared_candidate[1], Path(manifest.target_plist))
                except Exception as publish_exc:
                    diagnostics.append(f"candidate plist publish failed: {publish_exc}")
                write_status(manifest, "committed", finalization_pending=bool(diagnostics), committed_diagnostic="; ".join(diagnostics) or None)
            return "committed"
        except Exception as activation_exc:
            failure = str(activation_exc)
            disrupted = disrupted or launchctl.disruption_started
            if source128_post_marker:
                return settle_source128_failure(manifest, launchctl, failure)
            if refresh:
                try:
                    marker_or_start = _post_refresh_marker(manifest) or _refresh_start_proof(manifest)
                except (OSError, ValueError, ActivationError):
                    marker_or_start = True
                if marker_or_start:
                    return settle_source128_failure(manifest, launchctl, failure)
            if committed_durable:
                try:
                    write_status(manifest, "committed", committed_diagnostic=f"paired finalization interrupted: {failure}", finalization_pending=True)
                except Exception:
                    pass
                return "committed"
            if ordinary_forward_only and ordinary_candidate_started:
                teardown_failure = None
                try:
                    launchctl.stop()
                    state, pid = launchctl.inspect()
                    if state != "not_loaded" or pid is not None:
                        raise ActivationError("forward candidate remains loaded after failed activation")
                except Exception as teardown_exc:
                    teardown_failure = str(teardown_exc)
                write_status(
                    manifest,
                    "ordinary_activation_failed_forward_only",
                    failure=failure,
                    rollback_failure=teardown_failure,
                    finalization_pending=True,
                    committed_diagnostic="candidate may have committed migrations or initialization; predecessor launch is forbidden",
                )
                return "ordinary_activation_failed_forward_only"
            if (
                manifest.current_schema_upgrade is not None
                and database_snapshot
                and current_schema_migration_started
                and not current_schema_committed
            ):
                write_status(
                    manifest,
                    "activation_failed_database_committed",
                    failure=failure,
                    finalization_pending=True,
                    committed_diagnostic="offline migration may have committed database writes; predecessor rollback is forbidden",
                )
                return "activation_failed_database_committed"
            if not disrupted:
                if capacity_reservation is not None and not database_snapshot:
                    try:
                        abandon_capacity_reservation(manifest, capacity_reservation)
                        write_status(manifest, "precondition_failed", failure=failure)
                    except Exception as cleanup_error:
                        try:
                            write_status(
                                manifest,
                                "precondition_cleanup_pending",
                                failure=failure,
                                cleanup_failure=str(cleanup_error),
                                precondition_cleanup_completed=False,
                            )
                        except Exception:
                            pass
                        raise cleanup_error from activation_exc
                else:
                    write_status(manifest, "precondition_failed", failure=failure)
                if manifest.current_schema_upgrade is not None:
                    raise CurrentSchemaPreconditionFailure(failure) from activation_exc
                raise
            try:
                restore(
                    manifest,
                    launchctl,
                    prepared_rollback,
                    database_snapshot=database_snapshot,
                    candidate_binary_mutated=candidate_binary_mutated,
                    rollback_plist=quarantined_previous_plist,
                )
                finalize_verified_predecessor(manifest, failure)
                return "activation_failed_rolled_back"
            except Exception as rollback_exc:
                rollback_failure = str(rollback_exc)
                if prepared_database_upgrade(manifest) is not None and not isinstance(rollback_exc, VerifiedPredecessorFinalizationError):
                    try:
                        launchctl.stop()
                        state, pid = launchctl.inspect()
                        if state != "not_loaded" or pid is not None:
                            raise ActivationError("failed paired recovery teardown is unconfirmed")
                    except Exception as teardown_exc:
                        rollback_failure += f"; recovery teardown failed: {teardown_exc}"
                if prepared_database_upgrade(manifest) is not None:
                    try:
                        quarantine_paired_plist(manifest)
                    except Exception as quarantine:
                        rollback_failure += f"; durable plist quarantine failed: {quarantine}"
                failed_state = (
                    "activation_failed_rollback_failed" if prepared_database_upgrade(manifest) is not None
                    else "ordinary_activation_failed_rollback_failed"
                )
                try:
                    write_status(manifest, failed_state, failure=failure, rollback_failure=rollback_failure)
                except Exception as exc:
                    if isinstance(rollback_exc, VerifiedPredecessorFinalizationError):
                        raise VerifiedPredecessorFinalizationError(str(exc)) from exc
                    raise
                return failed_state
        finally:
            for prepared in prepared_installs:
                prepared.unlink(missing_ok=True)


def quarantine_source128_target(manifest: Manifest) -> list[Path]:
    upgrade = manifest.current_schema_upgrade
    if not _protected_129(manifest):
        raise ActivationError("protected target isolation requires exact prepared mode")
    target = Path(manifest.target_plist)
    transaction = _private_transaction_dir(Path(upgrade.proof_path).parent)
    retained = []
    for _ in range(SOURCE128_QUARANTINE_ATTEMPTS):
        if not os.path.lexists(target):
            fsync_dir(target.parent)
            return retained
        destination = transaction / f"unresolved-launchagent-{secrets.token_hex(16)}.plist.quarantined"
        if os.path.lexists(destination):
            raise ActivationError("source128 quarantine destination already exists")
        try:
            os.rename(target, destination)
        except FileNotFoundError:
            continue
        retained.append(destination)
        fsync_dir(transaction)
        fsync_dir(target.parent)
    if os.path.lexists(target):
        raise ActivationError("source128 persistent target keeps reappearing during bounded isolation")
    fsync_dir(target.parent)
    return retained


def quarantine_paired_plist(manifest: Manifest) -> Optional[Path]:
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
        return None
    if _protected_129(manifest):
        retained = quarantine_source128_target(manifest)
        return retained[-1] if retained else None
    target = Path(manifest.target_plist)
    if target.is_symlink():
        raise ActivationError("unresolved target plist must not be a symlink")
    if not target.exists():
        return None
    transaction = Path(upgrade.proof_path).parent
    fd, name = tempfile.mkstemp(prefix="unresolved-launchagent-", suffix=".plist.quarantined", dir=transaction)
    os.close(fd)
    destination = Path(name)
    os.replace(target, destination)
    destination.chmod(0o600)
    fsync_dir(transaction)
    fsync_dir(target.parent)
    return destination


def captured_paired_plist(manifest: Manifest) -> Optional[Path]:
    """Return the retained predecessor plist only when its hash is proven."""
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None or manifest.rollback_plist_sha256 is None:
        return None
    target = Path(manifest.target_plist)
    if target.exists() and not target.is_symlink() and sha256(target) == manifest.rollback_plist_sha256:
        return target
    transaction = Path(upgrade.proof_path).parent
    matches = [
        path for path in transaction.glob("unresolved-launchagent-*.plist.quarantined")
        if not path.is_symlink() and path.is_file() and sha256(path) == manifest.rollback_plist_sha256
    ]
    return max(matches, key=lambda path: path.stat().st_mtime_ns) if matches else None


def abandon_capacity_reservation(
    manifest: Manifest, reservation: DatabaseCapacityReservation
) -> None:
    """Delete both unsnapshotted capacity files before a pre-stop failure is terminal."""
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None:
        raise ActivationError("prepared database upgrade is not configured")
    expected_backup = Path(upgrade.backup_path)
    expected_restore = _restore_capacity_path(manifest, Path(upgrade.database_path))
    if (
        reservation.backup_path != expected_backup
        or reservation.restore_path != expected_restore
    ):
        raise ActivationError("capacity reservation does not belong to this transaction")
    _remove_capacity_files(*_owned_capacity_cleanup_paths(manifest, reservation))


def _source128_capacity_cleanup_authorized(
    manifest: Manifest, reservation: DatabaseCapacityReservation
) -> bool:
    evidence = _repair_json(_capacity_observation_path(reservation))
    if evidence.get("transaction_id") != manifest.transaction_id:
        raise ActivationError("source128 restore capacity evidence does not match")
    value = evidence.get("restore_capacity_cleanup_authorized_at")
    return isinstance(value, str) and bool(value)


def release_source128_capacity_reservation(
    manifest: Manifest, reservation: DatabaseCapacityReservation
) -> None:
    evidence_path = _capacity_observation_path(reservation)
    evidence = _repair_json(evidence_path)
    if evidence.get("transaction_id") != manifest.transaction_id:
        raise ActivationError("source128 restore capacity evidence does not match")
    if not _source128_capacity_cleanup_authorized(manifest, reservation):
        evidence["restore_capacity_cleanup_authorized_at"] = utc_now()
        atomic_write(
            evidence_path,
            json.dumps(evidence, sort_keys=True, separators=(",", ":")).encode(),
            0o600,
        )
    release_capacity_reservation(manifest, reservation)


def release_capacity_reservation(manifest: Manifest, reservation: DatabaseCapacityReservation) -> None:
    """Release only the temporary restore reservation; retain audit snapshot/proof."""
    upgrade = prepared_database_upgrade(manifest)
    if upgrade is None or reservation.restore_path != _restore_capacity_path(manifest, Path(upgrade.database_path)):
        raise ActivationError("restore reservation does not belong to this prepared transaction")
    path = reservation.restore_path
    if path.is_symlink():
        raise ActivationError("restore reservation is a symlink")
    path.unlink(missing_ok=True)
    fsync_dir(path.parent)


def release_unsnapshotted_capacity(manifest: Manifest) -> None:
    database, backup, proof, _transaction = _paired_path_context(manifest)
    if proof.exists() or proof.is_symlink():
        return
    restore = _restore_capacity_path(manifest, database)
    reservation = DatabaseCapacityReservation(backup, restore, 0)
    _remove_capacity_files(*_owned_capacity_cleanup_paths(manifest, reservation))


def record_recovery_error(manifest: Manifest, error: str | Exception, *, quarantine: bool = False) -> None:
    verified_finalization_failed = isinstance(error, VerifiedPredecessorFinalizationError)
    error = str(error)
    if quarantine:
        try:
            quarantine_paired_plist(manifest)
        except Exception as failure:
            error += f"; durable plist quarantine failed: {failure}"
    try:
        prior = read_status(manifest)
    except (OSError, json.JSONDecodeError):
        prior = {}
    if prior.get("transaction_id") != manifest.transaction_id:
        prior = {}
    if not prior or prior.get("transaction_id") != manifest.transaction_id:
        return
    if prior.get("state") in {
        "committed",
        "preparing",
        "migration_started",
        "activation_failed_database_committed",
    }:
        return
    previous = prior.get("rollback_failure")
    merged = f"{previous}; recovery attempt failed: {error}" if previous else error
    try:
        write_status(manifest, "activation_failed_rollback_failed", failure=prior.get("failure"), rollback_failure=merged)
    except Exception as exc:
        if verified_finalization_failed:
            raise VerifiedPredecessorFinalizationError(str(exc)) from exc
        raise


def publish_source128_plist(prepared: Path, target: Path) -> None:
    if os.path.lexists(target):
        raise ActivationError("persistent target plist already exists")
    os.link(prepared, target)
    fsync_dir(target.parent)
    prepared.unlink()
    fsync_dir(prepared.parent)


def settle_source128_failure(manifest: Manifest, launchctl: Launchctl, failure: str) -> str:
    teardown = []
    try:
        quarantine_paired_plist(manifest)
    except Exception as exc:
        teardown.append(f"persistent plist quarantine failed: {exc}")
    try:
        launchctl.stop()
    except Exception as exc:
        teardown.append(f"candidate stop failed: {exc}")
    try:
        quarantine_source128_target(manifest)
    except Exception as exc:
        teardown.append(f"post-stop persistent plist quarantine failed: {exc}")
    try:
        state, pid = launchctl.inspect()
        if state != "not_loaded" or pid is not None:
            teardown.append(f"candidate absence unconfirmed: {state}, pid={pid}")
    except Exception as exc:
        teardown.append(f"candidate absence unconfirmed: {exc}")
    try:
        require_launchd_absence(manifest.label, manifest.uid)
    except Exception as exc:
        teardown.append(f"candidate absence unconfirmed: {exc}")
    try:
        quarantine_source128_target(manifest)
    except Exception as exc:
        teardown.append(f"final persistent plist quarantine failed: {exc}")
    if os.path.lexists(manifest.target_plist):
        teardown.append("persistent target absence unconfirmed")
    write_status(
        manifest, "activation_failed_database_committed", failure=failure,
        rollback_failure="; ".join(teardown) or None, finalization_pending=True,
        committed_diagnostic="database may be committed; predecessor rollback is forbidden",
    )
    return "activation_failed_database_committed"


def require_loaded_plist(manifest: Manifest, launchctl: Launchctl, private: Path) -> None:
    result = launchctl.run(["launchctl", "print", launchctl.target], capture_output=True, text=True)
    match = re.search(r"^\s*path = (.+)$", result.stdout, re.MULTILINE)
    if result.returncode != 0 or match is None or not Path(match.group(1).strip()).samefile(private):
        raise ActivationError("loaded configuration does not match retained private plist")


def cleanup_precondition_capacity(manifest: Manifest, *, current_schema: bool) -> str:
    lock_path = Path(manifest.lock_path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is already activating") from exc

        try:
            validate_manifest_identities(manifest)
            validate_manifest_mode(manifest)
            upgrade = prepared_database_upgrade(manifest)
            if upgrade is None or (manifest.current_schema_upgrade is not None) != current_schema:
                raise ActivationError("precondition cleanup mode does not match the retained manifest")
            status_path = Path(manifest.status_path)
            if not status_path.is_file() or status_path.is_symlink():
                raise ActivationError(
                    "precondition cleanup requires a regular non-symlink status"
                )
            status = read_status(manifest)
            validate_status_manifest_mode(manifest, status)
            expected_prepared_mode = "current_schema" if current_schema else "paired"
            cleanup_pending = status.get("state") == "precondition_cleanup_pending"
            cleanup_release_pending = (
                status.get("state") == "precondition_failed"
                and status.get("precondition_cleanup_completed") is True
            )
            if (
                status.get("transaction_id") != manifest.transaction_id
                or status.get("prepared_mode") != expected_prepared_mode
                or not (cleanup_pending or cleanup_release_pending)
            ):
                raise ActivationError("precondition cleanup requires the matching pending status")
            claim = Path(manifest.active_path)
            if (
                not claim.is_file()
                or claim.is_symlink()
                or claim.read_text().strip() != manifest.transaction_id
            ):
                raise ActivationError("precondition cleanup requires the matching active claim")

            database, backup, proof, transaction_dir = _paired_path_context(manifest)
            _private_transaction_dir(transaction_dir)
            restore = _restore_capacity_path(manifest, database)
            capacity_evidence = backup.parent / "capacity.json"
            if proof.exists() or proof.is_symlink():
                raise ActivationError("precondition cleanup refuses a transaction with snapshot proof")
            for path in (backup, restore, capacity_evidence):
                if not path.is_absolute() or path.is_symlink():
                    raise ActivationError("precondition cleanup paths must be absolute non-symlinks")

            reservation = DatabaseCapacityReservation(backup, restore, 0)
        except Exception as exc:
            raise CleanupPreconditionRejected(str(exc)) from exc
        try:
            _remove_capacity_files(*_owned_capacity_cleanup_paths(manifest, reservation))
            write_status(
                manifest,
                "precondition_failed",
                failure=status.get("failure"),
                precondition_cleanup_completed=True,
            )
        except Exception as exc:
            try:
                write_status(
                    manifest,
                    "precondition_cleanup_pending",
                    failure=status.get("failure"),
                    cleanup_failure=str(exc),
                )
            except Exception:
                pass
            raise
        return "precondition_failed"


def _repair_private(path: Path) -> None:
    _regular_nosymlink(path, "current-schema repair evidence")
    if path != path.resolve() or path.stat().st_uid != os.getuid() or path.stat().st_mode & 0o077:
        raise ActivationError("repair evidence must be private, owned and canonical")


def _repair_json(path: Path) -> dict:
    _repair_private(path)
    if path.stat().st_size > MAX_DATABASE_PROOF_BYTES:
        raise ActivationError("repair evidence exceeds bounded size")
    try:
        def unique(pairs):
            result = {}
            for key, value in pairs:
                if key in result:
                    raise ValueError("duplicate evidence key")
                result[key] = value
            return result
        value = json.loads(path.read_text(), object_pairs_hook=unique)
        if not isinstance(value, dict):
            raise ValueError("evidence must be an object")
        return value
    except (ValueError, OSError) as exc:
        raise ActivationError("repair evidence is unreadable or malformed") from exc


def current_repair_entry(parent: Manifest) -> Path:
    return Path(parent.candidate_binary).parent / CURRENT_REPAIR_DIRECTORY / "manifest.json"


def refuse_selected_current_repair(parent: Manifest) -> None:
    entry = current_repair_entry(parent)
    if os.path.lexists(entry) or entry.parent.is_symlink():
        raise ActivationError(
            f"current-schema repair evidence fences original activation; use prod resume-current-repair {parent.transaction_id}"
        )


def require_launchd_absence(label: str, uid: int) -> None:
    result = subprocess.run(["launchctl", "print", f"gui/{uid}/{label}"], capture_output=True, text=True)
    if result.returncode == 0 or not service_absence_confirmed(result.stdout + "\n" + result.stderr, label, uid):
        raise ActivationError("exact launchd service/helper absence is unconfirmed")


def _current_repair_parent_records(parent_path: Path, status_path: Path) -> tuple[Manifest, dict]:
    raw = _repair_json(parent_path)
    if set(raw) - {field.name for field in dataclasses.fields(Manifest)}:
        raise ActivationError("repair parent manifest has unknown fields")
    parent = Manifest.load(parent_path)
    validate_manifest_identities(parent)
    validate_prepared_source114(parent)
    upgrade = parent.current_schema_upgrade
    if (parent.source_kind != "prepared_artifact" or parent.source_commit != PREPARED_SOURCE114_COMMIT
            or not isinstance(upgrade, PreparedSource114Upgrade)
            or parent.helper_label != f"{DEPLOY_HELPER_LABEL_PREFIX}.{parent.transaction_id}"
            or parent.uid != os.getuid()):
        raise ActivationError("repair requires the exact original prepared current-schema parent")
    if parent_path != Path(parent.candidate_binary).parent / "manifest.json":
        raise ActivationError("repair parent transaction path mismatch")
    _private_transaction_dir(parent_path.parent)
    if _paired_path_context(parent)[3] != parent_path.parent:
        raise ActivationError("repair snapshot is outside the original parent transaction")
    if Path(upgrade.controller_helper_path).parent != parent_path.parent or Path(upgrade.migration_registry_path).parent != parent_path.parent:
        raise ActivationError("repair parent helper/registry path crosswire")
    for path, digest in (
        (parent.candidate_plist, parent.candidate_plist_sha256),
        (upgrade.controller_helper_path, upgrade.controller_helper_sha256),
        (upgrade.migration_registry_path, upgrade.migration_registry_sha256),
    ):
        _repair_private(Path(path))
        verify_staged(path, digest, "original repair parent evidence")
    paths = [Path(getattr(parent, name)) for name in (
        "candidate_binary", "candidate_plist", "target_binary", "target_plist", "active_path",
        "status_path", "deployed_sha_path", "lock_path", "claim_lock_path",
    )] + [Path(getattr(upgrade, name)) for name in (
        "database_path", "backup_path", "proof_path", "controller_helper_path", "migration_registry_path",
    )]
    if (any(not item.is_absolute() or item != item.resolve() for item in paths)
            or len(set(paths)) != len(paths)
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", parent.transaction_id)
            or not re.fullmatch(r"[A-Za-z0-9._-]+", parent.label)):
        raise ActivationError("repair parent paths or routing are malformed")
    status = _repair_json(status_path)
    expected_status = {
        "transaction_id": parent.transaction_id, "state": "committed", "source_kind": parent.source_kind,
        "source_commit": parent.source_commit, "release_tag": None, "release_commit": None,
        "expected_version": parent.expected.version, "expected_git_sha": parent.expected.git_sha,
        "created_at": parent.created_at, "finalization_pending": True, "prepared_mode": "current_schema",
        "failure": None, "rollback_failure": None, "cleanup_failure": None,
        "precondition_cleanup_completed": False, "recovery_mode": None,
    }
    if (set(status) != set(expected_status) | {"updated_at", "committed_diagnostic"}
            or any(type(status[key]) is not type(value) or status[key] != value for key, value in expected_status.items())
            or not isinstance(status["updated_at"], str) or not status["updated_at"]
            or (status["committed_diagnostic"] is not None and not isinstance(status["committed_diagnostic"], str))):
        raise ActivationError("repair requires exact original committed/pending status shape")
    validate_status_manifest_mode(parent, status)
    proof = _repair_json(Path(upgrade.proof_path))
    target = [list(item) for item in migration_registry(_current_schema_registry(parent))]
    expected_proof = {
        "transaction_id": parent.transaction_id, "source_commit": parent.source_commit,
        "database": upgrade.database_path, "candidate_binary_sha256": parent.candidate_binary_sha256,
        "candidate_plist_sha256": parent.candidate_plist_sha256,
        "previous_binary_sha256": parent.rollback_binary_sha256, "previous_plist_sha256": parent.rollback_plist_sha256,
        "controller_source_commit": upgrade.controller_source_commit, "controller_helper_sha256": upgrade.controller_helper_sha256,
        "database_mode": "current_schema", "migration_registry_sha256": upgrade.migration_registry_sha256,
        "prepared_source_qualification": prepared_source_qualification(parent), "target_ledger": target, "migrated_ledger": target,
    }
    digest_keys = {"sha256", "source_logical_digest", "backup_logical_digest", "migrated_logical_digest"}
    if (set(proof) != set(expected_proof) | digest_keys | {"source_ledger", "migration_committed_at"}
            or any(json.dumps(proof[key], sort_keys=True) != json.dumps(value, sort_keys=True) for key, value in expected_proof.items())
            or any(not isinstance(proof[key], str) or not re.fullmatch(r"[0-9a-f]{64}", proof[key]) for key in digest_keys)
            or proof["backup_logical_digest"] != proof["source_logical_digest"]
            or json.dumps(proof["source_ledger"]) not in (json.dumps(target), json.dumps([item for item in target if item[0] <= 114]))
            or not isinstance(proof["migration_committed_at"], str) or not proof["migration_committed_at"]):
        raise ActivationError("repair requires the parent's exact committed target128 proof shape")
    return parent, proof


def _require_current_repair_claim(parent: Manifest) -> None:
    _repair_private(Path(parent.active_path))
    if Path(parent.active_path).read_text().strip() != parent.transaction_id:
        raise ActivationError("repair requires the exact retained parent claim")


def validate_current_repair_parent(parent_path: Path, *, require_claim: bool = True,
                                   status_path: Path | None = None) -> tuple[Manifest, dict]:
    parent = Manifest.load(parent_path)
    parent, proof = _current_repair_parent_records(parent_path, status_path or Path(parent.status_path))
    if require_claim:
        _require_current_repair_claim(parent)
    if _snapshot_proof(parent) != proof:
        raise ActivationError("repair parent proof changed during full validation")
    return parent, proof


def _repair_runtime(parent: Manifest, repair: CurrentSchemaRepairManifest, directory: Path) -> Manifest:
    return dataclasses.replace(
        parent, source_commit=repair.source_commit, expected=Identity(repair.version, repair.source_commit),
        candidate_binary=str(directory / "candidate"), candidate_binary_sha256=repair.candidate_sha256,
        candidate_plist=str(directory / "candidate.plist"), candidate_plist_sha256=repair.plist_sha256,
        helper_label=repair.helper_label,
    )


def classify_current_repair(path: Path) -> tuple[CurrentSchemaRepairManifest, Manifest, CurrentSchemaRepairStatus]:
    repair = CurrentSchemaRepairManifest.load(path)
    directory = path.parent
    status = CurrentSchemaRepairStatus.load(directory / "status.json", sha256(path))
    for name in ("candidate", "candidate.plist", "helper.py", "migrations.rs", "receipt.json"):
        _repair_private(directory / name)
    terminal = status.state == "committed" and not status.finalization_pending
    parent, proof = _current_repair_parent_records(Path(repair.parent_manifest_path), directory / "parent-status.json")
    upgrade = parent.current_schema_upgrade
    for evidence, digest in (
        (Path(repair.parent_manifest_path), repair.parent_manifest_sha256),
        (directory / "parent-status.json", repair.parent_status_sha256),
        (Path(upgrade.proof_path), repair.parent_proof_sha256),
    ):
        _repair_private(evidence)
        verify_staged(str(evidence), digest, "sealed repair evidence")
    if repair.helper_label != f"{parent.helper_label}.current-schema-repair":
        raise ActivationError("repair child helper label mismatch")
    if not terminal:
        _repair_private(Path(parent.status_path))
        verify_staged(parent.status_path, repair.parent_status_sha256, "live original repair parent status")
        _require_current_repair_claim(parent)
    if status.proof_sha256 is not None:
        repair_proof = _repair_json(directory / "proof.json")
        expected_proof = {
            "protocol_version": 1, "manifest_sha256": sha256(path),
            "parent_proof_sha256": repair.parent_proof_sha256, "source_commit": repair.source_commit,
            "registry_sha256": repair.registry_sha256, "target_ledger": proof["target_ledger"],
        }
        if (sha256(directory / "proof.json") != status.proof_sha256
                or json.dumps(repair_proof, sort_keys=True) != json.dumps(expected_proof, sort_keys=True)):
            raise ActivationError("repair target proof mismatch")
    return repair, parent, status


def validate_current_repair_staging(
    repair: CurrentSchemaRepairManifest,
    parent: Manifest,
    proof: dict,
    directory: Path,
    *,
    running_helper: bool,
) -> None:
    upgrade = parent.current_schema_upgrade
    if running_helper and Path(__file__).resolve() != directory / "helper.py":
        raise ActivationError("repair must execute its retained controller helper")
    for evidence, digest in (
        (directory / "helper.py", repair.helper_sha256),
        (directory / "candidate", repair.candidate_sha256),
        (directory / "candidate.plist", repair.plist_sha256),
        (directory / "migrations.rs", repair.registry_sha256),
        (directory / "receipt.json", repair.receipt_sha256),
    ):
        _repair_private(evidence)
        verify_staged(str(evidence), digest, "sealed repair artifact")
    if plistlib.loads((directory / "candidate.plist").read_bytes()) != plistlib.loads(Path(parent.candidate_plist).read_bytes()):
        raise ActivationError("repair configuration differs from the parent's captured private plist")
    plist = plistlib.loads((directory / "candidate.plist").read_bytes())
    if (plist.get("Label") != parent.label or plist.get("ProgramArguments") != [parent.target_binary]
            or plist.get("EnvironmentVariables", {}).get("PHOENIX_DB_PATH") != upgrade.database_path):
        raise ActivationError("repair private plist executable/database mismatch")
    if [list(item) for item in migration_registry(directory / "migrations.rs")] != proof["target_ledger"]:
        raise ActivationError("repair registry differs from original target128")
    receipt = _repair_json(directory / "receipt.json")
    checks = receipt.get("checks")
    expected_checks = {"developer_id_signature": "verified", "hardened_runtime": "verified", "notarization": "accepted", "stapled_ticket": "validated", "gatekeeper": "accepted", "embedded_helper_bytes": "identical"}
    target = receipt.get("target")
    digests = receipt.get("sha256")
    submission = checks.get("notarization_submission_id") if isinstance(checks, dict) else None
    if not isinstance(submission, str) or not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", submission):
        raise ActivationError("retained repair receipt submission UUID mismatch")
    if (type(receipt.get("schema")) is not int or receipt.get("schema") != 1 or receipt.get("operation") != "prepare-main"
            or receipt.get("commit") != repair.source_commit or receipt.get("version") != repair.version
            or not isinstance(target, str) or target not in {"aarch64-apple-darwin", "x86_64-apple-darwin"}
            or not isinstance(checks, dict) or any(checks.get(key) != value for key, value in expected_checks.items())
            or not isinstance(digests, dict)
            or digests.get(f"phoenix_ide-{target}-prepared-{repair.source_commit[:12]}") != repair.candidate_sha256):
        raise ActivationError("retained repair receipt identity/signature/checksum mismatch")


def validate_current_repair(path: Path, *, running_helper: bool = True) -> tuple[CurrentSchemaRepairManifest, Manifest, CurrentSchemaRepairStatus]:
    repair, parent, status = classify_current_repair(path)
    directory = path.parent
    parent, proof = validate_current_repair_parent(
        Path(repair.parent_manifest_path), require_claim=not (status.state == "committed" and not status.finalization_pending),
        status_path=directory / "parent-status.json",
    )
    validate_current_repair_staging(
        repair,
        parent,
        proof,
        directory,
        running_helper=running_helper,
    )
    return repair, parent, status


def _write_repair_status(path: Path, status: CurrentSchemaRepairStatus) -> None:
    atomic_write(path.parent / "status.json", (json.dumps(dataclasses.asdict(status), sort_keys=True) + "\n").encode())
    CurrentSchemaRepairStatus.load(path.parent / "status.json", sha256(path))


def _repair_capacity(parent: Manifest, *, reservation_required: bool) -> None:
    upgrade = parent.current_schema_upgrade
    restore = _restore_capacity_path(parent, Path(upgrade.database_path))
    if reservation_required:
        required = _source_capacity_bytes(Path(upgrade.database_path))
        _repair_private(restore)
        if restore.stat().st_size < required or restore.stat().st_blocks * 512 < required:
            raise ActivationError("repair live database exceeds retained restore capacity")
    # The repair adds a second bounded evidence lifecycle on the same pool.
    for directory in {Path(upgrade.database_path).parent, Path(upgrade.backup_path).parent,
                      Path(parent.target_binary).parent, Path(parent.target_plist).parent}:
        if shutil.disk_usage(directory).free < MINIMUM_FREE_AFTER_RESERVATION_BYTES + CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES:
            raise ActivationError("repair capacity cannot preserve safety floor and evidence allowance")


def _repair_verify_database(parent: Manifest, directory: Path) -> None:
    assert_database_exclusive(parent)
    registry = directory / "migrations.rs"
    repair_upgrade = dataclasses.replace(
        parent.current_schema_upgrade,
        migration_registry_path=str(registry),
        migration_registry_sha256=CURRENT_REPAIR_REGISTRY_SHA256,
    )
    repair_validation_parent = dataclasses.replace(parent, current_schema_upgrade=repair_upgrade)
    validate_current_schema_database(
        Path(parent.current_schema_upgrade.database_path),
        registry,
        source_kind=parent.source_kind,
        manifest=repair_validation_parent,
        migrated=True,
    )


def _repair_verify_installed(runtime: Manifest) -> None:
    verify_staged(runtime.target_binary, runtime.candidate_binary_sha256, "installed repair candidate")
    result = subprocess.run([runtime.target_binary, "--build-identity"], capture_output=True, text=True, timeout=30, check=True)
    try:
        identity = json.loads(result.stdout)
    except ValueError as exc:
        raise ActivationError("installed repair build identity unreadable") from exc
    if not isinstance(identity, dict) or identity.get("version") != runtime.expected.version or identity.get("git_sha") != runtime.expected.git_sha:
        raise ActivationError("installed repair identity mismatch")


def _repair_publication_path(parent: Manifest) -> Path:
    target = Path(parent.target_plist)
    return target.parent / f".{target.name}.current-schema-repair-{parent.transaction_id}"


def _current_repair_publication_quarantine_prefix(parent: Manifest) -> str:
    target = Path(parent.target_plist)
    return f".{target.name}.current-schema-repair-publication-{parent.transaction_id}-"


def _retire_current_repair_publication(runtime: Manifest, publication: Path) -> Path:
    target = Path(runtime.target_plist)
    retired = target.parent / f"{_current_repair_publication_quarantine_prefix(runtime)}{secrets.token_hex(16)}"
    try:
        os.rename(publication, retired)
    except FileNotFoundError as exc:
        raise ActivationError("repair publication disappeared before retirement") from exc
    fsync_dir(target.parent)
    _repair_private(retired)
    if _regular_file_digest(retired, "retired repair publication") != runtime.candidate_plist_sha256:
        raise ActivationError("repair publication changed before retirement")
    return retired


def _commit_current_repair_plist(runtime: Manifest, publication: Path) -> None:
    target = Path(runtime.target_plist)
    if _current_repair_target_plist_digest(runtime) is None:
        try:
            os.link(publication, target)
        except FileExistsError:
            pass
    if _current_repair_target_plist_digest(runtime) != runtime.candidate_plist_sha256:
        raise ActivationError("persistent target plist appeared with unknown bytes during publication")
    _retire_current_repair_publication(runtime, publication)
    if _current_repair_target_plist_digest(runtime) != runtime.candidate_plist_sha256:
        raise ActivationError("published repair plist changed during durable publication")


def _publish_current_repair(runtime: Manifest, parent: Manifest) -> None:
    publication = _repair_publication_path(parent)
    if os.path.lexists(publication):
        _retire_current_repair_publication(runtime, publication)
    os.link(runtime.candidate_plist, publication)
    fsync_dir(publication.parent)
    _commit_current_repair_plist(runtime, publication)


def _file_metadata_token(metadata: os.stat_result) -> CurrentRepairFileToken:
    return CurrentRepairFileToken(
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


def _regular_file_path_token(path: Path) -> CurrentRepairFileToken | None:
    try:
        return _file_metadata_token(os.stat(path, follow_symlinks=False))
    except FileNotFoundError:
        return None


def _regular_file_evidence(path: Path, label: str) -> tuple[str, CurrentRepairFileToken] | None:
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    except FileNotFoundError:
        return None
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise ActivationError(f"{label} is not a regular file")
        if before.st_size > CURRENT_REPAIR_PLIST_EVIDENCE_MAX_BYTES:
            raise ActivationError(f"{label} exceeds the bounded plist evidence size")
        digest = hashlib.sha256()
        remaining = before.st_size
        while remaining:
            chunk = os.read(fd, min(remaining, CURRENT_REPAIR_PLIST_EVIDENCE_MAX_BYTES))
            if not chunk:
                raise ActivationError(f"{label} changed while being read")
            digest.update(chunk)
            remaining -= len(chunk)
        if os.read(fd, 1):
            raise ActivationError(f"{label} changed while being read")
        after = os.fstat(fd)
        after_token = _file_metadata_token(after)
        if _file_metadata_token(before) != after_token:
            raise ActivationError(f"{label} changed while being read")
        if _regular_file_path_token(path) != after_token:
            raise ActivationError(f"{label} path changed while being read")
        return digest.hexdigest(), after_token
    finally:
        os.close(fd)


def _regular_file_digest(path: Path, label: str) -> str | None:
    evidence = _regular_file_evidence(path, label)
    return None if evidence is None else evidence[0]


def _current_repair_target_plist_digest(parent: Manifest) -> str | None:
    return _regular_file_digest(Path(parent.target_plist), "published repair plist")


def _current_repair_quarantine_prefix(parent: Manifest) -> str:
    target = Path(parent.target_plist)
    return f".{target.name}.current-schema-repair-fence-{parent.transaction_id}-"


def _current_repair_quarantines(parent: Manifest) -> list[Path]:
    target = Path(parent.target_plist)
    prefixes = (
        _current_repair_quarantine_prefix(parent),
        _current_repair_publication_quarantine_prefix(parent),
    )
    quarantines = []
    with os.scandir(target.parent) as entries:
        for entry in entries:
            if not entry.name.startswith(prefixes):
                continue
            quarantines.append(Path(entry.path))
            if len(quarantines) > CURRENT_REPAIR_QUARANTINE_MAX_ENTRIES:
                raise ActivationError("repair plist quarantine entry limit exceeded")
    return sorted(quarantines)


def _current_repair_directory_token(parent: Manifest) -> tuple[int, ...]:
    metadata = os.stat(Path(parent.target_plist).parent, follow_symlinks=False)
    if not stat.S_ISDIR(metadata.st_mode):
        raise ActivationError("repair plist directory is not a directory")
    return tuple(
        getattr(metadata, field)
        for field in ("st_dev", "st_ino", "st_mode", "st_mtime_ns", "st_ctime_ns")
    )


def _current_repair_persistence_owned(
    repair: CurrentSchemaRepairManifest,
    parent: Manifest,
    *,
    reserve_attempt_headroom: bool = True,
) -> bool:
    try:
        target = Path(parent.target_plist)
        directory_token = _current_repair_directory_token(parent)
        quarantines = _current_repair_quarantines(parent)
        reserved_entries = CURRENT_REPAIR_QUARANTINE_HEADROOM_ENTRIES if reserve_attempt_headroom else 0
        reserved_bytes = CURRENT_REPAIR_QUARANTINE_HEADROOM_BYTES if reserve_attempt_headroom else 0
        if len(quarantines) > CURRENT_REPAIR_QUARANTINE_MAX_ENTRIES - reserved_entries:
            return False
        quarantine_tokens: dict[Path, CurrentRepairFileToken] = {}
        target_token: CurrentRepairFileToken | None = None
        for _pass in range(2):
            pass_tokens: dict[Path, CurrentRepairFileToken] = {}
            total_bytes = 0
            for item in quarantines:
                evidence = _regular_file_evidence(item, "repair plist quarantine")
                if evidence is None or evidence[0] != repair.plist_sha256:
                    return False
                total_bytes += evidence[1].size
                if total_bytes > CURRENT_REPAIR_QUARANTINE_MAX_TOTAL_BYTES - reserved_bytes:
                    return False
                pass_tokens[item] = evidence[1]
            if _current_repair_quarantines(parent) != quarantines:
                return False
            target_evidence = _regular_file_evidence(target, "published repair plist")
            if target_evidence is not None and target_evidence[0] != repair.plist_sha256:
                return False
            if _current_repair_quarantines(parent) != quarantines:
                return False
            quarantine_tokens = pass_tokens
            target_token = None if target_evidence is None else target_evidence[1]
        if any(_regular_file_path_token(item) != token for item, token in quarantine_tokens.items()):
            return False
        if _regular_file_path_token(target) != target_token:
            return False
        return _current_repair_directory_token(parent) == directory_token
    except Exception:
        return False


def _current_repair_target_absent(target: Path) -> bool:
    try:
        target.lstat()
    except FileNotFoundError:
        return True
    return False


def _quarantine_current_repair_persistence(parent: Manifest) -> bool:
    target = Path(parent.target_plist)
    try:
        for _attempt in range(CURRENT_REPAIR_QUARANTINE_ATTEMPTS):
            if _current_repair_target_absent(target):
                fsync_dir(target.parent)
                return _current_repair_target_absent(target)
            quarantine = target.parent / f"{_current_repair_quarantine_prefix(parent)}{secrets.token_hex(16)}"
            try:
                os.rename(target, quarantine)
            except FileNotFoundError:
                fsync_dir(target.parent)
                if _current_repair_target_absent(target):
                    return True
                continue
            fsync_dir(target.parent)
        fsync_dir(target.parent)
        return _current_repair_target_absent(target)
    except Exception:
        return False


def _record_current_repair_failure(path: Path, parent: Manifest, launchctl: Launchctl) -> None:
    _quarantine_current_repair_persistence(parent)
    stopped = False
    try:
        launchctl.stop()
    except Exception:
        pass
    try:
        require_launchd_absence(parent.label, parent.uid)
        stopped = True
    except Exception:
        stopped = False
    _quarantine_current_repair_persistence(parent)
    try:
        require_launchd_absence(parent.label, parent.uid)
        stopped = True
    except Exception:
        stopped = False
    persistence_fenced = _quarantine_current_repair_persistence(parent)
    truth = "failed_stopped" if stopped and persistence_fenced else "stop_unconfirmed"
    print(
        f"current-schema repair teardown: {truth}; persistent auto-start "
        f"{'absent' if persistence_fenced else 'unconfirmed'}; original claim retained",
        file=sys.stderr,
        flush=True,
    )
    try:
        durable = CurrentSchemaRepairStatus.load(path.parent / "status.json", sha256(path))
    except Exception:
        # No valid checkpoint exists from which to construct a resumable status.
        print("child status malformed; preserved unchanged, no terminal/checkpoint fabricated", file=sys.stderr, flush=True)
        return
    failed = dataclasses.replace(durable, state=truth, resume_from=durable.resume_from or durable.state,
                                 finalization_pending=False, failure="repair failed; parent claim retained")
    _write_repair_status(path, failed)


def _require_current_repair_persistent_route(parent: Manifest) -> None:
    expected = Path.home() / "Library" / "LaunchAgents" / f"{CURRENT_REPAIR_SERVICE_LABEL}.plist"
    if parent.label != CURRENT_REPAIR_SERVICE_LABEL or Path(parent.target_plist) != expected:
        raise ActivationError("repair parent persistent route differs from the canonical launchd target")


def _current_repair_routing(path: Path) -> tuple[CurrentSchemaRepairManifest, Manifest]:
    repair = CurrentSchemaRepairManifest.load(path)
    parent_path = Path(repair.parent_manifest_path)
    _repair_private(parent_path)
    verify_staged(str(parent_path), repair.parent_manifest_sha256, "original repair parent manifest")
    parent = Manifest.load(parent_path)
    validate_manifest_identities(parent)
    _require_current_repair_persistent_route(parent)
    if (parent.helper_label != f"{DEPLOY_HELPER_LABEL_PREFIX}.{parent.transaction_id}" or parent.uid != os.getuid()
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", parent.transaction_id)
            or not re.fullmatch(r"[A-Za-z0-9._-]+", parent.label)
            or parent_path != Path(parent.candidate_binary).parent / "manifest.json"):
        raise ActivationError("repair parent routing identity mismatch")
    for name in ("lock_path", "active_path"):
        route = Path(getattr(parent, name))
        if not route.is_absolute() or route != route.resolve():
            raise ActivationError("repair parent lock/claim routing mismatch")
    _regular_nosymlink(Path(parent.lock_path), "parent operation lock", required=False)
    return repair, parent


def stop_invalid_current_repair(path: Path) -> None:
    _repair, parent = _current_repair_routing(path)
    with Path(parent.lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another operation owns current-schema repair") from exc
        _require_current_repair_claim(parent)
        _record_current_repair_failure(path, parent, Launchctl(parent))


def activate_current_repair(path: Path) -> str:
    repair, parent = _current_repair_routing(path)
    with Path(parent.lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another operation owns current-schema repair") from exc
        try:
            validated_repair, validated_parent, status = validate_current_repair(path)
            _require_current_repair_persistent_route(validated_parent)
            repair, parent = validated_repair, validated_parent
        except Exception:
            # Only the original owner can authorize teardown using hash-bound routing.
            _require_current_repair_claim(parent)
            _record_current_repair_failure(path, parent, Launchctl(parent))
            raise
        runtime = _repair_runtime(parent, repair, path.parent)
        launchctl = Launchctl(runtime)
        phase = status.resume_from or status.state
        terminal = status.state == "committed" and not status.finalization_pending
        try:
            require_launchd_absence(parent.helper_label, parent.uid)
            reserve_attempt_headroom = status.state not in {"failed_stopped", "stop_unconfirmed"}
            if not terminal and not _current_repair_persistence_owned(
                repair,
                parent,
                reserve_attempt_headroom=reserve_attempt_headroom,
            ):
                raise ActivationError("persistent target plist is not absent or the exact sealed repair plist")
            pending_running = False
            if status.state == "committed" and status.finalization_pending:
                observed, pid = launchctl.inspect()
                pending_running = observed in {"running", "active"} and pid is not None
                if not pending_running and observed != "not_loaded":
                    raise ActivationError("pending repair runtime state is ambiguous")
            if terminal or pending_running:
                _repair_verify_installed(runtime)
                require_loaded_plist(runtime, launchctl, Path(runtime.candidate_plist))
                wait_for_identity(runtime, runtime.expected)
                if terminal:
                    if sha256(Path(runtime.target_plist)) != repair.plist_sha256 or Path(runtime.deployed_sha_path).read_text().strip() != repair.source_commit:
                        raise ActivationError("terminal repair publication drift")
                    owner = Path(parent.active_path).read_text().strip() if Path(parent.active_path).exists() else None
                    if owner == parent.transaction_id and not release_claim(parent):
                        raise ActivationError("terminal repair claim release failed")
                    return "committed"
            else:
                if phase == "prepared":
                    require_launchd_absence(parent.label, parent.uid)
                else:
                    launchctl.stop()
                    require_launchd_absence(parent.label, parent.uid)
                _repair_verify_database(parent, path.parent)
                _repair_capacity(parent, reservation_required=phase != "committed")
                atomic_install(Path(runtime.candidate_binary), Path(runtime.target_binary), 0o755)
                _repair_verify_installed(runtime)
                require_launchd_absence(parent.label, parent.uid)
                assert_database_exclusive(parent)
                if phase in {"prepared", "reconciliation_started"}:
                    status = dataclasses.replace(status, state="reconciliation_started", resume_from=None, failure=None)
                    _write_repair_status(path, status)
                    phase = "reconciliation_started"
                    environment = os.environ.copy()
                    environment.update(plistlib.loads(Path(runtime.candidate_plist).read_bytes())["EnvironmentVariables"])
                    result = subprocess.run([runtime.target_binary, "--migrate-only"], env=environment,
                                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                            timeout=7200, check=False)
                    if result.returncode != 0:
                        raise ActivationError("exact repair reconciliation failed")
                    _repair_verify_database(parent, path.parent)
                    _repair_capacity(parent, reservation_required=True)
                    proof = {"protocol_version": 1, "manifest_sha256": sha256(path),
                             "parent_proof_sha256": repair.parent_proof_sha256, "source_commit": repair.source_commit,
                             "registry_sha256": repair.registry_sha256,
                             "target_ledger": [list(item) for item in migration_registry(path.parent / "migrations.rs")]}
                    atomic_write(path.parent / "proof.json", _encode_database_proof(proof))
                    status = dataclasses.replace(status, state="target_verified", proof_sha256=sha256(path.parent / "proof.json"))
                    _write_repair_status(path, status)
                    phase = "target_verified"
                _repair_verify_database(parent, path.parent)
                _repair_verify_installed(runtime)
                if phase != "committed":
                    status = dataclasses.replace(status, state="start_attempted", resume_from=None, failure=None)
                    _write_repair_status(path, status)
                    phase = "start_attempted"
                launchctl.start(None, plist_path=runtime.candidate_plist)
                require_loaded_plist(runtime, launchctl, Path(runtime.candidate_plist))
                wait_for_identity(runtime, runtime.expected)
            status = dataclasses.replace(status, state="committed", resume_from=None, failure=None, finalization_pending=True)
            _write_repair_status(path, status)
            phase = "committed"
            _repair_capacity(parent, reservation_required=False)
            atomic_write(Path(runtime.deployed_sha_path), (repair.source_commit + "\n").encode())
            _publish_current_repair(runtime, parent)
            restore = _restore_capacity_path(parent, Path(parent.current_schema_upgrade.database_path))
            _regular_nosymlink(restore, "original restore reservation", required=False)
            restore.unlink(missing_ok=True)
            fsync_dir(restore.parent)
            status = dataclasses.replace(status, finalization_pending=False)
            _write_repair_status(path, status)
            terminal = True
            if not release_claim(parent):
                raise ActivationError("repair completed but exact original claim release failed")
            return "committed"
        except Exception:
            if terminal:
                raise
            _record_current_repair_failure(path, parent, launchctl)
            raise


def finalize_schema129_refresh(manifest: Manifest, private: Path) -> str:
    current = manifest.current_schema_upgrade
    assert isinstance(current, PreparedSchema129Refresh)
    if not _post_refresh_marker(manifest):
        raise ActivationError("refresh finalization requires durable no-old-fallback marker")
    launchctl = Launchctl(manifest)
    try:
        proof = _snapshot_proof(manifest)
        rehearsed = SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS
        if set(proof) not in (rehearsed, rehearsed | SOURCE128_START_KEYS):
            raise ActivationError("refresh finalization requires exact retained same-schema rehearsal proof")
        state, pid = launchctl.inspect()
        if state in {"running", "active"} and pid is not None:
            if "candidate_start_attempted_at" not in proof:
                raise ActivationError("running refresh candidate has no durable start-attempt proof")
            require_loaded_plist(manifest, launchctl, private)
            wait_for_identity(manifest, manifest.expected)
            quarantine_source128_target(manifest)
            launchctl.stop()
            quarantine_source128_target(manifest)
        elif state != "not_loaded" or pid is not None:
            raise ActivationError("refresh finalization requires a stopped candidate")
        require_launchd_absence(manifest.label, manifest.uid)
        quarantine_source128_target(manifest)
        assert_database_exclusive(manifest)
        database = Path(current.database_path)
        validate_current_schema_database(database, _current_schema_registry(manifest), manifest=manifest, migrated=True)
        if "candidate_start_attempted_at" not in proof and database_logical_digest(database) != proof["source_logical_digest"]:
            raise ActivationError("unstarted refresh live database differs from retained source")
        evidence = _repair_json(_capacity_observation_path(DatabaseCapacityReservation(
            Path(current.backup_path), _restore_capacity_path(manifest, database), 0,
        )))
        if (evidence.get("transaction_id") != manifest.transaction_id
                or type(evidence.get("reserved_capacity_bytes")) is not int
                or evidence["reserved_capacity_bytes"] <= 0):
            raise ActivationError("refresh restore capacity evidence does not match")
        reservation = DatabaseCapacityReservation(
            Path(current.backup_path), _restore_capacity_path(manifest, database),
            evidence["reserved_capacity_bytes"],
        )
        if not _source128_capacity_cleanup_authorized(manifest, reservation):
            _reservation_still_sufficient(manifest, reservation)
        if os.path.lexists(manifest.target_plist):
            raise ActivationError("persistent target plist must be absent before private start")
        if "candidate_start_attempted_at" not in proof:
            checkpoint_source128_start(manifest)
        launchctl.start(None, plist_path=str(private))
        require_loaded_plist(manifest, launchctl, private)
        wait_for_identity(manifest, manifest.expected)
        write_status(manifest, "committed", finalization_pending=True, committed_diagnostic="private refresh candidate verified; publication pending")
        atomic_write(Path(manifest.deployed_sha_path), (manifest.source_commit + "\n").encode(), 0o600)
        publication = prepare_plist_publication(private, Path(manifest.target_plist))
        publish_source128_plist(publication, Path(manifest.target_plist))
        release_source128_capacity_reservation(manifest, reservation)
        write_status(manifest, "committed")
        if not release_durable_terminal_claim(manifest, required_state="committed"):
            raise ActivationError("refresh finalization claim release was unconfirmed")
        return "committed"
    except Exception as exc:
        settle_source128_failure(manifest, launchctl, str(exc))
        raise


def finalize_source128(manifest: Manifest, private: Path) -> str:
    current = manifest.current_schema_upgrade
    assert isinstance(current, PreparedSource128Upgrade)
    launchctl = Launchctl(manifest)
    was_running = False
    try:
        proof = _snapshot_proof(manifest)
        if set(proof) not in (
            SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS,
            SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS | SOURCE128_MIGRATED_KEYS,
            SOURCE128_PROOF_BASE_KEYS | SOURCE128_REHEARSAL_KEYS | SOURCE128_MIGRATED_KEYS | SOURCE128_START_KEYS,
        ):
            raise ActivationError("source128 finalization requires exact retained rehearsal proof")
        state, pid = launchctl.inspect()
        was_running = state in {"running", "active"} and pid is not None
        if was_running:
            if "candidate_start_attempted_at" not in proof:
                raise ActivationError("running source128 candidate lacks durable start-attempt proof")
            require_loaded_plist(manifest, launchctl, private)
            wait_for_identity(manifest, manifest.expected)
            quarantine_source128_target(manifest)
            launchctl.stop()
            quarantine_source128_target(manifest)
            require_launchd_absence(manifest.label, manifest.uid)
            quarantine_source128_target(manifest)
        elif state != "not_loaded" or pid is not None:
            raise ActivationError("source128 finalization requires a stopped candidate")
    except Exception as exc:
        settle_source128_failure(manifest, launchctl, str(exc))
        raise
    try:
        require_launchd_absence(manifest.label, manifest.uid)
        assert_database_exclusive(manifest)
        database = Path(current.database_path)
        registry = _current_schema_registry(manifest)
        source = tuple(tuple(row) for row in proof["source_ledger"])
        target = current_schema_target_registry(manifest, registry)
        ledger = database_ledger(database)
        if ledger not in (source, target):
            raise ActivationError("source128 finalization rejects an unqualified live ledger")
        if ledger == source:
            validate_current_schema_database(database, registry, source_kind=manifest.source_kind, manifest=manifest)
            if database_logical_digest(database) != proof["source_logical_digest"] or "migrated_ledger" in proof:
                raise ActivationError("source128 live database differs from retained source")
        elif "candidate_start_attempted_at" in proof and proof.get("migrated_ledger") != [list(row) for row in target]:
            raise ActivationError("source128 start proof does not bind target ledger")
    except Exception as exc:
        settle_source128_failure(manifest, launchctl, str(exc))
        raise
    try:
        evidence = _repair_json(
            _capacity_observation_path(
                DatabaseCapacityReservation(
                    Path(current.backup_path),
                    _restore_capacity_path(manifest, database),
                    0,
                )
            )
        )
        if (evidence.get("transaction_id") != manifest.transaction_id
                or type(evidence.get("reserved_capacity_bytes")) is not int
                or evidence["reserved_capacity_bytes"] <= 0):
            raise ActivationError("source128 restore capacity evidence does not match")
        reservation = DatabaseCapacityReservation(
            Path(current.backup_path),
            _restore_capacity_path(manifest, database),
            evidence["reserved_capacity_bytes"],
        )
        if not (
            ledger == target
            and _source128_capacity_cleanup_authorized(manifest, reservation)
        ):
            _reservation_still_sufficient(manifest, reservation)
        if ledger == source:
            run_current_schema_migration(manifest, reservation)
        else:
            validate_current_schema_database(database, registry, source_kind=manifest.source_kind, manifest=manifest, migrated=True)
            digest = database_logical_digest(database)
            if "migrated_logical_digest" in proof:
                if (proof.get("migrated_ledger") != [list(row) for row in target]
                        or ("candidate_start_attempted_at" not in proof and proof["migrated_logical_digest"] != digest)):
                    raise ActivationError("source128 committed proof differs from live database")
            else:
                proof["migration_committed_at"] = utc_now()
                proof["migrated_ledger"] = [list(row) for row in target]
                proof["migrated_logical_digest"] = digest
                encoded = _encode_database_proof(proof)
                proof_path = Path(current.proof_path)
                _validate_database_proof_lifecycle_budget(
                    b"0" * proof_path.stat().st_size, encoded,
                    max(os.statvfs(proof_path.parent).f_frsize, 4096),
                )
                atomic_write(proof_path, encoded, 0o600)
        assert_database_exclusive(manifest)
        if os.path.lexists(manifest.target_plist):
            raise ActivationError("persistent target plist must be absent before private start")
        if "candidate_start_attempted_at" not in _snapshot_proof(manifest):
            checkpoint_source128_start(manifest)
        launchctl.start(None, plist_path=str(private))
        require_loaded_plist(manifest, launchctl, private)
        wait_for_identity(manifest, manifest.expected)
        write_status(manifest, "committed", finalization_pending=True, committed_diagnostic="private candidate verified; publication pending")
        atomic_write(Path(manifest.deployed_sha_path), (manifest.source_commit + "\n").encode(), 0o600)
        publication = prepare_plist_publication(private, Path(manifest.target_plist))
        publish_source128_plist(publication, Path(manifest.target_plist))
        release_source128_capacity_reservation(
            manifest,
            DatabaseCapacityReservation(
                Path(current.backup_path),
                _restore_capacity_path(manifest, database),
                0,
            ),
        )
        write_status(manifest, "committed")
        if not release_durable_terminal_claim(manifest, required_state="committed"):
            raise ActivationError("source128 finalization claim release was unconfirmed")
        return "committed"
    except Exception as exc:
        settle_source128_failure(manifest, launchctl, str(exc))
        raise


def finalize_paired(manifest: Manifest, *, current_schema: bool = False) -> str:
    refuse_selected_current_repair(manifest)
    with Path(manifest.lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment owns finalization") from exc
        validate_manifest_identities(manifest)
        validate_manifest_mode(manifest)
        upgrade = prepared_database_upgrade(manifest)
        if upgrade is None:
            raise ActivationError("finalization requires a prepared database transaction")
        refuse_selected_current_repair(manifest)
        if current_schema != (manifest.current_schema_upgrade is not None):
            raise ActivationError("finalization command does not match prepared database mode")
        status = read_status(manifest)
        validate_status_manifest_mode(manifest, status)
        allowed_states = ({"committed", "migration_started", "activation_failed_database_committed"}
                          if _protected_129(manifest)
                          else {"committed", "migration_started"} if current_schema else {"committed"})
        if status.get("transaction_id") != manifest.transaction_id or status.get("state") not in allowed_states:
            raise ActivationError("finalization requires matching committed database evidence")
        old_helper = subprocess.run(["launchctl", "print", f"gui/{manifest.uid}/{manifest.helper_label}"], capture_output=True, text=True)
        if old_helper.returncode == 0 or not service_absence_confirmed(old_helper.stdout + "\n" + old_helper.stderr, manifest.helper_label, manifest.uid):
            raise ActivationError("activation helper absence unconfirmed")
        claim = Path(manifest.active_path)
        owner = claim.read_text().strip() if claim.exists() else None
        if owner != manifest.transaction_id and (owner is not None or status.get("finalization_pending")):
            raise ActivationError("finalization claim mismatch")
        if sha256(Path(__file__)) != upgrade.controller_helper_sha256:
            raise ActivationError("retained helper checksum mismatch")
        verify_staged(manifest.target_binary, manifest.candidate_binary_sha256, "committed candidate binary")
        private = verify_staged(manifest.candidate_plist, manifest.candidate_plist_sha256, "committed private plist")
        if _protected_129(manifest):
            expected_status = {
                "transaction_id": manifest.transaction_id, "source_kind": manifest.source_kind,
                "source_commit": manifest.source_commit, "release_tag": None, "release_commit": None,
                "expected_version": manifest.expected.version, "expected_git_sha": manifest.expected.git_sha,
                "created_at": manifest.created_at, "finalization_pending": True,
                "prepared_mode": "current_schema", "precondition_cleanup_completed": False,
                "recovery_mode": None, "cleanup_failure": None,
            }
            if (owner != manifest.transaction_id or Path(manifest.active_path).is_symlink()
                    or set(status) != set(expected_status) | {"state", "updated_at", "failure", "rollback_failure", "committed_diagnostic"}
                    or any(type(status[key]) is not type(value) or status[key] != value for key, value in expected_status.items())):
                raise ActivationError("source128 finalization requires exact retained claim and status")
            if _schema129_refresh(manifest):
                return finalize_schema129_refresh(manifest, private)
            return finalize_source128(manifest, private)
        if manifest.current_schema_upgrade is not None:
            proof = _snapshot_proof(manifest)
            if proof.get("migration_committed_at") is None:
                raise ActivationError("current-schema migration commit proof is unavailable")
            validate_current_schema_database(
                Path(manifest.current_schema_upgrade.database_path),
                _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest,
                migrated=True,
            )
        if owner is None and not status.get("finalization_pending"):
            if not Path(manifest.target_plist).samefile(private):
                raise ActivationError("completed publication identity mismatch")
            return "committed"
        launchctl = Launchctl(manifest)
        state, pid = launchctl.inspect()
        if state not in {"running", "active"} or pid is None:
            if not current_schema or state != "not_loaded":
                raise ActivationError("committed candidate is not running")
            assert_database_exclusive(manifest)
            launchctl.start(None, plist_path=str(private))
        require_loaded_plist(manifest, launchctl, private)
        wait_for_identity(manifest, manifest.expected)
        write_status(manifest, "committed", finalization_pending=True, committed_diagnostic="prepared finalization retry pending")
        try:
            reservation = _restore_capacity_path(manifest, Path(upgrade.database_path))
            if reservation.is_symlink():
                raise ActivationError("restore reservation is symlink")
            reservation.unlink(missing_ok=True)
            fsync_dir(reservation.parent)
            atomic_write(
                Path(manifest.deployed_sha_path),
                (manifest.source_commit + "\n").encode(),
                0o600,
            )
            publication = prepare_plist_publication(private, Path(manifest.target_plist))
            commit_atomic_install(publication, Path(manifest.target_plist))
            write_status(manifest, "committed")
        except Exception as exc:
            write_status(manifest, "committed", finalization_pending=True, committed_diagnostic=f"finalization failed: {exc}")
            raise
        release_claim(manifest)
        return "committed"


def resume_forward(manifest: Manifest) -> str:
    with Path(manifest.lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment owns forward recovery") from exc
        validate_manifest_identities(manifest)
        validate_manifest_mode(manifest)
        if manifest.ordinary_forward_migration is None:
            raise ActivationError("forward recovery requires an ordinary forward transaction")
        ordinary = manifest.ordinary_forward_migration
        registry = verify_staged(ordinary.migration_registry_path, ordinary.migration_registry_sha256, "ordinary candidate migration registry")
        target = ordinary_forward_target(ordinary, registry)
        if ordinary.migration_registry_sha256 == ORDINARY_SOURCE129_TARGET131_REGISTRY_SHA256:
            ledger = database_ledger(Path(ordinary.database_path))
            if ledger not in tuple(target[:length] for length in range(len(target) - 2, len(target) + 1)):
                raise ActivationError("forward recovery requires exact source129, partial130 or target131 ledger")
            validate_database(Path(ordinary.database_path))
        old_helper = subprocess.run(
            ["launchctl", "print", f"gui/{manifest.uid}/{manifest.helper_label}"],
            capture_output=True,
            text=True,
        )
        if old_helper.returncode == 0 or not service_absence_confirmed(
            old_helper.stdout + "\n" + old_helper.stderr,
            manifest.helper_label,
            manifest.uid,
        ):
            raise ActivationError("activation helper absence unconfirmed")
        status = read_status(manifest)
        validate_status_manifest_mode(manifest, status)
        if status.get("transaction_id") != manifest.transaction_id or status.get("state") not in {
            "candidate_may_migrate",
            "ordinary_activation_failed_forward_only",
        }:
            raise ActivationError("forward recovery requires matching unresolved status")
        claim = Path(manifest.active_path)
        if not claim.exists() or claim.read_text().strip() != manifest.transaction_id:
            raise ActivationError("forward recovery claim mismatch")
        verify_staged(manifest.target_binary, manifest.candidate_binary_sha256, "forward candidate binary")
        verify_staged(manifest.target_plist, manifest.candidate_plist_sha256, "forward candidate plist")
        launchctl = Launchctl(manifest)
        state, pid = launchctl.inspect()
        if state != "not_loaded" or pid is not None:
            raise ActivationError("forward recovery requires a stopped target")
        try:
            launchctl.start(None)
            wait_for_identity(manifest, manifest.expected)
            atomic_write(
                Path(manifest.deployed_sha_path),
                (manifest.source_commit + "\n").encode(),
                0o600,
            )
            write_status(manifest, "committed")
        except BaseException as exc:
            teardown_failure = None
            try:
                launchctl.stop()
                state, pid = launchctl.inspect()
                if state != "not_loaded" or pid is not None:
                    raise ActivationError("forward candidate teardown is unconfirmed")
            except Exception as teardown_exc:
                teardown_failure = str(teardown_exc)
            write_status(
                manifest,
                "ordinary_activation_failed_forward_only",
                failure=str(exc),
                rollback_failure=teardown_failure,
                finalization_pending=True,
                committed_diagnostic="forward candidate recovery failed; predecessor launch remains forbidden",
            )
            raise
        return "committed"


def recover_paired(manifest: Manifest, *, current_schema: bool = False) -> str:
    refuse_selected_current_repair(manifest)
    with Path(manifest.lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment operation owns recovery") from exc
        validate_manifest_identities(manifest)
        validate_manifest_mode(manifest)
        upgrade = prepared_database_upgrade(manifest)
        if upgrade is None:
            raise ActivationError("prepared recovery requires a prepared database transaction")
        if current_schema != (manifest.current_schema_upgrade is not None):
            raise RecoveryModeMismatch(
                "recovery command does not match prepared database mode"
            )
        claim = Path(manifest.active_path)
        try:
            status = read_status(manifest)
        except (OSError, json.JSONDecodeError):
            status = {}
        validate_status_manifest_mode(manifest, status)
        if _schema129_refresh(manifest) and (_post_refresh_marker(manifest) or _refresh_start_proof(manifest)):
            raise ActivationError("post-marker refresh is candidate-only; use finalize-current")
        if claim.read_text().strip() != manifest.transaction_id or status.get("transaction_id") != manifest.transaction_id or (status.get("state") not in {"activating", "activation_failed_rollback_failed"} and not (status.get("state") == "activation_failed_rolled_back" and status.get("recovery_mode") is not None)):
            raise ActivationError("paired recovery must own a retained unresolved transaction")
        launchctl = Launchctl(manifest)
        try:
            if status.get("recovery_mode") is not None:
                verify_staged(manifest.target_binary, manifest.rollback_binary_sha256, "running predecessor binary")
                private = verify_staged(manifest.rollback_plist, manifest.rollback_plist_sha256, "private predecessor plist")
                state, pid = launchctl.inspect()
                if state not in {"running", "active"} or pid is None:
                    raise ActivationError("post-restore checkpoint has no running predecessor; refuse snapshot replay")
                require_loaded_plist(manifest, launchctl, private)
                wait_for_identity(manifest, manifest.previous, health_url=manifest.previous_health_url, health_insecure_tls=manifest.previous_health_insecure_tls, health_json=manifest.previous_health_json)
                try:
                    restore_deployed_sha(manifest)
                    publication = prepare_plist_publication(private, Path(manifest.target_plist))
                    commit_atomic_install(publication, Path(manifest.target_plist))
                except Exception as exc:
                    raise VerifiedPredecessorFinalizationError(str(exc)) from exc
                finalize_verified_predecessor(manifest, status.get("failure"))
                return "activation_failed_rolled_back"
            snapshot_exists = Path(upgrade.proof_path).exists() or Path(upgrade.proof_path).is_symlink()
            if not snapshot_exists:
                plist = captured_paired_plist(manifest)
                if plist is None:
                    raise ActivationError("captured predecessor configuration is unverified")
                binary = Path(manifest.target_binary)
                if not binary.exists() or sha256(binary) != manifest.rollback_binary_sha256:
                    raise ActivationError("interrupted transaction has no snapshot proof and runtime changes are unresolved")
                if manifest.paired_database_upgrade is not None:
                    validate_legacy_database(Path(upgrade.database_path))
                else:
                    validate_current_schema_database(
                        Path(upgrade.database_path), _current_schema_registry(manifest), source_kind=manifest.source_kind, manifest=manifest
                    )
                    verify_source128_unchanged_without_snapshot(manifest)
                restore(
                    manifest,
                    launchctl,
                    None,
                    database_snapshot=False,
                    candidate_binary_mutated=False,
                    rollback_plist=plist,
                )
            else:
                restore(manifest, launchctl, None)
            finalize_verified_predecessor(manifest, status.get("failure"))
            return "activation_failed_rolled_back"
        except Exception as exc:
            failure = str(exc)
            if not isinstance(exc, VerifiedPredecessorFinalizationError):
                try:
                    launchctl.stop()
                    state, pid = launchctl.inspect()
                    if state != "not_loaded" or pid is not None:
                        raise ActivationError("paired recovery teardown is unconfirmed")
                except Exception as teardown:
                    failure += f"; recovery teardown failed: {teardown}"
            record_recovery_error(manifest, exc if isinstance(exc, VerifiedPredecessorFinalizationError) else failure, quarantine=True)
            return "activation_failed_rollback_failed"


def status_is_durable_terminal(manifest: Manifest) -> bool:
    if isinstance(getattr(manifest, "candidate_binary", None), str):
        repair_entry = current_repair_entry(manifest)
        if os.path.lexists(repair_entry) or repair_entry.parent.is_symlink():
            return False
    try:
        status = read_status(manifest)
        return (
            status.get("transaction_id") == manifest.transaction_id
            and status.get("state") in TERMINAL_STATES
            and not status.get("finalization_pending", False)
            and not (
                prepared_database_upgrade(manifest) is not None
                and status.get("state") == "activation_failed_rollback_failed"
            )
        )
    except (OSError, json.JSONDecodeError):
        return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "command",
        nargs="?",
        choices=[
            "activate",
            "cleanup-paired",
            "cleanup-current",
            "recover-paired",
            "recover-current",
            "finalize-paired",
            "finalize-current",
            "resume-forward",
            "activate-current-repair",
            "resume-current-repair",
        ],
    )
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("--probe-service-absence")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--helper-label")
    parser.add_argument("--uid", type=int)
    args = parser.parse_args()
    if args.protocol_version:
        print(HANDOFF_PROTOCOL_VERSION)
        return 0
    if args.probe_service_absence is not None and args.uid is not None:
        result = subprocess.run(["launchctl", "print", f"gui/{args.uid}/{args.probe_service_absence}"], capture_output=True, text=True)
        return 0 if result.returncode != 0 and service_absence_confirmed(result.stdout + "\n" + result.stderr, args.probe_service_absence, args.uid) else 1
    if args.manifest is None or args.helper_label is None or args.uid is None:
        parser.error("activation requires --manifest, --helper-label, and --uid")
    if args.command in {"activate-current-repair", "resume-current-repair"}:
        verified_repair_label = None
        try:
            repair, parent = _current_repair_routing(args.manifest)
            expected_label = f"{DEPLOY_HELPER_LABEL_PREFIX}.{parent.transaction_id}.current-schema-repair"
            if (repair.helper_label != expected_label or args.helper_label != expected_label
                    or args.uid != parent.uid or args.uid != os.getuid()):
                raise ActivationError("repair helper invocation identity mismatch")
            verified_repair_label = expected_label
            state = activate_current_repair(args.manifest)
            print(state, flush=True)
            return 0
        except Exception as exc:
            print(f"current-schema repair refused/failed: {exc}", file=sys.stderr)
            return 1
        finally:
            if verified_repair_label is not None:
                try:
                    request_helper_bootout(args.uid, verified_repair_label)
                except Exception:
                    pass
    manifest = None
    verified_helper_label: Optional[str] = None
    try:
        manifest_path = Path(args.manifest)
        if args.command in {"cleanup-paired", "cleanup-current"} and (
            not manifest_path.is_file() or manifest_path.is_symlink()
        ):
            raise CleanupPreconditionRejected(
                "precondition cleanup requires a regular non-symlink manifest"
            )
        manifest = Manifest.load(manifest_path)
        identity_rejection = (
            CleanupPreconditionRejected
            if args.command in {"cleanup-paired", "cleanup-current"}
            else ActivationError
        )
        if manifest.helper_label != args.helper_label or manifest.uid != args.uid:
            raise identity_rejection("helper identity does not match the immutable manifest")
        expected_helper_label = (
            f"{DEPLOY_HELPER_LABEL_PREFIX}.{manifest.transaction_id}"
        )
        if manifest.helper_label != expected_helper_label:
            raise identity_rejection("immutable manifest has an invalid helper label")
        verified_helper_label = expected_helper_label
        if args.command in {"cleanup-paired", "cleanup-current"}:
            state = cleanup_precondition_capacity(
                manifest, current_schema=args.command == "cleanup-current"
            )
        elif args.command in {"finalize-paired", "finalize-current"}:
            state = finalize_paired(
                manifest, current_schema=args.command == "finalize-current"
            )
        elif args.command in {"recover-paired", "recover-current"}:
            state = recover_paired(
                manifest, current_schema=args.command == "recover-current"
            )
        elif args.command == "resume-forward":
            state = resume_forward(manifest)
        else:
            state = activate(manifest)
        if state in TERMINAL_STATES:
            released = release_durable_terminal_claim(manifest)
            if args.command in {"cleanup-paired", "cleanup-current"} and not released:
                raise ActivationError(
                    "precondition cleanup completed but matching claim release failed"
                )
        print(state, flush=True)
        return 0 if (
            state == "committed"
            or (
                args.command in {"cleanup-paired", "cleanup-current"}
                and state == "precondition_failed"
            )
            or (
                args.command in {"recover-paired", "recover-current"}
                and state == "activation_failed_rolled_back"
            )
        ) else 1
    except ConcurrentDeploy as exc:
        protected_recovery = {
            "cleanup-paired",
            "cleanup-current",
            "finalize-paired",
            "finalize-current",
            "resume-forward",
        }
        if (
            manifest is not None
            and prepared_database_upgrade(manifest) is None
            and args.command not in protected_recovery
        ):
            try:
                if args.command in {"recover-paired", "recover-current"}:
                    record_recovery_error(manifest, str(exc))
                else:
                    write_status(manifest, "rejected_concurrent", failure=str(exc))
            finally:
                if status_is_durable_terminal(manifest):
                    release_claim(manifest)
        print(f"activation helper failed: {exc}", file=sys.stderr)
        return 1
    except Exception as exc:
        if (
            manifest is not None
            and args.command in {"cleanup-paired", "cleanup-current"}
            and not isinstance(exc, CleanupPreconditionRejected)
        ):
            try:
                cleanup_status = read_status(manifest)
                original_failure = cleanup_status.get("failure")
                write_status(
                    manifest,
                    "precondition_cleanup_pending",
                    failure=(
                        original_failure
                        if isinstance(original_failure, str)
                        else None
                    ),
                    cleanup_failure=str(exc),
                    precondition_cleanup_completed=False,
                )
            except Exception:
                pass
        if manifest is not None and args.command in {"recover-paired", "recover-current"}:
            try:
                recovery_status = read_status(manifest)
            except Exception:
                recovery_status = {}
            cleanup_only_state = (
                recovery_status.get("state") == "precondition_cleanup_pending"
                or (
                    recovery_status.get("state") == "precondition_failed"
                    and recovery_status.get("precondition_cleanup_completed") is True
                )
            )
            if not cleanup_only_state and not isinstance(exc, RecoveryModeMismatch):
                record_recovery_error(manifest, exc)
        if manifest is not None and args.command not in {
            "cleanup-paired",
            "cleanup-current",
            "finalize-paired",
            "finalize-current",
            "resume-forward",
        }:
            if prepared_database_upgrade(manifest) is None:
                if status_is_durable_terminal(manifest):
                    release_claim(manifest)
            elif (
                args.command in {None, "activate"}
                and isinstance(exc, CurrentSchemaPreconditionFailure)
            ):
                try:
                    release_durable_terminal_claim(
                        manifest, required_state="precondition_failed"
                    )
                except Exception as release_exc:
                    try:
                        terminal_status = read_status(manifest)
                        original_failure = terminal_status.get("failure")
                        write_status(
                            manifest,
                            "precondition_cleanup_pending",
                            failure=(
                                original_failure
                                if isinstance(original_failure, str)
                                else None
                            ),
                            cleanup_failure=str(release_exc),
                            precondition_cleanup_completed=False,
                        )
                    except Exception:
                        pass
                    print(
                        "activation helper could not release durable precondition claim: "
                        f"{release_exc}",
                        file=sys.stderr,
                    )
        print(f"activation helper failed: {exc}", file=sys.stderr)
        return 1
    finally:
        if (
            args.command not in {"finalize-paired", "finalize-current"}
            and verified_helper_label is not None
        ):
            request_helper_bootout(args.uid, verified_helper_label)


if __name__ == "__main__":
    raise SystemExit(main())
