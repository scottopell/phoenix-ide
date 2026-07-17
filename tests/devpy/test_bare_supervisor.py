import importlib.util
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_supervisor():
    path = ROOT / "scripts/bare_supervisor.py"
    spec = importlib.util.spec_from_file_location("bare_supervisor_test", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


supervisor = load_supervisor()


class BareSupervisorUnitTests(unittest.TestCase):
    def test_proc_start_time_parses_comm_with_spaces_and_parentheses(self):
        with tempfile.TemporaryDirectory() as td:
            proc = Path(td) / "42"
            proc.mkdir()
            after = ["S"] + ["0"] * 18 + ["987654"] + ["0"] * 5
            (proc / "stat").write_text(f"42 (name with ) paren) {' '.join(after)}\n")
            self.assertEqual(987654, supervisor.proc_start_time(42, Path(td)))

    def test_direct_child_rejects_reused_pid_start_time(self):
        child = mock.Mock(pid=42)
        child.poll.return_value = None
        identity = supervisor.ChildIdentity(42, 100, supervisor.RuntimeIdentity("1.0.0", "a" * 12))
        with mock.patch.object(supervisor, "proc_start_time", return_value=101):
            self.assertFalse(supervisor.direct_child_matches(child, identity))

    def test_dispatch_stop_preserves_supervisor(self):
        with tempfile.TemporaryDirectory() as td:
            owner = supervisor.Supervisor(supervisor.Layout(Path(td)))
            owner.stop_child = mock.Mock()
            response = owner.dispatch({"protocol_version": 1, "action": "stop"})
            self.assertTrue(response["ok"])
            self.assertTrue(owner.running)
            owner.stop_child.assert_called_once()

    def test_protocol_mismatch_is_rejected(self):
        owner = supervisor.Supervisor(supervisor.Layout(Path("unused")))
        with self.assertRaisesRegex(supervisor.SupervisorError, "unsupported supervisor protocol"):
            owner.dispatch({"protocol_version": 99, "action": "status"})


@unittest.skipUnless(sys.platform.startswith("linux"), "requires Linux /proc and SO_PEERCRED")
class BareSupervisorLinuxIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "phoenix"
        self.fixture = ROOT / "tests/integration/fixture_runtime.py"
        self.process = subprocess.Popen([
            sys.executable,
            str(ROOT / "scripts/bare_supervisor.py"),
            "--root", str(self.root),
            "run",
        ], start_new_session=True)
        deadline = time.monotonic() + 5
        while not (self.root / "run/supervisor.sock").exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        if not (self.root / "run/supervisor.sock").exists():
            self.fail("supervisor socket did not appear")

    def tearDown(self):
        if self.process.poll() is None:
            try:
                supervisor.request(
                    self.root / "run/supervisor.sock",
                    {"protocol_version": 1, "action": "shutdown-supervisor"},
                )
                self.process.wait(timeout=5)
            except Exception:
                self.process.kill()
                self.process.wait()
        self.temporary.cleanup()

    def test_supervisor_directly_owns_exact_fixture_and_stop_leaves_owner_alive(self):
        supervisor.request(
            self.root / "run/supervisor.sock",
            {"protocol_version": 1, "action": "shutdown-supervisor"},
        )
        self.process.wait(timeout=5)
        ready_port = 49321
        owner = supervisor.Supervisor(supervisor.Layout(self.root))
        child_identity = owner.start_child(
            [
                sys.executable,
                str(self.fixture),
                "--version", "1.0.0",
                "--git-sha", "aaaaaaaaaaaa",
                "--port", str(ready_port),
            ],
            os.environ.copy(),
            supervisor.RuntimeIdentity("1.0.0", "aaaaaaaaaaaa"),
            f"http://127.0.0.1:{ready_port}/api/version",
            5,
        )
        child = {"pid": child_identity.pid, "proc_start_time": child_identity.proc_start_time}
        status = owner.status()
        self.assertEqual(child["pid"], status["child"]["pid"])
        self.assertEqual(child["proc_start_time"], status["child"]["proc_start_time"])
        self.assertEqual(os.getpid(), int((Path("/proc") / str(child["pid"]) / "stat").read_text().split()[3]))
        owner.stop_child()
        self.assertIsNone(owner.status()["child"])


if __name__ == "__main__":
    unittest.main()
