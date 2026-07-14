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
import shutil
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import Callable, Optional

TERMINAL_STATES = {"committed", "activation_failed_rolled_back", "activation_failed_rollback_failed", "rejected_concurrent"}


@dataclasses.dataclass(frozen=True)
class Identity:
    version: str
    git_sha: str


@dataclasses.dataclass(frozen=True)
class Manifest:
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
    health_timeout_secs: float = 30.0

    @classmethod
    def load(cls, path: Path) -> "Manifest":
        raw = json.loads(path.read_text())
        raw["expected"] = Identity(**raw["expected"])
        raw["previous"] = Identity(**raw["previous"]) if raw.get("previous") else None
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
    if not candidate.is_file():
        raise ActivationError(f"{description} is not a regular file")
    actual = sha256(candidate)
    if actual != expected_hash:
        raise ActivationError(f"{description} checksum mismatch")
    return candidate


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
        if result.returncode != 0 or "Could not find service" in output:
            return "not_loaded", None
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


def fetch_identity(
    url: str,
    timeout: float = 2.0,
    insecure_tls: bool = False,
    expected_git_sha: Optional[str] = None,
) -> Identity:
    context = ssl._create_unverified_context() if insecure_tls else None
    with urllib.request.urlopen(url, timeout=timeout, context=context) as response:
        if expected_git_sha is not None:
            return Identity(version=response.read().decode().strip(), git_sha=expected_git_sha)
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
) -> None:
    deadline = time.monotonic() + manifest.health_timeout_secs
    url = health_url or manifest.health_url
    insecure_tls = manifest.health_insecure_tls if health_insecure_tls is None else health_insecure_tls
    last = "not responding"
    while time.monotonic() < deadline:
        try:
            actual = fetch_identity(
                url,
                insecure_tls=insecure_tls,
                expected_git_sha=None if health_json else expected.git_sha,
            )
            last = f"version={actual.version} git_sha={actual.git_sha}"
            if actual == expected:
                return
        except Exception as exc:
            last = type(exc).__name__
        time.sleep(0.2)
    raise ActivationError(
        f"exact health verification failed: expected version={expected.version} git_sha={expected.git_sha}; observed {last}"
    )


def restore_deployed_sha(manifest: Manifest) -> None:
    deployed_sha = Path(manifest.deployed_sha_path)
    if manifest.previous_deployed_sha is None:
        deployed_sha.unlink(missing_ok=True)
        if deployed_sha.parent.exists():
            fsync_dir(deployed_sha.parent)
    else:
        atomic_write(deployed_sha, (manifest.previous_deployed_sha + "\n").encode(), 0o600)


def restore(manifest: Manifest, launchctl: Launchctl) -> None:
    try:
        launchctl.stop()
    except ActivationError:
        pass
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
    rollback_binary = verify_staged(manifest.rollback_binary, manifest.rollback_binary_sha256, "rollback binary")
    rollback_plist = verify_staged(manifest.rollback_plist, manifest.rollback_plist_sha256, "rollback plist")
    atomic_install(rollback_binary, Path(manifest.target_binary), 0o755)
    atomic_install(rollback_plist, Path(manifest.target_plist), 0o600)
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


def activate(manifest: Manifest) -> str:
    lock_path = Path(manifest.lock_path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentDeploy("another deployment is already activating") from exc

        try:
            candidate_binary = verify_staged(manifest.candidate_binary, manifest.candidate_binary_sha256, "candidate binary")
            candidate_plist = verify_staged(manifest.candidate_plist, manifest.candidate_plist_sha256, "candidate plist")
            with candidate_plist.open("rb") as stream:
                plistlib.load(stream)
            if manifest.previous is not None:
                verify_staged(manifest.rollback_binary, manifest.rollback_binary_sha256, "rollback binary")
                rollback_plist = verify_staged(manifest.rollback_plist, manifest.rollback_plist_sha256, "rollback plist")
                with rollback_plist.open("rb") as stream:
                    plistlib.load(stream)
            elif any((
                manifest.rollback_binary,
                manifest.rollback_binary_sha256,
                manifest.rollback_plist,
                manifest.rollback_plist_sha256,
            )):
                raise ActivationError("first-install rollback inputs are inconsistent")
        except Exception as exc:
            write_status(manifest, "precondition_failed", failure=str(exc))
            raise

        launchctl = Launchctl(manifest)
        write_status(manifest, "activating")
        disrupted = False
        try:
            old_pid = launchctl.stop()
            disrupted = True
            atomic_install(candidate_binary, Path(manifest.target_binary), 0o755)
            atomic_install(candidate_plist, Path(manifest.target_plist), 0o600)
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
                restore(manifest, launchctl)
                write_status(manifest, "activation_failed_rolled_back", failure=failure)
                return "activation_failed_rolled_back"
            except Exception as rollback_exc:
                write_status(manifest, "activation_failed_rollback_failed", failure=failure, rollback_failure=str(rollback_exc))
                return "activation_failed_rollback_failed"


def status_is_durable_terminal(manifest: Manifest) -> bool:
    try:
        status = json.loads(Path(manifest.status_path).read_text())
        return (
            status.get("transaction_id") == manifest.transaction_id
            and status.get("state") in TERMINAL_STATES
        )
    except (OSError, json.JSONDecodeError):
        return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("activate", nargs="?")
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--helper-label", required=True)
    parser.add_argument("--uid", type=int, required=True)
    args = parser.parse_args()
    manifest = None
    try:
        manifest = Manifest.load(args.manifest)
        if manifest.helper_label != args.helper_label or manifest.uid != args.uid:
            raise ActivationError("helper identity does not match the immutable manifest")
        state = activate(manifest)
        if state in TERMINAL_STATES and status_is_durable_terminal(manifest):
            release_claim(manifest)
        print(state, flush=True)
        return 0 if state == "committed" else 1
    except ConcurrentDeploy as exc:
        if manifest is not None:
            try:
                write_status(manifest, "rejected_concurrent", failure=str(exc))
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
