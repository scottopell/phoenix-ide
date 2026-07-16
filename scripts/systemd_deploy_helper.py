#!/usr/bin/env python3
"""Root-owned transactional systemd activation helper."""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
import pwd
import re
import stat
import urllib.parse
from pathlib import Path
from typing import Optional

HANDOFF_PROTOCOL_VERSION = 1
PRODUCTION_UNIT = "phoenix-ide"
PRODUCTION_TRANSACTION_ROOT = Path("/var/lib/phoenix-ide/deploy/transactions")
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
    previous_deployed_sha: Optional[str]
    created_at: str

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
    ):
        path = Path(value)
        if path != allowed:
            raise ValidationError(f"{description} path is not allowed")
        validate_root_owned_tree(path.parent, policy.transaction_root.parent, policy)
        if path.is_symlink():
            raise ValidationError(f"{description} path must not be a symlink")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("activate", nargs="?")
    parser.add_argument("--manifest", type=Path)
    args = parser.parse_args()
    if args.protocol_version:
        print(HANDOFF_PROTOCOL_VERSION)
        return 0
    if args.activate != "activate" or args.manifest is None:
        parser.error("activation requires activate --manifest PATH")
    if os.geteuid() != 0:
        raise SystemExit("systemd activation helper must run as root")
    manifest = Manifest.load(args.manifest)
    validate_manifest(args.manifest, manifest, ValidationPolicy.production())
    raise SystemExit("systemd activation is not implemented")


if __name__ == "__main__":
    raise SystemExit(main())
