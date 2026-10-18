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
CANDIDATE = "237100c79d9ce672867b284167a099fa0b7a2bcf"
VERSION = "0.13.0-rc.4"
REGISTRY_PATH = "crates/phoenix-db/src/migrations.rs"
REGISTRY_SHA256 = "a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6"
TARGET = "aarch64-apple-darwin"
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


def gate_outputs(repo):
    verify_candidate(repo)
    return {
        "preparation": "true", "release": "false", "create_tag": "false", "tag": "",
        "version": VERSION, "channel": "rc", "commit": CANDIDATE,
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
    wrapper = commands.add_parser("write-wrapper")
    wrapper.add_argument("destination")
    commands.add_parser("provenance")
    args = parser.parse_args()
    if args.command == "gate":
        for key, value in gate_outputs(Path.cwd()).items():
            print(f"{key}={value}")
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
