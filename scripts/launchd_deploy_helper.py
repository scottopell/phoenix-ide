#!/usr/bin/env python3
"""Independent launchd activation: prepare, stop, migrate, start, verify.

The helper does not inspect application data or recover a failed deployment.
Its immutable inputs and diagnostics remain available for manual recovery.
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
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Callable

HANDOFF_PROTOCOL_VERSION = 2
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

    def stop(self) -> int | None:
        state, old_pid = self.inspect()
        if state != "not_loaded":
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
        phase = "preparing"
        launchctl = Launchctl(manifest)
        try:
            environment = validate_pre_disruption_handoff(manifest)
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
            old_pid = launchctl.stop()
            commit_atomic_install(staged_binary, Path(manifest.target_binary))
            phase = "migration_started"
            write_status(manifest, phase)
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
            write_status(manifest, "committed")
        except Exception as exc:
            stop_failure = None
            if disrupted:
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
            state = "failed" if disrupted else "precondition_failed"
            failure = f"{phase}: {exc}"
            if stop_failure:
                failure += f"; stopping failed: {stop_failure}"
            write_status(manifest, state, failure=failure)
            if not disrupted:
                release_claim(manifest)
            print(f"{manifest.transaction_id}: {failure}", file=sys.stderr, flush=True)
            if disrupted:
                print("Production requires manual recovery. The candidate may have changed the database; "
                      "do not start an older binary without restoring its matching database. "
                      "Inspect activation.log and the retained transaction.", file=sys.stderr, flush=True)
            return state
        finally:
            for path in prepared:
                path.unlink(missing_ok=True)
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
