#!/usr/bin/env python3
"""Prepare and activate a randomized systemd deployment inside the Lima VM."""

import argparse
import hashlib
import importlib.util
import json
import os
import pwd
import shutil
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

OLD = {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"}
NEW = {"version": "2.0.0", "git_sha": "bbbbbbbbbbbb"}


def load_helper(path):
    spec = importlib.util.spec_from_file_location("qa_systemd_helper", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def run(*command, check=True):
    return subprocess.run(command, text=True, capture_output=True, check=check)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def artifact(path):
    return {"path": str(path), "sha256": sha(path)}


def absent():
    return {"path": None, "sha256": None}


def write(path, content, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    path.chmod(mode)


def policy(helper, root, unit):
    return helper.ValidationPolicy(
        transaction_root=root / "transactions",
        unit_name=unit,
        targets=helper.SystemdTargets(
            binary=str(root / "install/phoenix-runtime"),
            service=f"/etc/systemd/system/{unit}.service",
            socket=f"/etc/systemd/system/{unit}.socket",
            environment=str(root / "config/runtime.env"),
            deployed_sha=str(root / "deployed.sha"),
        ),
        status_path=root / "status.json",
        active_path=root / "active",
        activation_lock_path=root / "activation.lock",
        claim_lock_path=root / "claim.lock",
    )


def wrapper(fixture, identity, *, wrong=False, startup_delay=0):
    mismatch = " --report-git-sha cccccccccccc" if wrong else ""
    delay = f" --startup-delay {startup_delay}" if startup_delay else ""
    return (
        "#!/bin/sh\n"
        f"exec /usr/bin/python3 {fixture} --version {identity['version']} "
        f"--git-sha {identity['git_sha']}{mismatch}{delay} --socket-activation\n"
    )


def unit_content(unit, user, target, env_target):
    return f"""[Unit]
Requires={unit}.socket
After={unit}.socket

[Service]
Type=simple
User={user}
EnvironmentFile=-{env_target}
ExecStart={target}
Restart=no

[Install]
WantedBy=multi-user.target
"""


def socket_content(port):
    return f"""[Socket]
ListenStream=127.0.0.1:{port}
NoDelay=true

[Install]
WantedBy=sockets.target
"""


def identity(port):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/version", timeout=3) as response:
        return json.load(response)


def wait_identity(port, expected, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if identity(port) == expected:
                return
        except Exception:
            pass
        time.sleep(0.1)
    raise RuntimeError(f"identity {expected} did not become ready")


def setup(args, helper):
    root = Path(args.root)
    if not root.name.startswith("phoenix-qa-systemd-") or not args.unit.startswith("phoenix-qa-systemd-"):
        raise SystemExit("refusing non-disposable systemd scenario")
    root.mkdir(parents=True, exist_ok=True)
    root.chmod(0o755)
    (root / "transactions").mkdir(mode=0o700, exist_ok=True)
    transaction_id = "b" * 32 if args.mode == "success" else "c" * 32
    transaction = root / "transactions" / transaction_id
    transaction.mkdir(mode=0o700)
    service_user = pwd.getpwuid(int(args.service_uid)).pw_name
    fixture = root / "fixture_runtime.py"
    shutil.copy2(args.fixture, fixture)
    fixture.chmod(0o755)
    selected = policy(helper, root, args.unit)

    write(Path(selected.targets.binary), wrapper(fixture, OLD), 0o755)
    write(Path(selected.targets.service), unit_content(args.unit, service_user, selected.targets.binary, selected.targets.environment), 0o644)
    write(Path(selected.targets.socket), socket_content(args.port), 0o644)
    write(Path(selected.targets.environment), "FIXTURE_DEPLOYMENT=old\n", 0o640)
    write(Path(selected.targets.deployed_sha), "a" * 40 + "\n")
    run("systemctl", "daemon-reload")
    run("systemctl", "enable", f"{args.unit}.socket", f"{args.unit}.service")
    run("systemctl", "start", f"{args.unit}.socket", f"{args.unit}.service")
    wait_identity(args.port, OLD)
    old_pid = int(run("systemctl", "show", f"{args.unit}.service", "-p", "MainPID", "--value").stdout)

    candidate_binary = transaction / "candidate-runtime"
    candidate_service = transaction / "candidate.service"
    candidate_socket = transaction / "candidate.socket"
    candidate_env = transaction / "candidate.env"
    write(candidate_binary, wrapper(fixture, NEW, wrong=args.mode == "rollback", startup_delay=3), 0o700)
    write(candidate_service, unit_content(args.unit, service_user, selected.targets.binary, selected.targets.environment))
    write(candidate_socket, socket_content(args.port))
    write(candidate_env, "FIXTURE_DEPLOYMENT=new\n")

    rollback = {}
    for name, target in (
        ("binary", selected.targets.binary),
        ("service", selected.targets.service),
        ("socket", selected.targets.socket),
        ("environment", selected.targets.environment),
    ):
        rollback_path = transaction / f"rollback-{name}"
        shutil.copy2(target, rollback_path)
        rollback_path.chmod(0o600)
        rollback[name] = artifact(rollback_path)

    manifest = {
        "manifest_version": 1,
        "transaction_id": transaction_id,
        "unit_name": args.unit,
        "service_user": service_user,
        "source_kind": "local_head",
        "source_commit": "b" * 40,
        "release_tag": None,
        "release_commit": None,
        "expected": NEW,
        "previous": OLD,
        "expected_health_url": f"http://127.0.0.1:{args.port}/api/version",
        "previous_health_url": f"http://127.0.0.1:{args.port}/api/version",
        "candidate": {
            "binary": artifact(candidate_binary),
            "service": artifact(candidate_service),
            "socket": artifact(candidate_socket),
            "environment": artifact(candidate_env),
        },
        "rollback": rollback,
        "targets": vars(selected.targets),
        "status_path": str(selected.status_path),
        "active_path": str(selected.active_path),
        "activation_lock_path": str(selected.activation_lock_path),
        "claim_lock_path": str(selected.claim_lock_path),
        "previous_deployed_sha": "a" * 40,
        "created_at": "2026-07-15T00:00:00+00:00",
        "transition_timeout_secs": 15,
        "health_timeout_secs": 3,
    }
    manifest_path = transaction / "manifest.json"
    write(manifest_path, json.dumps(manifest, sort_keys=True, indent=2) + "\n")
    write(selected.active_path, transaction_id + "\n")
    print(json.dumps({"manifest": str(manifest_path), "old_pid": old_pid, "transaction_id": transaction_id}))


def activate(args, helper):
    manifest_path = Path(args.manifest)
    root = Path(args.root)
    manifest = helper.Manifest.load(manifest_path)
    selected = policy(helper, root, args.unit)
    helper.validate_manifest(manifest_path, manifest, selected)
    state = helper.activate(manifest)
    if helper.status_is_durable_terminal(manifest):
        helper.release_claim(manifest)
    print(state)
    return 0 if state in {"committed", "activation_failed_rolled_back"} else 1


def verify(args, _helper):
    root = Path(args.root)
    status = json.loads((root / "status.json").read_text())
    expected_state = "committed" if args.mode == "success" else "activation_failed_rolled_back"
    expected_identity = NEW if args.mode == "success" else OLD
    expected_sha = "b" * 40 if args.mode == "success" else "a" * 40
    if status["state"] != expected_state:
        raise RuntimeError(f"unexpected status: {status}")
    if identity(args.port) != expected_identity:
        raise RuntimeError("runtime identity does not match terminal outcome")
    if (root / "deployed.sha").read_text().strip() != expected_sha:
        raise RuntimeError("deployed SHA does not match terminal outcome")
    if (root / "active").exists():
        raise RuntimeError("terminal transaction claim was not released")
    new_pid = int(run("systemctl", "show", f"{args.unit}.service", "-p", "MainPID", "--value").stdout)
    if new_pid == 0 or new_pid == args.old_pid:
        raise RuntimeError("systemd did not install a new MainPID")
    print(json.dumps({"state": expected_state, "identity": expected_identity, "main_pid": new_pid}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("setup", "activate", "verify"))
    parser.add_argument("--helper", required=True)
    parser.add_argument("--fixture")
    parser.add_argument("--root", required=True)
    parser.add_argument("--unit", required=True)
    parser.add_argument("--port", required=True, type=int)
    parser.add_argument("--mode", choices=("success", "rollback"), required=True)
    parser.add_argument("--service-uid", default=str(os.getuid()))
    parser.add_argument("--manifest")
    parser.add_argument("--old-pid", type=int)
    args = parser.parse_args()
    helper = load_helper(args.helper)
    if args.action == "setup":
        setup(args, helper)
    elif args.action == "activate":
        return activate(args, helper)
    else:
        verify(args, helper)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
