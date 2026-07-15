#!/usr/bin/env python3
"""Qualify external systemd activation ownership in a disposable Lima VM."""

import argparse
import os
import platform
import shutil
import signal
import subprocess
import tempfile
import time
import uuid
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = ROOT / "tests/integration/lima-systemd.yaml"
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
    return subprocess.run(command, check=check, text=True, capture_output=True, timeout=timeout)


def refuse_production_resources(instance, unit, guest_root, port=None):
    path = PurePosixPath(guest_root)
    if not instance.startswith(NAME_PREFIX) or not unit.startswith(NAME_PREFIX):
        raise ValueError("disposable instance and unit names must use the QA prefix")
    if unit in LIVE_UNITS or port == LIVE_PORT:
        raise ValueError("refusing production unit or port")
    if any(path == live or live in path.parents for live in LIVE_PATHS):
        raise ValueError("refusing production installation path")
    if path != PurePosixPath("/var/tmp") / instance:
        raise ValueError("guest root must exactly match the randomized disposable instance")


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
        return 0
    finally:
        if started:
            lima_shell(
                instance,
                f"sudo -n systemctl stop {unit}.service 2>/dev/null || true; "
                f"sudo -n systemctl reset-failed {unit}.service 2>/dev/null || true; "
                f"sudo -n rm -rf {guest_root}",
                check=False,
            )
        if not args.keep_vm:
            run(["limactl", "delete", "--force", instance], check=False, timeout=120)
        else:
            print(f"DEBUG: retained Lima VM {instance}")


if __name__ == "__main__":
    raise SystemExit(main())
