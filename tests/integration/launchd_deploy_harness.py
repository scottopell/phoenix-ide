#!/usr/bin/env python3
"""Disposable end-to-end launchd activation ownership harness."""
import hashlib
import json
import os
import plistlib
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LIVE_LABEL = "com.phoenix-ide.server"
LIVE_PORT = 8031
LIVE_HOME = Path.home() / ".phoenix-ide"


def refuse_live(label, root, port):
    root = root.resolve()
    forbidden = [LIVE_HOME.resolve(), (Path.home() / "Library/LaunchAgents").resolve()]
    if label == LIVE_LABEL or port == LIVE_PORT or any(root == path or path in root.parents for path in forbidden):
        raise SystemExit("refusing live launchd label, port, or production path")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def wait_json(path, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            try:
                return json.loads(path.read_text())
            except json.JSONDecodeError:
                pass
        time.sleep(0.1)
    raise SystemExit(f"timed out waiting for {path}")


def server_script(identity):
    payload = json.dumps(identity)
    return f'''#!/usr/bin/python3
import json, os
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
        "Label": label,
        "ProgramArguments": [str(binary)],
        "EnvironmentVariables": {"TEST_PORT": str(port)},
        "RunAtLoad": True,
        "KeepAlive": True,
        "StandardOutPath": str(log),
        "StandardErrorPath": str(log),
    })


def main():
    if sys.platform != "darwin":
        print("SKIP: launchd disposable harness requires macOS")
        return 0
    uid = os.getuid()
    suffix = uuid.uuid4().hex
    target_label = f"test.phoenix-ide.server.{suffix}"
    helper_label = f"test.phoenix-ide.deploy.{suffix}"
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="phoenix-launchd-harness-") as td:
        root = Path(td)
        refuse_live(target_label, root, port)
        old_identity = {"version": "1.0.0", "git_sha": "oldsha"}
        new_identity = {"version": "2.0.0", "git_sha": "newsha"}
        target_binary = root / "phoenix-ide"
        target_binary.write_text(server_script(old_identity)); target_binary.chmod(0o755)
        target_plist_path = root / "target.plist"
        target_plist_path.write_bytes(target_plist(target_label, target_binary, port, root / "target.log"))
        candidate_binary = root / "candidate"
        candidate_binary.write_text(server_script(new_identity)); candidate_binary.chmod(0o755)
        candidate_plist = root / "candidate.plist"
        candidate_plist.write_bytes(target_plist(target_label, target_binary, port, root / "target.log"))
        rollback_binary = root / "rollback"
        rollback_binary.write_bytes(target_binary.read_bytes()); rollback_binary.chmod(0o755)
        rollback_plist = root / "rollback.plist"
        rollback_plist.write_bytes(target_plist_path.read_bytes())
        status = root / "status.json"
        manifest_path = root / "manifest.json"
        active = root / "active"; active.mkdir()
        manifest = {
            "transaction_id": suffix, "source_kind": "local_head", "source_commit": "newsha",
            "release_tag": None, "expected": new_identity, "previous": old_identity,
            "candidate_binary": str(candidate_binary), "candidate_binary_sha256": digest(candidate_binary),
            "candidate_plist": str(candidate_plist), "candidate_plist_sha256": digest(candidate_plist),
            "rollback_binary": str(rollback_binary), "rollback_binary_sha256": digest(rollback_binary),
            "rollback_plist": str(rollback_plist), "rollback_plist_sha256": digest(rollback_plist),
            "target_binary": str(target_binary), "target_plist": str(target_plist_path),
            "label": target_label, "uid": uid, "health_url": f"http://127.0.0.1:{port}/api/version",
            "helper_label": helper_label,
            "health_insecure_tls": False, "active_path": str(active), "status_path": str(status),
            "deployed_sha_path": str(root / "deployed.sha"), "lock_path": str(root / "activate.lock"),
            "created_at": "2026-01-01T00:00:00+00:00", "transition_timeout_secs": 10,
            "health_timeout_secs": 10,
        }
        manifest_path.write_text(json.dumps(manifest))
        helper_plist = root / "helper.plist"
        helper_plist.write_bytes(plistlib.dumps({
            "Label": helper_label,
            "ProgramArguments": ["/usr/bin/python3", str(ROOT / "scripts/launchd_deploy_helper.py"),
                                 "activate", "--manifest", str(manifest_path)],
            "RunAtLoad": True,
            "StandardOutPath": str(root / "helper.log"),
            "StandardErrorPath": str(root / "helper.log"),
        }))
        domain = f"gui/{uid}"
        try:
            subprocess.run(["launchctl", "bootstrap", domain, str(target_plist_path)], check=True)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                try:
                    with urllib.request.urlopen(manifest["health_url"], timeout=1) as response:
                        if json.load(response) == old_identity:
                            break
                except Exception:
                    time.sleep(0.1)
            initiator = root / "initiator.py"
            initiator.write_text(
                "import os,subprocess\n"
                f"subprocess.run(['launchctl','bootstrap',{domain!r},{str(helper_plist)!r}],check=True)\n"
                "os._exit(0)\n"
            )
            subprocess.run([sys.executable, str(initiator)], check=True)
            result = wait_json(status)
            while result.get("state") not in {"committed", "activation_failed_rolled_back", "activation_failed_rollback_failed"}:
                time.sleep(0.1); result = wait_json(status)
            if result["state"] != "committed":
                raise SystemExit(f"activation failed: {result}; log={Path(root / 'helper.log').read_text()}")
            with urllib.request.urlopen(manifest["health_url"], timeout=2) as response:
                observed = json.load(response)
            if observed != new_identity or (root / "deployed.sha").read_text().strip() != "newsha":
                raise SystemExit(f"exact identity evidence mismatch: {observed}")
            print(f"PASS: independent helper committed {observed['version']} ({observed['git_sha']}) after initiator exited")
        finally:
            subprocess.run(["launchctl", "bootout", f"{domain}/{helper_label}"], capture_output=True)
            subprocess.run(["launchctl", "bootout", f"{domain}/{target_label}"], capture_output=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
