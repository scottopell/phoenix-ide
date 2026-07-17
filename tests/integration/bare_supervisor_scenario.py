#!/usr/bin/env python3
"""Exercise the bare-Linux supervisor ownership boundary on Linux."""

import argparse
import hashlib
import importlib.util
import json
import os
import shutil
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


def transaction(layout, fixture, port, transaction_id, *, previous, wrong=False):
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
        "previous": {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"} if previous else None,
        "expected_health_url": f"http://127.0.0.1:{port}/api/version",
        "previous_health_url": f"http://127.0.0.1:{port}/api/version" if previous else None,
        "candidate_binary": {"name": candidate.name, "sha256": digest(candidate)},
        "candidate_environment": {"name": environment.name, "sha256": digest(environment)},
        "rollback_binary": {"name": rollback_binary.name, "sha256": digest(rollback_binary)} if rollback_binary else None,
        "rollback_environment": {"name": rollback_environment.name, "sha256": digest(rollback_environment)} if rollback_environment else None,
        "source_commit": "b" * 40,
        "previous_deployed_sha": "a" * 40 if previous else None,
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
    owner = module.Supervisor(module.Layout(args.root))
    child = owner.start_child(
        [
            "/usr/bin/python3", str(args.fixture),
            "--version", "1.0.0", "--git-sha", "aaaaaaaaaaaa",
            "--port", str(args.port),
        ],
        os.environ.copy(),
        module.RuntimeIdentity("1.0.0", "aaaaaaaaaaaa"),
        f"http://127.0.0.1:{args.port}/api/version",
        10,
    )
    try:
        if parent_pid(child.pid) != os.getpid():
            raise RuntimeError("supervisor is not the direct parent of Phoenix")
        if module.proc_start_time(child.pid) != child.proc_start_time:
            raise RuntimeError("child proc start time is not stable")
        if owner.status()["child"]["runtime"] != {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"}:
            raise RuntimeError("supervisor status lost exact runtime identity")
        owner.stop_child()
        if owner.status()["child"] is not None:
            raise RuntimeError("child stop did not clear managed identity")

        layout = owner.layout
        write(layout.binary, wrapper(args.fixture, "1.0.0", "aaaaaaaaaaaa", args.port), 0o700)
        write(layout.environment, "MODE=old\n")
        write(layout.deployed_sha, "a" * 40 + "\n")
        old = owner.start_child(
            [str(layout.binary)],
            os.environ.copy(),
            module.RuntimeIdentity("1.0.0", "aaaaaaaaaaaa"),
            f"http://127.0.0.1:{args.port}/api/version",
            10,
        )
        success_id, success_hash = transaction(layout, args.fixture, args.port, "b" * 32, previous=True)
        if owner.activate(success_id, success_hash) != "committed":
            raise RuntimeError("bare success transaction did not commit")
        committed = owner.child_identity
        if committed is None or committed.pid == old.pid or committed.runtime.git_sha != "bbbbbbbbbbbb":
            raise RuntimeError("bare success transaction has wrong child identity")

        owner.stop_child()
        write(layout.binary, wrapper(args.fixture, "1.0.0", "aaaaaaaaaaaa", args.port), 0o700)
        write(layout.environment, "MODE=old\n")
        write(layout.deployed_sha, "a" * 40 + "\n")
        owner.start_child(
            [str(layout.binary)], os.environ.copy(),
            module.RuntimeIdentity("1.0.0", "aaaaaaaaaaaa"),
            f"http://127.0.0.1:{args.port}/api/version", 10,
        )
        rollback_id, rollback_hash = transaction(layout, args.fixture, args.port, "c" * 32, previous=True, wrong=True)
        if owner.activate(rollback_id, rollback_hash) != "activation_failed_rolled_back":
            raise RuntimeError("bare wrong-identity transaction did not roll back")
        if owner.child_identity is None or owner.child_identity.runtime.git_sha != "aaaaaaaaaaaa":
            raise RuntimeError("bare rollback did not restore exact old identity")
        print(json.dumps({
            "direct_parent": True,
            "start_time": child.proc_start_time,
            "child_only_stop": True,
            "committed": True,
            "rolled_back": True,
        }))
    finally:
        owner.stop_child()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
