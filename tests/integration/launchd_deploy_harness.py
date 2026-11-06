#!/usr/bin/env python3
"""Disposable ordinary deployment, manual failure, and restart journey."""
import hashlib
import json
import os
import plistlib
import signal
import sqlite3
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
CANDIDATE_COMMIT = "b" * 40
PREVIOUS_DEPLOYED_COMMIT = "a" * 40
RESTART_COMMIT = "c" * 40
TERMINAL = {"committed", "failed", "precondition_failed", "restart_failed", "rejected_concurrent"}


def refuse_live(label, root, port):
    root = root.resolve()
    temporary = Path(tempfile.gettempdir()).resolve()
    forbidden = [LIVE_HOME.resolve(), (Path.home() / "Library/LaunchAgents").resolve()]
    harness_root = any(part.startswith("phoenix-launchd-harness-") for part in root.parts)
    if (not label.startswith("test.phoenix-ide.") or label == LIVE_LABEL
            or type(port) is not int or not 0 < port <= 65535 or port == LIVE_PORT
            or temporary not in root.parents or not harness_root
            or any(root == path or path in root.parents for path in forbidden)):
        raise SystemExit("refusing non-disposable launchd label, port, or path")


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
        result = subprocess.run(["launchctl", "print", target], capture_output=True, text=True)
        if result.returncode != 0:
            if "Could not find service" in result.stdout + result.stderr:
                return
            raise RuntimeError(f"cannot confirm disposable service absence: {label}")
        time.sleep(0.1)
    raise RuntimeError(f"disposable launchd job still loaded: {label}")


def server_script(identity, *, healthy=True, migration_failure=False):
    payload = json.dumps(identity if healthy else {**identity, "version": "wrong-candidate"})
    return f'''#!{sys.executable}
import json, os, sqlite3, sys
from pathlib import Path
from http.server import BaseHTTPRequestHandler, HTTPServer
identity = {identity!r}
if sys.argv[1:] == ["--build-identity"]:
    print(json.dumps(identity))
    raise SystemExit(0)
if sys.argv[1:] == ["--migrate-only"]:
    status = json.loads(Path(os.environ["STATUS_PATH"]).read_text())
    assert status["state"] == "migration_started"
    with sqlite3.connect(os.environ["PHOENIX_DB_PATH"]) as database:
        database.execute("INSERT INTO migration_effects VALUES (?)", (identity["git_sha"],))
    if {migration_failure!r}:
        print("injected failure after committed migration write", file=sys.stderr, flush=True)
        raise SystemExit(7)
    raise SystemExit(0)
with Path(os.environ["STARTS_PATH"]).open("a") as starts:
    starts.write(identity["git_sha"] + chr(10))
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


def target_plist(label, binary, port, log, environment):
    return plistlib.dumps({
        "Label": label, "ProgramArguments": [str(binary)],
        "EnvironmentVariables": {"TEST_PORT": str(port), **environment},
        "RunAtLoad": True, "KeepAlive": True,
        "StandardOutPath": str(log), "StandardErrorPath": str(log),
    })


def socket_activated_server_script(identity):
    payload = json.dumps({**identity, "socket_activated": True})
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


def deployment_identity_fixture():
    return {
        "source_commit": CANDIDATE_COMMIT,
        "expected": {"version": "2.0.0", "git_sha": CANDIDATE_COMMIT},
        "previous": {"version": "1.0.0", "git_sha": PREVIOUS_DEPLOYED_COMMIT},
    }


def restart_identity_fixture():
    return {
        "expected": {"version": "2.0.0", "git_sha": RESTART_COMMIT},
        "deployed_sha": RESTART_COMMIT,
    }


def run_scenario(root, domain, *, failure=None):
    suffix = uuid.uuid4().hex
    target_label = f"test.phoenix-ide.server.{suffix}"
    helper_label = f"com.phoenix-ide.deploy.{suffix}"
    port = allocate_port()
    root = root / f"deployment-{suffix}"
    refuse_live(target_label, root, port)
    root.mkdir()
    fixture = deployment_identity_fixture()
    previous, expected = fixture["previous"], fixture["expected"]
    binary = root / "phoenix"
    binary.write_text(server_script(previous))
    binary.chmod(0o755)
    database = root / "application.db"
    with sqlite3.connect(database) as db:
        db.execute("CREATE TABLE migration_effects(candidate TEXT)")
    status, log, starts = root / "status.json", root / "helper.log", root / "starts"
    environment = {"PHOENIX_DB_PATH": str(database), "STATUS_PATH": str(status), "STARTS_PATH": str(starts)}
    canonical = root / "service.plist"
    canonical.write_bytes(target_plist(target_label, binary, port, root / "target.log", environment))
    candidate = root / "candidate"
    candidate.write_text(server_script(expected, healthy=failure != "health", migration_failure=failure == "migration"))
    candidate.chmod(0o755)
    candidate_plist = root / "candidate.plist"
    candidate_plist.write_bytes(canonical.read_bytes())
    helper = root / "activate.py"
    helper.write_bytes(HELPER.read_bytes())
    helper.chmod(0o500)
    active = root / "active"
    active.write_text(suffix + "\n")
    manifest = {
        "manifest_version": 2, "transaction_id": suffix,
        "source_kind": "local_head", "source_commit": fixture["source_commit"],
        "release_tag": None, "release_commit": None, "expected": expected,
        "candidate_binary": str(candidate), "candidate_binary_sha256": digest(candidate),
        "candidate_plist": str(candidate_plist), "candidate_plist_sha256": digest(candidate_plist),
        "target_binary": str(binary), "target_plist": str(canonical), "label": target_label,
        "helper_label": helper_label, "uid": os.getuid(),
        "health_url": f"http://127.0.0.1:{port}/api/version", "health_insecure_tls": False,
        "active_path": str(active), "status_path": str(status),
        "deployed_sha_path": str(root / "deployed.sha"), "lock_path": str(root / "activation.lock"),
        "claim_lock_path": str(root / "claim.lock"), "created_at": "2026-01-01T00:00:00+00:00",
        "transition_timeout_secs": 10, "health_timeout_secs": 2 if failure else 10,
    }
    manifest_path = root / "manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    status.write_text(json.dumps({"transaction_id": suffix, "state": "prepared"}))
    helper_plist = root / "helper.plist"
    helper_plist.write_bytes(plistlib.dumps({
        "Label": helper_label,
        "ProgramArguments": [sys.executable, str(helper), "activate", "--manifest", str(manifest_path),
                             "--helper-label", helper_label, "--uid", str(os.getuid())],
        "RunAtLoad": True, "StandardOutPath": str(log), "StandardErrorPath": str(log),
    }))
    handoff = root / "handed-off"
    initiator = root / "initiator.py"
    initiator.write_text(
        "import pathlib,subprocess,time\n"
        f"subprocess.run(['launchctl','bootstrap',{domain!r},{str(helper_plist)!r}],check=True)\n"
        f"pathlib.Path({str(handoff)!r}).write_text('handed-off')\n"
        "time.sleep(60)\n"
    )
    process = None
    try:
        subprocess.run(["launchctl", "bootstrap", domain, str(canonical)], check=True)
        wait_identity(manifest["health_url"], previous, time.monotonic() + 10)
        previous_pid = launchd_pid(domain, target_label)
        process = subprocess.Popen([sys.executable, str(initiator)], start_new_session=True)
        deadline = time.monotonic() + 10
        while not handoff.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        if not handoff.exists():
            raise RuntimeError("initiator did not report launchd handoff")
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)
        result = wait_terminal(status, log, time.monotonic() + 35)
        expected_state = "failed" if failure else "committed"
        if result["state"] != expected_state:
            raise RuntimeError(f"unexpected status: {result}; log={log.read_text()}")
        wait_unloaded(domain, helper_label, time.monotonic() + 5)
        with sqlite3.connect(database) as db:
            writes = db.execute("SELECT candidate FROM migration_effects").fetchall()
        if writes != [(CANDIDATE_COMMIT,)]:
            raise RuntimeError(f"candidate migration did not run exactly once behind checkpoint: {writes}")
        if starts.read_text().splitlines().count(PREVIOUS_DEPLOYED_COMMIT) != 1:
            raise RuntimeError("predecessor was restarted")
        if failure:
            wait_unloaded(domain, target_label, time.monotonic() + 5)
            if not active.is_file() or canonical.exists():
                raise RuntimeError("failed deployment lost its manual recovery marker or published its plist")
            if "migration_started" not in result["failure"] and "verifying" not in result["failure"]:
                raise RuntimeError(f"missing failed phase diagnostic: {result}")
            if failure == "migration" and "injected failure after committed migration write" not in log.read_text():
                raise RuntimeError("migration stderr was discarded")
        else:
            wait_identity(manifest["health_url"], expected, time.monotonic() + 3)
            if launchd_pid(domain, target_label) == previous_pid:
                raise RuntimeError("deployment did not replace the predecessor PID")
            if not canonical.samefile(candidate_plist):
                raise RuntimeError("loaded private plist differs from published canonical plist")
            if active.exists() or Path(manifest["deployed_sha_path"]).read_text().strip() != CANDIDATE_COMMIT:
                raise RuntimeError("deployment did not commit identity and release its claim")
        print(f"PASS: {expected_state} ({failure or 'healthy'}) after initiator SIGKILL; migration writes retained")
    finally:
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
        subprocess.run(["launchctl", "bootout", f"{domain}/{helper_label}"], capture_output=True)
        subprocess.run(["launchctl", "bootout", f"{domain}/{target_label}"], capture_output=True)


def run_restart_scenario(root, domain):
    suffix = uuid.uuid4().hex
    target_label = f"test.phoenix-ide.restart-target.{suffix}"
    helper_label = f"test.phoenix-ide.restart-helper.{suffix}"
    port = allocate_port()
    refuse_live(target_label, root, port)
    identity_fixture = restart_identity_fixture()
    identity = identity_fixture["expected"]
    runtime = {**identity, "socket_activated": True}
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
    deployed_sha.write_text(identity_fixture["deployed_sha"] + "\n")
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

    process = None
    try:
        subprocess.run(["launchctl", "bootstrap", domain, str(target_plist_path)], check=True)
        wait_identity(url, runtime, time.monotonic() + 10)
        previous_pid = launchd_pid(domain, target_label)
        manifest = {
            "manifest_version": 2,
            "transaction_id": suffix,
            "expected": identity,
            "previous_pid": previous_pid,
            "binary_path": str(target_binary),
            "binary_sha256": original_hashes[target_binary],
            "plist_path": str(target_plist_path),
            "plist_sha256": original_hashes[target_plist_path],
            "socket_service": port,
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
            "transition_timeout_secs": 30,
            "health_timeout_secs": 10,
        }
        manifest_path.write_text(json.dumps(manifest))
        helper_plist.write_bytes(plistlib.dumps({
            "Label": helper_label,
            "ProgramArguments": [
                sys.executable, str(helper), "restart",
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
        result = wait_terminal(status, log, time.monotonic() + 40)
        if result["state"] != "committed":
            raise RuntimeError(f"unexpected restart terminal status: {result}")
        if result["previous_pid"] != previous_pid or result["running_pid"] == previous_pid:
            raise RuntimeError(f"restart did not record a new PID: {result}")
        wait_identity(url, runtime, time.monotonic() + 3)
        for path, expected_hash in original_hashes.items():
            if digest(path) != expected_hash:
                raise RuntimeError(f"restart changed installed artifact: {path}")
        wait_unloaded(domain, helper_label, time.monotonic() + 5)
        print("PASS: installed-state-preserving restart after initiator exit")
    finally:
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
        subprocess.run(["launchctl", "bootout", f"{domain}/{helper_label}"], capture_output=True)
        subprocess.run(["launchctl", "bootout", f"{domain}/{target_label}"], capture_output=True)


def main():
    if sys.platform != "darwin":
        print("SKIP: launchd disposable harness requires macOS")
        return 0
    with tempfile.TemporaryDirectory(prefix="phoenix-launchd-harness-") as td:
        root = Path(td)
        domain = f"gui/{os.getuid()}"
        run_scenario(root, domain)
        run_scenario(root, domain, failure="migration")
        run_scenario(root, domain, failure="health")
        run_restart_scenario(root, domain)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
