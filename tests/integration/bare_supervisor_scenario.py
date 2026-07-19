#!/usr/bin/env python3
"""Exercise the bare-Linux supervisor ownership boundary on Linux."""

import argparse
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import time
import sys
from pathlib import Path


def load(path):
    spec = importlib.util.spec_from_file_location("bare_supervisor_scenario", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def parent_pid(pid):
    value = (Path("/proc") / str(pid) / "stat").read_text()
    return int(value[value.rfind(")") + 2 :].split()[1])


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write(path, content, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    path.chmod(mode)


def wrapper(fixture, version, git_sha, port, *, wrong=False):
    mismatch = " --report-git-sha cccccccccccc" if wrong else ""
    return (
        "#!/bin/sh\n"
        f"exec /usr/bin/python3 {fixture} --version {version} --git-sha {git_sha}"
        f"{mismatch} --port {port}\n"
    )


def transaction(
    layout, fixture, port, transaction_id, *, previous,
    previous_version="1.0.0", previous_git_sha="aaaaaaaaaaaa", wrong=False,
):
    directory = layout.transactions / transaction_id
    directory.mkdir(parents=True, mode=0o700)
    candidate = directory / "candidate-binary"
    environment = directory / "candidate.env"
    write(candidate, wrapper(fixture, "2.0.0", "bbbbbbbbbbbb", port, wrong=wrong), 0o700)
    write(environment, "MODE=new\n")
    rollback_binary = None
    rollback_environment = None
    if previous:
        rollback_binary = directory / "rollback-binary"
        rollback_environment = directory / "rollback.env"
        shutil.copy2(layout.binary, rollback_binary)
        shutil.copy2(layout.environment, rollback_environment)
        rollback_binary.chmod(0o600)
        rollback_environment.chmod(0o600)
    manifest = {
        "manifest_version": 1,
        "transaction_id": transaction_id,
        "expected": {"version": "2.0.0", "git_sha": "bbbbbbbbbbbb"},
        "previous": {"version": previous_version, "git_sha": previous_git_sha} if previous else None,
        "expected_health_url": f"http://127.0.0.1:{port}/api/version",
        "previous_health_url": f"http://127.0.0.1:{port}/api/version" if previous else None,
        "candidate_binary": {"name": candidate.name, "sha256": digest(candidate)},
        "candidate_environment": {"name": environment.name, "sha256": digest(environment)},
        "rollback_binary": {"name": rollback_binary.name, "sha256": digest(rollback_binary)} if rollback_binary else None,
        "rollback_environment": {"name": rollback_environment.name, "sha256": digest(rollback_environment)} if rollback_environment else None,
        "source_commit": "b" * 40,
        "previous_deployed_sha": previous_git_sha[0] * 40 if previous else None,
        "created_at": "2026-07-15T00:00:00+00:00",
        "health_timeout_secs": 3,
    }
    path = directory / "manifest.json"
    write(path, json.dumps(manifest, sort_keys=True))
    for artifact in directory.iterdir():
        artifact.chmod(0o400)
    directory.chmod(0o500)
    return transaction_id, digest(path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--supervisor", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--port", required=True, type=int)
    args = parser.parse_args()
    if not args.root.name.startswith("phoenix-qa-bare-") or args.port == 8031:
        raise SystemExit("refusing non-disposable bare supervisor scenario")

    module = load(args.supervisor)
    args.root.mkdir(mode=0o700)
    layout = module.Layout(args.root)
    write(layout.binary, wrapper(args.fixture, "1.0.0", "aaaaaaaaaaaa", args.port), 0o700)
    write(layout.environment, "MODE=old\n")
    write(layout.deployed_sha, "a" * 40 + "\n")
    launcher = (
        "import subprocess,sys; "
        "subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, "
        "stderr=subprocess.DEVNULL, start_new_session=True, close_fds=True)"
    )
    subprocess.run([
        sys.executable, "-c", launcher, sys.executable, str(args.supervisor),
        "--root", str(args.root), "run",
    ], check=True)
    deadline = time.monotonic() + 10
    while not layout.socket.exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    if not layout.socket.exists():
        raise RuntimeError("detached supervisor did not survive launcher exit")
    try:
        initial = module.request(layout.socket, {"protocol_version": 1, "action": "status"})
        supervisor_pid = initial["supervisor_pid"]
        if parent_pid(supervisor_pid) == os.getpid():
            raise RuntimeError("supervisor remained attached to scenario initiator")

        success_id, success_hash = transaction(layout, args.fixture, args.port, "b" * 32, previous=True)
        success = module.request(layout.socket, {
            "protocol_version": 1, "action": "activate",
            "transaction_id": success_id, "manifest_sha256": success_hash,
        })
        if success["state"] != "committed":
            raise RuntimeError("bare success transaction did not commit")
        committed = module.request(layout.socket, {"protocol_version": 1, "action": "status"})["child"]
        if committed["runtime"]["git_sha"] != "bbbbbbbbbbbb":
            raise RuntimeError("bare success transaction has wrong child identity")
        if parent_pid(committed["pid"]) != supervisor_pid:
            raise RuntimeError("detached supervisor is not the direct parent of Phoenix")
        if module.proc_start_time(committed["pid"]) != committed["proc_start_time"]:
            raise RuntimeError("child proc start time is not stable")

        rollback_id, rollback_hash = transaction(
            layout, args.fixture, args.port, "c" * 32, previous=True,
            previous_version="2.0.0", previous_git_sha="bbbbbbbbbbbb", wrong=True,
        )
        rollback = module.request(layout.socket, {
            "protocol_version": 1, "action": "activate",
            "transaction_id": rollback_id, "manifest_sha256": rollback_hash,
        })
        if rollback["state"] != "activation_failed_rolled_back":
            raise RuntimeError("bare wrong-identity transaction did not roll back")
        restored = module.request(layout.socket, {"protocol_version": 1, "action": "status"})["child"]
        if restored["runtime"] != {"version": "2.0.0", "git_sha": "bbbbbbbbbbbb"}:
            raise RuntimeError("bare rollback did not restore exact committed identity")
        if parent_pid(restored["pid"]) != supervisor_pid:
            raise RuntimeError("rollback runtime is not directly owned by detached supervisor")

        module.request(layout.socket, {"protocol_version": 1, "action": "stop"})
        stopped = module.request(layout.socket, {"protocol_version": 1, "action": "status"})
        if stopped["child"] is not None or stopped["supervisor_pid"] != supervisor_pid:
            raise RuntimeError("child-only stop did not preserve detached supervisor")
        print(json.dumps({
            "detached_after_launcher_exit": True,
            "direct_parent": True,
            "start_time": committed["proc_start_time"],
            "child_only_stop": True,
            "committed": True,
            "rolled_back": True,
        }))
    finally:
        if layout.socket.exists():
            module.request(layout.socket, {"protocol_version": 1, "action": "shutdown-supervisor"})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
