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


class BareTransactionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.layout = supervisor.Layout(Path(self.temporary.name) / "phoenix")
        self.layout.transactions.mkdir(parents=True, mode=0o700)
        self.transaction_id = "b" * 32
        self.transaction = self.layout.transactions / self.transaction_id
        self.transaction.mkdir(mode=0o700)

    def tearDown(self):
        self.temporary.cleanup()

    def artifact(self, name, content):
        path = self.transaction / name
        path.write_text(content)
        path.chmod(0o600)
        return {"name": name, "sha256": supervisor.sha256(path)}

    def manifest(self, *, previous=False):
        candidate_binary = self.artifact("candidate-binary", "new binary")
        candidate_environment = self.artifact("candidate.env", "MODE=new\n")
        rollback_binary = self.artifact("rollback-binary", "old binary") if previous else None
        rollback_environment = self.artifact("rollback.env", "MODE=old\n") if previous else None
        value = {
            "manifest_version": 1,
            "transaction_id": self.transaction_id,
            "expected": {"version": "2.0.0", "git_sha": "b" * 12},
            "previous": {"version": "1.0.0", "git_sha": "a" * 12} if previous else None,
            "expected_health_url": "http://127.0.0.1:49155/api/version",
            "previous_health_url": "http://127.0.0.1:49155/api/version" if previous else None,
            "candidate_binary": candidate_binary,
            "candidate_environment": candidate_environment,
            "rollback_binary": rollback_binary,
            "rollback_environment": rollback_environment,
            "source_commit": "b" * 40,
            "previous_deployed_sha": "a" * 40 if previous else None,
            "created_at": "2026-07-15T00:00:00+00:00",
            "health_timeout_secs": 1,
        }
        path = self.transaction / "manifest.json"
        path.write_text(__import__("json").dumps(value))
        path.chmod(0o600)
        for artifact in self.transaction.iterdir():
            artifact.chmod(0o400)
        self.transaction.chmod(0o500)
        return path

    def test_activation_accepts_only_transaction_id_and_manifest_hash(self):
        path = self.manifest()
        owner = supervisor.Supervisor(self.layout)
        owner.start_child = mock.Mock(return_value=supervisor.ChildIdentity(42, 100, supervisor.RuntimeIdentity("2.0.0", "b" * 12)))
        state = owner.activate(self.transaction_id, supervisor.sha256(path))
        self.assertEqual("committed", state)
        self.assertEqual("new binary", self.layout.binary.read_text())
        self.assertEqual("MODE=new\n", self.layout.environment.read_text())
        self.assertEqual("b" * 40, self.layout.deployed_sha.read_text().strip())
        self.assertFalse(self.layout.active_file.exists())
        self.assertEqual("committed", __import__("json").loads(self.layout.status_file.read_text())["state"])

    def test_identity_failure_restores_previous_runtime(self):
        self.layout.binary.parent.mkdir(parents=True)
        self.layout.binary.write_text("old binary")
        self.layout.environment.parent.mkdir(parents=True)
        self.layout.environment.write_text("MODE=old\n")
        self.layout.deployed_sha.write_text("a" * 40 + "\n")
        path = self.manifest(previous=True)
        owner = supervisor.Supervisor(self.layout)
        owner.child = mock.Mock()
        calls = 0

        def start(*args, **kwargs):
            nonlocal calls
            calls += 1
            if calls == 1:
                raise supervisor.SupervisorError("wrong identity")
            return supervisor.ChildIdentity(43, 101, supervisor.RuntimeIdentity("1.0.0", "a" * 12))

        owner.start_child = mock.Mock(side_effect=start)
        owner.stop_child = mock.Mock()
        state = owner.activate(self.transaction_id, supervisor.sha256(path))
        self.assertEqual("activation_failed_rolled_back", state)
        self.assertEqual("old binary", self.layout.binary.read_text())
        self.assertEqual("MODE=old\n", self.layout.environment.read_text())
        self.assertEqual("a" * 40, self.layout.deployed_sha.read_text().strip())
        self.assertFalse(self.layout.active_file.exists())

    def test_completed_transaction_cannot_be_replayed(self):
        path = self.manifest()
        owner = supervisor.Supervisor(self.layout)
        owner.start_child = mock.Mock(return_value=supervisor.ChildIdentity(42, 100, supervisor.RuntimeIdentity("2.0.0", "b" * 12)))
        owner.activate(self.transaction_id, supervisor.sha256(path))
        with self.assertRaisesRegex(supervisor.SupervisorError, "already been used"):
            owner.activate(self.transaction_id, supervisor.sha256(path))

    def test_rollback_failure_preserves_claim_and_both_failures(self):
        self.layout.binary.parent.mkdir(parents=True)
        self.layout.binary.write_text("old binary")
        self.layout.environment.parent.mkdir(parents=True)
        self.layout.environment.write_text("MODE=old\n")
        path = self.manifest(previous=True)
        owner = supervisor.Supervisor(self.layout)
        owner.stop_child = mock.Mock()
        owner.start_child = mock.Mock(side_effect=[
            supervisor.SupervisorError("candidate wrong"),
            supervisor.SupervisorError("rollback failed"),
        ])
        state = owner.activate(self.transaction_id, supervisor.sha256(path))
        self.assertEqual("activation_failed_rollback_failed", state)
        self.assertEqual(self.transaction_id, self.layout.active_file.read_text().strip())
        status = __import__("json").loads(self.layout.status_file.read_text())
        self.assertEqual("candidate wrong", status["failure"])
        self.assertEqual("rollback failed", status["rollback_failure"])

    def test_manifest_hash_mismatch_is_rejected_before_claim(self):
        self.manifest()
        owner = supervisor.Supervisor(self.layout)
        with self.assertRaisesRegex(supervisor.SupervisorError, "manifest checksum mismatch"):
            owner.activate(self.transaction_id, "0" * 64)
        self.assertFalse(self.layout.active_file.exists())


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
