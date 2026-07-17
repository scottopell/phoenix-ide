#!/usr/bin/env python3
"""Root-owned transactional systemd activation helper."""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import hashlib
import json
import os
import pwd
import re
import shutil
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
EMBEDDED_SHA_RE = re.compile(r"[0-9a-f]{12}")
VERSION_RE = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}")
RELEASE_TAG_RE = re.compile(r"v[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}")
SOURCE_KINDS = {"local_head", "published_release"}


class ValidationError(RuntimeError):
    pass


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

    @classmethod
    def load(cls, path: Path) -> "Manifest":
        try:
            raw = json.loads(path.read_text())
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


def validate_identity(identity: Identity, description: str) -> None:
    if not VERSION_RE.fullmatch(identity.version):
        raise ValidationError(f"{description} has malformed version")
    if not EMBEDDED_SHA_RE.fullmatch(identity.git_sha):
        raise ValidationError(f"{description} has malformed embedded git SHA")


def validate_health_url(value: Optional[str], description: str) -> None:
    if value is None:
        return
    parsed = urllib.parse.urlparse(value)
    if parsed.scheme not in {"http", "https"} or parsed.hostname not in {"127.0.0.1", "::1", "localhost"}:
        raise ValidationError(f"{description} must be a loopback HTTP endpoint")
    if parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path != "/api/version":
        raise ValidationError(f"{description} must be a credential-free /api/version endpoint")
    try:
        if parsed.port is None or not 1 <= parsed.port <= 65535:
            raise ValidationError(f"{description} has invalid port")
    except ValueError as exc:
        raise ValidationError(f"{description} has invalid port") from exc


def validate_manifest(manifest_path: Path, manifest: Manifest, policy: ValidationPolicy) -> None:
    if manifest.manifest_version != HANDOFF_PROTOCOL_VERSION:
        raise ValidationError(
            f"unsupported handoff protocol {manifest.manifest_version!r}; expected {HANDOFF_PROTOCOL_VERSION}"
        )
    if not TRANSACTION_RE.fullmatch(manifest.transaction_id):
        raise ValidationError("malformed transaction id")
    if not UNIT_RE.fullmatch(manifest.unit_name) or manifest.unit_name != policy.unit_name:
        raise ValidationError("unit name is not allowed")
    if not GIT_SHA_RE.fullmatch(manifest.source_commit):
        raise ValidationError("source commit must be a full lowercase git SHA")
    if manifest.source_kind not in SOURCE_KINDS:
        raise ValidationError("source kind is not allowed")
    if manifest.source_kind == "local_head":
        if manifest.release_tag is not None or manifest.release_commit is not None:
            raise ValidationError("local candidate has release metadata")
    else:
        if manifest.release_tag is None or not RELEASE_TAG_RE.fullmatch(manifest.release_tag):
            raise ValidationError("published candidate has malformed release tag")
        if manifest.release_commit != manifest.source_commit:
            raise ValidationError("published candidate release commit does not match source commit")
    validate_identity(manifest.expected, "candidate identity")
    if manifest.previous is not None:
        validate_identity(manifest.previous, "previous identity")
    validate_health_url(manifest.expected_health_url, "candidate health URL")
    validate_health_url(manifest.previous_health_url, "previous health URL")
    if manifest.previous_deployed_sha is not None and not GIT_SHA_RE.fullmatch(manifest.previous_deployed_sha):
        raise ValidationError("previous deployed SHA is malformed")
    if not manifest.source_commit.startswith(manifest.expected.git_sha):
        raise ValidationError("candidate identity does not match source commit")
    if manifest.targets != policy.targets:
        raise ValidationError("manifest target paths are not allowed")
    validate_service_user(manifest.service_user)

    transaction = policy.transaction_root / manifest.transaction_id
    root = require_descendant(manifest_path.parent, policy.transaction_root, "transaction directory")
    if root != transaction.resolve(strict=True):
        raise ValidationError("manifest is not in its transaction directory")
    validate_regular_file(str(manifest_path), sha256(manifest_path), root, policy, "manifest")

    validate_regular_file(manifest.candidate.binary.path, manifest.candidate.binary.sha256, root, policy, "candidate binary")
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
    "rejected_concurrent",
}

CLAIM_RELEASABLE_STATES = TERMINAL_STATES - {"activation_failed_rollback_failed"}


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
        return status.get("transaction_id") == manifest.transaction_id and status.get("state") in TERMINAL_STATES
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
            if status.get("state") not in CLAIM_RELEASABLE_STATES:
                return False
            if not status_is_durable_terminal(manifest):
                return False
            claim.unlink()
            fsync_dir(claim.parent)
            return True
        except FileNotFoundError:
            return False


class Systemctl:
    def __init__(self, manifest: Manifest, run=subprocess.run):
        self.manifest = manifest
        self.run = run
        self.service = f"{manifest.unit_name}.service"
        self.socket = f"{manifest.unit_name}.socket"
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

    def verify_units(self, service: Path, socket: Path, candidate_binary: Path) -> None:
        with tempfile.TemporaryDirectory(prefix="phoenix-systemd-verify-") as temporary:
            verification_service = Path(temporary) / service.name
            target_binary = self.manifest.targets.binary
            content = service.read_text()
            if target_binary not in content:
                raise ActivationError("candidate service does not execute the fixed production binary")
            verification_service.write_text(content.replace(target_binary, str(candidate_binary)))
            result = self.run(
                ["systemd-analyze", "verify", str(socket), str(verification_service)],
                capture_output=True,
                text=True,
            )
        if result.returncode != 0:
            raise ActivationError(f"systemd unit validation failed: {(result.stderr or result.stdout).strip()}")

    def stop(self) -> None:
        self.command("stop", self.service, self.socket, check=False)
        self.disruption_started = True
        deadline = time.monotonic() + self.manifest.transition_timeout_secs
        while time.monotonic() < deadline:
            if not self.is_active(self.service) and not self.is_active(self.socket):
                return
            time.sleep(0.1)
        raise ActivationError("timed out stopping previous systemd units")

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


def prepare_installs(manifest: Manifest, systemctl: Systemctl) -> PreparedInstalls:
    candidate_service = Path(manifest.candidate.service.path)
    candidate_socket = Path(manifest.candidate.socket.path)
    systemctl.verify_units(candidate_service, candidate_socket, Path(manifest.candidate.binary.path))
    service_gid = service_group_id(manifest.service_user)
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
                    owner_gid=service_gid,
                )
        return prepared
    except BaseException:
        for path in reserved:
            path.unlink(missing_ok=True)
        raise


def install_candidate(manifest: Manifest, prepared: PreparedInstalls) -> None:
    commit_atomic_install(prepared.candidate_binary, Path(manifest.targets.binary))
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


def restore(manifest: Manifest, systemctl: Systemctl, previous_state: UnitState, prepared: PreparedInstalls) -> None:
    systemctl.stop()
    if manifest.previous is None:
        systemctl.command("disable", systemctl.socket, systemctl.service, check=False)
        for target in dataclasses.astuple(manifest.targets)[:4]:
            remove_target(Path(target))
        systemctl.daemon_reload()
        restore_deployed_sha(manifest)
        return
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
    systemctl.daemon_reload()
    systemctl.restore_state(previous_state)
    if previous_state.service_active:
        if manifest.previous_health_url is None:
            raise ActivationError("previous health endpoint is unavailable")
        wait_for_identity(manifest, manifest.previous, manifest.previous_health_url)
    restore_deployed_sha(manifest)


def activate(manifest: Manifest, systemctl: Optional[Systemctl] = None) -> str:
    lock_path = Path(manifest.activation_lock_path)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is already activating") from exc
        controller = systemctl or Systemctl(manifest)
        prepared = None
        try:
            prepared = prepare_installs(manifest, controller)
        except Exception as exc:
            write_status(manifest, "precondition_failed", failure=str(exc))
            raise
        previous_state = controller.inspect()
        write_status(manifest, "activating")
        try:
            controller.stop()
            install_candidate(manifest, prepared)
            controller.daemon_reload()
            controller.start_candidate(previous_state.main_pid)
            wait_for_identity(manifest, manifest.expected, manifest.expected_health_url)
            atomic_write(Path(manifest.targets.deployed_sha), (manifest.source_commit + "\n").encode())
            write_status(manifest, "committed")
            return "committed"
        except Exception as activation_exc:
            failure = str(activation_exc)
            if not controller.disruption_started:
                write_status(manifest, "precondition_failed", failure=failure)
                raise
            try:
                restore(manifest, controller, previous_state, prepared)
                write_status(manifest, "activation_failed_rolled_back", failure=failure)
                return "activation_failed_rolled_back"
            except Exception as rollback_exc:
                write_status(
                    manifest,
                    "activation_failed_rollback_failed",
                    failure=failure,
                    rollback_failure=str(rollback_exc),
                )
                return "activation_failed_rollback_failed"
        finally:
            if prepared is not None:
                for path in prepared.paths():
                    path.unlink(missing_ok=True)


def acquire_claim(transaction_id: str, policy: ValidationPolicy) -> Path:
    if not TRANSACTION_RE.fullmatch(transaction_id):
        raise ValidationError("malformed transaction id")
    policy.transaction_root.mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod(policy.transaction_root, 0o700)
    policy.active_path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    with policy.claim_lock_path.open("a+") as lock:
        os.chmod(policy.claim_lock_path, 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        if policy.active_path.exists():
            owner = policy.active_path.read_text().strip()
            try:
                status = json.loads(policy.status_path.read_text())
            except (OSError, json.JSONDecodeError):
                status = {}
            if owner and not (
                status.get("transaction_id") == owner
                and status.get("state") in CLAIM_RELEASABLE_STATES
            ):
                raise ConcurrentDeploy(f"deployment transaction {owner} is unresolved")
        transaction = policy.transaction_root / transaction_id
        if transaction.exists():
            raise ValidationError("transaction directory already exists")
        transaction.mkdir(mode=0o700)
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
        if sha256_fd(source_fd) != expected_hash:
            raise ValidationError("handoff artifact checksum mismatch")
        os.lseek(source_fd, 0, os.SEEK_SET)
        destination.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        output_fd, temporary = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
        try:
            os.fchmod(output_fd, mode)
            while chunk := os.read(source_fd, 1024 * 1024):
                os.write(output_fd, chunk)
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


def capture_rollback(transaction: Path, policy: ValidationPolicy, manifest_path: Path) -> None:
    manifest = Manifest.load(manifest_path)
    previous = manifest.previous is not None
    sources = {
        "binary": Path(policy.targets.binary),
        "service": Path(policy.targets.service),
        "socket": Path(policy.targets.socket),
        "environment": Path(policy.targets.environment),
    }
    rollback = {name: {"path": None, "sha256": None} for name in sources}
    if previous and not all(sources[name].is_file() for name in ("binary", "service", "socket")):
        raise ValidationError("existing runtime lacks complete rollback binary and units")
    for name, source in sources.items():
        if source.is_symlink():
            raise ValidationError(f"rollback {name} must not be a symlink")
        if previous and source.is_file():
            destination = transaction / f"rollback-{name}"
            copy_handoff_file(source, destination, sha256(source), 0, 0o600)
            rollback[name] = {"path": str(destination), "sha256": sha256(destination)}
    manifest_raw = json.loads(manifest_path.read_text())
    manifest_raw["rollback"] = rollback
    deployed_sha = Path(policy.targets.deployed_sha)
    manifest_raw["previous_deployed_sha"] = deployed_sha.read_text().strip() if deployed_sha.is_file() else None
    atomic_write(manifest_path, (json.dumps(manifest_raw, sort_keys=True, indent=2) + "\n").encode())


def prepare_data_directory(service_user: str, deployed_sha_target: str) -> None:
    account = pwd.getpwnam(service_user)
    data_dir = Path(deployed_sha_target).parent
    data_dir.mkdir(parents=True, mode=0o750, exist_ok=True)
    os.chown(data_dir, account.pw_uid, account.pw_gid)
    os.chmod(data_dir, 0o750)
    db_path = data_dir / "prod.db"
    for path in (db_path, Path(f"{db_path}-wal"), Path(f"{db_path}-shm")):
        if path.exists():
            os.chown(path, account.pw_uid, account.pw_gid)


def stage_handoff(bundle_path: Path, source_uid: int, policy: ValidationPolicy) -> Path:
    try:
        bundle = json.loads(bundle_path.read_text())
        transaction_id = bundle["transaction_id"]
        files = bundle["files"]
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
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
    }
    if not isinstance(files, list) or not files or not all(isinstance(item, dict) for item in files):
        raise ValidationError("handoff bundle has invalid file entries")
    names = [item.get("name") for item in files]
    if len(names) != len(set(names)) or set(names) - set(allowed):
        raise ValidationError("handoff bundle contains duplicate or non-allowlisted artifacts")
    if not {"candidate-binary", "candidate.service", "candidate.socket", "helper.py", "manifest.json"} <= set(names):
        raise ValidationError("handoff bundle is missing required artifacts")
    transaction = acquire_claim(transaction_id, policy)
    try:
        for item in files:
            name = item["name"]
            copy_handoff_file(Path(item["source"]), transaction / name, item["sha256"], source_uid, allowed[name])
        manifest_path = transaction / "manifest.json"
        manifest = Manifest.load(manifest_path)
        prepare_data_directory(manifest.service_user, policy.targets.deployed_sha)
        capture_rollback(transaction, policy, manifest_path)
        manifest = Manifest.load(manifest_path)
        write_policy_status(manifest, policy, "prepared")
        return transaction / "manifest.json"
    except BaseException:
        shutil.rmtree(transaction, ignore_errors=True)
        with policy.claim_lock_path.open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if policy.active_path.exists() and policy.active_path.read_text().strip() == transaction_id:
                policy.active_path.unlink()
                fsync_dir(policy.active_path.parent)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("action", nargs="?", choices=("activate", "stage", "abandon"))
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--source-uid", type=int)
    args = parser.parse_args()
    if args.protocol_version:
        print(HANDOFF_PROTOCOL_VERSION)
        return 0
    if os.geteuid() != 0:
        raise SystemExit("systemd deployment helper must run as root")
    if args.action == "stage":
        if args.bundle is None or args.source_uid is None:
            parser.error("staging requires stage --bundle PATH --source-uid UID")
        try:
            print(stage_handoff(args.bundle, args.source_uid, ValidationPolicy.production()))
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
            write_status(manifest, "precondition_failed", failure="transient activation unit did not start")
            if not release_claim(manifest):
                raise ActivationError("failed to release abandoned activation claim")
            return 0
        except Exception as exc:
            print(f"systemd activation abandonment failed: {exc}", file=sys.stderr)
            return 1
    if args.action != "activate" or args.manifest is None:
        parser.error("activation requires activate --manifest PATH")
    manifest = None
    policy = ValidationPolicy.production()
    try:
        manifest = Manifest.load(args.manifest)
        validate_manifest(args.manifest, manifest, policy)
        state = activate(manifest)
        if state in CLAIM_RELEASABLE_STATES and status_is_durable_terminal(manifest):
            release_claim(manifest)
        print(state, flush=True)
        return 0 if state == "committed" else 1
    except ConcurrentDeploy as exc:
        print(f"systemd activation helper failed: {exc}", file=sys.stderr)
        return 1
    except Exception as exc:
        if manifest is not None:
            try:
                staged_transaction_id = args.manifest.parent.name
                expected_manifest_path = policy.transaction_root / staged_transaction_id / "manifest.json"
                if args.manifest != expected_manifest_path or manifest.transaction_id != staged_transaction_id:
                    raise ValidationError("staged transaction identity does not match manifest path")
                write_policy_status(manifest, policy, "precondition_failed", failure=str(exc))
                release_policy_claim(manifest.transaction_id, policy)
            except Exception:
                if status_is_durable_terminal(manifest):
                    release_claim(manifest)
        print(f"systemd activation helper failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
