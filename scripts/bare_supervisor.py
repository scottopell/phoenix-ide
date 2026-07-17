#!/usr/bin/env python3
"""Persistent same-user process owner for Phoenix on Linux without systemd."""

from __future__ import annotations

import argparse
import dataclasses
import fcntl
import hashlib
import json
import os
import re
import shutil
import tempfile
import socket
import struct
import subprocess
import sys
import time
import urllib.request
import urllib.parse
from pathlib import Path
from typing import Optional

PROTOCOL_VERSION = 1
MAX_REQUEST_BYTES = 16 * 1024
TRANSACTION_RE = re.compile(r"[0-9a-f]{32}")
SHA256_RE = re.compile(r"[0-9a-f]{64}")
GIT_SHA_RE = re.compile(r"[0-9a-f]{40}")
EMBEDDED_SHA_RE = re.compile(r"[0-9a-f]{12}")
VERSION_RE = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}")


class SupervisorError(RuntimeError):
    pass


@dataclasses.dataclass(frozen=True)
class RuntimeIdentity:
    version: str
    git_sha: str


@dataclasses.dataclass(frozen=True)
class ChildIdentity:
    pid: int
    proc_start_time: int
    runtime: RuntimeIdentity


@dataclasses.dataclass(frozen=True)
class Artifact:
    name: str
    sha256: str


@dataclasses.dataclass(frozen=True)
class TransactionManifest:
    manifest_version: int
    transaction_id: str
    expected: RuntimeIdentity
    previous: Optional[RuntimeIdentity]
    expected_health_url: str
    previous_health_url: Optional[str]
    candidate_binary: Artifact
    candidate_environment: Artifact
    rollback_binary: Optional[Artifact]
    rollback_environment: Optional[Artifact]
    source_commit: str
    previous_deployed_sha: Optional[str]
    created_at: str
    health_timeout_secs: float = 30

    @classmethod
    def load(cls, path: Path) -> "TransactionManifest":
        try:
            raw = json.loads(path.read_text())
            raw["expected"] = RuntimeIdentity(**raw["expected"])
            raw["previous"] = RuntimeIdentity(**raw["previous"]) if raw.get("previous") else None
            raw["candidate_binary"] = Artifact(**raw["candidate_binary"])
            raw["candidate_environment"] = Artifact(**raw["candidate_environment"])
            raw["rollback_binary"] = Artifact(**raw["rollback_binary"]) if raw.get("rollback_binary") else None
            raw["rollback_environment"] = Artifact(**raw["rollback_environment"]) if raw.get("rollback_environment") else None
            return cls(**raw)
        except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
            raise SupervisorError("invalid bare transaction manifest") from exc


@dataclasses.dataclass(frozen=True)
class Layout:
    root: Path

    @property
    def binary(self) -> Path:
        return self.root / "bin/phoenix-ide"

    @property
    def environment(self) -> Path:
        return self.root / "config/phoenix.env"

    @property
    def deployed_sha(self) -> Path:
        return self.root / "deployed.sha"

    @property
    def deploy_dir(self) -> Path:
        return self.root / "deploy"

    @property
    def transactions(self) -> Path:
        return self.deploy_dir / "transactions"

    @property
    def status_file(self) -> Path:
        return self.deploy_dir / "status.json"

    @property
    def active_file(self) -> Path:
        return self.deploy_dir / "active"

    @property
    def activation_lock(self) -> Path:
        return self.deploy_dir / "activation.lock"

    @property
    def run_dir(self) -> Path:
        return self.root / "run"

    @property
    def socket(self) -> Path:
        return self.run_dir / "supervisor.sock"

    @property
    def state(self) -> Path:
        return self.run_dir / "supervisor-state.json"

    @property
    def log(self) -> Path:
        return self.root / "prod.log"


def proc_start_time(pid: int, proc_root: Path = Path("/proc")) -> int:
    try:
        value = (proc_root / str(pid) / "stat").read_text()
        end = value.rfind(")")
        fields_after_comm = value[end + 2 :].split()
        return int(fields_after_comm[19])
    except (OSError, ValueError, IndexError) as exc:
        raise SupervisorError(f"cannot read process identity for PID {pid}") from exc


def direct_child_matches(child: subprocess.Popen[bytes], identity: ChildIdentity, proc_root: Path = Path("/proc")) -> bool:
    return (
        child.pid == identity.pid
        and child.poll() is None
        and proc_start_time(identity.pid, proc_root) == identity.proc_start_time
    )


def fetch_identity(url: str) -> RuntimeIdentity:
    with urllib.request.urlopen(url, timeout=2) as response:
        value = json.load(response)
    try:
        return RuntimeIdentity(version=str(value["version"]), git_sha=str(value["git_sha"]))
    except (KeyError, TypeError) as exc:
        raise SupervisorError("runtime returned no exact identity") from exc


def wait_for_identity(child: subprocess.Popen[bytes], expected: RuntimeIdentity, url: str, timeout: float) -> ChildIdentity:
    start_time = proc_start_time(child.pid)
    deadline = time.monotonic() + timeout
    observed = "not responding"
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise SupervisorError(f"managed child exited with status {child.returncode}")
        try:
            actual = fetch_identity(url)
            observed = f"{actual.version}/{actual.git_sha}"
            if actual == expected:
                identity = ChildIdentity(child.pid, start_time, actual)
                if direct_child_matches(child, identity):
                    return identity
        except Exception as exc:
            observed = type(exc).__name__
        time.sleep(0.1)
    raise SupervisorError(f"exact runtime identity did not become ready; observed {observed}")


def peer_uid(connection: socket.socket) -> int:
    if not sys.platform.startswith("linux") or not hasattr(socket, "SO_PEERCRED"):
        raise SupervisorError("Linux SO_PEERCRED is required")
    credentials = connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i"))
    _pid, uid, _gid = struct.unpack("3i", credentials)
    return uid


def write_bytes_atomic(path: Path, value: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    with temporary.open("wb") as stream:
        os.chmod(temporary, mode)
        stream.write(value)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def write_json_atomic(path: Path, value: object) -> None:
    write_bytes_atomic(path, (json.dumps(value, sort_keys=True, indent=2) + "\n").encode())


def write_text_atomic(path: Path, value: str) -> None:
    write_bytes_atomic(path, (value + "\n").encode())


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_health_url(value: str) -> None:
    parsed = urllib.parse.urlparse(value)
    if parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "::1", "localhost"}:
        raise SupervisorError("health endpoint must use loopback HTTP")
    if parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path != "/api/version":
        raise SupervisorError("health endpoint must be credential-free /api/version")
    try:
        if parsed.port is None or not 1 <= parsed.port <= 65535:
            raise SupervisorError("health endpoint has invalid port")
    except ValueError as exc:
        raise SupervisorError("health endpoint has invalid port") from exc


def validate_runtime_identity(identity: RuntimeIdentity) -> None:
    if not VERSION_RE.fullmatch(identity.version) or not EMBEDDED_SHA_RE.fullmatch(identity.git_sha):
        raise SupervisorError("runtime identity is malformed")


def validate_artifact(transaction: Path, artifact: Artifact) -> Path:
    if Path(artifact.name).name != artifact.name or not SHA256_RE.fullmatch(artifact.sha256):
        raise SupervisorError("invalid transaction artifact reference")
    path = transaction / artifact.name
    try:
        resolved = path.resolve(strict=True)
        resolved.relative_to(transaction.resolve(strict=True))
    except (OSError, ValueError) as exc:
        raise SupervisorError("transaction artifact escapes transaction directory") from exc
    metadata = path.lstat()
    if path.is_symlink() or not path.is_file() or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise SupervisorError("transaction artifact ownership or mode is unsafe")
    if sha256(resolved) != artifact.sha256:
        raise SupervisorError("transaction artifact checksum mismatch")
    return resolved


def parse_environment(path: Path) -> dict[str, str]:
    environment = os.environ.copy()
    for raw in path.read_text().splitlines():
        if not raw or raw.lstrip().startswith("#"):
            continue
        key, separator, value = raw.partition("=")
        if not separator or not key:
            raise SupervisorError("invalid environment snapshot")
        environment[key] = value.replace("\\n", "\n")
    return environment


def reserve_install(source: Path, target: Path, mode: int) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{target.name}.install-", dir=target.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as destination, source.open("rb") as incoming:
            shutil.copyfileobj(incoming, destination)
            destination.flush()
            os.fsync(destination.fileno())
        return Path(temporary)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def commit_install(prepared: Path, target: Path) -> None:
    os.replace(prepared, target)
    fd = os.open(target.parent, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class Supervisor:
    def __init__(self, layout: Layout, *, owner_uid: Optional[int] = None):
        self.layout = layout
        self.owner_uid = os.getuid() if owner_uid is None else owner_uid
        self.child: Optional[subprocess.Popen[bytes]] = None
        self.child_identity: Optional[ChildIdentity] = None
        self.running = True

    def prepare_layout(self) -> None:
        self.layout.root.mkdir(parents=True, exist_ok=True)
        os.chmod(self.layout.root, 0o700)
        self.layout.run_dir.mkdir(parents=True, exist_ok=True)
        os.chmod(self.layout.run_dir, 0o700)
        if self.layout.socket.is_symlink():
            raise SupervisorError("supervisor socket path must not be a symlink")
        self.layout.socket.unlink(missing_ok=True)

    def status(self) -> dict[str, object]:
        if self.child is not None and self.child_identity is not None and direct_child_matches(self.child, self.child_identity):
            child = dataclasses.asdict(self.child_identity)
            child["runtime"] = dataclasses.asdict(self.child_identity.runtime)
            return {"protocol_version": PROTOCOL_VERSION, "supervisor_pid": os.getpid(), "child": child}
        self.child_identity = None
        return {"protocol_version": PROTOCOL_VERSION, "supervisor_pid": os.getpid(), "child": None}

    def stop_child(self, timeout: float = 10) -> None:
        if self.child is not None and self.child.poll() is None:
            self.child.terminate()
            try:
                self.child.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=5)
        self.child = None
        self.child_identity = None
        write_json_atomic(self.layout.state, self.status())

    def stop_recorded_orphan(self, timeout: float = 10) -> None:
        if not self.layout.state.exists():
            return
        try:
            child = json.loads(self.layout.state.read_text()).get("child")
            if child is None:
                return
            pid = int(child["pid"])
            recorded_start = int(child["proc_start_time"])
            process = Path("/proc") / str(pid)
            if process.stat().st_uid != self.owner_uid or proc_start_time(pid) != recorded_start:
                return
        except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError, SupervisorError):
            return
        os.kill(pid, 15)
        deadline = time.monotonic() + timeout
        while process.exists() and time.monotonic() < deadline:
            try:
                if proc_start_time(pid) != recorded_start:
                    break
            except SupervisorError:
                break
            time.sleep(0.05)
        else:
            if process.exists():
                try:
                    if proc_start_time(pid) == recorded_start:
                        os.kill(pid, 9)
                except SupervisorError:
                    pass
        write_json_atomic(self.layout.state, self.status())

    def start_child(self, command: list[str], env: dict[str, str], expected: RuntimeIdentity, health_url: str, timeout: float) -> ChildIdentity:
        self.stop_child()
        with self.layout.log.open("ab", buffering=0) as log:
            self.child = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            self.child_identity = wait_for_identity(self.child, expected, health_url, timeout)
        except BaseException:
            self.stop_child()
            raise
        write_json_atomic(self.layout.state, self.status())
        return self.child_identity

    def dispatch(self, request: dict[str, object]) -> dict[str, object]:
        if request.get("protocol_version") != PROTOCOL_VERSION:
            raise SupervisorError("unsupported supervisor protocol")
        action = request.get("action")
        if action == "status":
            return {"ok": True, **self.status()}
        if action == "stop":
            self.stop_child()
            return {"ok": True, **self.status()}
        if action == "shutdown-supervisor":
            self.stop_child()
            self.running = False
            return {"ok": True}
        if action == "activate":
            transaction_id = request.get("transaction_id")
            manifest_hash = request.get("manifest_sha256")
            if not isinstance(transaction_id, str) or not isinstance(manifest_hash, str):
                raise SupervisorError("activation requires transaction ID and manifest hash")
            return {"ok": True, "state": self.activate(transaction_id, manifest_hash)}
        raise SupervisorError("unsupported supervisor action")

    def transaction_status(
        self,
        manifest: TransactionManifest,
        manifest_hash: str,
        state: str,
        failure: Optional[str] = None,
        rollback_failure: Optional[str] = None,
        phase: Optional[str] = None,
    ) -> None:
        write_json_atomic(self.layout.status_file, {
            "transaction_id": manifest.transaction_id,
            "manifest_sha256": manifest_hash,
            "state": state,
            "phase": phase,
            "source_commit": manifest.source_commit,
            "expected_version": manifest.expected.version,
            "expected_git_sha": manifest.expected.git_sha,
            "failure": failure,
            "rollback_failure": rollback_failure,
            "updated_at": time.time(),
        })

    def validated_transaction(
        self, transaction_id: str, manifest_hash: str
    ) -> tuple[TransactionManifest, Path, Path, Optional[Path], Optional[Path]]:
        if not TRANSACTION_RE.fullmatch(transaction_id) or not SHA256_RE.fullmatch(manifest_hash):
            raise SupervisorError("malformed immutable transaction reference")
        transaction = self.layout.transactions / transaction_id
        manifest_path = transaction / "manifest.json"
        if sha256(manifest_path) != manifest_hash:
            raise SupervisorError("transaction manifest checksum mismatch")
        manifest = TransactionManifest.load(manifest_path)
        if manifest.manifest_version != PROTOCOL_VERSION or manifest.transaction_id != transaction_id:
            raise SupervisorError("transaction protocol or identity mismatch")
        metadata = transaction.stat()
        if metadata.st_uid != self.owner_uid or metadata.st_mode & 0o077 or metadata.st_mode & 0o200:
            raise SupervisorError("transaction directory ownership or mode is unsafe")
        validate_runtime_identity(manifest.expected)
        validate_health_url(manifest.expected_health_url)
        if not GIT_SHA_RE.fullmatch(manifest.source_commit) or not manifest.source_commit.startswith(manifest.expected.git_sha):
            raise SupervisorError("source commit does not match expected identity")
        if manifest.previous is not None:
            validate_runtime_identity(manifest.previous)
            if manifest.previous_health_url is None:
                raise SupervisorError("previous runtime has no rollback endpoint")
            validate_health_url(manifest.previous_health_url)
        candidate_binary = validate_artifact(transaction, manifest.candidate_binary)
        candidate_environment = validate_artifact(transaction, manifest.candidate_environment)
        rollback_binary = validate_artifact(transaction, manifest.rollback_binary) if manifest.rollback_binary else None
        rollback_environment = validate_artifact(transaction, manifest.rollback_environment) if manifest.rollback_environment else None
        if manifest.previous is not None and (rollback_binary is None or rollback_environment is None):
            raise SupervisorError("existing runtime has incomplete rollback inputs")
        return manifest, candidate_binary, candidate_environment, rollback_binary, rollback_environment

    def restore_previous(
        self,
        manifest: TransactionManifest,
        manifest_hash: str,
        failure: str,
        rollback_binary: Optional[Path],
        rollback_environment: Optional[Path],
    ) -> str:
        self.transaction_status(manifest, manifest_hash, "activating", failure, phase="rolling_back")
        self.stop_child()
        self.stop_recorded_orphan()
        if manifest.previous is None:
            self.layout.binary.unlink(missing_ok=True)
            self.layout.environment.unlink(missing_ok=True)
            self.layout.deployed_sha.unlink(missing_ok=True)
        else:
            if rollback_binary is None or rollback_environment is None or manifest.previous_health_url is None:
                raise SupervisorError("existing runtime has incomplete rollback inputs")
            prepared_binary = reserve_install(rollback_binary, self.layout.binary, 0o700)
            prepared_environment = reserve_install(rollback_environment, self.layout.environment, 0o600)
            try:
                commit_install(prepared_binary, self.layout.binary)
                commit_install(prepared_environment, self.layout.environment)
            finally:
                prepared_binary.unlink(missing_ok=True)
                prepared_environment.unlink(missing_ok=True)
            self.start_child(
                [str(self.layout.binary)],
                parse_environment(self.layout.environment),
                manifest.previous,
                manifest.previous_health_url,
                manifest.health_timeout_secs,
            )
            if manifest.previous_deployed_sha is None:
                self.layout.deployed_sha.unlink(missing_ok=True)
            else:
                write_text_atomic(self.layout.deployed_sha, manifest.previous_deployed_sha)
        self.transaction_status(manifest, manifest_hash, "activation_failed_rolled_back", failure)
        self.layout.active_file.unlink(missing_ok=True)
        return "activation_failed_rolled_back"

    def activate(self, transaction_id: str, manifest_hash: str) -> str:
        manifest, candidate_binary, candidate_environment, rollback_binary, rollback_environment = self.validated_transaction(
            transaction_id, manifest_hash
        )

        self.layout.deploy_dir.mkdir(parents=True, exist_ok=True)
        os.chmod(self.layout.deploy_dir, 0o700)
        with self.layout.activation_lock.open("a+") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as exc:
                raise SupervisorError("another bare deployment is activating") from exc
            if self.layout.status_file.exists():
                try:
                    prior = json.loads(self.layout.status_file.read_text())
                except json.JSONDecodeError as exc:
                    raise SupervisorError("durable deployment status is malformed") from exc
                if prior.get("transaction_id") == transaction_id:
                    raise SupervisorError("transaction ID has already been used")
            active = self.layout.active_file.read_text().strip() if self.layout.active_file.exists() else ""
            if active and active != transaction_id:
                raise SupervisorError(f"deployment transaction {active} is unresolved")
            write_text_atomic(self.layout.active_file, transaction_id)
            reserved = []
            try:
                candidate_binary_install = reserve_install(candidate_binary, self.layout.binary, 0o700)
                reserved.append(candidate_binary_install)
                candidate_environment_install = reserve_install(candidate_environment, self.layout.environment, 0o600)
                reserved.append(candidate_environment_install)
                rollback_binary_install = reserve_install(rollback_binary, self.layout.binary, 0o700) if rollback_binary else None
                if rollback_binary_install:
                    reserved.append(rollback_binary_install)
                rollback_environment_install = reserve_install(rollback_environment, self.layout.environment, 0o600) if rollback_environment else None
                if rollback_environment_install:
                    reserved.append(rollback_environment_install)
            except BaseException as exc:
                for path in reserved:
                    path.unlink(missing_ok=True)
                self.transaction_status(manifest, manifest_hash, "precondition_failed", str(exc))
                if self.layout.active_file.read_text().strip() == transaction_id:
                    self.layout.active_file.unlink()
                raise

            self.transaction_status(manifest, manifest_hash, "activating", phase="activating")
            try:
                self.stop_child()
                commit_install(candidate_binary_install, self.layout.binary)
                commit_install(candidate_environment_install, self.layout.environment)
                reserved = [path for path in reserved if path.exists()]
                self.transaction_status(manifest, manifest_hash, "activating", phase="candidate_installed")
                self.start_child(
                    [str(self.layout.binary)],
                    parse_environment(self.layout.environment),
                    manifest.expected,
                    manifest.expected_health_url,
                    manifest.health_timeout_secs,
                )
                self.transaction_status(manifest, manifest_hash, "activating", phase="candidate_started")
                write_text_atomic(self.layout.deployed_sha, manifest.source_commit)
                self.transaction_status(manifest, manifest_hash, "committed")
                self.layout.active_file.unlink(missing_ok=True)
                return "committed"
            except Exception as activation_error:
                try:
                    return self.restore_previous(
                        manifest,
                        manifest_hash,
                        str(activation_error),
                        rollback_binary,
                        rollback_environment,
                    )
                except Exception as rollback_error:
                    self.transaction_status(
                        manifest,
                        manifest_hash,
                        "activation_failed_rollback_failed",
                        str(activation_error),
                        str(rollback_error),
                    )
                    return "activation_failed_rollback_failed"
            finally:
                for path in reserved:
                    path.unlink(missing_ok=True)

    def restart_installed(
        self,
        identity: Optional[RuntimeIdentity],
        health_url: Optional[str],
        timeout: float,
    ) -> None:
        self.stop_recorded_orphan()
        if identity is None:
            self.stop_child()
            return
        if health_url is None:
            raise SupervisorError("durable runtime has no health endpoint")
        self.start_child(
            [str(self.layout.binary)],
            parse_environment(self.layout.environment),
            identity,
            health_url,
            timeout,
        )

    def terminal_runtime(
        self, manifest: TransactionManifest, state: str
    ) -> tuple[Optional[RuntimeIdentity], Optional[str]]:
        if state == "committed":
            return manifest.expected, manifest.expected_health_url
        if state in {"activation_failed_rolled_back", "precondition_failed"}:
            return manifest.previous, manifest.previous_health_url
        raise SupervisorError(f"durable state {state!r} has no verified runtime")

    def reconcile(self) -> None:
        try:
            status = json.loads(self.layout.status_file.read_text())
        except (OSError, json.JSONDecodeError):
            status = {}
        active = self.layout.active_file.exists()
        transaction_id = self.layout.active_file.read_text().strip() if active else status.get("transaction_id")
        if not isinstance(transaction_id, str):
            return
        try:
            manifest_path = self.layout.transactions / transaction_id / "manifest.json"
            manifest_hash = sha256(manifest_path)
            manifest, _candidate_binary, _candidate_environment, rollback_binary, rollback_environment = self.validated_transaction(
                transaction_id, manifest_hash
            )
            if not active:
                if status.get("manifest_sha256") != manifest_hash:
                    raise SupervisorError("durable transaction manifest identity changed")
                identity, health_url = self.terminal_runtime(manifest, str(status.get("state")))
                self.restart_installed(identity, health_url, manifest.health_timeout_secs)
                return
            if status.get("transaction_id") != transaction_id:
                state = "activating"
                phase = "prepared"
            else:
                if status.get("manifest_sha256") != manifest_hash:
                    raise SupervisorError("interrupted transaction manifest identity changed")
                state = status.get("state")
                phase = status.get("phase")
            if state in {"committed", "activation_failed_rolled_back", "precondition_failed"}:
                identity, health_url = self.terminal_runtime(manifest, str(state))
                self.restart_installed(identity, health_url, manifest.health_timeout_secs)
                self.layout.active_file.unlink(missing_ok=True)
                return
            if state != "activating":
                raise SupervisorError(f"active claim has unrecoverable durable state {state!r}")
            if phase in {"candidate_installed", "candidate_started"}:
                self.stop_recorded_orphan()
                try:
                    self.start_child(
                        [str(self.layout.binary)],
                        parse_environment(self.layout.environment),
                        manifest.expected,
                        manifest.expected_health_url,
                        manifest.health_timeout_secs,
                    )
                    self.transaction_status(manifest, manifest_hash, "activating", phase="candidate_started")
                    write_text_atomic(self.layout.deployed_sha, manifest.source_commit)
                    self.transaction_status(manifest, manifest_hash, "committed")
                    self.layout.active_file.unlink(missing_ok=True)
                    return
                except Exception as activation_error:
                    failure = f"restart reconciliation candidate verification failed: {activation_error}"
            elif phase in {"prepared", "activating", "rolling_back"}:
                failure = status.get("failure") or f"restart reconciliation resumed from {phase}"
            else:
                raise SupervisorError(f"active claim has unrecoverable durable phase {phase!r}")
            self.restore_previous(
                manifest,
                manifest_hash,
                str(failure),
                rollback_binary,
                rollback_environment,
            )
        except Exception as reconciliation_error:
            if not active:
                raise
            try:
                status = json.loads(self.layout.status_file.read_text())
                manifest_hash = status.get("manifest_sha256")
                if isinstance(manifest_hash, str):
                    manifest, *_ = self.validated_transaction(transaction_id, manifest_hash)
                    self.transaction_status(
                        manifest,
                        manifest_hash,
                        "activation_failed_rollback_failed",
                        str(status.get("failure") or "supervisor interrupted during activation"),
                        str(reconciliation_error),
                    )
            except Exception:
                pass

    def serve(self) -> None:
        self.prepare_layout()
        self.reconcile()
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            server.bind(str(self.layout.socket))
            os.chmod(self.layout.socket, 0o600)
            server.listen(8)
            server.settimeout(0.25)
            while self.running:
                if self.child is not None and self.child.poll() is not None:
                    self.child = None
                    self.child_identity = None
                    write_json_atomic(self.layout.state, self.status())
                try:
                    connection, _ = server.accept()
                except socket.timeout:
                    continue
                with connection:
                    try:
                        if peer_uid(connection) != self.owner_uid:
                            raise SupervisorError("peer UID does not own supervisor")
                        payload = connection.recv(MAX_REQUEST_BYTES + 1)
                        if len(payload) > MAX_REQUEST_BYTES:
                            raise SupervisorError("supervisor request is too large")
                        request = json.loads(payload)
                        if not isinstance(request, dict):
                            raise SupervisorError("supervisor request must be an object")
                        response = self.dispatch(request)
                    except Exception as exc:
                        response = {"ok": False, "error": str(exc)}
                    connection.sendall((json.dumps(response, sort_keys=True) + "\n").encode())
        finally:
            self.stop_child()
            server.close()
            self.layout.socket.unlink(missing_ok=True)


def request(socket_path: Path, payload: dict[str, object]) -> dict[str, object]:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(str(socket_path))
        client.sendall(json.dumps(payload).encode())
        client.shutdown(socket.SHUT_WR)
        response = json.loads(client.makefile().readline())
        if not response.get("ok"):
            raise SupervisorError(str(response.get("error", "supervisor request failed")))
        return response
    finally:
        client.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("--root", type=Path, default=Path.home() / ".phoenix-ide")
    parser.add_argument("action", nargs="?", choices=("run", "status", "stop", "shutdown-supervisor", "activate"))
    parser.add_argument("--transaction-id")
    parser.add_argument("--manifest-sha256")
    args = parser.parse_args()
    if args.protocol_version:
        print(PROTOCOL_VERSION)
        return 0
    if args.action == "run":
        Supervisor(Layout(args.root)).serve()
        return 0
    if args.action is None:
        parser.error("an action is required")
    payload = {"protocol_version": PROTOCOL_VERSION, "action": args.action}
    if args.action == "activate":
        if args.transaction_id is None or args.manifest_sha256 is None:
            parser.error("activate requires --transaction-id and --manifest-sha256")
        payload.update(transaction_id=args.transaction_id, manifest_sha256=args.manifest_sha256)
    response = request(Layout(args.root).socket, payload)
    print(json.dumps(response, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
