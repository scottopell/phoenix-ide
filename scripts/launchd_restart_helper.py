#!/usr/bin/env python3
"""Restart an installed Phoenix LaunchAgent without replacing its artifacts."""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import fcntl
import hashlib
import json
import os
import re
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import Callable, Optional


HANDOFF_PROTOCOL_VERSION = 1
TERMINAL_STATES = {
    "committed",
    "precondition_failed",
    "restart_failed",
    "rejected_concurrent",
}


@dataclasses.dataclass(frozen=True)
class Identity:
    version: str
    git_sha: str


@dataclasses.dataclass(frozen=True)
class Manifest:
    manifest_version: int
    transaction_id: str
    expected: Identity
    previous_pid: int
    binary_path: str
    binary_sha256: str
    plist_path: str
    plist_sha256: str
    deployed_sha_path: str
    deployed_sha256: str
    label: str
    helper_label: str
    uid: int
    health_url: str
    health_insecure_tls: bool
    active_path: str
    status_path: str
    lock_path: str
    claim_lock_path: str
    created_at: str
    transition_timeout_secs: float = 30.0
    health_timeout_secs: float = 120.0

    @classmethod
    def load(cls, path: Path) -> "Manifest":
        raw = json.loads(path.read_text())
        if raw.get("manifest_version") != HANDOFF_PROTOCOL_VERSION:
            raise RestartError(
                f"unsupported handoff protocol {raw.get('manifest_version')!r}; "
                f"expected {HANDOFF_PROTOCOL_VERSION}"
            )
        raw["expected"] = Identity(**raw["expected"])
        manifest = cls(**raw)
        if not manifest.transaction_id or not manifest.label or not manifest.helper_label:
            raise RestartError("restart manifest has incomplete job identity")
        if manifest.previous_pid <= 0:
            raise RestartError("restart manifest has an invalid previous PID")
        if not manifest.expected.version or not manifest.expected.git_sha:
            raise RestartError("restart manifest has an incomplete runtime identity")
        for value, description in (
            (manifest.binary_sha256, "binary"),
            (manifest.plist_sha256, "plist"),
            (manifest.deployed_sha256, "deployed SHA"),
        ):
            if re.fullmatch(r"[0-9a-f]{64}", value) is None:
                raise RestartError(f"restart manifest has an invalid {description} checksum")
        if manifest.uid != os.getuid():
            raise RestartError("restart manifest UID does not match the helper process")
        return manifest


class RestartError(RuntimeError):
    pass


class ConcurrentRestart(RestartError):
    pass


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def write_status(
    manifest: Manifest,
    state: str,
    *,
    failure: Optional[str] = None,
    previous_pid: Optional[int] = None,
    running_pid: Optional[int] = None,
) -> None:
    status = {
        "transaction_id": manifest.transaction_id,
        "state": state,
        "source_kind": "installed_restart",
        "expected_version": manifest.expected.version,
        "expected_git_sha": manifest.expected.git_sha,
        "previous_pid": manifest.previous_pid if previous_pid is None else previous_pid,
        "running_pid": running_pid,
        "created_at": manifest.created_at,
        "updated_at": utc_now(),
        "failure": failure,
    }
    atomic_write(
        Path(manifest.status_path),
        (json.dumps(status, sort_keys=True, indent=2) + "\n").encode(),
    )


def verify_file(path_value: str, expected_hash: str, description: str) -> Path:
    path = Path(path_value)
    if not path.is_file():
        raise RestartError(f"installed {description} is not a regular file")
    if sha256(path) != expected_hash:
        raise RestartError(f"installed {description} checksum mismatch")
    return path


def verify_installed_artifacts(manifest: Manifest) -> None:
    verify_file(manifest.binary_path, manifest.binary_sha256, "binary")
    verify_file(manifest.plist_path, manifest.plist_sha256, "plist")
    verify_file(
        manifest.deployed_sha_path,
        manifest.deployed_sha256,
        "deployed SHA",
    )


def verify_claim(manifest: Manifest) -> None:
    try:
        owner = Path(manifest.active_path).read_text().strip()
    except OSError as exc:
        raise RestartError("restart no longer owns the active claim") from exc
    if owner != manifest.transaction_id:
        raise RestartError("restart no longer owns the active claim")


class Launchctl:
    def __init__(
        self,
        manifest: Manifest,
        run: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
        monotonic: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
    ):
        self.manifest = manifest
        self.run = run
        self.monotonic = monotonic
        self.sleep = sleep
        self.target = f"gui/{manifest.uid}/{manifest.label}"

    def inspect(self) -> tuple[str, Optional[int]]:
        result = self.run(
            ["launchctl", "print", self.target],
            capture_output=True,
            text=True,
        )
        output = result.stdout + "\n" + result.stderr
        if "Could not find service" in output:
            return "not_loaded", None
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip()
            suffix = f": {detail}" if detail else ""
            raise RestartError(
                f"launchctl could not inspect the installed service "
                f"(exit {result.returncode}){suffix}"
            )
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

    def signal_hup(self) -> int:
        state, pid = self.inspect()
        if state not in {"running", "active"} or pid is None:
            raise RestartError(
                "installed service changed immediately before restart; "
                f"observed state={state} pid={pid}"
            )
        result = self.run(
            ["launchctl", "kill", "HUP", self.target],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip()
            suffix = f": {detail}" if detail else ""
            raise RestartError(f"launchctl could not signal the installed service{suffix}")
        return pid

    def wait_for_new_pid(self, previous_pid: int) -> int:
        deadline = self.monotonic() + self.manifest.transition_timeout_secs
        observed: tuple[str, Optional[int]] = ("unknown", None)
        while self.monotonic() < deadline:
            observed = self.inspect()
            if observed[0] in {"running", "active"} and observed[1] not in {None, previous_pid}:
                return observed[1]
            self.sleep(0.1)
        raise RestartError(
            "timed out waiting for launchd to replace PID "
            f"{previous_pid}; state={observed[0]} pid={observed[1]}"
        )


def fetch_identity(url: str, timeout: float, insecure_tls: bool) -> Identity:
    context = ssl._create_unverified_context() if insecure_tls else None
    with urllib.request.urlopen(url, timeout=timeout, context=context) as response:
        value = json.load(response)
    if value.get("socket_activated") is not True:
        raise RestartError("runtime does not report launchd socket activation")
    try:
        identity = Identity(version=str(value["version"]), git_sha=str(value["git_sha"]))
    except (KeyError, TypeError) as exc:
        raise RestartError("health response has no exact runtime identity") from exc
    if not identity.version or not identity.git_sha:
        raise RestartError("health response has no exact runtime identity")
    return identity


def wait_for_identity(manifest: Manifest, expected: Identity) -> None:
    deadline = time.monotonic() + manifest.health_timeout_secs
    last_error = "not attempted"
    while time.monotonic() < deadline:
        try:
            observed = fetch_identity(
                manifest.health_url,
                timeout=min(2.0, max(0.1, deadline - time.monotonic())),
                insecure_tls=manifest.health_insecure_tls,
            )
            if observed == expected:
                return
            last_error = f"identity {observed} did not match {expected}"
        except Exception as exc:
            last_error = str(exc)
        time.sleep(0.2)
    raise RestartError(f"timed out waiting for exact runtime identity: {last_error}")


def restart(manifest: Manifest) -> str:
    lock_path = Path(manifest.lock_path)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ConcurrentRestart("another production operation is activating") from exc

        launchctl = Launchctl(manifest)
        disrupted = False
        signal_pid = manifest.previous_pid
        try:
            verify_claim(manifest)
            verify_installed_artifacts(manifest)
            state, pid = launchctl.inspect()
            if state not in {"running", "active"} or pid != manifest.previous_pid:
                raise RestartError(
                    "installed service changed before restart; "
                    f"expected running PID {manifest.previous_pid}, observed state={state} pid={pid}"
                )
            observed = fetch_identity(
                manifest.health_url,
                timeout=2.0,
                insecure_tls=manifest.health_insecure_tls,
            )
            if observed != manifest.expected:
                raise RestartError(
                    f"installed runtime identity changed before restart: {observed}"
                )

            signal_pid = launchctl.signal_hup()
            disrupted = True
            write_status(manifest, "restarting", previous_pid=signal_pid)
            running_pid = launchctl.wait_for_new_pid(signal_pid)
            wait_for_identity(manifest, manifest.expected)
            state, verified_pid = launchctl.inspect()
            if state not in {"running", "active"} or verified_pid != running_pid:
                raise RestartError(
                    "runtime PID changed during identity verification; "
                    f"expected running PID {running_pid}, observed state={state} pid={verified_pid}"
                )
            verify_installed_artifacts(manifest)
            write_status(
                manifest,
                "committed",
                previous_pid=signal_pid,
                running_pid=running_pid,
            )
            return "committed"
        except Exception as exc:
            state = "restart_failed" if disrupted else "precondition_failed"
            write_status(
                manifest,
                state,
                failure=str(exc),
                previous_pid=signal_pid,
            )
            if disrupted:
                return state
            raise


def status_is_durable_terminal(manifest: Manifest) -> bool:
    try:
        status = json.loads(Path(manifest.status_path).read_text())
        return (
            status.get("transaction_id") == manifest.transaction_id
            and status.get("state") in TERMINAL_STATES
        )
    except (OSError, json.JSONDecodeError):
        return False


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
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("restart", nargs="?")
    parser.add_argument("--protocol-version", action="store_true")
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--helper-label")
    parser.add_argument("--uid", type=int)
    args = parser.parse_args()
    if args.protocol_version:
        print(HANDOFF_PROTOCOL_VERSION)
        return 0
    if args.manifest is None or args.helper_label is None or args.uid is None:
        parser.error("restart requires --manifest, --helper-label, and --uid")

    manifest = None
    try:
        manifest = Manifest.load(args.manifest)
        if manifest.helper_label != args.helper_label or manifest.uid != args.uid:
            raise RestartError("helper identity does not match the immutable manifest")
        state = restart(manifest)
        if state in TERMINAL_STATES and status_is_durable_terminal(manifest):
            release_claim(manifest)
        print(state, flush=True)
        return 0 if state == "committed" else 1
    except ConcurrentRestart as exc:
        if manifest is not None:
            try:
                write_status(manifest, "rejected_concurrent", failure=str(exc))
            finally:
                if status_is_durable_terminal(manifest):
                    release_claim(manifest)
        print(f"restart helper failed: {exc}", file=sys.stderr)
        return 1
    except Exception as exc:
        if manifest is not None and status_is_durable_terminal(manifest):
            release_claim(manifest)
        print(f"restart helper failed: {exc}", file=sys.stderr)
        return 1
    finally:
        request_helper_bootout(args.uid, args.helper_label)


if __name__ == "__main__":
    raise SystemExit(main())
