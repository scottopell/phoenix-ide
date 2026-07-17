#!/usr/bin/env python3
"""Qualify external systemd activation ownership in a disposable Lima VM."""

import argparse
import os
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = ROOT / "tests/integration/lima-systemd.yaml"
HELPER = ROOT / "scripts/systemd_deploy_helper.py"
FIXTURE = ROOT / "tests/integration/fixture_runtime.py"
SCENARIO = ROOT / "tests/integration/systemd_transaction_scenario.py"
BARE_SUPERVISOR = ROOT / "scripts/bare_supervisor.py"
BARE_SCENARIO = ROOT / "tests/integration/bare_supervisor_scenario.py"
NAME_PREFIX = "phoenix-qa-systemd-"
LIVE_UNITS = {"phoenix-ide.service", "phoenix-ide.socket"}
LIVE_PORT = 8031
LIVE_PATHS = {
    PurePosixPath("/opt/phoenix-ide"),
    PurePosixPath("/etc/phoenix-ide"),
    PurePosixPath("/var/lib/phoenix-ide"),
}
GUEST_INITIATOR = r'''#!/usr/bin/env python3
import pathlib
import subprocess
import sys
import time

unit, root, token = sys.argv[1:]
subprocess.run([
    "sudo", "-n", "systemd-run", "--quiet", f"--unit={unit}",
    "--property=Type=oneshot", "--property=RemainAfterExit=yes",
    "/bin/sh", "-c",
    f"sleep 4; printf '%s\\n' {token!r} > {root!r}/completed; sync",
], check=True)
pathlib.Path(root, "handed-off").write_text("ready\\n")
time.sleep(300)
'''


def run(command, *, check=True, timeout=None):
    print("+", " ".join(map(str, command)), flush=True)
    result = subprocess.run(command, check=False, text=True, capture_output=True, timeout=timeout)
    if check and result.returncode != 0:
        if result.stdout:
            print(result.stdout, file=sys.stderr, end="")
        if result.stderr:
            print(result.stderr, file=sys.stderr, end="")
        result.check_returncode()
    return result


def refuse_production_resources(instance, unit, guest_root, port=None):
    path = PurePosixPath(guest_root)
    if not instance.startswith(NAME_PREFIX) or not unit.startswith(NAME_PREFIX):
        raise ValueError("disposable instance and unit names must use the QA prefix")
    if unit in LIVE_UNITS or port == LIVE_PORT:
        raise ValueError("refusing production unit or port")
    if any(path == live or live in path.parents for live in LIVE_PATHS):
        raise ValueError("refusing production installation path")
    expected = PurePosixPath("/var/tmp") / instance
    if path not in {
        expected,
        PurePosixPath(f"{expected}-success"),
        PurePosixPath(f"{expected}-rollback"),
    }:
        raise ValueError("guest root must be bound to the randomized disposable instance")


def lima_shell(instance, command, *, check=True, timeout=30):
    return run(
        ["limactl", "shell", "--shell=/bin/bash", instance, "--", "bash", "-lc", command],
        check=check,
        timeout=timeout,
    )


def wait_until(description, probe, timeout):
    deadline = time.monotonic() + timeout
    latest = None
    while time.monotonic() < deadline:
        try:
            latest = probe()
            if latest:
                return latest
        except (OSError, subprocess.SubprocessError):
            pass
        time.sleep(0.25)
    raise RuntimeError(f"timed out waiting for {description}; latest={latest!r}")


def qualify(instance, unit, guest_root, token, initiator_path):
    pid1 = lima_shell(instance, "ps -p 1 -o comm=").stdout.strip()
    if pid1 != "systemd":
        raise RuntimeError(f"systemd is not PID 1: {pid1!r}")

    state = lima_shell(instance, "systemctl is-system-running", check=False).stdout.strip()
    if state not in {"running", "degraded"}:
        raise RuntimeError(f"systemd did not reach an acceptable state: {state!r}")

    fs_type = lima_shell(instance, "stat -fc %T /sys/fs/cgroup").stdout.strip()
    if fs_type != "cgroup2fs":
        raise RuntimeError(f"cgroup v2 is unavailable: {fs_type!r}")
    lima_shell(instance, "sudo -n true")
    lima_shell(instance, f"sudo -n install -d -m 0755 -o $(id -u) -g $(id -g) {guest_root}")

    command = [
        "limactl", "shell", "--shell=/bin/bash", instance, "--",
        "python3", initiator_path, unit, guest_root, token,
    ]
    print("+", " ".join(command), flush=True)
    initiator = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        wait_until(
            "durable systemd handoff",
            lambda: lima_shell(instance, f"test -f {guest_root}/handed-off", check=False).returncode == 0,
            20,
        )
        active = lima_shell(instance, f"sudo -n systemctl show {unit}.service --property=ActiveState --value").stdout.strip()
        if active not in {"activating", "active"}:
            raise RuntimeError(f"transient unit was not accepted by systemd: {active!r}")

        os.killpg(initiator.pid, signal.SIGKILL)
        initiator.wait(timeout=10)

        wait_until(
            "transient helper completion after initiator death",
            lambda: lima_shell(instance, f"sudo -n cat {guest_root}/completed", check=False).stdout.strip() == token,
            30,
        )
        final_active = lima_shell(
            instance,
            f"sudo -n systemctl show {unit}.service --property=ActiveState --value",
        ).stdout.strip()
        final_result = lima_shell(
            instance,
            f"sudo -n systemctl show {unit}.service --property=Result --value",
        ).stdout.strip()
        if (final_active, final_result) != ("active", "success"):
            raise RuntimeError(
                f"unexpected durable transient unit result: active={final_active!r}, result={final_result!r}"
            )
        print("PASS: systemd transient helper survived initiator process-group death")
    finally:
        if initiator.poll() is None:
            os.killpg(initiator.pid, signal.SIGKILL)
            initiator.wait(timeout=10)


def copy_bundle(instance, guest_root):
    bundle = {
        HELPER: f"/tmp/{instance}-systemd-helper.py",
        FIXTURE: f"/tmp/{instance}-fixture.py",
        SCENARIO: f"/tmp/{instance}-scenario.py",
    }
    for source, destination in bundle.items():
        run(["limactl", "copy", "--backend=scp", str(source), f"{instance}:{destination}"])
    return tuple(bundle.values())


def bare_supervisor_journey(instance, fixture):
    bare_root = f"/var/tmp/phoenix-qa-bare-{instance.removeprefix(NAME_PREFIX)}"
    guest_supervisor = f"/tmp/{instance}-bare-supervisor.py"
    guest_scenario = f"/tmp/{instance}-bare-scenario.py"
    run(["limactl", "copy", "--backend=scp", str(BARE_SUPERVISOR), f"{instance}:{guest_supervisor}"])
    run(["limactl", "copy", "--backend=scp", str(BARE_SCENARIO), f"{instance}:{guest_scenario}"])
    try:
        result = lima_shell(
            instance,
            f"python3 {guest_scenario} --supervisor {guest_supervisor} --fixture {fixture} "
            f"--root {bare_root} --port 49154",
            timeout=30,
        )
        print(f"PASS: bare supervisor ownership {result.stdout.strip()}")
    finally:
        lima_shell(instance, f"rm -rf {bare_root}", check=False)


def transaction_journey(instance, guest_root, unit, port, mode, helper, fixture, scenario):
    refuse_production_resources(instance, unit, guest_root, port)
    uid = lima_shell(instance, "id -u").stdout.strip()
    setup = lima_shell(
        instance,
        f"sudo -n python3 {scenario} setup --helper {helper} --fixture {fixture} "
        f"--root {guest_root} --unit {unit} --port {port} --mode {mode} --service-uid {uid}",
        timeout=60,
    )
    details = __import__("json").loads(setup.stdout.strip())
    manifest = details["manifest"]
    old_pid = details["old_pid"]
    helper_unit = f"{unit}-activation"
    command = [
        "limactl", "shell", "--shell=/bin/bash", instance, "--", "bash", "-lc",
        f"sudo -n systemd-run --no-block --unit={helper_unit} --property=Type=oneshot -- "
        f"python3 {scenario} activate --helper {helper} --root {guest_root} "
        f"--unit {unit} --port {port} --mode {mode} --manifest {manifest}; "
        f"printf handed-off > /tmp/{helper_unit}-handed-off; sleep 300",
    ]
    print("+", " ".join(command), flush=True)
    initiator = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        wait_until(
            f"{mode} transaction handoff",
            lambda: lima_shell(instance, f"test -f /tmp/{helper_unit}-handed-off", check=False).returncode == 0,
            20,
        )
        os.killpg(initiator.pid, signal.SIGKILL)
        initiator.wait(timeout=10)
    finally:
        if initiator.poll() is None:
            os.killpg(initiator.pid, signal.SIGKILL)
            initiator.wait(timeout=10)
    wait_until(
        f"{mode} transaction terminal status",
        lambda: lima_shell(
            instance,
            f"sudo -n python3 -c 'import json; print(json.load(open(\"{guest_root}/status.json\"))[\"state\"])'",
            check=False,
        ).stdout.strip() in {"committed", "activation_failed_rolled_back", "activation_failed_rollback_failed"},
        45,
    )
    helper_result = lima_shell(
        instance,
        f"sudo -n systemctl show {helper_unit}.service --property=Result --value",
    ).stdout.strip()
    if helper_result != "success":
        journal = lima_shell(
            instance,
            f"sudo -n journalctl -u {helper_unit}.service -n 80 --no-pager",
            check=False,
        ).stdout
        raise RuntimeError(f"transient activation helper did not survive handoff: {helper_result!r}\n{journal}")
    verified = lima_shell(
        instance,
        f"sudo -n python3 {scenario} verify --helper {helper} --root {guest_root} "
        f"--unit {unit} --port {port} --mode {mode} --old-pid {old_pid}",
    )
    print(f"PASS: systemd {mode} journey {verified.stdout.strip()}")


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keep-vm", action="store_true", help="leave the randomized VM for debugging")
    parser.add_argument("--start-timeout", default="10m", help="Lima start timeout (default: 10m)")
    return parser.parse_args()


def main():
    args = parse_args()
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        print("SKIP: authoritative systemd qualification requires macOS arm64 with Lima/VZ")
        return 0
    if shutil.which("limactl") is None:
        print("SKIP: limactl is not installed")
        return 0

    suffix = uuid.uuid4().hex[:12]
    instance = f"{NAME_PREFIX}{suffix}"
    unit = f"{NAME_PREFIX}helper-{suffix}"
    guest_root = f"/var/tmp/{instance}"
    token = uuid.uuid4().hex
    refuse_production_resources(instance, unit, guest_root)

    started = False
    try:
        run(["limactl", "validate", str(TEMPLATE)])
        run([
            "limactl", "start", "--yes", f"--name={instance}",
            f"--timeout={args.start_timeout}", str(TEMPLATE),
        ], timeout=660)
        started = True
        with tempfile.TemporaryDirectory(prefix=f"{instance}-") as td:
            initiator = Path(td) / "systemd_initiator.py"
            initiator.write_text(GUEST_INITIATOR)
            initiator.chmod(0o755)
            guest_initiator = f"/tmp/{instance}-initiator.py"
            run(["limactl", "copy", "--backend=scp", str(initiator), f"{instance}:{guest_initiator}"])
            qualify(instance, unit, guest_root, token, guest_initiator)
            helper, fixture, scenario = copy_bundle(instance, guest_root)
            bare_supervisor_journey(instance, fixture)
            success_root = f"{guest_root}-success"
            transaction_journey(instance, success_root, f"{NAME_PREFIX}service-{suffix}", 49152, "success", helper, fixture, scenario)
            rollback_root = f"{guest_root}-rollback"
            transaction_journey(instance, rollback_root, f"{NAME_PREFIX}rollback-{suffix}", 49153, "rollback", helper, fixture, scenario)
        return 0
    finally:
        if started:
            lima_shell(
                instance,
                f"sudo -n systemctl stop {unit}.service 2>/dev/null || true; "
                f"sudo -n systemctl reset-failed {unit}.service 2>/dev/null || true; "
                f"for u in $(systemctl list-unit-files --no-legend 'phoenix-qa-systemd-*' | awk '{{print $1}}'); do sudo -n systemctl disable --now $u 2>/dev/null || true; done; "
                f"sudo -n rm -f /etc/systemd/system/phoenix-qa-systemd-*; "
                f"sudo -n systemctl daemon-reload; sudo -n rm -rf {guest_root} {guest_root}-success {guest_root}-rollback",
                check=False,
            )
        if not args.keep_vm:
            run(["limactl", "delete", "--force", instance], check=False, timeout=120)
        else:
            print(f"DEBUG: retained Lima VM {instance}")


if __name__ == "__main__":
    raise SystemExit(main())
