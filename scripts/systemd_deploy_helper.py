#!/usr/bin/env python3
"""Root-owned transactional systemd activation helper."""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import hashlib
import ipaddress
import json
import os
import pwd
import re
import shutil
import sqlite3
import shlex
from contextlib import closing
import ssl
import subprocess
import tempfile
import time
import stat
import sys
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Optional

HANDOFF_PROTOCOL_VERSION = 1
PRODUCTION_UNIT = "phoenix-ide"
PRODUCTION_TRANSACTION_ROOT = Path("/var/lib/phoenix-ide-deploy/transactions")
PRODUCTION_BINARY = Path("/opt/phoenix-ide/phoenix-ide")
PRODUCTION_SERVICE = Path("/etc/systemd/system/phoenix-ide.service")
PRODUCTION_SOCKET = Path("/etc/systemd/system/phoenix-ide.socket")
PRODUCTION_ENV = Path("/etc/phoenix-ide/phoenix.env")
PRODUCTION_DEPLOYED_SHA = Path("/var/lib/phoenix-ide/deployed.sha")
UNIT_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.@-]{0,127}")
TRANSACTION_RE = re.compile(r"[0-9a-f]{32}")
SHA256_RE = re.compile(r"[0-9a-f]{64}")
GIT_SHA_RE = re.compile(r"[0-9a-f]{40}")
LEGACY_GIT_SHA_RE = re.compile(r"[0-9a-f]{12}")
VERSION_RE = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}")
RELEASE_TAG_RE = re.compile(r"v[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}")
SOURCE_KINDS = {"local_head", "published_release", "prepared_artifact"}


class ValidationError(RuntimeError):
    pass


PREPARED_COMMIT = "237100c79d9ce672867b284167a099fa0b7a2bcf"
PREPARED_VERSION = "0.13.0-rc.4"
PREPARED_TARGET = "x86_64-unknown-linux-musl"
PREPARED_REGISTRY_SHA256 = "a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6"
PREPARED_OPERATION = "prepare-incident-237100-linux-x86_64"
PREPARED_PAYLOAD = "phoenix_ide-x86_64-unknown-linux-musl-prepared-237100c79d9c"
PREPARED_RECEIPT = "LINUX-PREPARATION-RECEIPT-x86_64-unknown-linux-musl.json"
PREPARED_HANDOFF_PROTOCOL_VERSION = 3


def parse_prepared_receipt(data: bytes) -> dict:
    def unique_object(pairs: list[tuple[str, object]]) -> dict:
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValidationError("Linux preparation receipt has duplicate keys")
            value[key] = item
        return value

    try:
        receipt = json.loads(data, object_pairs_hook=unique_object)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ValidationError("invalid Linux preparation receipt JSON") from exc
    if (type(receipt) is not dict or set(receipt) != {
            "schema", "operation", "target", "version", "commit", "controller_commit",
            "registry_sha256", "run_id", "run_attempt", "event", "ref", "sha256", "checks",
        } or type(receipt["sha256"]) is not dict
            or set(receipt["sha256"]) != {PREPARED_PAYLOAD}
            or type(receipt["checks"]) is not dict
            or set(receipt["checks"]) != {"clean_source", "static_elf", "embedded_identity"}
            or receipt["schema"] != 1 or type(receipt["schema"]) is not int
            or receipt["operation"] != PREPARED_OPERATION or receipt["target"] != PREPARED_TARGET
            or receipt["version"] != PREPARED_VERSION or receipt["commit"] != PREPARED_COMMIT
            or type(receipt["controller_commit"]) is not str
            or GIT_SHA_RE.fullmatch(receipt["controller_commit"]) is None
            or receipt["controller_commit"] == PREPARED_COMMIT
            or receipt["registry_sha256"] != PREPARED_REGISTRY_SHA256
            or any(type(receipt[field]) is not int or receipt[field] < 1 for field in ("run_id", "run_attempt"))
            or receipt["event"] != "workflow_dispatch" or receipt["ref"] != "refs/heads/main"
            or type(receipt["sha256"][PREPARED_PAYLOAD]) is not str
            or SHA256_RE.fullmatch(receipt["sha256"][PREPARED_PAYLOAD]) is None
            or receipt["checks"] != {"clean_source": "verified", "static_elf": "verified", "embedded_identity": "verified"}):
        raise ValidationError("Linux preparation receipt has invalid provenance or shape")
    return receipt


@dataclasses.dataclass(frozen=True)
class Identity:
    version: str
    git_sha: str


@dataclasses.dataclass(frozen=True)
class Artifact:
    path: str
    sha256: str


@dataclasses.dataclass(frozen=True)
class OptionalArtifact:
    path: Optional[str]
    sha256: Optional[str]


@dataclasses.dataclass(frozen=True)
class SystemdTargets:
    binary: str
    service: str
    socket: str
    environment: str
    deployed_sha: str


@dataclasses.dataclass(frozen=True)
class CandidateArtifacts:
    binary: Artifact
    service: Artifact
    socket: Artifact
    environment: OptionalArtifact


@dataclasses.dataclass(frozen=True)
class RollbackArtifacts:
    binary: OptionalArtifact
    service: OptionalArtifact
    socket: OptionalArtifact
    environment: OptionalArtifact


@dataclasses.dataclass(frozen=True)
class Manifest:
    manifest_version: int
    transaction_id: str
    unit_name: str
    service_user: str
    source_kind: str
    source_commit: str
    release_tag: Optional[str]
    release_commit: Optional[str]
    expected: Identity
    previous: Optional[Identity]
    expected_health_url: str
    previous_health_url: Optional[str]
    candidate: CandidateArtifacts
    rollback: RollbackArtifacts
    targets: SystemdTargets
    status_path: str
    active_path: str
    activation_lock_path: str
    claim_lock_path: str
    previous_deployed_sha: Optional[str]
    created_at: str
    transition_timeout_secs: float = 30.0
    health_timeout_secs: float = 30.0
    data_directory_preexisting: bool = True
    mode: str = "ordinary"
    controller_source_commit: Optional[str] = None
    controller_helper: Optional[Artifact] = None
    registry: Optional[Artifact] = None
    database_path: Optional[str] = None
    release_asset_name: Optional[str] = None
    release_asset_sha256: Optional[str] = None
    prepared_artifact_receipt: Optional[Artifact] = None

    @classmethod
    def load(cls, path: Path) -> "Manifest":
        try:
            raw = json.loads(path.read_text())
            if raw.get("source_kind") == "prepared_artifact":
                if set(raw) & {"release_tag", "release_commit", "release_asset_name", "release_asset_sha256"}:
                    raise ValidationError("prepared source manifest contains release metadata")
                raw["release_tag"] = None
                raw["release_commit"] = None
            raw["expected"] = Identity(**raw["expected"])
            raw["previous"] = Identity(**raw["previous"]) if raw.get("previous") else None
            raw["candidate"] = CandidateArtifacts(
                binary=Artifact(**raw["candidate"]["binary"]),
                service=Artifact(**raw["candidate"]["service"]),
                socket=Artifact(**raw["candidate"]["socket"]),
                environment=OptionalArtifact(**raw["candidate"]["environment"]),
            )
            raw["rollback"] = RollbackArtifacts(**{
                name: OptionalArtifact(**raw["rollback"][name])
                for name in ("binary", "service", "socket", "environment")
            })
            for field in ("registry", "controller_helper", "prepared_artifact_receipt"):
                if raw.get(field) is not None:
                    raw[field] = Artifact(**raw[field])
            raw["targets"] = SystemdTargets(**raw["targets"])
            return cls(**raw)
        except (KeyError, TypeError, json.JSONDecodeError) as exc:
            raise ValidationError("invalid systemd handoff manifest shape") from exc


@dataclasses.dataclass(frozen=True)
class ValidationPolicy:
    transaction_root: Path
    unit_name: str
    targets: SystemdTargets
    status_path: Path
    active_path: Path
    activation_lock_path: Path
    claim_lock_path: Path
    owner_uid: int = 0
    maximum_mode: int = 0o700

    @classmethod
    def production(cls) -> "ValidationPolicy":
        return cls(
            transaction_root=PRODUCTION_TRANSACTION_ROOT,
            unit_name=PRODUCTION_UNIT,
            targets=SystemdTargets(
                binary=str(PRODUCTION_BINARY),
                service=str(PRODUCTION_SERVICE),
                socket=str(PRODUCTION_SOCKET),
                environment=str(PRODUCTION_ENV),
                deployed_sha=str(PRODUCTION_DEPLOYED_SHA),
            ),
            status_path=PRODUCTION_TRANSACTION_ROOT.parent / "status.json",
            active_path=PRODUCTION_TRANSACTION_ROOT.parent / "active",
            activation_lock_path=PRODUCTION_TRANSACTION_ROOT.parent / "activation.lock",
            claim_lock_path=PRODUCTION_TRANSACTION_ROOT.parent / "claim.lock",
        )


def sha256_fd(fd: int) -> str:
    digest = hashlib.sha256()
    os.lseek(fd, 0, os.SEEK_SET)
    while chunk := os.read(fd, 1024 * 1024):
        digest.update(chunk)
    return digest.hexdigest()


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return sha256_fd(stream.fileno())


def require_descendant(path: Path, parent: Path, description: str) -> Path:
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(parent.resolve(strict=True))
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        raise ValidationError(f"{description} is outside the root-owned transaction") from exc
    return resolved


def validate_root_owned_tree(path: Path, stop: Path, policy: ValidationPolicy) -> None:
    stop = stop.resolve(strict=True)
    current = path.resolve(strict=True)
    while True:
        metadata = current.stat()
        if not stat.S_ISDIR(metadata.st_mode):
            raise ValidationError("transaction path contains a non-directory ancestor")
        if metadata.st_uid != policy.owner_uid:
            raise ValidationError("transaction directory has unexpected owner")
        if stat.S_IMODE(metadata.st_mode) & 0o022:
            raise ValidationError("transaction directory is group or world writable")
        if current == stop:
            return
        if stop not in current.parents:
            raise ValidationError("transaction path escapes its root")
        current = current.parent


def validate_regular_file(path_value: str, expected_hash: str, root: Path, policy: ValidationPolicy, description: str) -> Path:
    if not SHA256_RE.fullmatch(expected_hash):
        raise ValidationError(f"{description} has malformed checksum")
    path = Path(path_value)
    resolved = require_descendant(path, root, description)
    validate_root_owned_tree(resolved.parent, policy.transaction_root, policy)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise ValidationError(f"{description} cannot be opened safely") from exc
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode):
            raise ValidationError(f"{description} is not a regular non-symlink file")
        if metadata.st_uid != policy.owner_uid:
            raise ValidationError(f"{description} has unexpected owner")
        if stat.S_IMODE(metadata.st_mode) & ~policy.maximum_mode:
            raise ValidationError(f"{description} permissions are too broad")
        if sha256_fd(fd) != expected_hash:
            raise ValidationError(f"{description} checksum mismatch")
    finally:
        os.close(fd)
    return resolved


def validate_optional_artifact(artifact: OptionalArtifact, root: Path, policy: ValidationPolicy, description: str) -> Optional[Path]:
    if artifact.path is None and artifact.sha256 is None:
        return None
    if artifact.path is None or artifact.sha256 is None:
        raise ValidationError(f"{description} path and checksum must both be present")
    return validate_regular_file(artifact.path, artifact.sha256, root, policy, description)


def validate_service_user(name: str) -> None:
    try:
        account = pwd.getpwnam(name)
    except KeyError as exc:
        raise ValidationError("service user does not exist") from exc
    if account.pw_uid == 0:
        raise ValidationError("service user must not be root")


def validate_candidate_identity(identity: Identity, source_commit: str) -> None:
    if not VERSION_RE.fullmatch(identity.version):
        raise ValidationError("candidate identity has malformed version")
    if not GIT_SHA_RE.fullmatch(identity.git_sha):
        raise ValidationError("candidate identity must contain a full lowercase git SHA")
    if identity.git_sha != source_commit:
        raise ValidationError("candidate identity does not match source commit")


def validate_previous_identity(identity: Identity) -> None:
    if not VERSION_RE.fullmatch(identity.version):
        raise ValidationError("previous identity has malformed version")
    if not (LEGACY_GIT_SHA_RE.fullmatch(identity.git_sha) or GIT_SHA_RE.fullmatch(identity.git_sha)):
        raise ValidationError("previous identity has malformed git SHA")


def validate_health_url(value: Optional[str], description: str) -> None:
    if value is None:
        return
    parsed = urllib.parse.urlparse(value)
    try:
        loopback = parsed.hostname == "localhost" or ipaddress.ip_address(parsed.hostname or "").is_loopback
    except ValueError:
        loopback = False
    if parsed.scheme not in {"http", "https"} or not loopback:
        raise ValidationError(f"{description} must be a loopback HTTP endpoint")
    if parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path != "/api/version":
        raise ValidationError(f"{description} must be a credential-free /api/version endpoint")
    try:
        if parsed.port is None or not 1 <= parsed.port <= 65535:
            raise ValidationError(f"{description} has invalid port")
    except ValueError as exc:
        raise ValidationError(f"{description} has invalid port") from exc


def validate_manifest(manifest_path: Path, manifest: Manifest, policy: ValidationPolicy) -> None:
    expected_protocol = (PREPARED_HANDOFF_PROTOCOL_VERSION if manifest.source_kind == "prepared_artifact"
                         else PROTECTED_HANDOFF_PROTOCOL_VERSION if manifest.mode == PROTECTED_MODE
                         else HANDOFF_PROTOCOL_VERSION)
    if type(manifest.manifest_version) is not int or manifest.manifest_version != expected_protocol:
        raise ValidationError(
            f"unsupported handoff protocol {manifest.manifest_version!r}; expected {expected_protocol}"
        )
    if not TRANSACTION_RE.fullmatch(manifest.transaction_id):
        raise ValidationError("malformed transaction id")
    if not UNIT_RE.fullmatch(manifest.unit_name) or manifest.unit_name != policy.unit_name:
        raise ValidationError("unit name is not allowed")
    if not GIT_SHA_RE.fullmatch(manifest.source_commit):
        raise ValidationError("source commit must be a full lowercase git SHA")
    if manifest.source_kind not in SOURCE_KINDS:
        raise ValidationError("source kind is not allowed")
    if manifest.source_kind != "published_release":
        if manifest.release_tag is not None or manifest.release_commit is not None:
            raise ValidationError("local candidate has release metadata")
    else:
        if manifest.release_tag is None or not RELEASE_TAG_RE.fullmatch(manifest.release_tag):
            raise ValidationError("published candidate has malformed release tag")
        if manifest.release_commit != manifest.source_commit:
            raise ValidationError("published candidate release commit does not match source commit")
    validate_candidate_identity(manifest.expected, manifest.source_commit)
    if manifest.previous is not None:
        validate_previous_identity(manifest.previous)
    validate_health_url(manifest.expected_health_url, "candidate health URL")
    validate_health_url(manifest.previous_health_url, "previous health URL")
    if manifest.previous_deployed_sha is not None and not GIT_SHA_RE.fullmatch(manifest.previous_deployed_sha):
        raise ValidationError("previous deployed SHA is malformed")
    if manifest.targets != policy.targets:
        raise ValidationError("manifest target paths are not allowed")
    validate_service_user(manifest.service_user)

    transaction = policy.transaction_root / manifest.transaction_id
    root = require_descendant(manifest_path.parent, policy.transaction_root, "transaction directory")
    if root != transaction.resolve(strict=True):
        raise ValidationError("manifest is not in its transaction directory")
    validate_regular_file(str(manifest_path), sha256(manifest_path), root, policy, "manifest")

    validate_regular_file(manifest.candidate.binary.path, manifest.candidate.binary.sha256, root, policy, "candidate binary")
    validate_mode(manifest)
    if manifest.registry is not None:
        validate_regular_file(manifest.registry.path, manifest.registry.sha256, root, policy, "candidate registry")
    if manifest.prepared_artifact_receipt is not None:
        receipt_artifact = manifest.prepared_artifact_receipt
        receipt_path = validate_regular_file(receipt_artifact.path, receipt_artifact.sha256, root, policy, "prepared artifact receipt")
        if receipt_path != root / PREPARED_RECEIPT:
            raise ValidationError("prepared artifact receipt has wrong retained path")
        receipt = parse_prepared_receipt(receipt_path.read_bytes())
        if (receipt["controller_commit"] != manifest.controller_source_commit
                or receipt["commit"] != manifest.source_commit
                or receipt["registry_sha256"] != manifest.registry.sha256
                or receipt["sha256"][PREPARED_PAYLOAD] != manifest.candidate.binary.sha256
                or Path(manifest.candidate.binary.path) != root / "candidate-binary"):
            raise ValidationError("prepared artifact receipt does not bind retained candidate provenance")
        if installed_runtime_identity(Path(manifest.candidate.binary.path)) != manifest.expected:
            raise ValidationError("prepared artifact embedded identity differs from receipt")
    if manifest.controller_helper is not None:
        retained = validate_regular_file(manifest.controller_helper.path, manifest.controller_helper.sha256, root, policy, "controller helper")
        if retained != root / "helper.py" or sha256(Path(__file__)) != manifest.controller_helper.sha256:
            raise ValidationError("executing helper differs from retained controller helper")
    validate_regular_file(manifest.candidate.service.path, manifest.candidate.service.sha256, root, policy, "candidate service")
    validate_regular_file(manifest.candidate.socket.path, manifest.candidate.socket.sha256, root, policy, "candidate socket")
    validate_optional_artifact(manifest.candidate.environment, root, policy, "candidate environment")

    rollback = [
        validate_optional_artifact(manifest.rollback.binary, root, policy, "rollback binary"),
        validate_optional_artifact(manifest.rollback.service, root, policy, "rollback service"),
        validate_optional_artifact(manifest.rollback.socket, root, policy, "rollback socket"),
        validate_optional_artifact(manifest.rollback.environment, root, policy, "rollback environment"),
    ]
    if manifest.previous is None and any(item is not None for item in rollback):
        raise ValidationError("first-install rollback inputs are inconsistent")
    if manifest.previous is not None and any(item is None for item in rollback[:3]):
        raise ValidationError("rollback binary and units are required for an existing install")

    for description, value, allowed in (
        ("status", manifest.status_path, policy.status_path),
        ("active claim", manifest.active_path, policy.active_path),
        ("activation lock", manifest.activation_lock_path, policy.activation_lock_path),
        ("claim lock", manifest.claim_lock_path, policy.claim_lock_path),
    ):
        path = Path(value)
        if path != allowed:
            raise ValidationError(f"{description} path is not allowed")
        validate_root_owned_tree(path.parent, policy.transaction_root.parent, policy)
        if path.is_symlink():
            raise ValidationError(f"{description} path must not be a symlink")


TERMINAL_STATES = {
    "committed",
    "precondition_failed",
    "activation_failed_rolled_back",
    "activation_failed_rollback_failed",
    "ordinary_activation_failed_rollback_failed",
    "rejected_concurrent",
}

CLAIM_RELEASABLE_STATES = TERMINAL_STATES - {
    "activation_failed_rollback_failed",
    "ordinary_activation_failed_rollback_failed",
}


class ActivationError(RuntimeError):
    pass


class ConcurrentDeploy(ActivationError):
    pass


@dataclasses.dataclass(frozen=True)
class UnitState:
    service_active: bool
    service_enabled: bool
    socket_active: bool
    socket_enabled: bool
    main_pid: int


@dataclasses.dataclass
class PreparedInstalls:
    candidate_binary: Path
    candidate_service: Path
    candidate_socket: Path
    candidate_environment: Optional[Path]
    rollback_binary: Optional[Path]
    rollback_service: Optional[Path]
    rollback_socket: Optional[Path]
    rollback_environment: Optional[Path]

    def paths(self):
        return [value for value in dataclasses.astuple(self) if value is not None]


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def fsync_dir(path: Path) -> None:
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


def prepare_atomic_install(
    staged: Path,
    target: Path,
    mode: int,
    *,
    owner_uid: Optional[int] = None,
    owner_gid: Optional[int] = None,
) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{target.name}.install-", dir=target.parent)
    try:
        os.fchmod(fd, mode)
        if owner_uid is not None or owner_gid is not None:
            os.fchown(fd, -1 if owner_uid is None else owner_uid, -1 if owner_gid is None else owner_gid)
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


def remove_target(path: Path) -> None:
    path.unlink(missing_ok=True)
    if path.parent.exists():
        fsync_dir(path.parent)


def migration_marker_present(root: Path) -> bool:
    try:
        (root / "migration_started").lstat()
        return True
    except FileNotFoundError:
        return False


def write_status(manifest: Manifest, state: str, *, failure: Optional[str] = None, rollback_failure: Optional[str] = None) -> None:
    status = {
        "transaction_id": manifest.transaction_id,
        "state": state,
        "source_kind": manifest.source_kind,
        "mode": manifest.mode,
        "manifest_sha256": transaction_context(manifest)["manifest_sha256"] if manifest.mode == PROTECTED_MODE else None,
        "migration_started": migration_marker_present(Path(manifest.candidate.binary.path).parent),
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
    lock_path = Path(manifest.claim_lock_path)
    with lock_path.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        atomic_write(Path(manifest.status_path), (json.dumps(status, sort_keys=True, indent=2) + "\n").encode())


def write_policy_status(
    manifest: Manifest,
    policy: ValidationPolicy,
    state: str,
    *,
    failure: Optional[str] = None,
) -> None:
    status = {
        "transaction_id": manifest.transaction_id,
        "state": state,
        "source_kind": manifest.source_kind,
        "mode": manifest.mode,
        "manifest_sha256": transaction_context(manifest)["manifest_sha256"] if manifest.mode == PROTECTED_MODE else None,
        "migration_started": migration_marker_present(Path(manifest.candidate.binary.path).parent),
        "source_commit": manifest.source_commit,
        "release_tag": manifest.release_tag,
        "release_commit": manifest.release_commit,
        "expected_version": manifest.expected.version,
        "expected_git_sha": manifest.expected.git_sha,
        "created_at": manifest.created_at,
        "updated_at": utc_now(),
        "failure": failure,
        "rollback_failure": None,
    }
    with policy.claim_lock_path.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        atomic_write(policy.status_path, (json.dumps(status, sort_keys=True, indent=2) + "\n").encode())


def release_policy_claim(transaction_id: str, policy: ValidationPolicy) -> bool:
    with policy.claim_lock_path.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            if policy.active_path.read_text().strip() != transaction_id:
                return False
            policy.active_path.unlink()
            fsync_dir(policy.active_path.parent)
            return True
        except FileNotFoundError:
            return False


def status_is_durable_terminal(manifest: Manifest) -> bool:
    try:
        status = json.loads(Path(manifest.status_path).read_text())
        return (isinstance(status, dict) and status.get("transaction_id") == manifest.transaction_id
                and status.get("mode") == manifest.mode and type(status.get("migration_started")) is bool
                and status.get("source_commit") == manifest.source_commit
                and status.get("source_kind") == manifest.source_kind
                and status.get("release_tag") == manifest.release_tag
                and status.get("release_commit") == manifest.release_commit
                and status.get("expected_version") == manifest.expected.version
                and status.get("expected_git_sha") == manifest.expected.git_sha
                and status.get("state") in TERMINAL_STATES
                and status["migration_started"] == migration_marker_present(Path(manifest.candidate.binary.path).parent)
                and (manifest.mode == "ordinary" or status.get("manifest_sha256") == transaction_context(manifest)["manifest_sha256"])
                and (not status["migration_started"] or status["state"] == "committed"))
    except (OSError, json.JSONDecodeError):
        return False


def release_claim(manifest: Manifest) -> bool:
    claim = Path(manifest.active_path)
    lock_path = Path(manifest.claim_lock_path)
    with lock_path.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            if claim.read_text().strip() != manifest.transaction_id:
                return False
            try:
                status = json.loads(Path(manifest.status_path).read_text())
            except (OSError, json.JSONDecodeError):
                return False
            if not isinstance(status, dict) or status.get("state") not in CLAIM_RELEASABLE_STATES:
                return False
            if not status_is_durable_terminal(manifest):
                return False
            claim.unlink()
            fsync_dir(claim.parent)
            return True
        except FileNotFoundError:
            return False


class SystemdUnitControl:
    def __init__(self, unit_name: str, transition_timeout_secs: float = 30.0, run=subprocess.run):
        self.run = run
        self.service = f"{unit_name}.service"
        self.socket = f"{unit_name}.socket"
        self.transition_timeout_secs = transition_timeout_secs
        self.disruption_started = False

    def command(self, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        result = self.run(["systemctl", *args], capture_output=True, text=True)
        if check and result.returncode != 0:
            detail = (result.stderr or result.stdout).strip()
            raise ActivationError(f"systemctl {' '.join(args)} failed: {detail or result.returncode}")
        return result

    def property(self, unit: str, name: str) -> str:
        return self.command("show", unit, f"--property={name}", "--value").stdout.strip()

    def is_active(self, unit: str) -> bool:
        return self.command("is-active", unit, check=False).stdout.strip() == "active"

    def is_enabled(self, unit: str) -> bool:
        return self.command("is-enabled", unit, check=False).stdout.strip() in {"enabled", "static", "indirect"}

    def main_pid(self) -> int:
        value = self.property(self.service, "MainPID")
        return int(value) if value.isdigit() else 0

    def inspect(self) -> UnitState:
        return UnitState(
            service_active=self.is_active(self.service),
            service_enabled=self.is_enabled(self.service),
            socket_active=self.is_active(self.socket),
            socket_enabled=self.is_enabled(self.socket),
            main_pid=self.main_pid(),
        )

    def assert_inactive(self) -> None:
        for unit in (self.service, self.socket):
            if self.property(unit, "ActiveState") not in {"inactive", "failed"}:
                raise ActivationError("service and socket are not provably inactive")
        if self.main_pid():
            raise ActivationError("service still owns a MainPID")

    def stop(self) -> None:
        self.command("stop", self.service, self.socket, check=False)
        self.disruption_started = True
        deadline = time.monotonic() + self.transition_timeout_secs
        while time.monotonic() < deadline:
            if not self.is_active(self.service) and not self.is_active(self.socket):
                return
            time.sleep(0.1)
        raise ActivationError("timed out stopping previous systemd units")


class Systemctl(SystemdUnitControl):
    def __init__(self, manifest: Manifest, run=subprocess.run):
        super().__init__(manifest.unit_name, manifest.transition_timeout_secs, run)
        self.manifest = manifest

    def verify_protected_configuration(self) -> None:
        inhibits = start_inhibit_paths(self.manifest)
        inhibited = any(path.exists() or path.is_symlink() for path in inhibits)
        if inhibited:
            verify_start_inhibit_files(self.manifest)
        for (unit, path), inhibit in zip(((self.service, self.manifest.targets.service), (self.socket, self.manifest.targets.socket)), inhibits):
            expected_dropins = str(inhibit) if inhibited else ""
            if (self.property(unit, "FragmentPath") != path or self.property(unit, "DropInPaths") != expected_dropins
                    or self.property(unit, "NeedDaemonReload") != "no"
                    or (inhibited and self.property(unit, "RefuseManualStart") != "yes")):
                raise ActivationError("loaded systemd configuration differs from bound files")
        if self.property(self.service, "User") != self.manifest.service_user:
            raise ActivationError("loaded systemd service user drift")
        pid = self.main_pid()
        if pid:
            environment = dict(item.split(b"=", 1) for item in Path(f"/proc/{pid}/environ").read_bytes().split(b"\0") if b"=" in item)
            if environment.get(b"PHOENIX_DB_PATH") != self.manifest.database_path.encode():
                raise ActivationError("running predecessor database path differs from manifest")

    def verify_units(self, service: Path, socket: Path, candidate_binary: Path) -> None:
        with tempfile.TemporaryDirectory(prefix="phoenix-systemd-verify-") as temporary:
            verification_service = Path(temporary) / f"{self.manifest.unit_name}.service"
            verification_socket = Path(temporary) / f"{self.manifest.unit_name}.socket"
            target_binary = self.manifest.targets.binary
            content = service.read_text()
            if target_binary not in content:
                raise ActivationError("candidate service does not execute the fixed production binary")
            verification_service.write_text(content.replace(target_binary, str(candidate_binary)))
            verification_socket.write_bytes(socket.read_bytes())
            result = self.run(
                ["systemd-analyze", "verify", str(verification_socket), str(verification_service)],
                capture_output=True,
                text=True,
            )
        if result.returncode != 0:
            raise ActivationError(f"systemd unit validation failed: {(result.stderr or result.stdout).strip()}")

    def daemon_reload(self) -> None:
        self.command("daemon-reload")

    def start_candidate(self, old_pid: int) -> int:
        self.command("enable", self.socket, self.service)
        self.command("start", self.socket, self.service)
        deadline = time.monotonic() + self.manifest.transition_timeout_secs
        while time.monotonic() < deadline:
            pid = self.main_pid()
            if self.is_active(self.service) and self.is_active(self.socket) and pid not in {0, old_pid}:
                return pid
            time.sleep(0.1)
        raise ActivationError("systemd candidate did not become active with a new MainPID")

    def restore_state(self, previous: UnitState) -> int:
        for enabled, unit in ((previous.socket_enabled, self.socket), (previous.service_enabled, self.service)):
            self.command("enable" if enabled else "disable", unit, check=False)
        if previous.socket_active:
            self.command("start", self.socket)
        if previous.service_active:
            self.command("start", self.service)
        if not previous.service_active:
            return 0
        deadline = time.monotonic() + self.manifest.transition_timeout_secs
        while time.monotonic() < deadline:
            pid = self.main_pid()
            if self.is_active(self.service) and pid != 0:
                return pid
            time.sleep(0.1)
        raise ActivationError("restored systemd service did not become active")


def fetch_identity(url: str) -> Identity:
    context = ssl._create_unverified_context() if url.startswith("https://") else None
    with urllib.request.urlopen(url, timeout=2, context=context) as response:
        value = json.load(response)
    try:
        return Identity(version=str(value["version"]), git_sha=str(value["git_sha"]))
    except (KeyError, TypeError) as exc:
        raise ActivationError("health response has no exact runtime identity") from exc


def wait_for_identity(manifest: Manifest, expected: Identity, url: str) -> None:
    deadline = time.monotonic() + manifest.health_timeout_secs
    last = "not responding"
    while time.monotonic() < deadline:
        try:
            actual = fetch_identity(url)
            last = f"version={actual.version} git_sha={actual.git_sha}"
            if actual == expected:
                return
        except Exception as exc:
            last = type(exc).__name__
        time.sleep(0.2)
    raise ActivationError(
        f"exact health verification failed: expected version={expected.version} git_sha={expected.git_sha}; observed {last}"
    )


def artifact_path(artifact: OptionalArtifact) -> Optional[Path]:
    return Path(artifact.path) if artifact.path is not None else None


def service_group_id(service_user: str) -> int:
    return pwd.getpwnam(service_user).pw_gid


def service_user_from_unit(path: Path) -> str:
    users = [
        line.partition("=")[2].strip()
        for line in path.read_text().splitlines()
        if line.strip().startswith("User=")
    ]
    if len(users) != 1 or not users[0]:
        raise ActivationError("rollback service unit must declare exactly one User")
    validate_service_user(users[0])
    return users[0]


def legacy_override_dir(unit_name: str) -> Path:
    return Path("/etc/systemd/system") / f"{unit_name}.service.d"


def prepare_installs(manifest: Manifest, systemctl: Systemctl, *, allow_inhibit: bool = False) -> PreparedInstalls:
    override_dir = legacy_override_dir(manifest.unit_name)
    if allow_inhibit:
        verify_start_inhibit_files(manifest)
    allowed = set(start_inhibit_paths(manifest)) if allow_inhibit else set()
    if override_dir.is_dir() and any(path not in allowed for path in override_dir.glob("*.conf")):
        raise ActivationError(
            f"legacy systemd overrides exist at {override_dir}; move required values to .phoenix-ide.env and remove the drop-ins"
        )
    candidate_service = Path(manifest.candidate.service.path)
    candidate_socket = Path(manifest.candidate.socket.path)
    systemctl.verify_units(candidate_service, candidate_socket, Path(manifest.candidate.binary.path))
    service_gid = service_group_id(manifest.service_user)
    rollback_gid = (
        service_group_id(service_user_from_unit(Path(manifest.rollback.service.path)))
        if manifest.previous is not None
        else None
    )
    reserved: list[Path] = []

    def reserve(staged: Path, target: Path, mode: int, *, owner_gid: Optional[int] = None) -> Path:
        path = prepare_atomic_install(staged, target, mode, owner_gid=owner_gid)
        reserved.append(path)
        return path

    try:
        prepared = PreparedInstalls(
            candidate_binary=reserve(Path(manifest.candidate.binary.path), Path(manifest.targets.binary), 0o755),
            candidate_service=reserve(candidate_service, Path(manifest.targets.service), 0o644),
            candidate_socket=reserve(candidate_socket, Path(manifest.targets.socket), 0o644),
            candidate_environment=(
                reserve(
                    Path(manifest.candidate.environment.path),
                    Path(manifest.targets.environment),
                    0o640,
                    owner_gid=service_gid,
                )
                if manifest.candidate.environment.path is not None else None
            ),
            rollback_binary=None,
            rollback_service=None,
            rollback_socket=None,
            rollback_environment=None,
        )
        if manifest.previous is not None:
            prepared.rollback_binary = reserve(Path(manifest.rollback.binary.path), Path(manifest.targets.binary), 0o755)
            prepared.rollback_service = reserve(Path(manifest.rollback.service.path), Path(manifest.targets.service), 0o644)
            prepared.rollback_socket = reserve(Path(manifest.rollback.socket.path), Path(manifest.targets.socket), 0o644)
            if manifest.rollback.environment.path is not None:
                prepared.rollback_environment = reserve(
                    Path(manifest.rollback.environment.path),
                    Path(manifest.targets.environment),
                    0o640,
                    owner_gid=rollback_gid,
                )
        return prepared
    except BaseException:
        for path in reserved:
            path.unlink(missing_ok=True)
        raise


def install_candidate(manifest: Manifest, prepared: PreparedInstalls) -> None:
    commit_atomic_install(prepared.candidate_binary, Path(manifest.targets.binary))
    if manifest.mode == PROTECTED_MODE:
        return
    commit_atomic_install(prepared.candidate_service, Path(manifest.targets.service))
    commit_atomic_install(prepared.candidate_socket, Path(manifest.targets.socket))
    if prepared.candidate_environment is None:
        remove_target(Path(manifest.targets.environment))
    else:
        commit_atomic_install(prepared.candidate_environment, Path(manifest.targets.environment))


def restore_deployed_sha(manifest: Manifest) -> None:
    target = Path(manifest.targets.deployed_sha)
    if manifest.previous_deployed_sha is None:
        remove_target(target)
    else:
        atomic_write(target, (manifest.previous_deployed_sha + "\n").encode())


def restore(
    manifest: Manifest,
    systemctl: Systemctl,
    previous_state: UnitState,
    prepared: PreparedInstalls,
) -> Optional[Path]:
    systemctl.stop()
    if manifest.previous is None:
        systemctl.command("disable", systemctl.socket, systemctl.service, check=False)
        for target in dataclasses.astuple(manifest.targets)[:4]:
            remove_target(Path(target))
        systemctl.daemon_reload()
        restore_deployed_sha(manifest)
        return None if manifest.data_directory_preexisting else Path(manifest.targets.deployed_sha).parent
    assert prepared.rollback_binary and prepared.rollback_service and prepared.rollback_socket
    commit_atomic_install(prepared.rollback_binary, Path(manifest.targets.binary))
    commit_atomic_install(prepared.rollback_service, Path(manifest.targets.service))
    commit_atomic_install(prepared.rollback_socket, Path(manifest.targets.socket))
    if prepared.rollback_environment is None:
        remove_target(Path(manifest.targets.environment))
    else:
        commit_atomic_install(prepared.rollback_environment, Path(manifest.targets.environment))
    previous_user = service_user_from_unit(Path(manifest.rollback.service.path))
    prepare_data_directory(previous_user, manifest.targets.deployed_sha)
    if manifest.mode == PROTECTED_MODE:
        for artifact, target in ((manifest.rollback.binary, manifest.targets.binary),
                                 (manifest.rollback.service, manifest.targets.service),
                                 (manifest.rollback.socket, manifest.targets.socket),
                                 (manifest.rollback.environment, manifest.targets.environment)):
            if ((artifact.path is None and Path(target).exists())
                    or (artifact.path is not None and artifact.sha256 != sha256(Path(target)))):
                raise ActivationError("restored predecessor differs from captured artifacts")
        systemctl.daemon_reload()
        remove_start_inhibit(manifest, systemctl)
    else:
        systemctl.daemon_reload()
    systemctl.restore_state(previous_state)
    if manifest.mode == PROTECTED_MODE:
        observed = systemctl.inspect()
        if any(getattr(observed, field) != getattr(previous_state, field)
               for field in ("service_active", "service_enabled", "socket_active", "socket_enabled")):
            raise ActivationError("restored predecessor unit state differs from captured state")
    if previous_state.service_active:
        if manifest.previous_health_url is None:
            raise ActivationError("previous health endpoint is unavailable")
        wait_for_identity(manifest, manifest.previous, manifest.previous_health_url)
    restore_deployed_sha(manifest)
    return None


PROTECTED_MODE = "systemd_schema65_to128"
PROTECTED_RELEASE_TAG = "v0.13.0-rc.4"
PROTECTED_RELEASE_COMMIT = "3f0ba4602faa71ebad5088e1ffc856f4c7d511d9"
PROTECTED_RELEASE_VERSION = "0.13.0-rc.4"
PROTECTED_ASSET_NAME = "phoenix_ide-x86_64-unknown-linux-musl"
PROTECTED_ASSET_SHA256 = "bcb74d565c828843596e5109ea9034a2b0e505aab4ba6c18c5cca8e116f5677e"
PROTECTED_REGISTRY_SHA256 = "700162dd8e41305546cf2acd2763bad5603c642840f5be851fb788353879a36a"
PROTECTED_HANDOFF_PROTOCOL_VERSION = 2
REQUIRED_MIGRATIONS = {
    126: "retire_wake_feature_preserving_history",
    127: "coordinator_owned_work_scope_home",
    128: "product_creation_provisioning_stage",
}


def validate_mode(manifest: Manifest) -> None:
    if manifest.mode not in {"ordinary", PROTECTED_MODE}:
        raise ValidationError("unknown systemd deployment mode")
    if manifest.mode == "ordinary":
        if (manifest.source_kind == "prepared_artifact" or manifest.prepared_artifact_receipt is not None
                or any(value is not None for value in (manifest.controller_source_commit, manifest.controller_helper,
                                                       manifest.release_asset_name, manifest.release_asset_sha256))):
            raise ValidationError("ordinary/source mode mismatch")
        return
    if manifest.source_kind == "prepared_artifact":
        if (manifest.previous is None or manifest.manifest_version != PREPARED_HANDOFF_PROTOCOL_VERSION
                or manifest.source_commit != PREPARED_COMMIT
                or manifest.expected != Identity(PREPARED_VERSION, PREPARED_COMMIT)
                or manifest.release_tag is not None or manifest.release_commit is not None
                or manifest.release_asset_name is not None or manifest.release_asset_sha256 is not None
                or manifest.prepared_artifact_receipt is None or manifest.registry is None
                or manifest.registry.sha256 != PREPARED_REGISTRY_SHA256
                or manifest.controller_helper is None
                or not isinstance(manifest.controller_source_commit, str)
                or GIT_SHA_RE.fullmatch(manifest.controller_source_commit) is None
                or manifest.controller_source_commit == PREPARED_COMMIT):
            raise ValidationError("protected prepared source requires exact receipt and separate controller")
    elif (manifest.source_kind != "published_release" or manifest.previous is None
            or manifest.release_commit != manifest.source_commit
            or not isinstance(manifest.release_tag, str) or not RELEASE_TAG_RE.fullmatch(manifest.release_tag)
            or not isinstance(manifest.controller_source_commit, str)
            or not GIT_SHA_RE.fullmatch(manifest.controller_source_commit)
            or manifest.controller_source_commit == manifest.source_commit
            or manifest.controller_helper is None or manifest.registry is None
            or manifest.manifest_version != PROTECTED_HANDOFF_PROTOCOL_VERSION
            or manifest.release_tag != PROTECTED_RELEASE_TAG
            or manifest.source_commit != PROTECTED_RELEASE_COMMIT
            or manifest.expected != Identity(PROTECTED_RELEASE_VERSION, PROTECTED_RELEASE_COMMIT)
            or manifest.release_asset_name != PROTECTED_ASSET_NAME
            or manifest.release_asset_sha256 != PROTECTED_ASSET_SHA256
            or manifest.candidate.binary.sha256 != PROTECTED_ASSET_SHA256
            or manifest.registry.sha256 != PROTECTED_REGISTRY_SHA256
            or manifest.prepared_artifact_receipt is not None):
        raise ValidationError("protected mode requires exact release, separate controller provenance and predecessor")
    import platform
    if sys.platform != "linux" or platform.machine().lower() not in {"x86_64", "amd64"}:
        raise ValidationError("protected mode requires Linux x86_64")
    if manifest.database_path != str(Path(manifest.targets.deployed_sha).parent / "prod.db"):
        raise ValidationError("protected database path is not canonical")


def candidate_registry(manifest: Manifest) -> tuple[tuple[int, str], ...]:
    artifact = manifest.registry
    if artifact is None or sha256(Path(artifact.path)) != artifact.sha256:
        raise ActivationError("candidate registry missing or changed")
    if manifest.mode == PROTECTED_MODE and artifact.sha256 != (PREPARED_REGISTRY_SHA256 if manifest.source_kind == "prepared_artifact" else PROTECTED_REGISTRY_SHA256):
        raise ActivationError("protected candidate registry hash is not the reviewed release")
    source = Path(artifact.path).read_text()
    sections = re.findall(r"const MIGRATIONS: &\[Migration\] = &\[(.*?)\n\];", source, re.S)
    if len(sections) != 1:
        raise ActivationError("invalid candidate registry")
    # This intentionally accepts only the release registry's concrete Rust shape.
    entry = re.compile(r'\s*Migration\s*\{\s*version:\s*([0-9]+),\s*name:\s*"([^"\\]+)",\s*sql:\s*(?:[A-Za-z_][A-Za-z0-9_]*|"(?:\\.|[^"\\])*"|include_str!\("[^"\\]+"\)),?\s*\},', re.S)
    body, position, entries = sections[0], 0, []
    while body[position:].strip():
        match = entry.match(body, position)
        if match is None:
            raise ActivationError("invalid candidate registry entry")
        entries.append((int(match[1]), match[2]))
        position = match.end()
    versions = tuple(version for version, _ in entries)
    if not versions or versions != tuple(sorted(set(versions))) or versions[0] < 1:
        raise ActivationError("candidate registry must be nonempty, sorted and unique")
    if manifest.mode == PROTECTED_MODE and (versions != tuple(v for v in range(1, 129) if v != 99)
            or any(dict(entries).get(v) != n for v, n in REQUIRED_MIGRATIONS.items())):
        raise ActivationError("candidate registry is not exact qualified target128")
    return tuple(entries)


def readonly_database(path: Path):
    if not path.is_absolute() or path.resolve() != path or not path.is_file() or path.is_symlink():
        raise ActivationError("database must be an existing canonical regular file")
    return sqlite3.connect("file:" + urllib.parse.quote(str(path), safe="/") + "?mode=ro", uri=True, timeout=2)


def database_evidence(path: Path) -> tuple[tuple[tuple[int, str], ...], str]:
    with closing(readonly_database(path)) as db:
        db.execute("BEGIN")
        if db.execute("PRAGMA integrity_check").fetchall() != [("ok",)] or db.execute("PRAGMA foreign_key_check").fetchall():
            raise ActivationError("database integrity/foreign key verification failed")
        ledger = tuple(db.execute("SELECT version, name FROM _migrations ORDER BY version").fetchall())
        schema = db.execute("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name").fetchall()
        digest = hashlib.sha256(json.dumps(schema, separators=(",", ":")).encode())
        for kind, name, _, _ in schema:
            if kind == "table":
                quoted = '"' + name.replace('"', '""') + '"'
                rows = []
                for row in db.execute(f"SELECT * FROM {quoted}"):
                    values = [(type(v).__name__, v.hex() if isinstance(v, bytes) else v) for v in row]
                    rows.append(hashlib.sha256(json.dumps(values, separators=(",", ":")).encode()).digest())
                digest.update(name.encode())
                digest.update(len(rows).to_bytes(8, "big"))
                for row in sorted(rows):
                    digest.update(row)
        return ledger, digest.hexdigest()


def effective_database(manifest: Manifest, *, installed: bool = False) -> Path:
    service = Path(manifest.targets.service if installed else manifest.candidate.service.path)
    environment = Path(manifest.targets.environment) if installed else artifact_path(manifest.candidate.environment)
    values = {}
    for raw in service.read_text().splitlines():
        raw = raw.strip()
        if raw.startswith("Environment="):
            for assignment in shlex.split(raw.partition("=")[2]):
                key, sep, value = assignment.partition("=")
                if not sep:
                    raise ActivationError("invalid service environment")
                values[key] = value
    if environment is not None:
        values.update(installed_environment(environment))
    path = Path(values.get("PHOENIX_DB_PATH", str(Path(manifest.targets.deployed_sha).parent / "prod.db")))
    if not path.is_absolute() or ((manifest.mode == PROTECTED_MODE or path.exists()) and path.resolve() != path):
        raise ActivationError("effective database path must be canonical")
    if manifest.database_path is not None and str(path) != manifest.database_path:
        raise ActivationError("effective database path drift")
    return path


def verify_preserved_configuration(manifest: Manifest) -> None:
    for name in ("service", "socket", "environment"):
        candidate = getattr(manifest.candidate, name)
        rollback = getattr(manifest.rollback, name)
        target = Path(getattr(manifest.targets, name))
        if candidate.sha256 != rollback.sha256 or candidate.sha256 is None or sha256(target) != candidate.sha256:
            raise ActivationError("protected installed configuration drift")
    if service_user_from_unit(Path(manifest.targets.service)) != manifest.service_user:
        raise ActivationError("protected service user drift")
    directives = {}
    for raw in Path(manifest.targets.service).read_text().splitlines():
        line = raw.strip()
        if line and not line.startswith(("#", ";", "[")):
            key, sep, value = line.partition("=")
            if not sep or key != key.strip() or value != value.strip() or line.endswith("\\"):
                raise ActivationError("unsupported protected service syntax")
            directives.setdefault(key, []).append(value)
    allowed = {"Description", "Documentation", "Requires", "After", "Type", "User", "Environment",
               "EnvironmentFile", "ExecStart", "ExecReload", "Restart", "RestartSec", "TimeoutStopSec", "WantedBy"}
    if set(directives) - allowed:
        raise ActivationError("unsupported protected service customization")
    if directives.get("ExecStart") != [manifest.targets.binary] or directives.get("EnvironmentFile") != [manifest.targets.environment]:
        raise ActivationError("protected service execution/environment is not exact")
    effective_database(manifest, installed=True)


def ordinary_candidate_registry(manifest: Manifest) -> tuple[tuple[int, str], ...]:
    # Protocol v1 carries no registry; the retained candidate can expose it offline.
    with tempfile.TemporaryDirectory(prefix="ordinary-registry-", dir=Path(manifest.candidate.binary.path).parent) as home:
        database = Path(home) / "registry.sqlite"
        subprocess.run([manifest.candidate.binary.path, "--migrate-only"], cwd="/",
                       env={"PATH": "/usr/bin:/bin", "HOME": home, "PHOENIX_DB_PATH": str(database)},
                       check=True, capture_output=True, text=True, timeout=900)
        registry = database_evidence(database)[0]
        versions = tuple(v for v, _ in registry)
        if not versions or versions != tuple(sorted(set(versions))) or versions[0] < 1:
            raise ActivationError("ordinary candidate registry must be nonempty, sorted and unique")
        return registry


def database_preflight(manifest: Manifest) -> Optional[tuple[tuple[tuple[int, str], ...], str]]:
    validate_mode(manifest)
    path = effective_database(manifest)
    if manifest.previous is not None and Path(manifest.targets.service).is_file():
        installed = effective_database(manifest, installed=True)
        if not installed.is_file():
            raise ActivationError("installed source database missing")
        if installed != path:
            raise ActivationError("installed database path differs from candidate")
    if not path.exists():
        if (manifest.mode == PROTECTED_MODE or manifest.previous is not None
                or any(getattr(manifest.rollback, name).path is not None
                       for name in ("binary", "service", "socket", "environment"))):
            raise ActivationError("installed source database missing")
        return None
    if manifest.previous is None:
        raise ActivationError("existing database without predecessor is not a first install")
    registry = ordinary_candidate_registry(manifest) if manifest.registry is None and manifest.mode == "ordinary" else candidate_registry(manifest)
    evidence = database_evidence(path)
    if manifest.mode == PROTECTED_MODE:
        verify_preserved_configuration(manifest)
        if evidence[0] != registry[:65]:
            raise ActivationError("protected source must equal exact registry prefix65")
    elif evidence[0] != registry:
        raise ActivationError("ordinary systemd pending migration interval is not qualified")
    return evidence


def preflight_executable(name: str, *probe: str, expected: str) -> Path:
    try:
        found = shutil.which(name)
        if found is None:
            raise ValidationError(f"required {name} executable is unavailable")
        path = Path(found).resolve(strict=True)
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValidationError(f"required {name} executable is not usable")
        result = subprocess.run([str(path), *probe], check=True, capture_output=True, text=True, timeout=10)
        if expected not in result.stdout + result.stderr:
            raise ValidationError(f"required {name} executable failed its probe")
        return path
    except (OSError, subprocess.SubprocessError) as exc:
        raise ValidationError(f"required {name} executable failed its probe") from exc


def preflight_database_exclusivity(controller: Systemctl) -> None:
    controller.database_lsof = preflight_executable("lsof", "-v", expected="lsof")


def assert_database_exclusive(manifest: Manifest, controller: Systemctl) -> None:
    state = controller.inspect()
    if state.service_active or state.socket_active or state.main_pid:
        raise ActivationError("service and socket must both be inactive")
    controller.assert_inactive()
    database = effective_database(manifest, installed=True)
    paths = [p for p in (database, Path(str(database) + "-wal"), Path(str(database) + "-shm")) if p.exists()]
    for path in paths:
        if not path.is_file() or path.is_symlink():
            raise ActivationError("unsafe SQLite sidecar")
    lsof = getattr(controller, "database_lsof", None)
    if lsof is None:
        raise ActivationError("database exclusivity requires preflighted lsof")
    result = subprocess.run([str(lsof), "-nP", "-t", "--", *map(str, paths)], capture_output=True, text=True, timeout=10)
    if result.returncode != 1 or result.stdout.strip() or result.stderr.strip():
        raise ActivationError("could not prove exclusive database access")


def transaction_context(manifest: Manifest) -> dict:
    return {"transaction_id": manifest.transaction_id, "mode": manifest.mode,
            "source_commit": manifest.source_commit, "controller_source_commit": manifest.controller_source_commit,
            "database_path": manifest.database_path, "candidate_sha256": manifest.candidate.binary.sha256,
            "registry_sha256": manifest.registry.sha256,
            "manifest_sha256": hashlib.sha256(json.dumps(dataclasses.asdict(manifest), sort_keys=True).encode()).hexdigest()}


def start_inhibit_paths(manifest: Manifest) -> tuple[Path, Path]:
    return tuple(Path(path + ".d") / "90-phoenix-deploy-inhibit.conf"
                 for path in (manifest.targets.service, manifest.targets.socket))


def start_inhibit_content(manifest: Manifest) -> bytes:
    binding = {**transaction_context(manifest), "paths": [str(path) for path in start_inhibit_paths(manifest)]}
    return ("# Phoenix deployment start inhibit " + json.dumps(binding, sort_keys=True)
            + "\n[Unit]\nRefuseManualStart=yes\nConditionPathExists=!/\n").encode()


def verify_start_inhibit_file(manifest: Manifest, path: Path) -> None:
    directory = path.parent
    if directory.is_symlink() or directory.resolve() != directory:
        raise ValidationError("start inhibit directory is not canonical")
    metadata = directory.stat()
    if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) & 0o022:
        raise ValidationError("unsafe start inhibit directory")
    metadata = path.lstat()
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
            or stat.S_IMODE(metadata.st_mode) != 0o644 or path.read_bytes() != start_inhibit_content(manifest)):
        raise ValidationError("start inhibit content/owner/mode does not match transaction")


def verify_start_inhibit_files(manifest: Manifest) -> None:
    for path in start_inhibit_paths(manifest):
        verify_start_inhibit_file(manifest, path)


def install_start_inhibit(manifest: Manifest, controller: Systemctl) -> None:
    for path in start_inhibit_paths(manifest):
        if path.exists() or path.is_symlink():
            verify_start_inhibit_file(manifest, path)
            continue
        if not path.parent.exists():
            path.parent.mkdir(mode=0o755)
            fsync_dir(path.parent.parent)
        if path.parent.is_symlink() or path.parent.resolve() != path.parent:
            raise ValidationError("unsafe start inhibit directory")
        atomic_write(path, start_inhibit_content(manifest), mode=0o644)
    verify_start_inhibit_files(manifest)
    controller.daemon_reload()
    controller.verify_protected_configuration()


def remove_start_inhibit(manifest: Manifest, controller: Systemctl) -> None:
    verify_start_inhibit_files(manifest)
    controller.verify_protected_configuration()
    for path in start_inhibit_paths(manifest):
        remove_target(path)
    controller.daemon_reload()
    controller.verify_protected_configuration()


def retain_failed_start_inhibit(manifest: Manifest, controller: Systemctl) -> None:
    try:
        install_start_inhibit(manifest, controller)
    finally:
        controller.stop()


def authorize_candidate_start(manifest: Manifest, controller: Systemctl) -> None:
    verify_rehearsal_proof(manifest)
    verify_preserved_configuration(manifest)
    assert_database_exclusive(manifest, controller)
    if (sha256(Path(manifest.targets.binary)) != manifest.candidate.binary.sha256
            or installed_runtime_identity(Path(manifest.targets.binary)) != manifest.expected
            or database_evidence(effective_database(manifest, installed=True))[0] != candidate_registry(manifest)):
        raise ActivationError("exact candidate/target proof failed before start authorization")
    remove_start_inhibit(manifest, controller)


def snapshot_current(manifest: Manifest, controller: Systemctl) -> tuple[tuple[tuple[int, str], ...], str]:
    assert_database_exclusive(manifest, controller)
    source = effective_database(manifest, installed=True)
    evidence = database_evidence(source)
    if evidence[0] != candidate_registry(manifest)[:65]:
        raise ActivationError("stopped source ledger changed")
    root = Path(manifest.candidate.binary.path).parent
    backup = root / "source.sqlite"
    fd = os.open(backup, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(fd)
    with closing(readonly_database(source)) as db, closing(sqlite3.connect(backup)) as destination:
        db.execute("BEGIN")
        db.execute("SELECT count(*) FROM sqlite_master").fetchone()
        db.backup(destination)
    if database_evidence(backup) != evidence or database_evidence(source) != evidence:
        raise ActivationError("snapshot differs from exact stopped source")
    backup.chmod(0o400)
    with backup.open("rb") as stream:
        os.fsync(stream.fileno())
    fsync_dir(root)
    proof = {**transaction_context(manifest), "backup_sha256": sha256(backup), "source_digest": evidence[1],
             "source_ledger": evidence[0]}
    atomic_write(root / "source-proof.json", (json.dumps(proof, sort_keys=True) + "\n").encode())
    return evidence


def verify_snapshot_proof(manifest: Manifest) -> dict:
    root = Path(manifest.candidate.binary.path).parent
    proof = json.loads((root / "source-proof.json").read_text())
    if any(proof.get(k) != v for k, v in transaction_context(manifest).items()):
        raise ActivationError("snapshot proof context mismatch")
    backup = root / "source.sqlite"
    ledger, digest = database_evidence(backup)
    if (sha256(backup) != proof.get("backup_sha256") or digest != proof.get("source_digest")
            or ledger != candidate_registry(manifest)[:65] or [list(row) for row in ledger] != proof.get("source_ledger")):
        raise ActivationError("snapshot proof mismatch")
    return proof


def rehearse_current_migration(manifest: Manifest) -> None:
    proof = verify_snapshot_proof(manifest)
    root = Path(manifest.candidate.binary.path).parent
    candidate = Path(manifest.candidate.binary.path)
    if sha256(candidate) != manifest.candidate.binary.sha256 or installed_runtime_identity(candidate) != manifest.expected:
        raise ActivationError("retained rehearsal candidate identity/checksum mismatch")
    rehearsal = root / "rehearsal.sqlite"
    copy_handoff_file(root / "source.sqlite", rehearsal, proof["backup_sha256"], os.geteuid(), 0o600)
    result = {**transaction_context(manifest), "backup_sha256": proof["backup_sha256"], "success": False}
    try:
        with tempfile.TemporaryDirectory(prefix="rehearsal-home-", dir=root) as home:
            subprocess.run([str(candidate), "--migrate-only"], cwd="/",
                           env={"PATH": "/usr/bin:/bin", "HOME": home, "PHOENIX_DB_PATH": str(rehearsal)},
                           check=True, capture_output=True, text=True, timeout=900)
        ledger, digest = database_evidence(rehearsal)
        if ledger != candidate_registry(manifest):
            raise ActivationError("rehearsal did not reach exact target128")
        verify_snapshot_proof(manifest)
        rehearsal.chmod(0o400)
        with rehearsal.open("rb") as stream:
            os.fsync(stream.fileno())
        fsync_dir(root)
        result.update(success=True, rehearsal_sha256=sha256(rehearsal), target_ledger=ledger, target_digest=digest)
    finally:
        atomic_write(root / "rehearsal-proof.json", (json.dumps(result, sort_keys=True) + "\n").encode())


def verify_rehearsal_proof(manifest: Manifest) -> None:
    source = verify_snapshot_proof(manifest)
    root = Path(manifest.candidate.binary.path).parent
    proof = json.loads((root / "rehearsal-proof.json").read_text())
    if (any(proof.get(k) != v for k, v in transaction_context(manifest).items())
            or proof.get("success") is not True or proof.get("backup_sha256") != source["backup_sha256"]):
        raise ActivationError("rehearsal proof context/result mismatch")
    ledger, digest = database_evidence(root / "rehearsal.sqlite")
    if (ledger != candidate_registry(manifest) or [list(row) for row in ledger] != proof.get("target_ledger")
            or digest != proof.get("target_digest") or sha256(root / "rehearsal.sqlite") != proof.get("rehearsal_sha256")):
        raise ActivationError("rehearsal proof mismatch")


def run_current_migration(manifest: Manifest, controller: Systemctl) -> None:
    verify_rehearsal_proof(manifest)
    if sha256(Path(manifest.targets.binary)) != manifest.candidate.binary.sha256:
        raise ActivationError("installed migration candidate differs from retained candidate")
    if installed_runtime_identity(Path(manifest.targets.binary)) != manifest.expected:
        raise ActivationError("migration candidate identity mismatch")
    environment = {}
    for raw in Path(manifest.targets.service).read_text().splitlines():
        raw = raw.strip()
        if raw.startswith("Environment="):
            environment.update(assignment.split("=", 1) for assignment in shlex.split(raw.partition("=")[2]))
    environment.update(installed_environment(Path(manifest.targets.environment)))
    environment["PHOENIX_DB_PATH"] = manifest.database_path
    account = pwd.getpwnam(manifest.service_user)
    environment.update(PATH="/usr/bin:/bin", USER=account.pw_name, LOGNAME=account.pw_name)
    environment.setdefault("HOME", account.pw_dir)
    root = Path(manifest.candidate.binary.path).parent
    marker_data = (json.dumps(transaction_context(manifest), sort_keys=True) + "\n").encode()
    if not migration_marker_present(root):
        source = verify_snapshot_proof(manifest)
        assert_database_exclusive(manifest, controller)
        ledger, digest = database_evidence(effective_database(manifest, installed=True))
        if [list(row) for row in ledger] != source["source_ledger"] or digest != source["source_digest"]:
            raise ActivationError("live source differs from stopped snapshot; migration forbidden")
    atomic_write(root / "migration_started", marker_data)
    write_status(manifest, "activating")
    subprocess.run([manifest.targets.binary, "--migrate-only"], env=environment, cwd="/",
                   user=account.pw_uid, group=account.pw_gid,
                   extra_groups=os.getgrouplist(account.pw_name, account.pw_gid),
                   check=True, capture_output=True, text=True, timeout=900)


def activate(manifest: Manifest, systemctl: Optional[Systemctl] = None) -> str:
    lock_path = Path(manifest.activation_lock_path)
    lock_path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is already activating") from exc
        controller = systemctl or Systemctl(manifest)
        validate_mode(manifest)
        if manifest.mode == PROTECTED_MODE:
            retained = retained_status(manifest)
            if retained["state"] != "prepared" or retained["migration_started"] or migration_marker_present(Path(manifest.candidate.binary.path).parent):
                raise ValidationError("protected activation is not an untouched prepared transaction")
        prepared = None
        try:
            database_preflight(manifest)
            if manifest.mode == PROTECTED_MODE:
                controller.verify_protected_configuration()
                preflight_database_exclusivity(controller)
                environment = {}
                for raw in Path(manifest.targets.service).read_text().splitlines():
                    if raw.strip().startswith("Environment="):
                        environment.update(assignment.split("=", 1) for assignment in shlex.split(raw.strip().partition("=")[2]))
                environment.update(installed_environment(Path(manifest.targets.environment)))
                validate_installed_tls(installed_tls(environment), manifest.service_user)
            prepared = prepare_installs(manifest, controller)
            previous_state = controller.inspect()
            if manifest.mode == PROTECTED_MODE:
                atomic_write(Path(manifest.candidate.binary.path).parent / "predecessor-state.json",
                             (json.dumps({**transaction_context(manifest),
                                          "unit_state": dataclasses.asdict(previous_state)}, sort_keys=True) + "\n").encode())
        except Exception as exc:
            if prepared is not None:
                for path in prepared.paths():
                    path.unlink(missing_ok=True)
            write_status(manifest, "precondition_failed", failure=str(exc))
            raise
        write_status(manifest, "activating")
        inhibit_ready = False
        try:
            if manifest.mode == PROTECTED_MODE:
                install_start_inhibit(manifest, controller)
                inhibit_ready = True
            controller.stop()
            if manifest.mode == PROTECTED_MODE:
                snapshot_current(manifest, controller)
                rehearse_current_migration(manifest)
            install_candidate(manifest, prepared)
            if manifest.mode == PROTECTED_MODE:
                verify_preserved_configuration(manifest)
                assert_database_exclusive(manifest, controller)
                verify_snapshot_proof(manifest)
                run_current_migration(manifest, controller)
                if database_evidence(effective_database(manifest, installed=True))[0] != candidate_registry(manifest):
                    raise ActivationError("candidate did not reach exact target128")
            if manifest.mode == PROTECTED_MODE:
                authorize_candidate_start(manifest, controller)
            else:
                controller.daemon_reload()
            controller.start_candidate(previous_state.main_pid)
            wait_for_identity(manifest, manifest.expected, manifest.expected_health_url)
            atomic_write(Path(manifest.targets.deployed_sha), (manifest.source_commit + "\n").encode())
            write_status(manifest, "committed")
            return "committed"
        except Exception as activation_exc:
            failure = str(activation_exc)
            if manifest.mode == PROTECTED_MODE and not inhibit_ready:
                if any(path.exists() or path.is_symlink() for path in start_inhibit_paths(manifest)):
                    write_status(manifest, "activation_failed_rollback_failed", failure=failure,
                                 rollback_failure="start inhibit setup incomplete; predecessor not stopped")
                    return "activation_failed_rollback_failed"
                write_status(manifest, "precondition_failed", failure=failure)
                raise
            if manifest.mode == PROTECTED_MODE and migration_marker_present(Path(manifest.candidate.binary.path).parent):
                try:
                    retain_failed_start_inhibit(manifest, controller)
                except Exception:
                    pass
                write_status(manifest, "current_forward_failed", failure=failure)
                return "current_forward_failed"
            if not controller.disruption_started and not (manifest.mode == PROTECTED_MODE and any(
                    path.exists() or path.is_symlink() for path in start_inhibit_paths(manifest))):
                write_status(manifest, "precondition_failed", failure=failure)
                raise
            try:
                if manifest.mode == PROTECTED_MODE:
                    assert_database_exclusive(manifest, controller)
                    evidence = database_evidence(effective_database(manifest, installed=True))
                    if evidence[0] != candidate_registry(manifest)[:65]:
                        raise ActivationError("source ledger changed; predecessor restart forbidden")
                    root = Path(manifest.candidate.binary.path).parent
                    if any((root / name).exists() or (root / name).is_symlink()
                           for name in ("source.sqlite", "source-proof.json")):
                        if evidence[1] != verify_snapshot_proof(manifest)["source_digest"]:
                            raise ActivationError("source database changed; predecessor restart forbidden")
                cleanup_data_dir = restore(manifest, controller, previous_state, prepared)
                write_status(manifest, "activation_failed_rolled_back", failure=failure)
                if cleanup_data_dir is not None:
                    shutil.rmtree(cleanup_data_dir)
                return "activation_failed_rolled_back"
            except Exception as rollback_exc:
                state = "activation_failed_rollback_failed" if manifest.mode == PROTECTED_MODE else "ordinary_activation_failed_rollback_failed"
                if manifest.mode == PROTECTED_MODE:
                    try:
                        retain_failed_start_inhibit(manifest, controller)
                    except Exception:
                        pass
                write_status(manifest, state, failure=failure, rollback_failure=str(rollback_exc))
                return state
        finally:
            if prepared is not None:
                for path in prepared.paths():
                    path.unlink(missing_ok=True)


def retire_partial_staging(policy: ValidationPolicy) -> None:
    try:
        status = json.loads(policy.status_path.read_text())
    except FileNotFoundError:
        return
    except (OSError, json.JSONDecodeError) as exc:
        raise ConcurrentDeploy("unreadable retained staging status") from exc
    if not isinstance(status, dict):
        raise ConcurrentDeploy("ambiguous retained status")
    if status.get("state") != "staging":
        if not policy.active_path.exists():
            owner = status.get("transaction_id")
            try:
                if not isinstance(owner, str) or not TRANSACTION_RE.fullmatch(owner):
                    raise ValidationError("invalid retained owner")
                retained = Manifest.load(policy.transaction_root / owner / "manifest.json")
                if (retained.transaction_id != owner or retained.status_path != str(policy.status_path)
                        or retained.active_path != str(policy.active_path)
                        or status.get("state") not in CLAIM_RELEASABLE_STATES or not status_is_durable_terminal(retained)):
                    raise ValidationError("unresolved retained status")
            except (OSError, ValueError, TypeError, ValidationError) as exc:
                raise ConcurrentDeploy("unresolved or ambiguous status without claim") from exc
        return
    owner = status.get("transaction_id")
    if (not isinstance(owner, str) or not TRANSACTION_RE.fullmatch(owner)
            or set(status) != {"transaction_id", "state", "migration_started"}
            or status["migration_started"] is not False):
        raise ConcurrentDeploy("ambiguous staging status")
    transaction = policy.transaction_root / owner
    if (transaction.is_symlink() or (transaction / "migration_started").exists()
            or (transaction / "migration_started").is_symlink()):
        raise ConcurrentDeploy("staging owner has ambiguous or irreversible evidence")
    if policy.active_path.exists() and policy.active_path.read_text().strip() != owner:
        raise ConcurrentDeploy("staging status/claim mismatch")
    if transaction.exists():
        shutil.rmtree(transaction)
        fsync_dir(policy.transaction_root)
    if policy.active_path.exists():
        policy.active_path.unlink()
        fsync_dir(policy.active_path.parent)
    policy.status_path.unlink()
    fsync_dir(policy.status_path.parent)


def acquire_claim(transaction_id: str, policy: ValidationPolicy, *, retire_staging: bool = False) -> Path:
    if not TRANSACTION_RE.fullmatch(transaction_id):
        raise ValidationError("malformed transaction id")
    policy.transaction_root.mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod(policy.transaction_root, 0o700)
    policy.active_path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    with policy.claim_lock_path.open("a+") as lock:
        os.chmod(policy.claim_lock_path, 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        if retire_staging:
            retire_partial_staging(policy)
        if policy.active_path.exists():
            owner = policy.active_path.read_text().strip()
            try:
                status = json.loads(policy.status_path.read_text())
            except (OSError, json.JSONDecodeError):
                status = {}
            try:
                retained = Manifest.load(policy.transaction_root / owner / "manifest.json")
                releasable = (isinstance(status, dict) and retained.transaction_id == owner
                              and retained.status_path == str(policy.status_path)
                              and retained.active_path == str(policy.active_path)
                              and status.get("state") in CLAIM_RELEASABLE_STATES
                              and status_is_durable_terminal(retained))
            except (OSError, ValueError, TypeError, ValidationError):
                releasable = False
            if not owner or not releasable:
                raise ConcurrentDeploy(f"deployment transaction {owner} is unresolved")
        transaction = policy.transaction_root / transaction_id
        if transaction.exists():
            raise ValidationError("transaction directory already exists")
        if policy.active_path.exists():
            policy.active_path.unlink()
            fsync_dir(policy.active_path.parent)
        atomic_write(policy.status_path, (json.dumps({"transaction_id": transaction_id, "state": "staging",
                                                    "migration_started": False}, sort_keys=True) + "\n").encode())
        transaction.mkdir(mode=0o700)
        fsync_dir(policy.transaction_root)
        atomic_write(policy.active_path, (transaction_id + "\n").encode())
        return transaction


def copy_handoff_file(source: Path, destination: Path, expected_hash: str, source_uid: int, mode: int) -> None:
    if not SHA256_RE.fullmatch(expected_hash):
        raise ValidationError("handoff artifact has malformed checksum")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        source_fd = os.open(source, flags)
    except OSError as exc:
        raise ValidationError("handoff artifact cannot be opened safely") from exc
    temporary = None
    try:
        metadata = os.fstat(source_fd)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != source_uid:
            raise ValidationError("handoff artifact has unexpected type or owner")
        destination.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        output_fd, temporary = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
        try:
            os.fchmod(output_fd, mode)
            copied_hash = hashlib.sha256()
            while chunk := os.read(source_fd, 1024 * 1024):
                copied_hash.update(chunk)
                remaining = memoryview(chunk)
                while remaining:
                    written = os.write(output_fd, remaining)
                    if written <= 0:
                        raise OSError("short handoff artifact write")
                    remaining = remaining[written:]
            if copied_hash.hexdigest() != expected_hash:
                raise ValidationError("handoff artifact checksum mismatch")
            os.fsync(output_fd)
        finally:
            os.close(output_fd)
        os.replace(temporary, destination)
        temporary = None
        fsync_dir(destination.parent)
    finally:
        os.close(source_fd)
        if temporary is not None:
            Path(temporary).unlink(missing_ok=True)


def installed_runtime_identity(binary: Path) -> Identity:
    result = subprocess.run(
        [str(binary), "--build-identity"], capture_output=True, text=True, timeout=10, check=True
    )
    try:
        value = json.loads(result.stdout)
        identity = Identity(version=str(value["version"]), git_sha=str(value["git_sha"]))
    except (json.JSONDecodeError, KeyError, TypeError) as exc:
        raise ActivationError("installed runtime has invalid embedded identity") from exc
    if not identity.version or not identity.git_sha or identity.git_sha == "unknown":
        raise ActivationError("installed runtime has incomplete embedded identity")
    return identity


def installed_environment(path: Path) -> dict[str, str]:
    environment: dict[str, str] = {}
    if not path.exists():
        return environment
    for raw in path.read_text().splitlines():
        if not raw or raw.lstrip().startswith("#"):
            continue
        key, separator, value = raw.partition("=")
        if not separator or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key) is None or key in environment:
            raise ActivationError("installed environment snapshot is invalid")
        environment[key] = value.replace("\\n", "\n")
    return environment


@dataclasses.dataclass(frozen=True)
class InstalledHttp:
    pass


@dataclasses.dataclass(frozen=True)
class InstalledAutoTls:
    pass


@dataclasses.dataclass(frozen=True)
class InstalledManualTls:
    cert_path: Path
    key_path: Path
    ca_path: Optional[Path] = None


InstalledTls = InstalledHttp | InstalledAutoTls | InstalledManualTls


def installed_tls(environment: dict[str, str]) -> InstalledTls:
    cert = environment.get("PHOENIX_TLS_CERT_PATH")
    key = environment.get("PHOENIX_TLS_KEY_PATH")
    if cert is not None and key is not None:
        return InstalledManualTls(Path(cert), Path(key),
                                  Path(environment["PHOENIX_TLS_CA_CERT_PATH"])
                                  if "PHOENIX_TLS_CA_CERT_PATH" in environment else None)
    if cert is not None or key is not None:
        raise ValidationError("installed TLS requires both certificate and key paths")
    mode = environment.get("PHOENIX_TLS", "").strip().lower()
    if mode in {"", "0", "false", "off", "none"}:
        return InstalledHttp()
    if mode in {"1", "true", "on", "auto"}:
        return InstalledAutoTls()
    raise ValidationError("installed TLS mode is unsupported or lacks manual paths")


def validate_installed_tls(tls: InstalledTls, service_user: str) -> None:
    if isinstance(tls, InstalledManualTls):
        try:
            account = pwd.getpwnam(service_user)
            groups = set(os.getgrouplist(account.pw_name, account.pw_gid)) | {account.pw_gid}
        except (KeyError, OSError) as exc:
            raise ValidationError("installed TLS service user identity cannot be resolved") from exc

        def permitted(metadata: os.stat_result, owner_bit: int) -> bool:
            if account.pw_uid == 0:
                return True
            if metadata.st_uid == account.pw_uid:
                bit = owner_bit
            elif metadata.st_gid in groups:
                bit = owner_bit >> 3
            else:
                bit = owner_bit >> 6
            return bool(metadata.st_mode & bit)

        for path in (tls.cert_path, tls.key_path, *((tls.ca_path,) if tls.ca_path is not None else ())):
            if ".." in path.parts:
                raise ValidationError("installed TLS paths must not contain parent traversal")
            try:
                if not path.is_absolute():
                    raise ValidationError("installed TLS paths must be absolute regular non-symlink files")
                for parent in reversed(path.parents):
                    metadata = parent.lstat()
                    if not stat.S_ISDIR(metadata.st_mode):
                        raise ValidationError("installed TLS paths require non-symlink directory ancestors")
                    if not permitted(metadata, stat.S_IXUSR):
                        raise ValidationError("installed TLS paths are not traversable by service user")
                metadata = path.lstat()
                if not stat.S_ISREG(metadata.st_mode):
                    raise ValidationError("installed TLS paths must be absolute regular non-symlink files")
                if not permitted(metadata, stat.S_IRUSR):
                    raise ValidationError("installed TLS paths are not readable by service user")
            except OSError as exc:
                raise ValidationError("installed TLS paths cannot be inspected safely") from exc

        try:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(tls.cert_path, tls.key_path, password=lambda: b"")
            if tls.ca_path is not None:
                ca_bytes = tls.ca_path.read_bytes()
                ca_pem = ca_bytes.decode("utf-8").strip()
                if (len(ca_bytes) > 32 * 1024 or not re.fullmatch(
                        r"-----BEGIN CERTIFICATE-----\s+[A-Za-z0-9+/=\s]+-----END CERTIFICATE-----", ca_pem)):
                    raise ValidationError("installed TLS CA must be exactly one PEM certificate within 32 KiB")
                context.load_verify_locations(cafile=str(tls.ca_path))
                openssl = preflight_executable("openssl", "version", expected="SSL")
                details = subprocess.run([str(openssl), "x509", "-in", str(tls.ca_path), "-noout", "-text"],
                                         check=True, capture_output=True, text=True, timeout=10).stdout
                constraints = re.findall(r"X509v3 Basic Constraints:[^\n]*\n\s*([^\n]+)", details)
                usage = re.findall(r"X509v3 Key Usage:[^\n]*\n\s*([^\n]+)", details)
                if (len(constraints) != 1 or not re.search(r"\bCA:TRUE\b", constraints[0])
                        or len(usage) > 1 or (usage and "Certificate Sign" not in usage[0])):
                    raise ValidationError("installed TLS CA lacks certificate-authority capability")
                subprocess.run([str(openssl), "verify", "-trusted", str(tls.ca_path), "-partial_chain",
                                "-untrusted", str(tls.cert_path), "-purpose", "sslserver", str(tls.cert_path)],
                               check=True, capture_output=True, text=True, timeout=10)
        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            raise ValidationError("installed TLS certificate/key/CA or CA chain is invalid") from exc


def installed_socket_listener(path: Path) -> tuple[str, int]:
    section = None
    sections: set[str] = set()
    listeners = []
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith(("#", ";")):
            continue
        if line in {"[Unit]", "[Socket]", "[Install]"}:
            if line in sections:
                raise ValidationError("installed socket has repeated sections")
            sections.add(line)
            section = line
            continue
        key, sep, value = line.partition("=")
        if (section is None or not sep or re.fullmatch(r"[A-Za-z][A-Za-z0-9]*", key.strip()) is None
                or line.endswith("\\")):
            raise ValidationError("unsupported installed socket syntax")
        key, value = key.strip(), value.strip()
        if section == "[Socket]" and key not in {"ListenStream", "NoDelay", "Backlog", "BindIPv6Only"}:
            raise ValidationError("unsupported installed socket customization")
        if key.startswith("Listen"):
            if section != "[Socket]" or key != "ListenStream":
                raise ValidationError("installed socket requires one TCP ListenStream only")
            listeners.append(value)
    if len(listeners) != 1:
        raise ValidationError("installed socket requires one TCP ListenStream only")
    listener = listeners[0]
    host = "::"  # systemd interprets a bare port as an IPv6 listener.
    if re.fullmatch(r"[0-9]{1,5}", listener):
        port_text = listener
    else:
        match = re.fullmatch(r"(?:\[([0-9A-Fa-f:.]+)\]|([0-9.]+)):([0-9]{1,5})", listener)
        if match is None:
            raise ValidationError("unsupported installed socket listener")
        host = match[1] or match[2]
        port_text = match[3]
        try:
            address = ipaddress.ip_address(host)
        except ValueError as exc:
            raise ValidationError("unsupported installed socket address") from exc
        if (address.version == 6) != (match[1] is not None):
            raise ValidationError("unsupported installed socket address syntax")
        if not address.is_unspecified and not address.is_loopback:
            raise ValidationError("installed socket address must be wildcard or loopback")
        host = str(address)
    port = int(port_text)
    if not 1 <= port <= 65535:
        raise ValidationError("installed socket has invalid port")
    host = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(host, host)
    return host, port


def installed_health_url(environment: dict[str, str], socket: Path, service_user: str) -> str:
    tls = installed_tls(environment)
    validate_installed_tls(tls, service_user)
    host, port = installed_socket_listener(socket)
    configured_port = environment.get("PHOENIX_PORT")
    if configured_port is not None and (re.fullmatch(r"[0-9]{1,5}", configured_port) is None
                                        or int(configured_port) != port):
        raise ValidationError("installed PHOENIX_PORT must match socket listener port")
    scheme = "http" if isinstance(tls, InstalledHttp) else "https"
    authority = f"[{host}]" if ":" in host else host
    return f"{scheme}://{authority}:{port}/api/version"


def capture_rollback(transaction: Path, policy: ValidationPolicy, manifest_path: Path) -> None:
    sources = {
        "binary": Path(policy.targets.binary),
        "service": Path(policy.targets.service),
        "socket": Path(policy.targets.socket),
        "environment": Path(policy.targets.environment),
    }
    installed = any(source.exists() for source in sources.values())
    if installed and not all(sources[name].is_file() for name in ("binary", "service", "socket")):
        raise ValidationError("existing runtime lacks complete rollback binary and units")
    rollback: dict[str, dict[str, Optional[str]]] = {
        name: {"path": None, "sha256": None} for name in sources
    }
    if installed:
        for name, source in sources.items():
            if source.is_symlink():
                raise ValidationError(f"rollback {name} must not be a symlink")
            if source.is_file():
                destination = transaction / f"rollback-{name}"
                copy_handoff_file(source, destination, sha256(source), 0, 0o700 if name == "binary" else 0o600)
                rollback[name] = {"path": str(destination), "sha256": sha256(destination)}
    manifest_raw = json.loads(manifest_path.read_text())
    manifest_raw["rollback"] = rollback
    deployed_sha = Path(policy.targets.deployed_sha)
    if installed:
        assert rollback["binary"]["path"] is not None
        previous = installed_runtime_identity(Path(rollback["binary"]["path"]))
        manifest_raw["previous"] = dataclasses.asdict(previous)
        rollback_environment = rollback["environment"]
        environment_path = rollback_environment["path"]
        environment = {}
        for raw in Path(rollback["service"]["path"]).read_text().splitlines():
            line = raw.strip()
            if line.startswith("Environment="):
                for assignment in shlex.split(line.partition("=")[2]):
                    key, sep, value = assignment.partition("=")
                    if not sep:
                        raise ValidationError("invalid installed service environment")
                    environment[key] = value
        if environment_path is not None:
            environment.update(installed_environment(Path(environment_path)))
        previous_user = service_user_from_unit(Path(rollback["service"]["path"]))
        if manifest_raw.get("mode", "ordinary") == PROTECTED_MODE:
            manifest_raw["service_user"] = previous_user
        manifest_raw["previous_health_url"] = installed_health_url(
            environment, Path(rollback["socket"]["path"]), previous_user
        )
    else:
        manifest_raw["previous"] = None
        manifest_raw["previous_health_url"] = None
    manifest_raw["previous_deployed_sha"] = deployed_sha.read_text().strip() if deployed_sha.is_file() else None
    if manifest_raw.get("mode", "ordinary") == PROTECTED_MODE:
        if not installed or rollback["environment"]["path"] is None:
            raise ValidationError("protected upgrade requires complete installed predecessor and environment")
        for name in ("service", "socket", "environment"):
            retained = Path(manifest_raw["candidate"][name]["path"])
            if retained.parent != transaction:
                raise ValidationError("protected candidate configuration escapes transaction")
            atomic_write(retained, Path(rollback[name]["path"]).read_bytes())
            manifest_raw["candidate"][name] = {"path": str(retained), "sha256": sha256(retained)}
        manifest_raw["expected_health_url"] = manifest_raw["previous_health_url"]
    atomic_write(manifest_path, (json.dumps(manifest_raw, sort_keys=True, indent=2) + "\n").encode())


def prepare_data_directory(service_user: str, deployed_sha_target: str) -> bool:
    account = pwd.getpwnam(service_user)
    data_dir = Path(deployed_sha_target).parent
    preexisting = data_dir.exists()
    data_dir.mkdir(parents=True, mode=0o750, exist_ok=True)
    os.chown(data_dir, account.pw_uid, account.pw_gid)
    os.chmod(data_dir, 0o750)
    db_path = data_dir / "prod.db"
    for path in (db_path, Path(f"{db_path}-wal"), Path(f"{db_path}-shm")):
        if path.exists():
            os.chown(path, account.pw_uid, account.pw_gid)
    return preexisting


def _read_bound_bundle(bundle_path: Path, expected_sha256: str, source_uid: int) -> dict:
    if SHA256_RE.fullmatch(expected_sha256) is None:
        raise ValidationError("invalid handoff bundle digest")
    try:
        bundle_fd = os.open(bundle_path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as exc:
        raise ValidationError("invalid handoff bundle") from exc
    try:
        bundle_stat = os.fstat(bundle_fd)
        if not stat.S_ISREG(bundle_stat.st_mode) or bundle_stat.st_uid != source_uid:
            raise ValidationError("invalid handoff bundle ownership or type")
        if stat.S_IMODE(bundle_stat.st_mode) & 0o022:
            raise ValidationError("handoff bundle is group/world writable")
        with os.fdopen(bundle_fd, "rb", closefd=False) as stream:
            bundle_bytes = stream.read()
    finally:
        os.close(bundle_fd)
    if hashlib.sha256(bundle_bytes).hexdigest() != expected_sha256:
        raise ValidationError("handoff bundle digest mismatch")
    try:
        bundle = json.loads(bundle_bytes)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise ValidationError("invalid handoff bundle") from exc
    if not isinstance(bundle, dict):
        raise ValidationError("invalid handoff bundle")
    return bundle


def _read_legacy_bundle(bundle_path: Path) -> dict:
    try:
        bundle = json.loads(bundle_path.read_text())
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValidationError("invalid handoff bundle") from exc
    if not isinstance(bundle, dict):
        raise ValidationError("invalid handoff bundle")
    return bundle


def stage_handoff(bundle_path: Path, expected_bundle_sha256: str, source_uid: int, policy: ValidationPolicy) -> Path:
    policy.activation_lock_path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    with policy.activation_lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is staging or activating") from exc
        return _stage_handoff_locked(bundle_path, expected_bundle_sha256, source_uid, policy)


def stage_handoff_legacy(bundle_path: Path, source_uid: int, policy: ValidationPolicy) -> Path:
    policy.activation_lock_path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    with policy.activation_lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is staging or activating") from exc
        return _stage_handoff_locked(bundle_path, None, source_uid, policy)


def _stage_handoff_locked(
    bundle_path: Path,
    expected_bundle_sha256: Optional[str],
    source_uid: int,
    policy: ValidationPolicy,
) -> Path:
    try:
        bundle = (
            _read_bound_bundle(bundle_path, expected_bundle_sha256, source_uid)
            if expected_bundle_sha256 is not None
            else _read_legacy_bundle(bundle_path)
        )
        transaction_id = bundle["transaction_id"]
        files = bundle["files"]
    except (KeyError, TypeError) as exc:
        raise ValidationError("invalid handoff bundle") from exc
    allowed = {
        "candidate-binary": 0o700,
        "candidate.service": 0o600,
        "candidate.socket": 0o600,
        "candidate.env": 0o600,
        "rollback-binary": 0o600,
        "rollback.service": 0o600,
        "rollback.socket": 0o600,
        "rollback.env": 0o600,
        "helper.py": 0o700,
        "manifest.json": 0o600,
        "migrations.rs": 0o600,
        PREPARED_RECEIPT: 0o400,
    }
    if not isinstance(files, list) or not files or not all(isinstance(item, dict) for item in files):
        raise ValidationError("handoff bundle has invalid file entries")
    names = [item.get("name") for item in files]
    if len(names) != len(set(names)) or set(names) - set(allowed):
        raise ValidationError("handoff bundle contains duplicate or non-allowlisted artifacts")
    if not {"candidate-binary", "candidate.service", "candidate.socket", "helper.py", "manifest.json"} <= set(names):
        raise ValidationError("handoff bundle is missing required artifacts")
    transaction = acquire_claim(transaction_id, policy, retire_staging=True)
    try:
        for item in files:
            name = item["name"]
            copy_handoff_file(Path(item["source"]), transaction / name, item["sha256"], source_uid, allowed[name])
        manifest_path = transaction / "manifest.json"
        manifest = Manifest.load(manifest_path)
        if manifest.mode not in {"ordinary", PROTECTED_MODE}:
            raise ValidationError("unknown systemd deployment mode")
        if manifest.source_kind == "prepared_artifact" and expected_bundle_sha256 is None:
            raise ValidationError("prepared systemd staging requires a controller-bound bundle")
        capture_rollback(transaction, policy, manifest_path)
        manifest = Manifest.load(manifest_path)
        if manifest.previous is None:
            database = Path(manifest.database_path or str(Path(policy.targets.deployed_sha).parent / "prod.db"))
            if database.exists():
                raise ValidationError("existing database without predecessor is not a first install")
        if manifest.mode == PROTECTED_MODE:
            validate_manifest(manifest_path, manifest, policy)
            database_preflight(manifest)
        data_directory_preexisting = (True if manifest.mode == PROTECTED_MODE else
                                     prepare_data_directory(manifest.service_user, policy.targets.deployed_sha))
        if not isinstance(data_directory_preexisting, bool):
            raise ActivationError("data directory preparation did not return presence state")
        manifest_raw = json.loads(manifest_path.read_text())
        manifest_raw["data_directory_preexisting"] = data_directory_preexisting
        atomic_write(manifest_path, (json.dumps(manifest_raw, sort_keys=True, indent=2) + "\n").encode())
        manifest = Manifest.load(manifest_path)
        if manifest.mode == PROTECTED_MODE:
            os.chmod(manifest_path, 0o400)
            with manifest_path.open("rb") as sealed:
                os.fsync(sealed.fileno())
            fsync_dir(transaction)
        write_policy_status(manifest, policy, "prepared")
        return transaction / "manifest.json"
    except BaseException:
        with policy.claim_lock_path.open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            try:
                status = json.loads(policy.status_path.read_text())
                if status == {"transaction_id": transaction_id, "state": "staging", "migration_started": False}:
                    retire_partial_staging(policy)
            except (OSError, ValueError, ConcurrentDeploy):
                pass
        raise


def validate_recovery_authority(manifest_path: Path, manifest: Manifest, policy: ValidationPolicy) -> None:
    if (not TRANSACTION_RE.fullmatch(manifest.transaction_id)
            or manifest_path != policy.transaction_root / manifest.transaction_id / "manifest.json"
            or manifest.unit_name != policy.unit_name or manifest.targets != policy.targets
            or (manifest.status_path, manifest.active_path, manifest.activation_lock_path, manifest.claim_lock_path)
            != tuple(str(path) for path in (policy.status_path, policy.active_path, policy.activation_lock_path, policy.claim_lock_path))):
        raise ValidationError("recovery paths/unit do not match fixed authority")
    validate_regular_file(str(manifest_path), sha256(manifest_path), manifest_path.parent, policy, "manifest")


def retained_status(manifest: Manifest) -> dict:
    status = json.loads(Path(manifest.status_path).read_text())
    if (not isinstance(status, dict) or status.get("transaction_id") != manifest.transaction_id
            or status.get("mode") != manifest.mode or status.get("source_commit") != manifest.source_commit
            or status.get("expected_git_sha") != manifest.expected.git_sha
            or status.get("expected_version") != manifest.expected.version
            or status.get("source_kind") != manifest.source_kind
            or status.get("release_tag") != manifest.release_tag
            or status.get("release_commit") != manifest.release_commit
            or (status.get("state") == "current_forward_failed" and status.get("migration_started") is not True)
            or (manifest.mode == "ordinary" and status.get("migration_started") is not False)
            or (manifest.mode == PROTECTED_MODE and status.get("manifest_sha256") != transaction_context(manifest)["manifest_sha256"])
            or type(status.get("migration_started")) is not bool
            or Path(manifest.active_path).read_text().strip() != manifest.transaction_id):
        raise ValidationError("retained status/mode/claim mismatch")
    return status


def validate_recovery_request(manifest_path: Path, policy: ValidationPolicy) -> str:
    transaction_id = manifest_path.parent.name
    if (not TRANSACTION_RE.fullmatch(transaction_id)
            or manifest_path != policy.transaction_root / transaction_id / "manifest.json"):
        raise ValidationError("recovery request does not match fixed transaction path")
    metadata = manifest_path.parent.lstat()
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValidationError("recovery transaction must be a real directory")
    validate_root_owned_tree(manifest_path.parent, policy.transaction_root, policy)
    validate_root_owned_tree(policy.active_path.parent, policy.transaction_root.parent, policy)
    claim = policy.active_path.lstat()
    if (not stat.S_ISREG(claim.st_mode) or claim.st_uid != policy.owner_uid
            or stat.S_IMODE(claim.st_mode) & 0o022
            or policy.active_path.read_text().strip() != transaction_id):
        raise ValidationError("recovery requires exact owned active claim")
    if not migration_marker_present(manifest_path.parent):
        metadata = policy.status_path.lstat()
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != policy.owner_uid
                or stat.S_IMODE(metadata.st_mode) & 0o022):
            raise ValidationError("unsafe retained pre-marker status")
        status = json.loads(policy.status_path.read_text())
        if (not isinstance(status, dict) or status.get("transaction_id") != transaction_id
                or status.get("mode") != PROTECTED_MODE or status.get("migration_started") is not False
                or status.get("state") not in {"activating", "activation_failed_rollback_failed"}):
            raise ValidationError("pre-marker recovery requires disrupted activating status; predecessor left unchanged")
    return transaction_id


def write_undecoded_recovery_failure(transaction_id: str, policy: ValidationPolicy, failure: str) -> None:
    with policy.claim_lock_path.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if policy.active_path.read_text().strip() != transaction_id:
            raise ValidationError("recovery claim changed")
        metadata = policy.status_path.lstat()
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != policy.owner_uid
                or stat.S_IMODE(metadata.st_mode) & 0o022):
            raise ValidationError("unsafe retained recovery status")
        status = json.loads(policy.status_path.read_text())
        if (not isinstance(status, dict) or status.get("transaction_id") != transaction_id
                or status.get("mode") != PROTECTED_MODE):
            raise ValidationError("cannot attribute retained recovery status")
        status.update(state="current_forward_failed", migration_started=True,
                      failure=failure, updated_at=utc_now())
        atomic_write(policy.status_path, (json.dumps(status, sort_keys=True, indent=2) + "\n").encode())


def recover_current_path(manifest_path: Path, policy: ValidationPolicy) -> str:
    with policy.activation_lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("activation is still running") from exc
        transaction_id = validate_recovery_request(manifest_path, policy)
        if not migration_marker_present(manifest_path.parent):
            manifest = Manifest.load(manifest_path)
            validate_recovery_authority(manifest_path, manifest, policy)
            return _recover_pre_marker_locked(manifest, None, policy)
        controller = SystemdUnitControl(policy.unit_name)
        try:
            controller.stop()
            controller.assert_inactive()
            manifest = Manifest.load(manifest_path)
            validate_recovery_authority(manifest_path, manifest, policy)
            return _recover_current_locked(manifest, None, policy)
        except Exception as exc:
            print(f"systemd forward recovery failed before detailed activation: {exc}", file=sys.stderr)
            try:
                controller.stop()
                controller.assert_inactive()
            except Exception:
                pass
            try:
                write_undecoded_recovery_failure(transaction_id, policy, str(exc))
            except Exception:
                pass
            return "current_forward_failed"


def recover_current(manifest: Manifest, controller: Optional[Systemctl] = None,
                    validation_policy: Optional[ValidationPolicy] = None) -> str:
    lock_path = Path(manifest.activation_lock_path)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("activation is still running") from exc
        return _recover_current_locked(manifest, controller, validation_policy)


def _recover_pre_marker_locked(manifest: Manifest, controller: Optional[Systemctl],
                               policy: Optional[ValidationPolicy]) -> str:
    root = Path(manifest.candidate.binary.path).parent
    if policy is not None:
        validate_recovery_request(root / "manifest.json", policy)
        validate_manifest(root / "manifest.json", manifest, policy)
    status = retained_status(manifest)
    if (manifest.mode != PROTECTED_MODE or migration_marker_present(root)
            or status["migration_started"] is not False
            or status["state"] not in {"activating", "activation_failed_rollback_failed"}):
        raise ValidationError("pre-marker recovery requires disrupted activating status; predecessor left unchanged")
    verify_start_inhibit_files(manifest)
    ctl = controller or Systemctl(manifest)
    prepared = None
    try:
        ctl.stop()
        ctl.assert_inactive()
        preflight_database_exclusivity(ctl)
        ctl.verify_protected_configuration()
        verify_preserved_configuration(manifest)
        assert_database_exclusive(manifest, ctl)
        evidence = database_evidence(effective_database(manifest, installed=True))
        if evidence[0] != candidate_registry(manifest)[:65]:
            raise ValidationError("pre-marker live database must equal exact source prefix65")

        def retained_json(name: str) -> dict:
            path = root / name
            metadata = path.lstat()
            if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
                    or stat.S_IMODE(metadata.st_mode) & 0o077):
                raise ValidationError("unsafe pre-marker recovery evidence")
            value = json.loads(path.read_text())
            if not isinstance(value, dict) or any(value.get(k) != v for k, v in transaction_context(manifest).items()):
                raise ValidationError("pre-marker recovery evidence context mismatch")
            return value

        captured = retained_json("predecessor-state.json").get("unit_state")
        fields = {"service_active", "service_enabled", "socket_active", "socket_enabled", "main_pid"}
        if (not isinstance(captured, dict) or set(captured) != fields
                or any(type(captured[key]) is not bool for key in fields - {"main_pid"})
                or type(captured["main_pid"]) is not int or captured["main_pid"] < 0):
            raise ValidationError("invalid captured predecessor unit state")
        previous_state = UnitState(**captured)
        if installed_runtime_identity(Path(manifest.rollback.binary.path)) != manifest.previous:
            raise ValidationError("captured predecessor identity mismatch")
        if any((root / name).exists() or (root / name).is_symlink()
               for name in ("source-proof.json", "source.sqlite")):
            retained_json("source-proof.json")
            backup = root / "source.sqlite"
            metadata = backup.lstat()
            if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
                    or stat.S_IMODE(metadata.st_mode) & 0o077):
                raise ValidationError("unsafe retained source backup")
            proof = verify_snapshot_proof(manifest)
            if evidence[1] != proof["source_digest"]:
                raise ValidationError("source database changed after stopped snapshot; predecessor restart forbidden")
        if (root / "rehearsal-proof.json").exists() or (root / "rehearsal-proof.json").is_symlink():
            rehearsal = retained_json("rehearsal-proof.json")
            if type(rehearsal.get("success")) is not bool:
                raise ValidationError("invalid retained rehearsal result")
            if rehearsal["success"]:
                verify_rehearsal_proof(manifest)
        prepared = prepare_installs(manifest, ctl, allow_inhibit=True)
        if migration_marker_present(root):
            raise ValidationError("irreversible marker appeared during pre-marker recovery")
        assert_database_exclusive(manifest, ctl)
        if database_evidence(effective_database(manifest, installed=True)) != evidence:
            raise ValidationError("source changed during pre-marker recovery")
        restore(manifest, ctl, previous_state, prepared)
        write_status(manifest, "activation_failed_rolled_back", failure="interrupted pre-marker activation recovered")
        if not release_claim(manifest):
            raise ValidationError("recovered predecessor but exact claim release failed")
        return "activation_failed_rolled_back"
    except Exception as exc:
        try:
            retain_failed_start_inhibit(manifest, ctl)
        except Exception:
            pass
        write_status(manifest, "activation_failed_rollback_failed", failure="interrupted pre-marker activation",
                     rollback_failure=str(exc))
        return "activation_failed_rollback_failed"
    finally:
        if prepared is not None:
            for path in prepared.paths():
                path.unlink(missing_ok=True)


def _recover_current_locked(manifest: Manifest, controller: Optional[Systemctl],
                            validation_policy: Optional[ValidationPolicy]) -> str:
    root = (validation_policy.transaction_root / manifest.transaction_id if validation_policy
            else Path(manifest.candidate.binary.path).parent)
    marker_path = root / "migration_started"
    if not migration_marker_present(root):
        return _recover_pre_marker_locked(manifest, controller, validation_policy)
    ctl = controller or Systemctl(manifest)
    try:
        ctl.stop()
        ctl.assert_inactive()
        preflight_database_exclusivity(ctl)
        if validation_policy is not None:
            validate_manifest(root / "manifest.json", manifest, validation_policy)
        if manifest.mode != PROTECTED_MODE:
            raise ValidationError("recover-current requires protected mode")
        verify_start_inhibit_files(manifest)
        ctl.verify_protected_configuration()
        assert_database_exclusive(manifest, ctl)
        status = retained_status(manifest)
        if status.get("state") not in {"activating", "current_forward_failed"}:
            raise ValidationError("retained transaction is not forward-recoverable")
        if marker_path.is_symlink() or json.loads(marker_path.read_text()) != transaction_context(manifest):
            raise ValidationError("irreversible marker does not match transaction")
        registry = candidate_registry(manifest)
        verify_rehearsal_proof(manifest)
        verify_preserved_configuration(manifest)
        if (sha256(Path(manifest.targets.binary)) != manifest.candidate.binary.sha256
                or installed_runtime_identity(Path(manifest.targets.binary)) != manifest.expected):
            raise ValidationError("installed candidate changed; recovery cannot replace artifacts")
        ledger = database_evidence(effective_database(manifest, installed=True))[0]
        if ledger not in tuple(registry[:n] for n in range(65, len(registry) + 1)):
            raise ValidationError("stopped database diverges from candidate registry")
        write_status(manifest, "activating")
        if ledger != registry:
            run_current_migration(manifest, ctl)
        if database_evidence(effective_database(manifest, installed=True))[0] != candidate_registry(manifest):
            raise ActivationError("recovery did not reach exact target128")
        if sha256(Path(manifest.targets.binary)) != manifest.candidate.binary.sha256:
            raise ActivationError("candidate changed before recovery start")
        authorize_candidate_start(manifest, ctl)
        ctl.start_candidate(0)
        wait_for_identity(manifest, manifest.expected, manifest.expected_health_url)
        atomic_write(Path(manifest.targets.deployed_sha), (manifest.source_commit + "\n").encode())
        write_status(manifest, "committed")
        return "committed"
    except Exception as exc:
        try:
            retain_failed_start_inhibit(manifest, ctl)
        except Exception:
            pass
        write_status(manifest, "current_forward_failed", failure=str(exc))
        return "current_forward_failed"


def abandon(manifest: Manifest) -> None:
    with Path(manifest.activation_lock_path).open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("activation is running; cannot abandon") from exc
        status = retained_status(manifest)
        if status.get("state") != "prepared" or status["migration_started"] or migration_marker_present(Path(manifest.candidate.binary.path).parent):
            raise ValidationError("only a pre-disruption prepared transaction may be abandoned")
        write_status(manifest, "precondition_failed", failure="transient activation unit did not start")
        if not release_claim(manifest):
            raise ActivationError("failed to release abandoned activation claim")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("--protected-protocol-version", action="store_true")
    parser.add_argument("--protected-prepared-protocol-version", action="store_true")
    parser.add_argument("action", nargs="?", choices=("activate", "stage", "abandon", "recover-current"),
                        help="recover-current restores only a proven pre-marker predecessor or resumes the exact post-marker candidate")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--bundle-sha256")
    parser.add_argument("--source-uid", type=int)
    args = parser.parse_args()
    if args.protected_prepared_protocol_version:
        print(PREPARED_HANDOFF_PROTOCOL_VERSION)
        return 0
    if args.protected_protocol_version:
        print(PROTECTED_HANDOFF_PROTOCOL_VERSION)
        return 0
    if args.protocol_version:
        print(HANDOFF_PROTOCOL_VERSION)
        return 0
    if os.geteuid() != 0:
        raise SystemExit("systemd deployment helper must run as root")
    if args.action == "stage":
        if args.bundle is None or args.source_uid is None:
            parser.error("staging requires stage --bundle PATH --source-uid UID")
        try:
            policy = ValidationPolicy.production()
            manifest_path = (
                stage_handoff(args.bundle, args.bundle_sha256, args.source_uid, policy)
                if args.bundle_sha256 is not None
                else stage_handoff_legacy(args.bundle, args.source_uid, policy)
            )
            print(manifest_path)
            return 0
        except Exception as exc:
            print(f"systemd handoff staging failed: {exc}", file=sys.stderr)
            return 1
    if args.action == "abandon":
        if args.manifest is None:
            parser.error("abandon requires abandon --manifest PATH")
        try:
            manifest = Manifest.load(args.manifest)
            validate_manifest(args.manifest, manifest, ValidationPolicy.production())
            abandon(manifest)
            return 0
        except Exception as exc:
            print(f"systemd activation abandonment failed: {exc}", file=sys.stderr)
            return 1
    if args.action not in {"activate", "recover-current"} or args.manifest is None:
        parser.error("activation requires activate --manifest PATH")
    manifest = None
    validated = False
    policy = ValidationPolicy.production()
    try:
        if args.action == "recover-current":
            state = recover_current_path(args.manifest, policy)
            if state in {"committed", "activation_failed_rolled_back"}:
                manifest = Manifest.load(args.manifest)
                if status_is_durable_terminal(manifest):
                    release_claim(manifest)
            print(state, flush=True)
            return 0 if state in {"committed", "activation_failed_rolled_back"} else 1
        manifest = Manifest.load(args.manifest)
        validate_manifest(args.manifest, manifest, policy)
        validated = True
        if manifest.mode == PROTECTED_MODE and args.action == "activate":
            status = retained_status(manifest)
            if status["state"] != "prepared" or status["migration_started"] or migration_marker_present(args.manifest.parent):
                raise ValidationError("protected activation requires untouched prepared status; use recover-current")
        state = activate(manifest)
        if state in CLAIM_RELEASABLE_STATES and status_is_durable_terminal(manifest):
            release_claim(manifest)
        print(state, flush=True)
        return 0 if state == "committed" else 1
    except ConcurrentDeploy as exc:
        print(f"systemd activation helper failed: {exc}", file=sys.stderr)
        return 1
    except Exception as exc:
        if validated and manifest is not None and manifest.mode == PROTECTED_MODE and args.action == "activate":
            try:
                if (migration_marker_present(args.manifest.parent)
                        or any(path.exists() or path.is_symlink() for path in start_inhibit_paths(manifest))):
                    raise ValidationError("disruption evidence forbids precondition claim release")
                if retained_status(manifest).get("state") == "precondition_failed":
                    release_claim(manifest)
            except Exception:
                pass
        if manifest is not None and manifest.mode == "ordinary" and args.action == "activate":
            try:
                staged_transaction_id = args.manifest.parent.name
                expected_manifest_path = policy.transaction_root / staged_transaction_id / "manifest.json"
                if args.manifest != expected_manifest_path or manifest.transaction_id != staged_transaction_id:
                    raise ValidationError("staged transaction identity does not match manifest path")
                if (expected_manifest_path.parent / "migration_started").exists():
                    raise ValidationError("irreversible marker forbids precondition cleanup")
                if policy.status_path.exists():
                    retained = json.loads(policy.status_path.read_text())
                    if (retained.get("mode") != "ordinary" or retained.get("migration_started") is not False
                            or retained.get("transaction_id") != manifest.transaction_id
                            or retained.get("state") not in {"prepared", "precondition_failed"}):
                        raise ValidationError("unsafe retained status; claim must remain")
                write_policy_status(manifest, policy, "precondition_failed", failure=str(exc))
                release_policy_claim(manifest.transaction_id, policy)
            except Exception:
                pass
        print(f"systemd activation helper failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
