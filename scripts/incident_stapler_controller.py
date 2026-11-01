#!/usr/bin/env python3
"""Exact ARM packaging incident controller; no candidate or target selection inputs."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

OPERATION = "prepare-incident-237100-arm64"
LINUX_OPERATION = "prepare-incident-237100-linux-x86_64"
CANDIDATE = "237100c79d9ce672867b284167a099fa0b7a2bcf"
VERSION = "0.13.0-rc.4"
REGISTRY_PATH = "crates/phoenix-db/src/migrations.rs"
REGISTRY_SHA256 = "a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6"
REGISTRY_TARGET_VERSION = 128
TARGET = "aarch64-apple-darwin"
LINUX_TARGET = "x86_64-unknown-linux-musl"
LINUX_ASSET = "phoenix_ide-x86_64-unknown-linux-musl"
LINUX_PREPARED_ASSET = f"{LINUX_ASSET}-prepared-{CANDIDATE[:12]}"
LINUX_RECEIPT = "LINUX-PREPARATION-RECEIPT-x86_64-unknown-linux-musl.json"
LINUX_CHECKS = {"clean_source": "verified", "static_elf": "verified", "embedded_identity": "verified"}
LINUX_REF = "refs/heads/main"
LINUX_EVENT = "workflow_dispatch"


class DuplicateJsonKey(ValueError):
    pass


def reject_duplicate_json_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise DuplicateJsonKey(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def artifact_directory_payload(artifact, destination=None):
    artifact = Path(artifact)
    if artifact.name != LINUX_PREPARED_ASSET or artifact.is_symlink() or not artifact.is_file() or artifact.stat().st_size == 0:
        raise ValueError("artifact refused")
    directory = artifact.parent
    if destination is not None:
        destination = Path(destination)
        if (destination.name != LINUX_RECEIPT or destination.parent.resolve() != directory.resolve()
                or destination.exists() or destination.is_symlink()):
            raise ValueError("receipt destination refused")
    entries = list(directory.iterdir())
    if len(entries) != 1 or entries[0] != artifact:
        raise ValueError("artifact directory refused")
    return artifact, directory


REAL_XCRUN = "/usr/bin/xcrun"
WRAPPER_NAME = "incident-xcrun"
RETRY_CODES = (68,)
DELAYS = (10, 30, 60)
COMMAND_TIMEOUT = 45
TOTAL_TIMEOUT = 300
RETRY_POLICY = "stapler-service-68-v1"


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], stderr=subprocess.DEVNULL)


def verify_candidate(repo):
    """Read immutable objects, never controller working-tree registry/version bytes."""
    if git(repo, "rev-parse", "--verify", f"{CANDIDATE}^{{commit}}").decode().strip() != CANDIDATE:
        raise ValueError("candidate identity refused")
    subprocess.run(
        ["git", "-C", str(repo), "merge-base", "--is-ancestor", CANDIDATE, "origin/main"],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    cargo = git(repo, "show", f"{CANDIDATE}:crates/phoenix-ide/Cargo.toml").decode()
    version = re.search(r'^version\s*=\s*"([^"\n]+)"\s*$', cargo, re.MULTILINE)
    if version is None or version.group(1) != VERSION:
        raise ValueError("candidate version refused")
    registry = git(repo, "show", f"{CANDIDATE}:{REGISTRY_PATH}")
    if hashlib.sha256(registry).hexdigest() != REGISTRY_SHA256:
        raise ValueError("candidate registry refused")
    verify_registry_bytes(registry)


def verify_registry_bytes(registry):
    bounded_registry = re.search(
        rb"(?s)\bconst\s+MIGRATIONS\s*:\s*&\[\s*Migration\s*\]\s*=\s*&\[\s*(.*?)\s*\n\];",
        registry,
    )
    if bounded_registry is None:
        raise ValueError("candidate migration registry refused")
    migrations = re.findall(rb"\bversion:\s*(\d+),", bounded_registry.group(1))
    if not migrations or int(migrations[-1]) != REGISTRY_TARGET_VERSION:
        raise ValueError("candidate migration target refused")


def verify_linux_artifact(artifact):
    artifact = Path(artifact)
    if (artifact.name != LINUX_PREPARED_ASSET or artifact.is_symlink()
            or not artifact.is_file() or artifact.stat().st_size == 0):
        raise ValueError("artifact refused")
    header = subprocess.check_output(["readelf", "-h", str(artifact)], text=True, timeout=10)
    if "ELF64" not in header or "Advanced Micro Devices X86-64" not in header:
        raise ValueError("artifact ELF identity refused")
    program_headers = subprocess.check_output(["readelf", "-l", str(artifact)], text=True, timeout=10)
    if re.search(r"(?m)^\s*INTERP\s", program_headers):
        raise ValueError("artifact is dynamically linked")
    identity = subprocess.check_output([str(artifact), "--build-identity"], text=True, timeout=10)
    try:
        identity = json.loads(identity, object_pairs_hook=reject_duplicate_json_keys)
    except (json.JSONDecodeError, DuplicateJsonKey) as error:
        raise ValueError("artifact build identity refused") from error
    if (not isinstance(identity, dict) or set(identity) != {"version", "git_sha"}
            or identity["version"] != VERSION or identity["git_sha"] != CANDIDATE):
        raise ValueError("artifact build identity refused")
    return artifact


def verify_clean_source(repo):
    status = subprocess.check_output(["git", "-C", str(repo), "status", "--porcelain"], text=True)
    if status:
        raise ValueError("source tree is dirty")


def _positive_json_integer(value):
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def linux_receipt(controller, run_id, run_attempt, artifact, *, event, ref, repo):
    if not re.fullmatch(r"[0-9a-f]{40}", controller) or controller == CANDIDATE:
        raise ValueError("controller identity refused")
    if not _positive_json_integer(run_id) or not _positive_json_integer(run_attempt):
        raise ValueError("workflow identity refused")
    if event != LINUX_EVENT or ref != LINUX_REF:
        raise ValueError("workflow source refused")
    verify_clean_source(repo)
    artifact = verify_linux_artifact(artifact)
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    return {
        "schema": 1, "operation": LINUX_OPERATION, "target": LINUX_TARGET, "version": VERSION,
        "commit": CANDIDATE, "controller_commit": controller, "registry_sha256": REGISTRY_SHA256,
        "run_id": run_id, "run_attempt": run_attempt, "event": LINUX_EVENT, "ref": LINUX_REF,
        "sha256": {LINUX_PREPARED_ASSET: digest}, "checks": dict(LINUX_CHECKS),
    }


def write_linux_receipt(destination, controller, run_id, run_attempt, artifact, *, event, ref, repo):
    artifact, _ = artifact_directory_payload(artifact, destination)
    receipt = linux_receipt(controller, run_id, run_attempt, artifact,
                            event=event, ref=ref, repo=repo)
    Path(destination).write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return receipt


def gate_outputs(repo):
    verify_candidate(repo)
    return {
        "preparation": "true", "release": "false", "create_tag": "false", "tag": "",
        "version": VERSION, "channel": "rc", "commit": CANDIDATE,
    }


def linux_gate_outputs(repo):
    verify_candidate(repo)
    return {
        "preparation": "true", "release": "false", "create_tag": "false", "tag": "",
        "version": VERSION, "channel": "rc", "commit": CANDIDATE,
        "operation": LINUX_OPERATION, "target": LINUX_TARGET,
    }


def provenance(controller, run_id, run_attempt):
    if not re.fullmatch(r"[0-9a-f]{40}", controller) or controller == CANDIDATE:
        raise ValueError("controller identity refused")
    if not re.fullmatch(r"[0-9]+", run_id) or not re.fullmatch(r"[1-9][0-9]*", run_attempt):
        raise ValueError("workflow identity refused")
    return {
        "schema": 1, "operation": OPERATION, "candidate_commit": CANDIDATE,
        "controller_commit": controller, "target": TARGET,
        "run_id": run_id, "run_attempt": run_attempt,
        "retry_policy": {
            "id": RETRY_POLICY, "exit_codes": list(RETRY_CODES),
            "max_attempts": len(DELAYS) + 1, "delays_seconds": list(DELAYS),
            "command_timeout_seconds": COMMAND_TIMEOUT, "total_timeout_seconds": TOTAL_TIMEOUT,
        },
    }


def write_wrapper(destination):
    destination = Path(destination)
    if destination.name != WRAPPER_NAME:
        raise ValueError("wrapper name refused")
    with destination.open("x", encoding="utf-8") as output:
        output.write(Path(__file__).read_text(encoding="utf-8"))
    destination.chmod(0o700)


def retry_eligible(args, runner_temp):
    if len(args) != 3 or args[:2] not in (["stapler", "staple"], ["stapler", "validate"]):
        return False
    app = Path(args[2])
    if not runner_temp or not app.is_absolute() or not app.is_dir():
        return False
    try:
        relative = app.resolve(strict=True).relative_to(Path(runner_temp).resolve(strict=True))
    except (OSError, ValueError):
        return False
    return re.fullmatch(
        r"phoenix-desktop-aarch64-apple-darwin\.[A-Za-z0-9]{6}/Build/Products/Release/Phoenix\.app",
        relative.as_posix(),
    ) is not None


class Interrupted(Exception):
    def __init__(self, signum):
        self.signum = signum


def stop_and_reap(child):
    mask = signal.pthread_sigmask(signal.SIG_BLOCK, (signal.SIGINT, signal.SIGTERM, signal.SIGHUP))
    try:
        # Kill the session even if its leader exited but left a descendant behind.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, mask)


def run_stapler(args):
    deadline = time.monotonic() + TOTAL_TIMEOUT
    previous = {}

    def interrupted(signum, _frame):
        raise Interrupted(signum)

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        previous[signum] = signal.signal(signum, interrupted)
    try:
        for attempt in range(len(DELAYS) + 1):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return 124
            child = None
            command_deadline = min(deadline, time.monotonic() + COMMAND_TIMEOUT)
            try:
                # Block cancellation during Popen so every launched session has an owner.
                mask = signal.pthread_sigmask(signal.SIG_BLOCK, previous)
                try:
                    child = subprocess.Popen(
                        [REAL_XCRUN, *args], start_new_session=True,
                        preexec_fn=lambda: signal.pthread_sigmask(signal.SIG_SETMASK, mask),
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    )
                finally:
                    signal.pthread_sigmask(signal.SIG_SETMASK, mask)
                code = child.wait(timeout=max(0, command_deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                print("Incident stapler command timed out; refusing packaging.", file=sys.stderr)
                return 124
            finally:
                if child is not None:
                    stop_and_reap(child)
            if code == 0:
                return 0
            if code < 0:
                raise Interrupted(-code)
            if code not in RETRY_CODES or attempt == len(DELAYS):
                print("Incident stapler failed; refusing packaging.", file=sys.stderr)
                return code
            delay = DELAYS[attempt]
            if time.monotonic() + delay >= deadline:
                return 124
            print("Incident stapler service retry within bounded policy.", file=sys.stderr)
            time.sleep(delay)
        raise AssertionError("unreachable")
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def wrapper_main(args):
    if not retry_eligible(args, os.environ.get("RUNNER_TEMP")):
        os.execv(REAL_XCRUN, [REAL_XCRUN, *args])
    try:
        return run_stapler(args)
    except Interrupted as error:
        if error.signum not in (signal.SIGKILL, signal.SIGSTOP):
            signal.signal(error.signum, signal.SIG_DFL)
        os.kill(os.getpid(), error.signum)
        return 128 + error.signum


def main():
    if Path(sys.argv[0]).name == WRAPPER_NAME:
        return wrapper_main(sys.argv[1:])
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("gate")
    commands.add_parser("linux-gate")
    receipt = commands.add_parser("linux-receipt")
    receipt.add_argument("destination")
    receipt.add_argument("artifact")
    receipt.add_argument("--repo", default=".")
    wrapper = commands.add_parser("write-wrapper")
    wrapper.add_argument("destination")
    commands.add_parser("provenance")
    args = parser.parse_args()
    if args.command == "gate":
        for key, value in gate_outputs(Path.cwd()).items():
            print(f"{key}={value}")
    elif args.command == "linux-gate":
        for key, value in linux_gate_outputs(Path.cwd()).items():
            print(f"{key}={value}")
    elif args.command == "linux-receipt":
        write_linux_receipt(
            args.destination, os.environ["GITHUB_SHA"], int(os.environ["GITHUB_RUN_ID"]),
            int(os.environ["GITHUB_RUN_ATTEMPT"]), args.artifact,
            event=os.environ["GITHUB_EVENT_NAME"], ref=os.environ["GITHUB_REF"], repo=args.repo,
        )
    elif args.command == "write-wrapper":
        write_wrapper(args.destination)
    else:
        print(json.dumps(provenance(
            os.environ["GITHUB_SHA"], os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"],
        ), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError):
        sys.exit("Incident controller refused operation; private details withheld.")
    except KeyError:
        sys.exit("Incident controller requires workflow identity.")
