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
RESTART_HELPER = ROOT / "scripts/launchd_restart_helper.py"
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


def socket_activated_server_script(identity):
    payload = json.dumps(identity)
    return f'''#!/usr/bin/python3
import ctypes
import json
import os
import socket
from http.server import BaseHTTPRequestHandler, HTTPServer

libc = ctypes.CDLL(None)
libc.launch_activate_socket.argtypes = [
    ctypes.c_char_p,
    ctypes.POINTER(ctypes.POINTER(ctypes.c_int)),
    ctypes.POINTER(ctypes.c_size_t),
]
libc.launch_activate_socket.restype = ctypes.c_int
libc.free.argtypes = [ctypes.c_void_p]
fds = ctypes.POINTER(ctypes.c_int)()
count = ctypes.c_size_t()
rc = libc.launch_activate_socket(b"Listeners", ctypes.byref(fds), ctypes.byref(count))
if rc != 0 or count.value != 1:
    raise SystemExit(f"launch_activate_socket failed: rc={{rc}} count={{count.value}}")
listener = socket.fromfd(fds[0], socket.AF_INET, socket.SOCK_STREAM)
os.close(fds[0])
libc.free(fds)

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/api/version":
            self.send_response(404); self.end_headers(); return
        body = {payload!r}.encode()
        self.send_response(200); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *args): pass

server = HTTPServer(("127.0.0.1", 0), Handler, bind_and_activate=False)
server.socket.close()
server.socket = listener
server.server_address = listener.getsockname()
server.serve_forever()
'''


def socket_activated_target_plist(label, binary, port, log):
    return plistlib.dumps({
        "Label": label,
        "ProgramArguments": [str(binary)],
        "Sockets": {"Listeners": {
            "SockFamily": "IPv4",
            "SockProtocol": "TCP",
            "SockServiceName": str(port),
            "SockType": "stream",
        }},
        "RunAtLoad": True,
        "KeepAlive": True,
        "StandardOutPath": str(log),
        "StandardErrorPath": str(log),
    })


def launchd_pid(domain, label):
    result = subprocess.run(
        ["launchctl", "print", f"{domain}/{label}"],
        capture_output=True,
        text=True,
        check=True,
    )
    for raw in result.stdout.splitlines():
        line = raw.strip()
        if line.startswith("pid = "):
            return int(line.split(" = ", 1)[1])
    raise RuntimeError(f"launchd job has no PID: {label}")


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
        "manifest_version": 1,
        "transaction_id": suffix, "source_kind": "local_head", "source_commit": "newsha",
        "release_tag": None, "release_commit": None, "expected": new_identity, "previous": old_identity,
        "previous_deployed_sha": "oldsha",
        "candidate_binary": str(candidate_binary), "candidate_binary_sha256": digest(candidate_binary),
        "candidate_plist": str(candidate_plist), "candidate_plist_sha256": digest(candidate_plist),
        "rollback_binary": str(rollback_binary), "rollback_binary_sha256": digest(rollback_binary),
        "rollback_plist": str(rollback_plist), "rollback_plist_sha256": digest(rollback_plist),
        "target_binary": str(target_binary), "target_plist": str(target_plist_path),
        "label": target_label, "helper_label": helper_label, "uid": os.getuid(), "health_url": url,
        "health_insecure_tls": False, "active_path": str(active), "status_path": str(status),
        "previous_health_url": url, "previous_health_insecure_tls": False,
        "previous_health_json": True,
        "deployed_sha_path": str(root / f"deployed-{suffix}.sha"), "lock_path": str(root / f"lock-{suffix}"),
        "claim_lock_path": str(root / f"claim-lock-{suffix}"),
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
            if Path(manifest["deployed_sha_path"]).read_text().strip() != "oldsha":
                raise RuntimeError("failed candidate did not restore previous deployed.sha")
        print(f"PASS: {expected_state} after external SIGKILL of initiating process group")
    finally:
        subprocess.run(["launchctl", "bootout", f"{domain}/{helper_label}"], capture_output=True)
        subprocess.run(["launchctl", "bootout", f"{domain}/{target_label}"], capture_output=True)


def run_restart_scenario(root, domain):
    suffix = uuid.uuid4().hex
    target_label = f"test.phoenix-ide.restart-target.{suffix}"
    helper_label = f"test.phoenix-ide.restart-helper.{suffix}"
    port = allocate_port()
    refuse_live(target_label, root, port)
    identity = {"version": "2.0.0", "git_sha": "a" * 12}
    target_binary = root / f"restart-phoenix-{suffix}"
    target_binary.write_text(socket_activated_server_script(identity))
    target_binary.chmod(0o755)
    target_plist_path = root / f"restart-target-{suffix}.plist"
    target_plist_path.write_bytes(socket_activated_target_plist(
        target_label,
        target_binary,
        port,
        root / f"restart-target-{suffix}.log",
    ))
    deployed_sha = root / f"restart-deployed-{suffix}.sha"
    deployed_sha.write_text("a" * 40 + "\n")
    original_hashes = {
        target_binary: digest(target_binary),
        target_plist_path: digest(target_plist_path),
        deployed_sha: digest(deployed_sha),
    }
    url = f"http://127.0.0.1:{port}/api/version"
    status = root / f"restart-status-{suffix}.json"
    log = root / f"restart-helper-{suffix}.log"
    active = root / f"restart-active-{suffix}"
    active.write_text(suffix + "\n")
    manifest_path = root / f"restart-manifest-{suffix}.json"
    helper = root / f"restart-helper-{suffix}.py"
    helper.write_bytes(RESTART_HELPER.read_bytes())
    helper.chmod(0o500)
    helper_plist = root / f"restart-helper-{suffix}.plist"
    handoff = root / f"restart-handoff-{suffix}"
    initiator = root / f"restart-initiator-{suffix}.py"

    try:
        subprocess.run(["launchctl", "bootstrap", domain, str(target_plist_path)], check=True)
        wait_identity(url, identity, time.monotonic() + 10)
        previous_pid = launchd_pid(domain, target_label)
        manifest = {
            "manifest_version": 1,
            "transaction_id": suffix,
            "expected": identity,
            "previous_pid": previous_pid,
            "binary_path": str(target_binary),
            "binary_sha256": original_hashes[target_binary],
            "plist_path": str(target_plist_path),
            "plist_sha256": original_hashes[target_plist_path],
            "deployed_sha_path": str(deployed_sha),
            "deployed_sha256": original_hashes[deployed_sha],
            "label": target_label,
            "helper_label": helper_label,
            "uid": os.getuid(),
            "health_url": url,
            "health_insecure_tls": False,
            "active_path": str(active),
            "status_path": str(status),
            "lock_path": str(root / f"restart-lock-{suffix}"),
            "claim_lock_path": str(root / f"restart-claim-lock-{suffix}"),
            "created_at": "2026-01-01T00:00:00+00:00",
            "transition_timeout_secs": 10,
            "health_timeout_secs": 10,
        }
        manifest_path.write_text(json.dumps(manifest))
        helper_plist.write_bytes(plistlib.dumps({
            "Label": helper_label,
            "ProgramArguments": [
                "/usr/bin/python3", str(helper), "restart",
                "--manifest", str(manifest_path),
                "--helper-label", helper_label,
                "--uid", str(os.getuid()),
            ],
            "RunAtLoad": True,
            "StandardOutPath": str(log),
            "StandardErrorPath": str(log),
        }))
        initiator.write_text(
            "import pathlib,subprocess,time\n"
            f"subprocess.run(['launchctl','bootstrap',{domain!r},{str(helper_plist)!r}],check=True)\n"
            f"pathlib.Path({str(handoff)!r}).write_text('handed-off')\n"
            "time.sleep(60)\n"
        )
        process = subprocess.Popen([sys.executable, str(initiator)], start_new_session=True)
        deadline = time.monotonic() + 10
        while not handoff.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        if not handoff.exists():
            process.kill()
            raise RuntimeError("restart initiator did not report launchd handoff")
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)
        result = wait_terminal(status, log, time.monotonic() + 20)
        if result["state"] != "committed":
            raise RuntimeError(f"unexpected restart terminal status: {result}")
        if result["previous_pid"] != previous_pid or result["running_pid"] == previous_pid:
            raise RuntimeError(f"restart did not record a new PID: {result}")
        wait_identity(url, identity, time.monotonic() + 3)
        for path, expected_hash in original_hashes.items():
            if digest(path) != expected_hash:
                raise RuntimeError(f"restart changed installed artifact: {path}")
        wait_unloaded(domain, helper_label, time.monotonic() + 5)
        print("PASS: installed-state-preserving restart after initiator exit")
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
        run_restart_scenario(root, domain)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
