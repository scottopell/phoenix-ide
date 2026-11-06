"""Ordinary deploy failures stop for manual recovery without inspecting SQLite."""
import dataclasses
import fcntl
import importlib.util
import json
import os
import plistlib
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("simple_launchd_helper_test", ROOT / "scripts/launchd_deploy_helper.py")
helper = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = helper
SPEC.loader.exec_module(helper)


class FakeLaunchctl:
    def __init__(self, manifest):
        self.manifest = manifest
        self.pid = 41
        self.starts = []
        self.stops = 0

    def inspect(self):
        return ("running", self.pid) if self.pid is not None else ("not_loaded", None)

    def stop(self):
        self.stops += 1
        old, self.pid = self.pid, None
        with (Path(self.manifest.candidate_binary).parent / "events").open("a") as stream:
            stream.write("stop\n")
        return old

    def start(self, old_pid, *, plist_path=None):
        self.starts.append(plist_path)
        self.pid = 99
        with (Path(self.manifest.candidate_binary).parent / "events").open("a") as stream:
            stream.write("start\n")
        return self.pid


class SimpleLaunchdHelperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        binary = self.root / "candidate"
        binary.write_text(f'''#!{sys.executable}
import json, os, sqlite3, sys
from pathlib import Path
if sys.argv[1:] == ["--build-identity"]:
    print(json.dumps({{"version": "1.0", "git_sha": "{'a' * 40}"}}))
    raise SystemExit(0)
assert sys.argv[1:] == ["--migrate-only"]
status = json.loads(Path(os.environ["TEST_STATUS_PATH"]).read_text())
assert status["state"] == "migration_started"
with Path(os.environ["TEST_EVENTS"]).open("a") as stream:
    stream.write("migrate\\n")
with sqlite3.connect(os.environ["PHOENIX_DB_PATH"]) as db:
    db.execute("INSERT INTO changes VALUES (?)", (status["state"],))
if os.environ.get("FAIL_MIGRATION"):
    print("actual migration failure detail", file=sys.stderr, flush=True)
    raise SystemExit(7)
''')
        binary.chmod(0o755)
        self.database = self.root / "application.db"
        with sqlite3.connect(self.database) as db:
            db.execute("CREATE TABLE changes(phase TEXT)")
        candidate_plist = self.root / "candidate.plist"
        self.environment = {
            "PHOENIX_DB_PATH": str(self.database),
            "TEST_STATUS_PATH": str(self.root / "status.json"),
            "TEST_EVENTS": str(self.root / "events"),
        }
        self.write_plist(candidate_plist)
        self.manifest = helper.Manifest(
            manifest_version=2, transaction_id="unit-test", source_kind="local_head",
            source_commit="a" * 40, expected=helper.Identity("1.0", "a" * 40),
            candidate_binary=str(binary), candidate_binary_sha256=helper.sha256(binary),
            candidate_plist=str(candidate_plist), candidate_plist_sha256=helper.sha256(candidate_plist),
            target_binary=str(self.root / "installed"), target_plist=str(self.root / "installed.plist"),
            label="test.phoenix", helper_label="com.phoenix-ide.deploy.unit-test", uid=os.getuid(),
            health_url="http://127.0.0.1:18111/api/version", health_insecure_tls=False,
            active_path=str(self.root / "active"), status_path=str(self.root / "status.json"),
            deployed_sha_path=str(self.root / "deployed.sha"), lock_path=str(self.root / "activate.lock"),
            claim_lock_path=str(self.root / "claim.lock"), created_at=helper.utc_now(),
        )
        Path(self.manifest.target_binary).write_bytes(b"previous binary")
        Path(self.manifest.target_plist).write_bytes(b"previous plist")
        Path(self.manifest.active_path).write_text(self.manifest.transaction_id)
        helper.write_status(self.manifest, "prepared")
        self.launch = FakeLaunchctl(self.manifest)

    def write_plist(self, path):
        path.write_bytes(plistlib.dumps({
            "Label": "test.phoenix", "ProgramArguments": [str(self.root / "installed")],
            "EnvironmentVariables": self.environment,
        }))

    def refresh_plist(self):
        path = Path(self.manifest.candidate_plist)
        self.write_plist(path)
        self.manifest = dataclasses.replace(self.manifest, candidate_plist_sha256=helper.sha256(path))

    def activate(self, health_error=None):
        with mock.patch.object(helper, "Launchctl", return_value=self.launch), mock.patch.object(
            helper, "wait_for_identity", side_effect=health_error,
        ):
            return helper.activate(self.manifest)

    def status(self):
        return json.loads(Path(self.manifest.status_path).read_text())

    def changes(self):
        with sqlite3.connect(self.database) as db:
            return db.execute("SELECT phase FROM changes").fetchall()

    def test_success_invokes_migration_once_after_durable_checkpoint_and_stop(self):
        previous_binary = self.root / "previous-phoenix-ide"
        previous_binary.write_bytes(b"previous binary")
        with mock.patch.object(sqlite3, "connect", side_effect=AssertionError("helper opened application DB")):
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.changes(), [("migration_started",)])
        self.assertEqual((self.root / "events").read_text().splitlines(), ["stop", "migrate", "start"])
        self.assertEqual(self.launch.starts, [self.manifest.candidate_plist])
        self.assertEqual((self.root / "previous.plist").read_bytes(), b"previous plist")
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), Path(self.manifest.candidate_plist).read_bytes())
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.candidate_plist))
        self.assertEqual(Path(self.manifest.target_plist).stat().st_mode & 0o777, 0o600)
        self.assertEqual(Path(self.manifest.deployed_sha_path).read_text(), "a" * 40 + "\n")
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertFalse(Path(self.manifest.candidate_binary).exists())
        self.assertFalse(previous_binary.exists())
        self.assertEqual(self.status()["state"], "committed")

    def test_success_cleanup_error_keeps_verified_runtime_running(self):
        candidate = Path(self.manifest.candidate_binary)
        unlink = Path.unlink

        def unlink_except_candidate(path, *args, **kwargs):
            if path == candidate:
                raise PermissionError("temporary binary is busy")
            return unlink(path, *args, **kwargs)

        with mock.patch.object(Path, "unlink", autospec=True, side_effect=unlink_except_candidate), \
             mock.patch("builtins.print") as output:
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.launch.stops, 1)
        self.assertEqual(self.launch.pid, 99)
        self.assertEqual(self.status()["state"], "committed")
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertTrue(candidate.is_file())
        self.assertTrue(any("left temporary binary" in str(call) for call in output.call_args_list))

    def test_committed_report_failure_preserves_successful_runtime_and_status(self):
        def report(*args, **_kwargs):
            if args == (f"{self.manifest.transaction_id}: committed",):
                raise OSError("activation log is full")

        with mock.patch("builtins.print", side_effect=report):
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.launch.stops, 1)
        self.assertEqual(self.launch.pid, 99)
        self.assertEqual(self.status()["state"], "committed")
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.candidate_plist))
        self.assertEqual(Path(self.manifest.deployed_sha_path).read_text(), "a" * 40 + "\n")

    def test_committed_cleanup_warning_failure_preserves_success(self):
        candidate = Path(self.manifest.candidate_binary)
        unlink = Path.unlink

        def unlink_except_candidate(path, *args, **kwargs):
            if path == candidate:
                raise PermissionError("temporary binary is busy")
            return unlink(path, *args, **kwargs)

        def report(*args, **_kwargs):
            if args and "left temporary binary" in args[0]:
                raise OSError("activation log is full")

        with mock.patch.object(Path, "unlink", autospec=True, side_effect=unlink_except_candidate), \
             mock.patch("builtins.print", side_effect=report):
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.launch.stops, 1)
        self.assertEqual(self.launch.pid, 99)
        self.assertEqual(self.status()["state"], "committed")
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertTrue(candidate.is_file())
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.candidate_plist))

    def test_committed_claim_release_failure_preserves_runtime_and_evidence(self):
        with mock.patch.object(helper, "release_claim", return_value=False), \
             mock.patch("builtins.print") as output:
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.launch.stops, 1)
        self.assertEqual(self.launch.pid, 99)
        self.assertEqual(self.status()["state"], "committed")
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertTrue(Path(self.manifest.candidate_binary).is_file())
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.candidate_plist))
        self.assertTrue(any("claim no longer matches" in str(call) for call in output.call_args_list))

    def test_committed_claim_unlink_fsync_and_warning_failures_preserve_runtime(self):
        active = Path(self.manifest.active_path)
        fsync_dir = helper.fsync_dir

        def fail_after_claim_unlink(path):
            if path == active.parent and not active.exists():
                raise OSError("claim directory fsync failed")
            return fsync_dir(path)

        def report(*args, **_kwargs):
            if args and "could not release its claim" in args[0]:
                raise OSError("activation log is full")

        with mock.patch.object(helper, "fsync_dir", side_effect=fail_after_claim_unlink), \
             mock.patch("builtins.print", side_effect=report):
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.launch.stops, 1)
        self.assertEqual(self.launch.pid, 99)
        self.assertEqual(self.status()["state"], "committed")
        self.assertFalse(active.exists())
        self.assertTrue(Path(self.manifest.candidate_binary).is_file())
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.candidate_plist))

    def test_partial_migration_failure_never_starts_a_runtime_or_restores_data(self):
        self.environment["FAIL_MIGRATION"] = "1"
        self.refresh_plist()
        self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.changes(), [("migration_started",)])
        self.assertEqual(self.launch.starts, [])
        self.assertIsNone(self.launch.pid)
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue(self.status()["failure"].startswith("migration_started:"))
        self.assertIn("exited 7", self.status()["failure"])
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), Path(self.manifest.candidate_binary).read_bytes())

    def test_migration_stderr_is_preserved_in_helper_output(self):
        self.environment["FAIL_MIGRATION"] = "1"
        self.refresh_plist()
        manifest_path = self.root / "manifest.json"
        manifest_path.write_text(json.dumps(dataclasses.asdict(self.manifest)))
        runner = '''import importlib.util, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("runner_helper", sys.argv[1])
h = importlib.util.module_from_spec(spec); sys.modules[spec.name] = h; spec.loader.exec_module(h)
class Launch:
    def __init__(self, manifest): pass
    def stop(self): return 41
h.Launchctl = Launch
result = h.activate(h.Manifest.load(Path(sys.argv[2])))
raise SystemExit(0 if result == "committed" else 1)
'''
        result = subprocess.run([sys.executable, "-c", runner, str(ROOT / "scripts/launchd_deploy_helper.py"),
                                 str(manifest_path)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("actual migration failure detail", result.stderr)
        self.assertIn("requires manual recovery", result.stderr)
        self.assertIn("running candidate migrations", result.stdout)

    def test_failed_health_stops_candidate_without_starting_predecessor(self):
        self.assertEqual(self.activate(helper.ActivationError("wrong candidate identity")), "failed")
        self.assertEqual(self.changes(), [("migration_started",)])
        self.assertEqual(self.launch.starts, [self.manifest.candidate_plist])
        self.assertEqual(self.launch.stops, 2)
        self.assertIsNone(self.launch.pid)
        self.assertTrue(self.status()["failure"].startswith("verifying:"))
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertFalse(Path(self.manifest.deployed_sha_path).exists())

    def test_teardown_error_is_reported_without_claiming_service_stopped(self):
        original_stop = self.launch.stop
        def stop():
            if self.launch.stops:
                raise helper.ActivationError("launchctl stop denied")
            return original_stop()
        self.launch.stop = stop
        self.assertEqual(self.activate(helper.ActivationError("unhealthy candidate")), "failed")
        self.assertIn("stopping failed: launchctl stop denied", self.status()["failure"])
        self.assertEqual(self.launch.pid, 99)
        self.assertTrue(Path(self.manifest.active_path).exists())

    def test_checksum_failure_leaves_current_runtime_artifacts_unchanged(self):
        Path(self.manifest.candidate_binary).write_bytes(b"damaged candidate")
        self.assertEqual(self.activate(), "precondition_failed")
        self.assertEqual(self.launch.stops, 0)
        self.assertEqual(self.launch.pid, 41)
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), b"previous binary")
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), b"previous plist")
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertEqual(self.changes(), [])

    def test_installer_staging_failure_happens_before_stop(self):
        with mock.patch.object(helper, "prepare_atomic_install", side_effect=OSError("no space")):
            self.assertEqual(self.activate(), "precondition_failed")
        self.assertEqual(self.launch.stops, 0)
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), b"previous plist")

    def test_wrong_embedded_identity_is_refused_before_stop(self):
        expected = helper.Identity("different-version", "a" * 40)
        self.manifest = dataclasses.replace(self.manifest, expected=expected)
        self.assertEqual(self.activate(), "precondition_failed")
        self.assertEqual(self.launch.stops, 0)

    def test_failed_migration_checkpoint_prevents_candidate_database_write(self):
        actual_write = helper.write_status
        def write(manifest, state, **kwargs):
            if state == "migration_started":
                raise OSError("cannot persist migration checkpoint")
            return actual_write(manifest, state, **kwargs)
        with mock.patch.object(helper, "write_status", side_effect=write):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.changes(), [])
        self.assertEqual(self.launch.starts, [])
        self.assertTrue(self.status()["failure"].startswith("migration_started:"))

    def test_failed_publication_stops_candidate_and_removes_autoload_plist(self):
        actual_write = helper.atomic_write
        def write(path, data, mode=0o600):
            if path == Path(self.manifest.deployed_sha_path):
                raise OSError("identity persistence failed")
            return actual_write(path, data, mode)
        with mock.patch.object(helper, "atomic_write", side_effect=write):
            self.assertEqual(self.activate(), "failed")
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertIsNone(self.launch.pid)
        self.assertTrue(self.status()["failure"].startswith("publishing:"))

    def test_pid_change_after_health_is_refused_and_candidate_is_stopped(self):
        def health(_manifest):
            self.launch.pid = 100
        with mock.patch.object(helper, "Launchctl", return_value=self.launch), \
             mock.patch.object(helper, "wait_for_identity", side_effect=health):
            self.assertEqual(helper.activate(self.manifest), "failed")
        self.assertIsNone(self.launch.pid)
        self.assertIn("PID changed", self.status()["failure"])
        self.assertFalse(Path(self.manifest.target_plist).exists())

    def test_migration_timeout_stops_without_starting_any_runtime(self):
        run = helper.subprocess.run
        def execute(command, **kwargs):
            if command[-1] == "--migrate-only":
                raise subprocess.TimeoutExpired(command, helper.MIGRATION_TIMEOUT_SECS)
            return run(command, **kwargs)
        with mock.patch.object(helper.subprocess, "run", side_effect=execute):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.changes(), [])
        self.assertEqual(self.launch.starts, [])
        self.assertIsNone(self.launch.pid)
        self.assertIn("migration_started:", self.status()["failure"])

    def test_concurrent_main_invocation_does_not_bootout_running_owner_helper(self):
        manifest_path = self.root / "manifest.json"
        manifest_path.write_text(json.dumps(dataclasses.asdict(self.manifest)))
        argv = ["helper", "--manifest", str(manifest_path), "--helper-label",
                self.manifest.helper_label, "--uid", str(os.getuid())]
        with mock.patch.object(sys, "argv", argv), \
             mock.patch.object(helper, "activate", side_effect=helper.ConcurrentDeploy("busy")), \
             mock.patch.object(helper, "request_helper_bootout") as bootout:
            self.assertEqual(helper.main(), 1)
        bootout.assert_not_called()

    def test_owned_claim_and_shared_operation_lock_are_required(self):
        Path(self.manifest.active_path).write_text("someone-else")
        with self.assertRaises(helper.ConcurrentDeploy):
            self.activate()
        self.assertEqual(Path(self.manifest.active_path).read_text(), "someone-else")
        Path(self.manifest.active_path).write_text(self.manifest.transaction_id)
        with Path(self.manifest.lock_path).open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(helper.ConcurrentDeploy):
                self.activate()
        self.assertEqual(self.launch.stops, 0)
        self.assertEqual(self.status()["state"], "prepared")

    def test_claim_release_never_removes_another_owner(self):
        Path(self.manifest.active_path).write_text("other")
        self.assertFalse(helper.release_claim(self.manifest))
        self.assertEqual(Path(self.manifest.active_path).read_text(), "other")

    def test_new_helper_refuses_old_manifest_without_mutation(self):
        manifest_path = self.root / "manifest.json"
        data = dataclasses.asdict(self.manifest) | {"manifest_version": 1}
        manifest_path.write_text(json.dumps(data))
        status_before = Path(self.manifest.status_path).read_bytes()
        with self.assertRaisesRegex(helper.ActivationError, "unsupported deployment manifest"):
            helper.Manifest.load(manifest_path)
        self.assertEqual(json.loads(manifest_path.read_text()), data)
        self.assertEqual(Path(self.manifest.status_path).read_bytes(), status_before)

    def test_manifest_validates_plist_source_and_identity_bindings(self):
        cases = [
            {"source_commit": "b" * 40},
            {"source_kind": "prepared_artifact"},
            {"source_kind": "published_release", "release_tag": "v1.0", "release_commit": "b" * 40},
            {"helper_label": self.manifest.label},
            {"target_binary": self.manifest.candidate_binary},
            {"health_url": "http://user:secret@localhost/api/version"},
        ]
        for replacement in cases:
            with self.subTest(replacement=replacement), self.assertRaises(helper.ActivationError):
                helper.validate_manifest_identities(dataclasses.replace(self.manifest, **replacement))
        path = Path(self.manifest.candidate_plist)
        plist = plistlib.loads(path.read_bytes())
        plist["ProgramArguments"] = ["/incorrect/binary"]
        path.write_bytes(plistlib.dumps(plist))
        self.manifest = dataclasses.replace(self.manifest, candidate_plist_sha256=helper.sha256(path))
        self.assertEqual(self.activate(), "precondition_failed")
        self.assertEqual(self.launch.stops, 0)

    def test_replaying_failed_transaction_is_refused(self):
        helper.write_status(self.manifest, "failed", failure="interrupted")
        with self.assertRaisesRegex(helper.ActivationError, "fresh prepared"):
            self.activate()
        self.assertEqual(self.launch.stops, 0)

    def test_health_verification_requires_exact_sha(self):
        self.manifest = dataclasses.replace(self.manifest, health_timeout_secs=0.5)
        with mock.patch.object(helper, "fetch_identity", return_value=helper.Identity("1.0", "a" * 12)), \
             mock.patch.object(helper.time, "monotonic", side_effect=[0, 0.1, 0.6]), \
             mock.patch.object(helper.time, "sleep"):
            with self.assertRaisesRegex(helper.ActivationError, "exact candidate health"):
                helper.wait_for_identity(self.manifest)

    def test_launchctl_absence_requires_recognized_diagnostic(self):
        missing = subprocess.CompletedProcess([], 113, "", 'Could not find service "test.phoenix" in domain gui/' + str(os.getuid()))
        with mock.patch.object(helper.subprocess, "run"):
            self.assertEqual(helper.Launchctl(self.manifest, run=lambda *a, **kw: missing).inspect(), ("not_loaded", None))
        denied = subprocess.CompletedProcess([], 1, "", "permission denied")
        with self.assertRaisesRegex(helper.ActivationError, "could not establish"):
            helper.Launchctl(self.manifest, run=lambda *a, **kw: denied).inspect()


if __name__ == "__main__":
    unittest.main()
