#!/usr/bin/env python3
"""Disposable launchd activation, ownership, and rollback harness."""
import hashlib
import json
import os
import plistlib
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "scripts/launchd_deploy_helper.py"
LIVE_LABEL = "com.phoenix-ide.server"
LIVE_PORT = 8031
LIVE_HOME = Path.home() / ".phoenix-ide"
TERMINAL = {"committed", "activation_failed_rolled_back", "activation_failed_rollback_failed"}


def refuse_live(label, root, port):
    root = root.resolve()
    forbidden = [LIVE_HOME.resolve(), (Path.home() / "Library/LaunchAgents").resolve()]
    if label == LIVE_LABEL or port == LIVE_PORT or any(root == path or path in root.parents for path in forbidden):
        raise SystemExit("refusing live launchd label, port, or production path")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def allocate_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


def read_identity(url):
    with urllib.request.urlopen(url, timeout=1) as response:
        return json.load(response)


def wait_identity(url, expected, deadline):
    last = None
    while time.monotonic() < deadline:
        try:
            last = read_identity(url)
            if last == expected:
                return
        except Exception as exc:
            last = type(exc).__name__
        time.sleep(0.1)
    raise RuntimeError(f"baseline identity not healthy: expected={expected}, observed={last}")


def wait_terminal(path, log, deadline):
    latest = None
    while time.monotonic() < deadline:
        if path.exists():
            try:
                latest = json.loads(path.read_text())
                if latest.get("state") in TERMINAL:
                    return latest
            except json.JSONDecodeError:
                pass
        time.sleep(0.1)
    log_text = log.read_text() if log.exists() else "<missing>"
    raise RuntimeError(f"transaction did not become terminal; latest={latest}; helper_log={log_text}")


def wait_unloaded(domain, label, deadline):
    target = f"{domain}/{label}"
    while time.monotonic() < deadline:
        result = subprocess.run(["launchctl", "print", target], capture_output=True)
        if result.returncode != 0:
            return
        time.sleep(0.1)
    raise RuntimeError(f"one-shot helper remained registered: {target}")


def server_script(identity, healthy=True):
    if not healthy:
        return "#!/usr/bin/python3\nraise SystemExit(23)\n"
    payload = json.dumps(identity)
    return f'''#!/usr/bin/python3
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/api/version":
            self.send_response(404); self.end_headers(); return
        body = {payload!r}.encode()
        self.send_response(200); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *args): pass
HTTPServer(("127.0.0.1", int(os.environ["TEST_PORT"])), Handler).serve_forever()
'''


def target_plist(label, binary, port, log):
    return plistlib.dumps({
        "Label": label, "ProgramArguments": [str(binary)],
        "EnvironmentVariables": {"TEST_PORT": str(port)},
        "RunAtLoad": True, "KeepAlive": True,
        "StandardOutPath": str(log), "StandardErrorPath": str(log),
    })


def run_scenario(root, domain, *, healthy_candidate, expected_state):
    suffix = uuid.uuid4().hex
    target_label = f"test.phoenix-ide.server.{suffix}"
    helper_label = f"test.phoenix-ide.deploy.{suffix}"
    port = allocate_port()
    refuse_live(target_label, root, port)
    old_identity = {"version": "1.0.0", "git_sha": "oldsha"}
    new_identity = {"version": "2.0.0", "git_sha": "newsha"}
    target_binary = root / f"phoenix-{suffix}"
    target_binary.write_text(server_script(old_identity)); target_binary.chmod(0o755)
    target_plist_path = root / f"target-{suffix}.plist"
    target_plist_path.write_bytes(target_plist(target_label, target_binary, port, root / f"target-{suffix}.log"))
    candidate_binary = root / f"candidate-{suffix}"
    candidate_binary.write_text(server_script(new_identity, healthy_candidate)); candidate_binary.chmod(0o755)
    candidate_plist = root / f"candidate-{suffix}.plist"
    candidate_plist.write_bytes(target_plist(target_label, target_binary, port, root / f"target-{suffix}.log"))
    rollback_binary = root / f"rollback-{suffix}"
    rollback_binary.write_bytes(target_binary.read_bytes()); rollback_binary.chmod(0o755)
    rollback_plist = root / f"rollback-{suffix}.plist"
    rollback_plist.write_bytes(target_plist_path.read_bytes())
    old_binary_hash, old_plist_hash = digest(target_binary), digest(target_plist_path)
    status, log = root / f"status-{suffix}.json", root / f"helper-{suffix}.log"
    active = root / f"active-{suffix}"
    active.write_text(suffix + "\n")
    manifest_path = root / f"manifest-{suffix}.json"
    url = f"http://127.0.0.1:{port}/api/version"
    manifest = {
        "transaction_id": suffix, "source_kind": "local_head", "source_commit": "newsha",
        "release_tag": None, "release_commit": None, "expected": new_identity, "previous": old_identity,
        "candidate_binary": str(candidate_binary), "candidate_binary_sha256": digest(candidate_binary),
        "candidate_plist": str(candidate_plist), "candidate_plist_sha256": digest(candidate_plist),
        "rollback_binary": str(rollback_binary), "rollback_binary_sha256": digest(rollback_binary),
        "rollback_plist": str(rollback_plist), "rollback_plist_sha256": digest(rollback_plist),
        "target_binary": str(target_binary), "target_plist": str(target_plist_path),
        "label": target_label, "helper_label": helper_label, "uid": os.getuid(), "health_url": url,
        "health_insecure_tls": False, "active_path": str(active), "status_path": str(status),
        "deployed_sha_path": str(root / f"deployed-{suffix}.sha"), "lock_path": str(root / f"lock-{suffix}"),
        "created_at": "2026-01-01T00:00:00+00:00", "transition_timeout_secs": 10,
        "health_timeout_secs": 2 if not healthy_candidate else 10,
    }
    manifest_path.write_text(json.dumps(manifest))
    helper_plist = root / f"helper-{suffix}.plist"
    helper_plist.write_bytes(plistlib.dumps({
        "Label": helper_label,
        "ProgramArguments": ["/usr/bin/python3", str(HELPER), "activate", "--manifest", str(manifest_path),
                             "--helper-label", helper_label, "--uid", str(os.getuid())],
        "RunAtLoad": True, "StandardOutPath": str(log), "StandardErrorPath": str(log),
    }))
    handoff = root / f"handoff-{suffix}"
    initiator = root / f"initiator-{suffix}.py"
    initiator.write_text(
        "import pathlib,subprocess,time\n"
        f"subprocess.run(['launchctl','bootstrap',{domain!r},{str(helper_plist)!r}],check=True)\n"
        f"pathlib.Path({str(handoff)!r}).write_text('handed-off')\n"
        "time.sleep(60)\n"
    )
    try:
        subprocess.run(["launchctl", "bootstrap", domain, str(target_plist_path)], check=True)
        wait_identity(url, old_identity, time.monotonic() + 10)
        process = subprocess.Popen([sys.executable, str(initiator)], start_new_session=True)
        deadline = time.monotonic() + 10
        while not handoff.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        if not handoff.exists():
            process.kill()
            raise RuntimeError("initiator did not report launchd handoff")
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)
        result = wait_terminal(status, log, time.monotonic() + 20)
        if result["state"] != expected_state:
            raise RuntimeError(f"unexpected terminal status: {result}")
        wait_unloaded(domain, helper_label, time.monotonic() + 5)
        if expected_state == "committed":
            wait_identity(url, new_identity, time.monotonic() + 3)
            if Path(manifest["deployed_sha_path"]).read_text().strip() != "newsha":
                raise RuntimeError("committed SHA does not match candidate")
        else:
            wait_identity(url, old_identity, time.monotonic() + 3)
            if digest(target_binary) != old_binary_hash or digest(target_plist_path) != old_plist_hash:
                raise RuntimeError("rollback did not restore exact binary and plist")
            if Path(manifest["deployed_sha_path"]).exists():
                raise RuntimeError("failed candidate advanced deployed.sha")
        print(f"PASS: {expected_state} after external SIGKILL of initiating process group")
    finally:
        subprocess.run(["launchctl", "bootout", f"{domain}/{helper_label}"], capture_output=True)
        subprocess.run(["launchctl", "bootout", f"{domain}/{target_label}"], capture_output=True)


def main():
    if sys.platform != "darwin":
        print("SKIP: launchd disposable harness requires macOS")
        return 0
    with tempfile.TemporaryDirectory(prefix="phoenix-launchd-harness-") as td:
        root = Path(td)
        domain = f"gui/{os.getuid()}"
        run_scenario(root, domain, healthy_candidate=True, expected_state="committed")
        run_scenario(root, domain, healthy_candidate=False, expected_state="activation_failed_rolled_back")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
