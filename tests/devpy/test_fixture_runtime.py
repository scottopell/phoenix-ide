import json
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/integration/fixture_runtime.py"


class FixtureRuntimeTests(unittest.TestCase):
    def test_build_identity_is_exact_and_configurable(self):
        result = subprocess.run(
            [sys.executable, str(FIXTURE), "--build-identity", "--version", "1.0.0", "--git-sha", "aaaaaaaaaaaa"],
            text=True,
            capture_output=True,
            check=True,
        )
        self.assertEqual(
            {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"},
            json.loads(result.stdout),
        )

    def test_direct_bind_serves_both_identity_endpoints_and_stops_gracefully(self):
        with tempfile.TemporaryDirectory() as td:
            ready = Path(td) / "ready"
            process = subprocess.Popen([
                sys.executable,
                str(FIXTURE),
                "--version", "1.0.0",
                "--git-sha", "aaaaaaaaaaaa",
                "--port", "0",
                "--ready-file", str(ready),
            ])
            try:
                deadline = time.monotonic() + 5
                while not ready.exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertTrue(ready.exists())
                port = int(ready.read_text())
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/version") as response:
                    self.assertEqual(
                        {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"},
                        json.load(response),
                    )
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/version") as response:
                    self.assertEqual("phoenix-ide 1.0.0\n", response.read().decode())
                process.send_signal(signal.SIGTERM)
                self.assertEqual(0, process.wait(timeout=5))
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()

    def test_reported_identity_can_deliberately_mismatch_embedded_identity(self):
        with tempfile.TemporaryDirectory() as td:
            ready = Path(td) / "ready"
            process = subprocess.Popen([
                sys.executable,
                str(FIXTURE),
                "--version", "2.0.0",
                "--git-sha", "bbbbbbbbbbbb",
                "--report-version", "9.9.9",
                "--report-git-sha", "cccccccccccc",
                "--ready-file", str(ready),
            ])
            try:
                deadline = time.monotonic() + 5
                while not ready.exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                port = int(ready.read_text())
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/version") as response:
                    self.assertEqual(
                        {"version": "9.9.9", "git_sha": "cccccccccccc"},
                        json.load(response),
                    )
            finally:
                process.send_signal(signal.SIGTERM)
                process.wait(timeout=5)

    def test_crash_mode_exits_with_deterministic_code(self):
        result = subprocess.run([sys.executable, str(FIXTURE), "--crash"])
        self.assertEqual(23, result.returncode)


if __name__ == "__main__":
    unittest.main()
