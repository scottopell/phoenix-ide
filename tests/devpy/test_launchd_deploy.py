import datetime
import fcntl
import importlib.util
import contextlib
import io
import json
import os
import plistlib
import subprocess
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


helper = load(ROOT / "scripts" / "launchd_deploy_helper.py", "launchd_deploy_helper_test")


class FakeClock:
    def __init__(self):
        self.now = 0.0

    def monotonic(self):
        return self.now

    def sleep(self, duration):
        self.now += duration


class FakeLaunchctl:
    events = []
    fail_start = False

    def __init__(self, manifest):
        self.manifest = manifest

    def inspect(self):
        return "running", 100

    def stop(self):
        self.events.append("stop")
        return 100

    def start(self, old_pid, **kwargs):
        self.events.append("start")
        if self.fail_start:
            raise helper.ActivationError("injected bootstrap failure")
        return 101


def make_manifest(root: Path, *, expected=None, previous=None):
    source_commit = "b" * 40
    expected = expected or helper.Identity("2.0.0", source_commit)
    previous = previous or helper.Identity("1.0.0", "a" * 12)
    files = {}
    for name, content in {
        "candidate_binary": b"new binary",
        "candidate_plist": b"<?xml version='1.0'?><plist version='1.0'><dict/></plist>",
        "rollback_binary": b"old binary",
        "rollback_plist": b"<?xml version='1.0'?><plist version='1.0'><dict/></plist>",
    }.items():
        path = root / name
        path.write_bytes(content)
        files[name] = path
    return helper.Manifest(
        manifest_version=helper.HANDOFF_PROTOCOL_VERSION,
        transaction_id="tx", source_kind="published_release", source_commit=source_commit,
        release_tag="v2.0.0", release_commit=source_commit,
        expected=expected, previous=previous,
        previous_deployed_sha="a" * 40 if previous is not None else None,
        candidate_binary=str(files["candidate_binary"]), candidate_binary_sha256=helper.sha256(files["candidate_binary"]),
        candidate_plist=str(files["candidate_plist"]), candidate_plist_sha256=helper.sha256(files["candidate_plist"]),
        rollback_binary=str(files["rollback_binary"]), rollback_binary_sha256=helper.sha256(files["rollback_binary"]),
        rollback_plist=str(files["rollback_plist"]), rollback_plist_sha256=helper.sha256(files["rollback_plist"]),
        target_binary=str(root / "live-binary"), target_plist=str(root / "live.plist"),
        label="test.phoenix.server", helper_label="test.phoenix.deploy", uid=os.getuid(), health_url="http://127.0.0.1:1/api/version",
        health_insecure_tls=False, active_path=str(root / "active"), status_path=str(root / "status.json"),
        previous_health_url="http://127.0.0.1:2/api/version", previous_health_insecure_tls=False,
        previous_health_json=True,
        deployed_sha_path=str(root / "deployed.sha"), lock_path=str(root / "activate.lock"),
        claim_lock_path=str(root / "claim.lock"),
        created_at="2026-01-01T00:00:00+00:00", transition_timeout_secs=0.1, health_timeout_secs=0.1,
    )


def make_paired_manifest(root: Path, database: Path) -> helper.Manifest:
    manifest = make_manifest(root)
    plist = plistlib.dumps({"EnvironmentVariables": {"PHOENIX_DB_PATH": str(database)}})
    for name in ("candidate_plist", "rollback_plist"):
        path = root / name
        path.write_bytes(plist)
    transaction = root / "transaction"
    transaction.mkdir(mode=0o700)
    copied_helper = transaction / "copied-helper.py"
    shutil.copy2(Path(helper.__file__), copied_helper)
    copied_helper.chmod(0o700)
    return helper.dataclasses.replace(
        manifest,
        source_kind="prepared_artifact",
        candidate_plist=str(root / "candidate_plist"),
        candidate_plist_sha256=helper.sha256(root / "candidate_plist"),
        rollback_plist=str(root / "rollback_plist"),
        rollback_plist_sha256=helper.sha256(root / "rollback_plist"),
        paired_database_upgrade=helper.PairedDatabaseUpgrade(
            database_path=str(database),
            backup_path=str(transaction / "backup.sqlite3"),
            proof_path=str(transaction / "proof.json"),
            controller_source_commit="c" * 40,
            controller_helper_sha256=helper.sha256(copied_helper),
            controller_helper_path=str(copied_helper),
        ),
    )


class ActivationTests(unittest.TestCase):
    def test_paired_sqlite_backup_is_private_and_context_bound(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            database = root / "legacy.db"
            with sqlite3.connect(database) as conn:
                conn.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
                conn.execute("INSERT INTO _migrations VALUES (69, 'legacy')")
                conn.execute("CREATE TABLE preserved (value TEXT)")
                conn.execute("INSERT INTO preserved VALUES ('original')")
            manifest = make_paired_manifest(root, database)
            copied_helper = Path(manifest.paired_database_upgrade.controller_helper_path)
            lsof = subprocess.CompletedProcess([], 1, "", "")
            with mock.patch.object(helper, "__file__", str(copied_helper)), \
                 mock.patch.object(helper.subprocess, "run", return_value=lsof):
                helper.validate_manifest_mode(manifest)
                helper.validate_legacy_database(database)
                helper.create_database_backup(manifest)
            backup = Path(manifest.paired_database_upgrade.backup_path)
            proof = json.loads(Path(manifest.paired_database_upgrade.proof_path).read_text())
            self.assertEqual("original", sqlite3.connect(backup).execute("SELECT value FROM preserved").fetchone()[0])
            self.assertEqual(manifest.transaction_id, proof["transaction_id"])
            self.assertEqual(0o600, backup.stat().st_mode & 0o777)
            self.assertEqual(0o700, backup.parent.stat().st_mode & 0o777)
            self.assertFalse((backup.parent / (backup.name + "-wal")).exists())
            self.assertFalse((backup.parent / (backup.name + "-shm")).exists())

    def test_paired_legacy_gate_rejects_product_conversations_without_mutation(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as td:
            database = Path(td) / "legacy.db"
            with sqlite3.connect(database) as conn:
                conn.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
                conn.execute("INSERT INTO _migrations VALUES (69, 'legacy')")
                conn.execute("CREATE TABLE product_conversations (id TEXT PRIMARY KEY)")
            with self.assertRaisesRegex(helper.ActivationError, "without ProductConversation"):
                helper.validate_legacy_database(database)
            self.assertIn("product_conversations", sqlite3.connect(database).execute("SELECT name FROM sqlite_master").fetchall()[1][0])
    def _paired_snapshot(self, root: Path):
        import sqlite3
        database = root / "legacy.db"
        with sqlite3.connect(database) as conn:
            conn.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            conn.execute("INSERT INTO _migrations VALUES (69, 'legacy')")
            conn.execute("CREATE TABLE preserved (value TEXT)")
            conn.execute("INSERT INTO preserved VALUES ('original')")
        manifest = make_paired_manifest(root, database)
        copied_helper = Path(manifest.paired_database_upgrade.controller_helper_path)
        with mock.patch.object(helper, "__file__", str(copied_helper)), \
             mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
            helper.validate_manifest_mode(manifest)
            helper.create_database_backup(manifest)
        return manifest, database

    def _full_paired_fixture(self, root: Path):
        import sqlite3

        database = root / "legacy.db"
        with sqlite3.connect(database) as conn:
            conn.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            conn.execute("INSERT INTO _migrations VALUES (69, 'legacy')")
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            conn.execute("INSERT INTO users VALUES (1, 'original')")
        manifest = make_paired_manifest(root, database)
        target_binary = Path(manifest.target_binary)
        target_plist = Path(manifest.target_plist)
        target_binary.write_bytes(Path(manifest.rollback_binary).read_bytes())
        target_plist.write_bytes(Path(manifest.rollback_plist).read_bytes())
        copied_helper = root / "transaction" / "copied-helper.py"
        shutil.copy2(Path(helper.__file__), copied_helper)
        copied_helper.chmod(0o700)
        paired = manifest.paired_database_upgrade
        assert paired is not None
        manifest = helper.dataclasses.replace(
            manifest,
            paired_database_upgrade=helper.dataclasses.replace(
                paired,
                controller_helper_path=str(copied_helper),
                controller_helper_sha256=helper.sha256(copied_helper),
            ),
        )
        return manifest, database, target_binary, target_plist, copied_helper

    def _activate_full_paired(self, root: Path, *, health_failure=None, stop_failure=None, lsof=None, corrupt_backup=False, predecessor_health_failure=None, predecessor_start_failure=None):
        import sqlite3

        manifest, database, target_binary, target_plist, copied_helper = self._full_paired_fixture(root)
        original_binary = target_binary.read_bytes()
        original_plist = target_plist.read_bytes()
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id + "\n")
        events = []

        class PairedFakeLaunchctl(FakeLaunchctl):
            def __init__(self, current):
                super().__init__(current)
                self.loaded = True
                self.pid = 100
                self.starts = 0

            def inspect(self):
                return ("running", self.pid) if self.loaded else ("not_loaded", None)

            def stop(self):
                events.append("stop")
                if stop_failure is not None and self.starts >= 1:
                    raise stop_failure
                old_pid = self.pid
                self.loaded = False
                return old_pid

            def start(self, old_pid, **kwargs):
                del old_pid
                self.starts += 1
                events.append("start")
                if kwargs.get("plist_path") is not None and Path(kwargs["plist_path"]) == Path(manifest.candidate_plist):
                    assert not target_plist.exists(), "unverified candidate must not be auto-loaded at login"
                    with sqlite3.connect(database) as conn:
                        conn.execute("INSERT INTO _migrations VALUES (70, 'product')")
                        conn.execute("CREATE TABLE product_conversations (id TEXT PRIMARY KEY)")
                        conn.execute("UPDATE users SET name = 'candidate' WHERE id = 1")
                elif self.starts == 2:
                    assert not target_plist.exists(), "unverified predecessor must not be auto-loaded during rollback verification"
                self.pid += 1
                self.loaded = True
                if self.starts == 2 and predecessor_start_failure is not None:
                    raise predecessor_start_failure
                return self.pid

        launchctl = PairedFakeLaunchctl(manifest)
        failure = health_failure
        def verify(_manifest, identity, **_kwargs):
            if identity == manifest.expected:
                if corrupt_backup:
                    Path(manifest.paired_database_upgrade.backup_path).write_bytes(b"corrupt")
                if failure is not None:
                    raise failure
            elif predecessor_health_failure is not None:
                raise predecessor_health_failure

        lsof_result = lsof or subprocess.CompletedProcess([], 1, "", "")
        with mock.patch.object(helper, "__file__", str(copied_helper)), \
             mock.patch.object(helper, "Launchctl", return_value=launchctl), \
             mock.patch.object(helper, "wait_for_identity", side_effect=verify), \
             mock.patch.object(helper.subprocess, "run", return_value=lsof_result):
            state = helper.activate(manifest)
        return state, manifest, database, target_binary, target_plist, original_binary, original_plist, claim, events, launchctl

    def test_full_paired_activate_success_preserves_candidate_database(self):
        import sqlite3

        with tempfile.TemporaryDirectory() as td:
            state, manifest, database, target_binary, target_plist, _old_binary, _old_plist, _claim, events, launchctl = self._activate_full_paired(Path(td))
            self.assertEqual("committed", state)
            self.assertEqual(["stop", "start"], events)
            self.assertEqual(1, launchctl.starts)
            with sqlite3.connect(database) as conn:
                self.assertEqual((70,), conn.execute("SELECT MAX(version) FROM _migrations").fetchone())
                self.assertIsNotNone(conn.execute("SELECT 1 FROM sqlite_master WHERE name = 'product_conversations'").fetchone())
                self.assertEqual(("candidate",), conn.execute("SELECT name FROM users WHERE id = 1").fetchone())
            self.assertEqual(b"new binary", target_binary.read_bytes())
            self.assertEqual(Path(manifest.candidate_plist).read_bytes(), target_plist.read_bytes())
            self.assertFalse(helper._restore_capacity_path(manifest, database).exists())
            self.assertTrue(Path(manifest.candidate_plist).samefile(target_plist))
            self.assertTrue(Path(manifest.paired_database_upgrade.backup_path).exists())
            self.assertTrue(Path(manifest.paired_database_upgrade.proof_path).exists())

    def test_baseexception_during_paired_activation_retains_claim_and_unpublished_plist(self):
        for phase in ("candidate_install", "candidate_start", "health"):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as td:
                manifest, _database, target_binary, target_plist, copied_helper = self._full_paired_fixture(Path(td))
                old_binary = target_binary.read_bytes()
                claim = Path(manifest.active_path)
                claim.write_text(manifest.transaction_id + "\n")
                events = []

                class UnloadedLaunchctl(FakeLaunchctl):
                    def inspect(self):
                        return ("running", 100) if not events or events[-1] != "stop" else ("not_loaded", None)

                    def stop(self):
                        events.append("stop")
                        return 100

                    def start(self, old_pid, **kwargs):
                        del old_pid, kwargs
                        raise BaseException("injected crash")

                launchctl = UnloadedLaunchctl(manifest)
                patch = (
                    mock.patch.object(helper, "commit_atomic_install", side_effect=BaseException("injected crash"))
                    if phase == "candidate_install"
                    else mock.patch.object(helper, "wait_for_identity", side_effect=BaseException("injected crash"))
                    if phase == "health"
                    else mock.patch.object(UnloadedLaunchctl, "start", side_effect=BaseException("injected crash"))
                )
                with mock.patch.object(helper, "__file__", str(copied_helper)), \
                     mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                     mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")), \
                     patch, self.assertRaisesRegex(BaseException, "injected crash"):
                    helper.activate(manifest)
                self.assertTrue(claim.exists())
                self.assertEqual(manifest.transaction_id, claim.read_text().strip())
                self.assertFalse(target_plist.exists())
                expected_binary = old_binary if phase == "candidate_install" else b"new binary"
                self.assertEqual(expected_binary, target_binary.read_bytes())
                self.assertEqual("activating", json.loads(Path(manifest.status_path).read_text())["state"])

    def test_postcommit_interruption_keeps_pending_diagnostic_and_claim(self):
        for phase in ("cleanup", "publication"):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                original_commit = helper.commit_atomic_install

                def crash_publication(prepared, target):
                    if Path(target).name == "live.plist":
                        raise SystemExit("postcommit crash")
                    return original_commit(prepared, target)

                patch = mock.patch.object(helper, "release_capacity_reservation", side_effect=SystemExit("postcommit crash")) if phase == "cleanup" else mock.patch.object(helper, "commit_atomic_install", side_effect=crash_publication)
                with patch, self.assertRaises(SystemExit):
                    self._activate_full_paired(root)
                status = json.loads((root / "status.json").read_text())
                self.assertEqual(status["state"], "committed")
                self.assertTrue(status["finalization_pending"])
                self.assertFalse(helper.status_is_durable_terminal(type("StatusOwner", (), {"transaction_id": status["transaction_id"], "status_path": str(root / "status.json"), "paired_database_upgrade": object()})()))
                self.assertIn("pending", status["committed_diagnostic"])
                self.assertFalse((root / "live.plist").exists())
                self.assertTrue((root / "active").exists())

    def test_recovery_checkpoint_preserves_new_database_writes_on_retry(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as td:
            _state, manifest, database, _binary, _plist, *_ = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate failed"))
            with sqlite3.connect(database) as conn:
                conn.execute("CREATE TABLE post_recovery_write(value TEXT)")
                conn.execute("INSERT INTO post_recovery_write VALUES ('keep')")
            status = json.loads(Path(manifest.status_path).read_text())
            status["state"] = "activation_failed_rollback_failed"
            Path(manifest.status_path).write_text(json.dumps(status))
            backend = mock.Mock()
            backend.inspect.return_value = ("running", 123)
            with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "restore_database") as restore_db:
                self.assertEqual(helper.recover_paired(manifest), "activation_failed_rolled_back")
                restore_db.assert_not_called()
                backend.start.assert_not_called()
                backend.stop.assert_not_called()
            with sqlite3.connect(database) as conn:
                self.assertEqual(conn.execute("SELECT value FROM post_recovery_write").fetchone()[0], "keep")

    def test_verified_predecessor_publication_failure_preserves_running_runtime(self):
        with tempfile.TemporaryDirectory() as td:
            _state, manifest, database, *_ = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate failed"))
            helper.write_status(manifest, "activation_failed_rollback_failed")
            backend = mock.Mock()
            backend.inspect.return_value = ("running", 101)
            before = database.read_bytes()
            with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "commit_atomic_install", side_effect=OSError("publication failed")), mock.patch.object(helper, "restore_database") as restored:
                self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
                backend.stop.assert_not_called()
                backend.start.assert_not_called()
                restored.assert_not_called()
            self.assertEqual(database.read_bytes(), before)
            self.assertTrue(Path(manifest.active_path).exists())

    def test_nonobject_recovery_status_is_rejected_before_disruption(self):
        for value in ("[]", "null", '"bad"'):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as td:
                manifest, *_ = self._full_paired_fixture(Path(td))
                Path(manifest.active_path).write_text(manifest.transaction_id)
                Path(manifest.status_path).write_text(value)
                with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl") as backend:
                    with self.assertRaises(helper.ActivationError):
                        helper.recover_paired(manifest)
                    backend.assert_not_called()
                helper.record_recovery_error(manifest, "malformed status")
                self.assertEqual(Path(manifest.status_path).read_text(), value)

    def test_loaded_plist_finalization_proof_rejects_lookalike_inode(self):
        with tempfile.TemporaryDirectory() as td:
            _state, manifest, _database, _binary, plist, *_ = self._activate_full_paired(Path(td))
            backend = helper.Launchctl(manifest)
            same_contents = Path(td) / "lookalike.plist"
            shutil.copy2(plist, same_contents)
            for loaded, valid in ((manifest.candidate_plist, True), (str(same_contents), False)):
                with mock.patch.object(backend, "run", return_value=subprocess.CompletedProcess([], 0, f"path = {loaded}\n", "")):
                    if valid:
                        helper.require_loaded_plist(manifest, backend, Path(manifest.candidate_plist))
                    else:
                        with self.assertRaises(helper.ActivationError):
                            helper.require_loaded_plist(manifest, backend, Path(manifest.candidate_plist))

    def test_finalization_retry_only_publishes_and_cleans_owned_capacity(self):
        for fail in (None, "publish", "cleanup", "claim", "identity"):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as td:
                state, manifest, database, binary, plist, *_ = self._activate_full_paired(Path(td))
                helper.write_status(manifest, "committed", finalization_pending=True)
                Path(manifest.active_path).write_text("other" if fail == "claim" else manifest.transaction_id)
                plist.unlink()
                reserved = helper._restore_capacity_path(manifest, database)
                reserved.write_bytes(b"temporary")
                before = database.read_bytes()
                backend = mock.Mock()
                backend.inspect.return_value = ("active", 101)
                absent = subprocess.CompletedProcess([], 113, "", f'Could not find service "{manifest.helper_label}" in domain gui/{manifest.uid}')
                with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("mismatch") if fail == "identity" else None), mock.patch.object(helper.subprocess, "run", return_value=absent), mock.patch.object(helper, "restore_database") as restore_db:
                    if fail == "publish":
                        with mock.patch.object(helper, "commit_atomic_install", side_effect=OSError("publish failed")), self.assertRaises(OSError):
                            helper.finalize_paired(manifest)
                        self.assertEqual(helper.finalize_paired(manifest), "committed")
                    elif fail == "cleanup":
                        with mock.patch.object(helper, "fsync_dir", side_effect=OSError("cleanup failed")), self.assertRaises(OSError):
                            helper.finalize_paired(manifest)
                        self.assertEqual(helper.finalize_paired(manifest), "committed")
                    elif fail in ("claim", "identity"):
                        with self.assertRaises(helper.ActivationError):
                            helper.finalize_paired(manifest)
                    else:
                        self.assertEqual(helper.finalize_paired(manifest), "committed")
                        self.assertEqual(helper.finalize_paired(manifest), "committed")
                        self.assertTrue(Path(manifest.candidate_plist).samefile(plist))
                        self.assertFalse(reserved.exists())
                        self.assertFalse(Path(manifest.active_path).exists())
                    restore_db.assert_not_called()
                    backend.start.assert_not_called()
                    backend.stop.assert_not_called()
                self.assertEqual(database.read_bytes(), before)
                if fail in ("claim", "identity"):
                    self.assertTrue(Path(manifest.active_path).exists())
                    self.assertTrue(json.loads(Path(manifest.status_path).read_text())["finalization_pending"])

    def test_private_bootstrap_publication_preserves_strict_restart_configuration(self):
        from types import SimpleNamespace
        restart = load(ROOT / "scripts" / "launchd_restart_helper.py", "paired_restart_seam_test")
        for health_failure in (None, helper.ActivationError("health failed")):
            with self.subTest(rollback=health_failure is not None), tempfile.TemporaryDirectory() as td:
                _state, manifest, _db, binary, plist, *_ = self._activate_full_paired(Path(td), health_failure=health_failure)
                loaded_path = manifest.candidate_plist if health_failure is None else manifest.rollback_plist
                restart_manifest = SimpleNamespace(plist_path=str(plist), binary_path=str(binary), socket_service=1, uid=manifest.uid, label=manifest.label)
                job = restart.LoadedJob("running", 101, True, loaded_path, str(binary), ("1",))
                backend = restart.Launchctl(restart_manifest)
                self.assertTrue(backend.configuration_matches(job))
                copy = Path(td) / "lookalike.plist"
                shutil.copy2(plist, copy)
                self.assertFalse(backend.configuration_matches(restart.dataclasses.replace(job, plist_path=str(copy))))

    def test_postcommit_cleanup_or_publication_failure_never_rolls_back(self):
        for failing_step in ("release_capacity_reservation", "publication"):
            with self.subTest(step=failing_step), tempfile.TemporaryDirectory() as td:
                original = helper.commit_atomic_install

                def fail_publication(prepared, target):
                    if Path(target).name == "live.plist":
                        raise OSError("publication failure")
                    return original(prepared, target)

                patch = mock.patch.object(helper, "release_capacity_reservation", side_effect=OSError("cleanup failure")) if failing_step == "release_capacity_reservation" else mock.patch.object(helper, "commit_atomic_install", side_effect=fail_publication)
                with patch:
                    state, manifest, _database, binary, _plist, *_rest, events, launchctl = self._activate_full_paired(Path(td))
                self.assertEqual(state, "committed")
                self.assertEqual(events, ["stop", "start"])
                self.assertEqual(binary.read_bytes(), b"new binary")
                self.assertTrue(launchctl.loaded)
                self.assertTrue(json.loads(Path(manifest.status_path).read_text())["committed_diagnostic"])
                self.assertTrue(json.loads(Path(manifest.status_path).read_text())["finalization_pending"])
                self.assertFalse(helper.status_is_durable_terminal(manifest))
                self.assertTrue(Path(manifest.active_path).exists())
                absent = subprocess.CompletedProcess([], 113, "", f'Could not find service "{manifest.helper_label}" in domain gui/{manifest.uid}')
                backend = mock.Mock()
                backend.inspect.return_value = ("running", 101)
                with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper.subprocess, "run", return_value=absent):
                    self.assertEqual(helper.finalize_paired(manifest), "committed")
                self.assertFalse(Path(manifest.active_path).exists())
                self.assertFalse(json.loads(Path(manifest.status_path).read_text())["finalization_pending"])
                backend.stop.assert_not_called()
                backend.start.assert_not_called()

    def test_full_paired_activate_health_failure_restores_database_and_predecessor(self):
        import sqlite3

        with tempfile.TemporaryDirectory() as td:
            result = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("forced health failure"))
            state, manifest, database, target_binary, target_plist, old_binary, old_plist, claim, events, launchctl = result
            self.assertEqual("activation_failed_rolled_back", state)
            self.assertEqual(["stop", "start", "stop", "start"], events)
            self.assertEqual(2, launchctl.starts)
            with sqlite3.connect(database) as conn:
                self.assertEqual((69,), conn.execute("SELECT MAX(version) FROM _migrations").fetchone())
                self.assertIsNone(conn.execute("SELECT 1 FROM sqlite_master WHERE name = 'product_conversations'").fetchone())
                self.assertEqual(("original",), conn.execute("SELECT name FROM users WHERE id = 1").fetchone())
            self.assertEqual(old_binary, target_binary.read_bytes())
            self.assertEqual(old_plist, target_plist.read_bytes())
            self.assertTrue(claim.exists(), "failed rollback must retain the active claim")
            self.assertTrue(Path(manifest.rollback_plist).samefile(target_plist))
            self.assertEqual(manifest.transaction_id, claim.read_text().strip())
            self.assertEqual("activation_failed_rolled_back", json.loads(Path(manifest.status_path).read_text())["state"])

    def test_paired_failed_predecessor_verification_or_loaded_transition_tears_down_job(self):
        for failure_option in ("predecessor_health_failure", "predecessor_start_failure"):
            with self.subTest(failure_option=failure_option), tempfile.TemporaryDirectory() as td:
                result = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate health failed"), **{failure_option: helper.ActivationError("recovery verification failed")})
                state, manifest, *_rest, events, launchctl = result
                self.assertEqual(state, "activation_failed_rollback_failed")
                self.assertEqual(events, ["stop", "start", "stop", "start", "stop"])
                self.assertFalse(launchctl.loaded)
                self.assertFalse(helper.status_is_durable_terminal(manifest))
                self.assertFalse(Path(manifest.target_plist).exists())
                quarantined = list(Path(manifest.paired_database_upgrade.proof_path).parent.glob("*.plist.quarantined"))
                self.assertTrue(quarantined)
                self.assertTrue(all(p.stat().st_mode & 0o777 == 0o600 for p in quarantined))
                status = json.loads(Path(manifest.status_path).read_text())
                self.assertNotIn("recovery teardown failed", status["rollback_failure"])

    def test_checkpointed_recovery_refuses_snapshot_replay_without_running_predecessor(self):
        with tempfile.TemporaryDirectory() as td:
            result = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate failed"), predecessor_health_failure=helper.ActivationError("recovery failed"))
            _state, manifest, database, *_rest, launchctl = result
            status = json.loads(Path(manifest.status_path).read_text())
            status["state"] = "activating"
            Path(manifest.status_path).write_text(json.dumps(status))
            with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "restore_deployed_sha"), mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
                with mock.patch.object(helper, "restore_database") as restore_db:
                    self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
                    restore_db.assert_not_called()
                self.assertFalse(helper.status_is_durable_terminal(manifest))
            self.assertTrue(Path(manifest.active_path).exists())
            import sqlite3
            self.assertFalse(Path(manifest.target_plist).exists())
            with sqlite3.connect(database) as conn:
                self.assertEqual(conn.execute("SELECT MAX(version) FROM _migrations").fetchone()[0], 69)

    def test_recovery_entrypoint_errors_preserve_prior_diagnostics_and_claim(self):
        for exception in (helper.ActivationError("early proof rejected"), helper.ConcurrentDeploy("recovery lock occupied")):
            with self.subTest(exception=type(exception).__name__), tempfile.TemporaryDirectory() as td:
                manifest, *_ = self._full_paired_fixture(Path(td))
                Path(manifest.active_path).write_text(manifest.transaction_id + "\n")
                helper.write_status(manifest, "activation_failed_rollback_failed", failure="original activation", rollback_failure="prior restore failure")
                args = ["helper", "recover-paired", "--manifest", "unused", "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
                with mock.patch.object(helper.sys, "argv", args), mock.patch.object(helper.Manifest, "load", return_value=manifest), mock.patch.object(helper, "recover_paired", side_effect=exception), mock.patch.object(helper, "request_helper_bootout"):
                    self.assertEqual(helper.main(), 1)
                status = json.loads(Path(manifest.status_path).read_text())
                self.assertEqual(status["state"], "activation_failed_rollback_failed")
                self.assertEqual(status["failure"], "original activation")
                self.assertIn("prior restore failure", status["rollback_failure"])
                self.assertIn(str(exception), status["rollback_failure"])
                self.assertEqual(Path(manifest.active_path).read_text().strip(), manifest.transaction_id)

    def test_repeated_recovery_failure_retains_claim_and_stops_unverified_runtime(self):
        with tempfile.TemporaryDirectory() as td:
            result = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate failed"), predecessor_health_failure=helper.ActivationError("recovery failed"))
            _state, manifest, _database, *_rest, backend = result
            with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("recovery failed again")), mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
                self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
            self.assertFalse(backend.loaded)
            self.assertEqual(Path(manifest.active_path).read_text().strip(), manifest.transaction_id)
            self.assertFalse(helper.status_is_durable_terminal(manifest))

    def test_interrupted_pre_snapshot_recovery_only_restores_unchanged_legacy(self):
        for phase in ("prepared", "activating", None):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as td:
                manifest, database, _binary, _plist, copied_helper = self._full_paired_fixture(Path(td))
                Path(manifest.active_path).write_text(manifest.transaction_id + "\n")
                reserved = helper.reserve_database_capacity(manifest)
                if phase is not None:
                    helper.write_status(manifest, phase)
                backend = FakeLaunchctl(manifest)
                backend.inspect = mock.Mock(return_value=("not_loaded", None))
                backend.events = []
                with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "restore_deployed_sha"), mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
                    if phase in ("prepared", None):
                        with self.assertRaises(helper.ActivationError):
                            helper.recover_paired(manifest)
                        self.assertEqual(backend.events, [])
                        continue
                    outcome = helper.recover_paired(manifest)
                    self.assertEqual(outcome, "activation_failed_rolled_back", Path(manifest.status_path).read_text())
                    self.assertFalse(Path(manifest.paired_database_upgrade.proof_path).exists())
                    self.assertTrue(helper.status_is_durable_terminal(manifest))
                    self.assertFalse(reserved.backup_path.exists())
                    self.assertFalse(reserved.restore_path.exists())
                    self.assertTrue(Path(manifest.rollback_plist).samefile(_plist))

    def test_paired_capacity_failure_occurs_before_any_stop(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(helper, "reserve_database_capacity", side_effect=helper.ActivationError("insufficient capacity")), mock.patch.object(helper, "Launchctl") as backend:
            manifest, _db, _bin, _plist, copied_helper = self._full_paired_fixture(Path(td))
            with mock.patch.object(helper, "__file__", str(copied_helper)), self.assertRaisesRegex(helper.ActivationError, "insufficient capacity"):
                helper.activate(manifest)
            backend.assert_not_called()
            self.assertEqual(json.loads(Path(manifest.status_path).read_text())["state"], "precondition_failed")

    def test_wal_growth_capacity_failure_resumes_verified_unchanged_predecessor(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(
            helper, "_reservation_still_sufficient", side_effect=helper.ActivationError("paired database grew beyond its reserved capacity")
        ):
            result = self._activate_full_paired(Path(td))
            state, _manifest, database, target_binary, target_plist, old_binary, old_plist, _claim, events, launchctl = result
            self.assertEqual("activation_failed_rolled_back", state)
            self.assertEqual(["stop", "stop", "start"], events)
            self.assertEqual(1, launchctl.starts)
            self.assertEqual(old_binary, target_binary.read_bytes())
            self.assertEqual(old_plist, target_plist.read_bytes())
            self.assertTrue(database.exists())
            self.assertFalse(helper._restore_capacity_path(_manifest, database).exists())
            self.assertFalse(Path(_manifest.paired_database_upgrade.backup_path).exists())

    def test_no_snapshot_fallback_refuses_modern_database_changed_runtime_config_or_lsof(self):
        cases = ("modern_database", "changed_binary", "changed_plist", "uncertain_lsof")
        for case in cases:
            with self.subTest(case=case), tempfile.TemporaryDirectory() as td:
                manifest, database, target_binary, target_plist, copied_helper = self._full_paired_fixture(Path(td))
                launchctl = mock.Mock()
                launchctl.stop.return_value = 100
                launchctl.inspect.return_value = ("not_loaded", None)
                if case == "modern_database":
                    with __import__("sqlite3").connect(database) as conn:
                        conn.execute("INSERT INTO _migrations VALUES (70, 'modern')")
                elif case == "changed_binary":
                    target_binary.write_bytes(b"changed")
                elif case == "changed_plist":
                    target_plist.write_bytes(b"changed")
                lsof = subprocess.CompletedProcess([], 1, "", "warning") if case == "uncertain_lsof" else subprocess.CompletedProcess([], 1, "", "")
                with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper.subprocess, "run", return_value=lsof):
                    with self.assertRaises(helper.ActivationError):
                        helper.restore(manifest, launchctl, database_snapshot=False, candidate_binary_mutated=False)
                launchctl.start.assert_not_called()

    def test_real_reservation_enospc_is_reported_before_production_stop(self):
        with tempfile.TemporaryDirectory() as td:
            manifest, *_ = self._full_paired_fixture(Path(td))
            with mock.patch.object(helper.os, "open", side_effect=OSError(28, "No space left on device")), self.assertRaisesRegex(helper.ActivationError, "reserve paired database capacity"):
                helper.reserve_database_capacity(manifest)

    def test_each_reservation_creation_fsyncs_its_parent(self):
        with tempfile.TemporaryDirectory() as td:
            manifest, *_ = self._full_paired_fixture(Path(td))
            with mock.patch.object(helper, "fsync_dir") as sync:
                reserved = helper.reserve_database_capacity(manifest)
            sync.assert_any_call(reserved.backup_path.parent)
            sync.assert_any_call(reserved.restore_path.parent)

    def test_capacity_reservation_includes_wal_and_is_consumed_by_sqlite_backup(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as td:
            manifest, database, *_rest, copied_helper = self._full_paired_fixture(Path(td))
            wal = Path(str(database) + "-wal")
            wal.write_bytes(b"\0" * 32768)
            reserved = helper.reserve_database_capacity(manifest)
            self.assertGreaterEqual(reserved.capacity_bytes, database.stat().st_size + wal.stat().st_size)
            self.assertEqual(reserved.backup_path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(reserved.restore_path.stat().st_mode & 0o777, 0o600)
            wal.unlink()
            with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
                helper.create_database_backup(manifest, reserved)
            with sqlite3.connect(reserved.backup_path) as conn:
                self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone(), ("ok",))
                self.assertEqual(conn.execute("SELECT MAX(version) FROM _migrations").fetchone()[0], 69)

    def test_snapshot_failure_resumes_only_verified_unchanged_predecessor(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(
            helper, "create_database_backup", side_effect=helper.ActivationError("snapshot failed")
        ):
            result = self._activate_full_paired(Path(td))
        state, _manifest, _database, _target_binary, _target_plist, _old_binary, _old_plist, _claim, events, launchctl = result
        self.assertEqual("activation_failed_rolled_back", state)
        self.assertEqual(["stop", "stop", "start"], events)
        self.assertEqual(1, launchctl.starts)

    def test_full_paired_activate_restore_corruption_does_not_start_predecessor(self):
        with tempfile.TemporaryDirectory() as td:
            result = self._activate_full_paired(
                Path(td), health_failure=helper.ActivationError("forced health failure"), corrupt_backup=True
            )
        state, _manifest, _database, _target_binary, _target_plist, _old_binary, _old_plist, _claim, events, launchctl = result
        self.assertEqual("activation_failed_rollback_failed", state)
        self.assertEqual(["stop", "start", "stop", "stop"], events)
        self.assertEqual(1, launchctl.starts)

    def test_full_paired_activate_candidate_stop_failure_does_not_start_predecessor(self):
        with tempfile.TemporaryDirectory() as td:
            result = self._activate_full_paired(
                Path(td), health_failure=helper.ActivationError("forced health failure"),
                stop_failure=helper.ActivationError("candidate stop failed"),
            )
        state, _manifest, _database, _target_binary, _target_plist, _old_binary, _old_plist, _claim, events, launchctl = result
        self.assertEqual("activation_failed_rollback_failed", state)
        self.assertEqual(["stop", "start", "stop", "stop"], events)
        self.assertEqual(1, launchctl.starts)

    def test_full_paired_activate_lsof_warning_after_stop_does_not_start_predecessor(self):
        with tempfile.TemporaryDirectory() as td:
            result = self._activate_full_paired(
                Path(td), lsof=subprocess.CompletedProcess([], 1, "", "lsof warning after stop")
            )
        state, _manifest, _database, _target_binary, _target_plist, _old_binary, _old_plist, _claim, events, launchctl = result
        self.assertEqual("activation_failed_rollback_failed", state)
        self.assertEqual(["stop", "stop", "stop"], events)
        self.assertEqual(0, launchctl.starts)

    def test_paired_lsof_warning_cannot_prove_ownership_without_leaking_stderr(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest, _database = self._paired_snapshot(root)
            warning = "lsof: warning: /private/secret/token"
            with mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", warning)):
                with self.assertRaisesRegex(helper.ActivationError, "could not prove database exclusivity") as raised:
                    helper.assert_database_exclusive(manifest)
            self.assertNotIn("secret", str(raised.exception))

    def test_corrupt_backup_and_proof_context_fail_without_restarting_predecessor(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest, database = self._paired_snapshot(root)
            backup = Path(manifest.paired_database_upgrade.backup_path)
            backup.write_bytes(b"corrupt")
            launchctl = mock.Mock()
            launchctl.stop.return_value = None
            launchctl.inspect.return_value = ("not_loaded", None)
            with mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
                with self.assertRaises(helper.ActivationError):
                    helper.restore(manifest, launchctl)
            launchctl.start.assert_not_called()
            proof = Path(manifest.paired_database_upgrade.proof_path)
            proof.write_text(json.dumps({"transaction_id": "wrong"}))
            with mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
                with self.assertRaisesRegex(helper.ActivationError, "proof"):
                    helper.restore_database(manifest)
            self.assertTrue(database.exists())

    def test_candidate_stop_failure_never_starts_predecessor_in_paired_mode(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest, _database = self._paired_snapshot(root)
            launchctl = mock.Mock()
            launchctl.stop.side_effect = helper.ActivationError("stop failed")
            with self.assertRaises(helper.ActivationError):
                helper.restore(manifest, launchctl)
            launchctl.start.assert_not_called()

    def test_paired_helper_binding_rejects_arbitrary_helper_file(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            database = root / "legacy.db"
            database.write_bytes(b"sqlite")
            manifest = make_paired_manifest(root, database)
            paired = manifest.paired_database_upgrade
            assert paired is not None
            bad = root / "lookalike-helper.py"
            bad.write_bytes(Path(helper.__file__).read_bytes())
            manifest = helper.dataclasses.replace(
                manifest,
                paired_database_upgrade=helper.dataclasses.replace(
                    paired, controller_helper_path=str(bad), controller_helper_sha256=helper.sha256(bad)
                ),
            )
            with self.assertRaisesRegex(helper.ActivationError, "not the running helper"):
                helper.validate_manifest_mode(manifest)

    def test_snapshot_failure_preserves_existing_backup_and_proof(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            database = root / "legacy.db"
            database.write_bytes(b"not sqlite")
            manifest = make_paired_manifest(root, database)
            paired = manifest.paired_database_upgrade
            assert paired is not None
            backup = Path(paired.backup_path)
            proof = Path(paired.proof_path)
            backup.write_bytes(b"old backup")
            proof.write_text("old proof")
            with mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
                with self.assertRaises(helper.ActivationError):
                    helper.create_database_backup(manifest)
            self.assertEqual(backup.read_bytes(), b"old backup")
            self.assertEqual(proof.read_text(), "old proof")

    def test_manifest_rejects_short_candidate_before_disruption(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(
                Path(td), expected=helper.Identity("2.0.0", "b" * 12)
            )
            with self.assertRaisesRegex(helper.ActivationError, "exact full source commit"):
                helper.activate(manifest)
            self.assertFalse(Path(manifest.status_path).exists() and "activating" in Path(manifest.status_path).read_text())

    def test_manifest_accepts_legacy_previous_only_in_rollback_role(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            helper.validate_manifest_identities(manifest)
            self.assertEqual(12, len(manifest.previous.git_sha))

    def test_manifest_rejects_malformed_previous_identity(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(
                Path(td), previous=helper.Identity("1.0.0", "a" * 13)
            )
            with self.assertRaisesRegex(helper.ActivationError, "previous runtime identity"):
                helper.validate_manifest_identities(manifest)
    def test_missing_service_text_is_treated_as_unloaded(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            result = subprocess.CompletedProcess([], 113, "", f'Could not find service "{manifest.label}" in domain for user gui: {manifest.uid}')
            launchctl = helper.Launchctl(manifest, run=mock.Mock(return_value=result))
            self.assertEqual(("not_loaded", None), launchctl.inspect())
    def test_rejected_recovery_never_changes_committed_or_preparing_status(self):
        for state in ("committed", "preparing"):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as td:
                manifest, *_ = self._full_paired_fixture(Path(td))
                helper.write_status(manifest, state, finalization_pending=state == "committed")
                before = Path(manifest.status_path).read_bytes()
                helper.record_recovery_error(manifest, "rejected state")
                self.assertEqual(Path(manifest.status_path).read_bytes(), before)

    def test_absence_probe_loads_without_sqlite_module(self):
        import builtins
        original = builtins.__import__
        def without_sqlite(name, *args, **kwargs):
            if name == "sqlite3":
                raise ImportError("optional sqlite unavailable")
            return original(name, *args, **kwargs)
        with mock.patch.object(builtins, "__import__", side_effect=without_sqlite):
            probe = load(ROOT / "scripts" / "launchd_deploy_helper.py", "sqlite_free_absence_probe")
            with mock.patch.object(probe.sys, "argv", ["helper", "--probe-service-absence", "dev.test", "--uid", "501"]), mock.patch.object(probe.subprocess, "run", return_value=subprocess.CompletedProcess([], 113, "", 'Could not find service "dev.test" in domain gui/501')):
                self.assertEqual(probe.main(), 0)

    def test_absence_probe_requires_nonzero_print_and_exact_target_diagnostic(self):
        label, uid = "dev.phoenix.activation.test", 501
        diagnostic = f'Could not find service "{label}" in domain gui/{uid}'
        for code, output, expected in ((0, diagnostic, 1), (113, diagnostic, 0), (5, "I/O error", 1)):
            with self.subTest(code=code), mock.patch.object(helper.sys, "argv", ["helper", "--probe-service-absence", label, "--uid", str(uid)]), mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], code, "", output)):
                self.assertEqual(helper.main(), expected)

    def test_launchctl_inspect_accepts_both_precise_absence_spellings(self):
        for absence in (
            f'Could not find service "{{label}}" in domain gui/{{uid}}',
            f'Could not find service "{{label}}" in domain for user gui: {{uid}}',
        ):
            with self.subTest(absence=absence), tempfile.TemporaryDirectory() as td:
                manifest = make_manifest(Path(td))
                output = absence.format(label=manifest.label, uid=manifest.uid)
                run = mock.Mock(return_value=subprocess.CompletedProcess([], 113, "", output))
                launchctl = helper.Launchctl(manifest, run=run)
                self.assertEqual(("not_loaded", None), launchctl.inspect())
                run.assert_called_once_with(
                    ["launchctl", "print", f"gui/{manifest.uid}/{manifest.label}"],
                    capture_output=True,
                    text=True,
                )

    def test_launchctl_rejects_wrong_label_uid_and_unknown_print_errors(self):
        cases = (
            'Could not find service "unrelated" in domain for user gui: {uid}',
            'Could not find service "{label}" in domain for user gui: 99999',
            "Input/output error",
        )
        for detail in cases:
            with self.subTest(detail=detail), tempfile.TemporaryDirectory() as td:
                manifest = make_manifest(Path(td))
                result = subprocess.CompletedProcess([], 113 if "Could not" in detail else 5, "", detail.format(label=manifest.label, uid=manifest.uid))
                launchctl = helper.Launchctl(manifest, run=mock.Mock(return_value=result))
                with self.assertRaisesRegex(helper.ActivationError, "absence is unconfirmed"):
                    launchctl.inspect()

    def test_launchctl_stop_targets_service_when_target_plist_is_missing(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            Path(manifest.target_plist).unlink(missing_ok=True)
            calls = []
            absent = f'Could not find service "{manifest.label}" in domain gui/{manifest.uid}'

            def run(command, **_kwargs):
                calls.append(command)
                if command[1] == "print":
                    if len([call for call in calls if call[1] == "print"]) == 1:
                        return subprocess.CompletedProcess(command, 0, "state = running\npid = 42\n", "")
                    return subprocess.CompletedProcess(command, 113, "", absent)
                return subprocess.CompletedProcess(command, 0, "", "")

            launchctl = helper.Launchctl(manifest, run=run)
            self.assertEqual(42, launchctl.stop())
            self.assertEqual(["launchctl", "bootout", f"gui/{manifest.uid}/{manifest.label}"], calls[1])
            self.assertNotIn(str(manifest.target_plist), calls[1])

    def test_launchctl_start_uses_private_bootstrap_plist(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            private_plist = Path(td) / "private-candidate.plist"
            private_plist.write_bytes(b"candidate")
            calls = []

            def run(command, **_kwargs):
                calls.append(command)
                if command[1] == "bootstrap":
                    return subprocess.CompletedProcess(command, 0, "", "")
                return subprocess.CompletedProcess(command, 0, "state = active\npid = 43\n", "")

            launchctl = helper.Launchctl(manifest, run=run)
            self.assertEqual(43, launchctl.start(42, plist_path=str(private_plist)))
            self.assertEqual(
                ["launchctl", "bootstrap", f"gui/{manifest.uid}", str(private_plist)],
                calls[0],
            )


    def test_unknown_launchctl_print_error_prevents_paired_restore(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_paired_manifest(Path(td), Path(td) / "database.sqlite3")
            result = subprocess.CompletedProcess([], 5, "", "Input/output error")
            launchctl = helper.Launchctl(manifest, run=mock.Mock(return_value=result))
            with mock.patch.object(helper, "restore_database") as restore_database, mock.patch.object(helper, "atomic_install") as install:
                with self.assertRaisesRegex(helper.ActivationError, "absence is unconfirmed"):
                    helper.restore(manifest, launchctl)
                restore_database.assert_not_called()
                install.assert_not_called()
            wrong_service = subprocess.CompletedProcess([], 113, "", 'Could not find service "unrelated" in domain for user gui: 501')
            with mock.patch.object(launchctl, "run", return_value=wrong_service), self.assertRaises(helper.ActivationError):
                launchctl.inspect()

    def test_bootout_timeout_marks_disruption_and_triggers_rollback(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = mock.Mock()
            launchctl.disruption_started = False
            def stop():
                launchctl.disruption_started = True
                raise helper.ActivationError("teardown timeout")
            launchctl.stop.side_effect = stop
            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(helper, "restore") as restore:
                state = helper.activate(manifest)
            self.assertEqual("activation_failed_rolled_back", state)
            restore.assert_called_once()
            self.assertEqual((manifest, launchctl), restore.call_args.args[:2])
            self.assertEqual(2, len(restore.call_args.args[2]))

    def setUp(self):
        FakeLaunchctl.events = []
        FakeLaunchctl.fail_start = False

    def test_success_installs_atomically_and_records_selected_commit_after_verification(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            events = FakeLaunchctl.events
            def verified(_manifest, identity):
                events.append(f"verified:{identity.git_sha}")
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=verified), \
                 mock.patch.object(helper, "fsync_dir", wraps=helper.fsync_dir) as fsync_dir, \
                 mock.patch.object(helper.os, "replace", wraps=os.replace) as replace:
                state = helper.activate(manifest)
            self.assertEqual("committed", state)
            self.assertEqual("b" * 40 + "\n", Path(manifest.deployed_sha_path).read_text())
            self.assertEqual(["stop", "start", f"verified:{'b' * 40}"], events)
            live_binary_replaces = [call for call in replace.call_args_list if Path(call.args[1]) == Path(manifest.target_binary)]
            self.assertEqual(1, len(live_binary_replaces))
            self.assertNotEqual(Path(manifest.target_binary), Path(live_binary_replaces[0].args[0]))
            self.assertTrue(any(call.args[0] == Path(manifest.target_binary).parent for call in fsync_dir.call_args_list))
            self.assertEqual(b"new binary", Path(manifest.target_binary).read_bytes())

    def test_failed_first_install_stops_candidate_and_removes_artifacts(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(root, previous=None)
            manifest = helper.dataclasses.replace(
                manifest, previous=None, rollback_binary=None, rollback_binary_sha256=None,
                previous_deployed_sha=None,
                rollback_plist=None, rollback_plist_sha256=None,
            )
            launchctl = FakeLaunchctl(manifest)
            launchctl.inspect = lambda: ("not_loaded", None) if FakeLaunchctl.events.count("stop") >= 2 else ("running", 100)
            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("bad health")):
                state = helper.activate(manifest)
            self.assertEqual("activation_failed_rolled_back", state)
            self.assertEqual(["stop", "start", "stop"], FakeLaunchctl.events)
            self.assertFalse(Path(manifest.target_binary).exists())
            self.assertFalse(Path(manifest.target_plist).exists())
            self.assertFalse(Path(manifest.deployed_sha_path).exists())

    def test_wrong_version_rolls_back_and_has_distinct_status(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            identities = []
            def verify(_manifest, identity, **_kwargs):
                identities.append(identity.git_sha)
                if identity == manifest.expected:
                    raise helper.ActivationError("wrong version")
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=verify):
                state = helper.activate(manifest)
            self.assertEqual("activation_failed_rolled_back", state)
            self.assertEqual(["b" * 40, "a" * 12], identities)
            self.assertEqual("a" * 40 + "\n", Path(manifest.deployed_sha_path).read_text())
            self.assertEqual(b"old binary", Path(manifest.target_binary).read_bytes())
            self.assertEqual(state, json.loads(Path(manifest.status_path).read_text())["state"])

    def test_rollback_restores_previous_deployed_sha_after_commit_status_failure(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            Path(manifest.deployed_sha_path).write_text("candidate-sha\n")
            launchctl = FakeLaunchctl(manifest)
            with mock.patch.object(helper, "wait_for_identity"):
                helper.restore(manifest, launchctl)
            self.assertEqual("a" * 40 + "\n", Path(manifest.deployed_sha_path).read_text())

    def test_failed_rollback_is_explicit(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("health timeout")):
                state = helper.activate(manifest)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("activation_failed_rollback_failed", state)
            self.assertIn("health timeout", status["failure"])
            self.assertIn("health timeout", status["rollback_failure"])

    def test_concurrent_activation_rejected_before_stop(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            lock = open(manifest.lock_path, "w")
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                with self.assertRaises(helper.ConcurrentDeploy):
                    helper.activate(manifest)
            finally:
                lock.close()
            self.assertEqual([], FakeLaunchctl.events)

    def test_tampered_candidate_fails_before_disruption(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            Path(manifest.candidate_binary).write_text("tampered")
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl):
                with self.assertRaises(helper.ActivationError):
                    helper.activate(manifest)
            self.assertEqual([], FakeLaunchctl.events)

    def test_install_space_is_reserved_before_disruption(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            events = []

            def prepare(staged, target, mode):
                events.append(f"prepare:{Path(staged).name}")
                prepared = Path(td) / f"prepared-{len(events)}"
                prepared.write_bytes(Path(staged).read_bytes())
                return prepared

            class OrderedLaunchctl(FakeLaunchctl):
                def stop(self):
                    events.append("stop")
                    return super().stop()

            with mock.patch.object(helper, "Launchctl", OrderedLaunchctl), \
                 mock.patch.object(helper, "prepare_atomic_install", side_effect=prepare), \
                 mock.patch.object(helper, "wait_for_identity"):
                self.assertEqual("committed", helper.activate(manifest))
            self.assertEqual(["candidate_binary", "candidate_plist", "rollback_binary", "rollback_plist"], [
                event.removeprefix("prepare:") for event in events[:4]
            ])
            self.assertEqual("stop", events[4])

    def test_install_space_failure_leaves_service_running(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl), \
                 mock.patch.object(
                     helper,
                     "prepare_atomic_install",
                     side_effect=OSError(28, "No space left on device"),
                 ):
                with self.assertRaises(OSError):
                    helper.activate(manifest)
            self.assertEqual([], FakeLaunchctl.events)
            self.assertEqual("precondition_failed", json.loads(Path(manifest.status_path).read_text())["state"])

    def test_manifest_status_and_failure_do_not_copy_plist_secrets(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            secret = "SENTINEL_SECRET_7b3f2"
            Path(manifest.candidate_plist).write_bytes(plistlib.dumps({
                "Label": "test", "EnvironmentVariables": {"TOKEN": secret},
            }))
            manifest = helper.dataclasses.replace(
                manifest, candidate_plist_sha256=helper.sha256(Path(manifest.candidate_plist))
            )
            encoded = json.dumps(helper.dataclasses.asdict(manifest))
            helper.write_status(manifest, "precondition_failed", failure="candidate plist invalid")
            diagnostics = encoded + Path(manifest.status_path).read_text()
            self.assertNotIn(secret, diagnostics)
            self.assertIn("candidate plist invalid", diagnostics)

    def test_claim_cleanup_cannot_delete_newer_owner(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            claim = Path(manifest.active_path)
            claim.write_text("newer-transaction\n")
            self.assertFalse(helper.release_claim(manifest))
            self.assertEqual("newer-transaction\n", claim.read_text())

    def test_helper_requests_bootout_even_when_manifest_is_malformed(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = Path(td) / "manifest.json"
            manifest.write_text("not-json")
            argv = ["helper", "activate", "--manifest", str(manifest),
                    "--helper-label", "test.helper", "--uid", str(os.getuid())]
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "request_helper_bootout") as bootout:
                self.assertEqual(1, helper.main())
            bootout.assert_called_once_with(os.getuid(), "test.helper")

    def test_helper_requests_bootout_on_concurrent_rejection(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            manifest_path = Path(td) / "manifest.json"
            manifest_path.write_text(json.dumps(helper.dataclasses.asdict(manifest)))
            argv = ["helper", "activate", "--manifest", str(manifest_path),
                    "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "activate", side_effect=helper.ConcurrentDeploy("busy")), \
                 mock.patch.object(helper, "request_helper_bootout") as bootout:
                self.assertEqual(1, helper.main())
            bootout.assert_called_once_with(manifest.uid, manifest.helper_label)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("rejected_concurrent", status["state"])
            self.assertFalse(Path(manifest.active_path).exists())



class PreparationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load(ROOT / "dev.py", "devpy_launchd_deploy_test")

    def test_broken_pipe_after_handoff_does_not_release_claim(self):
        with mock.patch("builtins.print", side_effect=BrokenPipeError), \
             mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release:
            self.dev._report_launchd_handoff("tx", self.dev.RuntimeIdentity("1.0.0", "abc123"))
        release.assert_not_called()

    def test_positional_version_is_rejected_with_release_guidance(self):
        result = subprocess.run(
            ["python3", str(ROOT / "dev.py"), "prod", "deploy", "v1.2.3"],
            cwd=ROOT, capture_output=True, text=True,
        )
        self.assertNotEqual(0, result.returncode)
        self.assertIn("--release", result.stderr)

    def test_release_path_skips_checks_and_build(self):
        with mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev, "prod_build") as build:
            self.dev.cmd_prod_deploy("v1.2.3")
        deploy.assert_called_once_with("v1.2.3")
        check.assert_not_called()
        build.assert_not_called()

    def test_controller_release_revalidates_exact_tag_to_expected_commit(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "abc123def456" + "0" * 28}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            candidate = self.dev._prepare_release_candidate(
                "v1.2.3", staging, expected_full_commit=release_commit
            )
        self.assertEqual(release_commit, candidate.release_commit)
        self.assertEqual(3, len(run.call_args_list))

    def test_controller_rejects_latest_release_alias(self):
        with tempfile.TemporaryDirectory() as td:
            with self.assertRaisesRegex(SystemExit, "exact release tag"):
                self.dev._prepare_release_candidate("latest", Path(td), expected_full_commit="a" * 40)

    def test_controller_uses_installed_launchd_env_and_transaction_id(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v1.2.3",
            expected_full_commit="a" * 40,
            transaction_id="tx-123",
        )
        installed = {"PHOENIX_PASSWORD": "installed", "PHOENIX_PORT": "9443"}
        with mock.patch.object(
            self.dev, "_launchd_env_from_plist", return_value=installed
        ) as read_installed:
            env, path = self.dev._launchd_candidate_env(controller)
        self.assertEqual(installed, env)
        self.assertEqual("tx-123", controller.transaction_id)
        self.assertIsNone(path)
        read_installed.assert_called_once_with(self.dev.LAUNCHD_PLIST_PATH)

    def test_controller_release_path_skips_checks_and_build(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v1.2.3",
            expected_full_commit="a" * 40,
            transaction_id="tx-123",
            backend="launchd",
        )
        with mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev, "prod_build") as build:
            self.dev.cmd_prod_deploy("v1.2.3", controller=controller)
        deploy.assert_called_once_with("v1.2.3", controller=controller)
        check.assert_not_called()
        build.assert_not_called()

    def test_release_asset_selection_supports_all_native_targets(self):
        import platform

        cases = [
            ("darwin", "arm64", "phoenix_ide-aarch64-apple-darwin"),
            ("darwin", "x86_64", "phoenix_ide-x86_64-apple-darwin"),
            ("linux", "aarch64", "phoenix_ide-aarch64-unknown-linux-musl"),
            ("linux", "amd64", "phoenix_ide-x86_64-unknown-linux-musl"),
        ]
        for host_platform, machine, expected in cases:
            with self.subTest(host_platform=host_platform, machine=machine), \
                 mock.patch.object(self.dev.sys, "platform", host_platform), \
                 mock.patch.object(platform, "machine", return_value=machine):
                self.assertEqual(expected, self.dev._release_asset_name())

    def test_exact_rc_opt_in_validates_metadata_checksum_and_full_build_identity(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"rc release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v0.13.0-rc.2", "isPrerelease": True, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with mock.patch.object(
                self.dev,
                "_binary_identity",
                return_value={"version": "0.13.0-rc.2", "git_sha": release_commit},
            ) as identity:
                candidate = self.dev._prepare_release_candidate("v0.13.0-rc.2", staging)

        self.assertEqual("v0.13.0-rc.2", candidate.release_tag)
        self.assertEqual("0.13.0-rc.2", candidate.identity.version)
        self.assertEqual(release_commit, candidate.identity.git_sha)
        self.assertEqual(release_commit, candidate.release_commit)
        identity.assert_called_once_with(asset)
        self.assertIn("v0.13.0-rc.2", run.call_args_list[2].args[0])

    def test_exact_rc_rejects_build_identity_without_rc_suffix(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity") as identity, \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"rc release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            identity.return_value = {"version": "0.13.0", "git_sha": release_commit}
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v0.13.0-rc.2", "isPrerelease": True, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "expected 0.13.0-rc.2"):
                self.dev._prepare_release_candidate("v0.13.0-rc.2", staging)

    def test_explicit_release_rejects_prerelease_metadata_mismatch(self):
        cases = [
            ("v1.2.3", True, "isPrerelease=false"),
            ("v0.13.0-rc.2", False, "isPrerelease=true"),
        ]
        for tag, is_prerelease, message in cases:
            with self.subTest(tag=tag), tempfile.TemporaryDirectory() as td, \
                 mock.patch.object(
                     self.dev.subprocess,
                     "run",
                     return_value=subprocess.CompletedProcess(
                         [],
                         0,
                         json.dumps({"tagName": tag, "isPrerelease": is_prerelease, "isDraft": False}),
                         "",
                     ),
                 ) as run:
                with self.assertRaisesRegex(SystemExit, message):
                    self.dev._prepare_release_candidate(tag, Path(td))
            run.assert_called_once()

    def test_release_rejects_unsupported_explicit_tag_before_lookup(self):
        invalid_tags = [
            "1.2.3",
            "v01.2.3",
            "v1.2.3-beta.1",
            "v0.13.0-rc.0",
            "v0.12.9-rc.1",
            "v1.100.0",
        ]
        with mock.patch.object(self.dev.subprocess, "run") as run:
            for tag in invalid_tags:
                with self.subTest(tag=tag), self.assertRaisesRegex(
                    SystemExit, "unsupported release tag"
                ):
                    self.dev._prepare_release_candidate(tag, Path("unused"))
        run.assert_not_called()

    def test_latest_rejects_rc_release(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(
            self.dev.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                [],
                0,
                json.dumps({"tagName": "v0.13.0-rc.2", "isPrerelease": True, "isDraft": False}),
                "",
            ),
        ) as run:
            with self.assertRaisesRegex(SystemExit, "latest must resolve to a stable"):
                self.dev._prepare_release_candidate("latest", Path(td))
        run.assert_called_once()

    def test_latest_resolves_stable_once_then_downloads_immutable_tag_and_checks_checksum(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "abc123def456" + "0" * 28}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            candidate = self.dev._prepare_release_candidate("latest", staging)
            self.assertEqual(asset, candidate.binary)
            self.assertTrue(candidate.binary.stat().st_mode & 0o100)
        self.assertEqual(self.dev.ProdSourceKind.PUBLISHED_RELEASE, candidate.source_kind)
        self.assertEqual(
            ("v1.2.3", release_commit, release_commit),
            (candidate.release_tag, candidate.identity.git_sha, candidate.release_commit),
        )
        self.assertIn("v1.2.3", run.call_args_list[2].args[0])

    def test_release_rejects_private_draft_before_download(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess(
                     [], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": True}), ""
                 ),
             ) as run:
            with self.assertRaisesRegex(SystemExit, "private drafts are not deployable"):
                self.dev._prepare_release_candidate("v1.2.3", Path(td))
        self.assertEqual("view", run.call_args.args[0][2])

    def test_release_rejects_asset_from_different_commit(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "bad123bad123"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            (staging / "SHA256SUMS").write_text(f"{self.dev._file_sha256(asset)}  {asset.name}\n")
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, "abc123" + "0" * 34 + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "asset embeds"):
                self.dev._prepare_release_candidate("latest", staging)

    def test_release_rejects_truncated_embedded_identity(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "a"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            (staging / "SHA256SUMS").write_text(f"{self.dev._file_sha256(asset)}  {asset.name}\n")
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, "abc123def456" + "0" * 28 + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "malformed git identity"):
                self.dev._prepare_release_candidate("latest", staging)

    def test_local_candidate_binds_exact_head_to_typed_identity(self):
        commit = "abc123def456" + "0" * 28
        identity = self.dev.RuntimeIdentity("2.0.0", commit)
        with mock.patch.object(self.dev, "prod_build", return_value=Path("candidate")) as build, \
             mock.patch.object(self.dev, "_binary_identity", return_value=identity), \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess([], 0, commit + "\n", ""),
             ):
            candidate = self.dev._prepare_local_candidate(target=None)
        self.assertEqual(self.dev.ProdSourceKind.LOCAL_HEAD, candidate.source_kind)
        self.assertEqual(commit, candidate.source_commit)
        self.assertEqual(identity, candidate.identity)
        self.assertIsNone(candidate.release_tag)
        self.assertIsNone(candidate.release_commit)
        build.assert_called_once_with(target=None)

    def test_local_candidate_rejects_identity_from_other_commit(self):
        with mock.patch.object(self.dev, "prod_build", return_value=Path("candidate")), \
             mock.patch.object(
                 self.dev,
                 "_binary_identity",
                 return_value=self.dev.RuntimeIdentity("2.0.0", "b" * 40),
             ), \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess([], 0, "a" * 40 + "\n", ""),
             ):
            with self.assertRaisesRegex(SystemExit, "does not exactly match selected HEAD"):
                self.dev._prepare_local_candidate(target=None)

    def test_failed_paired_claim_blocks_later_deploy_and_restart(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            active.write_text("broken-pair\n")
            status.write_text(json.dumps({"transaction_id": "broken-pair", "state": "activation_failed_rollback_failed", "source_kind": "prepared_artifact"}))
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), \
                 mock.patch.object(self.dev, "LAUNCHD_RESTART_DIR", root / "restart"), \
                 mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"):
                self.assertFalse(self.dev._status_is_terminal_for_owner(status, "broken-pair", self.dev._DEPLOY_TERMINAL_STATES))
                for admission in (self.dev._claim_launchd_deploy, self.dev._claim_launchd_restart):
                    with self.subTest(admission=admission.__name__), self.assertRaises(SystemExit) as error:
                        admission("new-operation")
                    self.assertIn("Do not remove", str(error.exception))
                    self.assertIn("recover-paired", str(error.exception))
                    self.assertNotIn("remove the marker", str(error.exception))
                    self.assertEqual(active.read_text(), "broken-pair\n")

    def test_interrupted_paired_manifest_fences_deploy_and_restart_without_terminal_status(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = root / "transactions" / "interrupted" / "manifest.json"
            manifest.parent.mkdir(parents=True)
            manifest.write_text(json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": {"database_path": "retained"}}))
            active = root / "active"
            active.write_text("interrupted\n")
            status = root / "status.json"
            for state in ("prepared", "activating", None):
                if state is None:
                    status.unlink(missing_ok=True)
                else:
                    status.write_text(json.dumps({"transaction_id": "interrupted", "state": state}))
                with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev, "LAUNCHD_RESTART_DIR", root / "restart"):
                    for admission in (self.dev._claim_launchd_deploy, self.dev._claim_launchd_restart):
                        with self.subTest(state=state, admission=admission.__name__), self.assertRaises(SystemExit) as error:
                            admission("new")
                        self.assertIn("Do not remove", str(error.exception))
                        self.assertEqual(active.read_text(), "interrupted\n")

    def test_pre_manifest_recovery_releases_only_dead_preparation_without_handoff(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            active.write_text("early\n")
            status.write_text(json.dumps({"transaction_id": "early", "source_kind": "prepared_artifact", "state": "preparing", "preparing_pid": 123, "updated_at": "2026-10-04T00:00:00Z"}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")):
                self.dev.cmd_prod_recover_paired("early")
            self.assertFalse(active.exists())
            self.assertEqual(json.loads(status.read_text())["state"], "precondition_failed")

    def test_finalization_controller_uses_retained_helper_without_bootstrap(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "transactions" / "pending"
            staging.mkdir(parents=True)
            script = staging / "launchd_deploy_helper.py"
            script.write_bytes(b"retained helper")
            (staging / "manifest.json").write_text(json.dumps({"transaction_id": "pending", "helper_label": "dev.activation.pending", "paired_database_upgrade": {"controller_helper_sha256": self.dev._file_sha256(script), "controller_helper_path": str(script)}}))
            active, status = root / "active", root / "status.json"
            active.write_text("pending\n")
            status.write_text(json.dumps({"transaction_id": "pending", "state": "committed", "finalization_pending": True}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
                self.dev.cmd_prod_finalize_paired("pending")
            self.assertEqual(run.call_count, 3)
            self.assertIn("sqlite3", run.call_args_list[0].args[0][-1])
            self.assertIn("finalize-paired", run.call_args.args[0])
            self.assertFalse(any("bootstrap" in call.args[0] for call in run.call_args_list))

    def test_committed_recovery_refusal_never_bootstraps_or_rewrites_status(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "transactions" / "committed"
            staging.mkdir(parents=True)
            (staging / "manifest.json").write_text(json.dumps({"paired_database_upgrade": {}}))
            active, status = root / "active", root / "status.json"
            active.write_text("committed\n")
            payload = {"transaction_id": "committed", "source_kind": "prepared_artifact", "state": "committed", "finalization_pending": True}
            status.write_text(json.dumps(payload))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev.subprocess, "run") as run:
                with self.assertRaisesRegex(SystemExit, "cannot enter database rollback"):
                    self.dev.cmd_prod_recover_paired("committed")
                run.assert_not_called()
            self.assertEqual(json.loads(status.read_text()), payload)
            self.assertTrue(active.exists())

    def test_manifest_persisted_preparing_recovery_does_not_touch_runtime(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "transactions" / "preparing"
            staging.mkdir(parents=True)
            manifest = staging / "manifest.json"
            manifest.write_text(json.dumps({"paired_database_upgrade": {"backup_path": str(staging / "backup.sqlite3"), "proof_path": str(staging / "proof.json"), "database_path": str(root / "prod.db")}}))
            active, status = root / "active", root / "status.json"
            active.write_text("preparing\n")
            status.write_text(json.dumps({"transaction_id": "preparing", "source_kind": "prepared_artifact", "state": "preparing", "preparing_pid": 123}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
                self.dev.cmd_prod_recover_paired("preparing")
            self.assertEqual(run.call_count, 1)
            self.assertNotIn("bootstrap", run.call_args.args[0])
            self.assertEqual(json.loads(status.read_text())["state"], "precondition_failed")
            self.assertFalse(active.exists())
            self.assertIn("backup_path", json.loads(manifest.read_text())["paired_database_upgrade"])

    def test_prepared_handoff_absent_abandons_without_runtime_mutation(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "transactions" / "prepared"
            staging.mkdir(parents=True)
            (staging / "manifest.json").write_text(json.dumps({"paired_database_upgrade": {"backup_path": str(staging / "backup.sqlite3"), "proof_path": str(staging / "proof.json"), "database_path": str(root / "prod.db")}}))
            active, status = root / "active", root / "status.json"
            active.write_text("prepared\n")
            status.write_text(json.dumps({"transaction_id": "prepared", "source_kind": "prepared_artifact", "state": "prepared", "preparing_pid": 123}))
            backup = staging / "backup.sqlite3"
            reserve = root / ".prod.db.restore-prepared"
            backup.write_bytes(b"allocated seed")
            reserve.write_bytes(b"allocated restore")
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
                self.dev.cmd_prod_recover_paired("prepared")
            self.assertEqual(run.call_count, 1)
            self.assertIn("--probe-service-absence", run.call_args.args[0])
            self.assertFalse(active.exists())
            self.assertEqual(json.loads(status.read_text())["state"], "precondition_failed")
            self.assertFalse(backup.exists())
            self.assertFalse(reserve.exists())

    def test_status_recovers_paired_guidance_from_claim_without_readable_status(self):
        for contents in (None, "{", "[]", "null"):
            with self.subTest(contents=contents), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                staging = root / "transactions" / "lost"
                staging.mkdir(parents=True)
                (staging / "manifest.json").write_text(json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": {}}))
                active, status = root / "active", root / "status.json"
                active.write_text("lost\n")
                if contents is not None:
                    status.write_text(contents)
                output = io.StringIO()
                with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), contextlib.redirect_stdout(output):
                    self.dev._print_launchd_deploy_status()
                self.assertIn("prod recover-paired", output.getvalue())
                self.assertIn("Do not remove", output.getvalue())
                self.assertTrue(active.exists())

    def test_pending_paired_commit_blocks_admission_without_authorizing_rollback(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            status.write_text(json.dumps({"transaction_id": "pending", "source_kind": "prepared_artifact", "state": "committed", "finalization_pending": True}))
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status):
                guidance = self.dev._paired_recovery_refusal("pending")
            self.assertIn("pending publication/cleanup", guidance)
            self.assertIn("Do not remove", guidance)
            self.assertIn("or invoke database rollback", guidance)
            self.assertNotIn("prod recover-paired", guidance)

    def test_initial_status_is_durable_before_owned_claim_publication(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            initial = {"transaction_id": "early", "state": "preparing", "source_kind": "prepared_artifact", "preparing_pid": os.getpid()}
            real_open = os.open

            def open_after_status(path, *args, **kwargs):
                if Path(path) == active:
                    self.assertEqual(json.loads(status.read_text()), initial)
                return real_open(path, *args, **kwargs)

            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev, "_restart_claim_owner", return_value=None), mock.patch.object(self.dev.os, "open", side_effect=open_after_status):
                self.dev._claim_launchd_deploy("early", initial_status=initial)
            self.assertEqual(active.read_text(), "early\n")

    def test_pre_manifest_recovery_refuses_unknown_helper_or_missing_pid(self):
        for missing_pid in (False, True):
            with self.subTest(missing_pid=missing_pid), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                active, status = root / "active", root / "status.json"
                active.write_text("early\n")
                payload = {"transaction_id": "early", "state": "preparing", "source_kind": "prepared_artifact"}
                if not missing_pid:
                    payload["preparing_pid"] = 123
                status.write_text(json.dumps(payload))
                with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "I/O error")) as run:
                    with self.assertRaises(SystemExit):
                        self.dev.cmd_prod_recover_paired("early")
                    self.assertEqual(run.call_count, 0 if missing_pid else 1)
                self.assertEqual(active.read_text(), "early\n")
                self.assertEqual(json.loads(status.read_text()), payload)

    def test_pre_manifest_recovery_refuses_live_preparation_pid(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            active.write_text("live\n")
            status.write_text(json.dumps({"transaction_id": "live", "source_kind": "prepared_artifact", "state": "preparing", "preparing_pid": 123}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill") as kill:
                kill.return_value = None
                with self.assertRaisesRegex(SystemExit, "still alive"):
                    self.dev.cmd_prod_recover_paired("live")
            self.assertEqual("live", active.read_text().strip())

    def test_recovery_interpreter_probe_failure_prevents_bootstrap_and_keeps_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "failed.pair"
            transaction.mkdir(parents=True)
            retained = transaction / "helper.py"
            retained.write_text("retained helper")
            payload = {"paired_database_upgrade": {"controller_helper_path": str(retained), "controller_helper_sha256": self.dev._file_sha256(retained)}, "helper_label": "com.phoenix-ide.deploy.failed-pair"}
            (transaction / "manifest.json").write_text(json.dumps(payload))
            active = root / "active"
            active.write_text("failed.pair\n")
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "_deploy_claim_owner", return_value="failed.pair"), mock.patch.object(self.dev, "_paired_recovery_refusal", return_value="retain claim"), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")) as run:
                with self.assertRaisesRegex(SystemExit, "active Python interpreter"):
                    self.dev.cmd_prod_recover_paired("failed.pair")
            self.assertFalse((transaction / "recovery-helper.plist").exists())
            self.assertEqual("failed.pair", active.read_text().strip())
            self.assertIn("sqlite3", run.call_args.args[0][-1])

    def test_supported_recovery_handoff_uses_retained_helper_without_clearing_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "failed.pair"
            transaction.mkdir(parents=True)
            retained = transaction / "helper.py"
            retained.write_text("retained helper")
            payload = {"paired_database_upgrade": {"controller_helper_path": str(retained), "controller_helper_sha256": self.dev._file_sha256(retained)}, "helper_label": "com.phoenix-ide.deploy.failed-pair"}
            (transaction / "manifest.json").write_text(json.dumps(payload))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "_deploy_claim_owner", return_value="failed.pair"), mock.patch.object(self.dev, "_paired_recovery_refusal", return_value="retain claim"), mock.patch.object(self.dev.subprocess, "run", side_effect=[subprocess.CompletedProcess([], 0, "", ""), subprocess.CompletedProcess([], 0, "", ""), subprocess.CompletedProcess([], 0, "", "")]) as backend, mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release:
                self.dev.cmd_prod_recover_paired("failed.pair")
                release.assert_not_called()
                plist = plistlib.loads((transaction / "recovery-helper.plist").read_bytes())
                self.assertEqual(plist["ProgramArguments"][2], "recover-paired")
                self.assertEqual(plist["ProgramArguments"][1], str(retained))
                self.assertEqual(backend.call_count, 3)
                self.assertIn("sqlite3", backend.call_args_list[0].args[0][-1])
                self.assertIn("--probe-service-absence", backend.call_args_list[1].args[0])

    def test_pruning_preserves_paired_and_unknown_transactions(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            for i in range(8):
                ordinary = root / f"ordinary-{i}"
                ordinary.mkdir()
                (ordinary / "manifest.json").write_text(json.dumps({"paired_database_upgrade": None}))
                os.utime(ordinary, (100 + i, 100 + i))
            for name, content in (("paired", json.dumps({"paired_database_upgrade": {"backup_path": "private"}})), ("unreadable", "invalid-json"), ("unknown", None)):
                transaction = root / name
                transaction.mkdir()
                if content is not None:
                    (transaction / "manifest.json").write_text(content)
                os.utime(transaction, (1, 1))
            self.dev._prune_launchd_deploy_transactions(root, "current")
            self.assertEqual(sorted(p.name for p in root.iterdir()), ["ordinary-3", "ordinary-4", "ordinary-5", "ordinary-6", "ordinary-7", "paired", "unknown", "unreadable"])

    def test_claim_release_is_transaction_owned(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", Path(td)), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", Path(td) / "active"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", Path(td) / "claim.lock"), \
             mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", Path(td) / "restart-active"):
            self.dev._claim_launchd_deploy("first")
            self.assertFalse(self.dev._release_launchd_deploy_claim("second"))
            self.assertEqual("first", self.dev._deploy_claim_owner())
            with self.assertRaisesRegex(SystemExit, "first"):
                self.dev._claim_launchd_deploy("second")

    def test_release_rejects_dirty_embedded_identity(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "abc123-dirty"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            (staging / "SHA256SUMS").write_text(f"{self.dev._file_sha256(asset)}  {asset.name}\n")
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, "abc123" + "0" * 34 + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "dirty git identity"):
                self.dev._prepare_release_candidate("latest", staging)

    def test_candidate_health_url_uses_candidate_tls_and_port(self):
        env = {"PHOENIX_TLS": "auto", "PHOENIX_PORT": "9443"}
        self.assertEqual("https://localhost:9443/version", self.dev._prod_local_health_url(env))

    def test_stopped_install_identity_falls_back_to_binary_probe(self):
        with mock.patch.object(self.dev, "_current_prod_identity", return_value=None), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.0.0", "git_sha": "oldsha"}) as probe:
            identity = self.dev._current_prod_identity({}) or self.dev._binary_identity(Path("installed"))
        self.assertEqual({"version": "1.0.0", "git_sha": "oldsha"}, identity)
        probe.assert_called_once_with(Path("installed"))

    def test_local_source_commit_stays_full_for_deployed_sha_comparison(self):
        full_sha = "abc123" + "0" * 34
        embedded = "abc123"
        self.assertTrue(full_sha.startswith(embedded.removesuffix("-dirty")))
        self.assertNotEqual(full_sha, embedded)

    def test_local_helper_is_materialized_from_selected_commit(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(self.dev.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, b"#!/usr/bin/python3\nprint('selected')\n", b"")
            destination = Path(td) / "helper.py"
            self.dev._materialize_helper("abc123", destination, "local_head")
            self.assertIn(b"selected", destination.read_bytes())
        self.assertEqual(["git", "show", "abc123:scripts/launchd_deploy_helper.py"], run.call_args.args[0])

    def test_release_helper_is_fetched_from_selected_commit(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(self.dev.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, b"#!/usr/bin/python3\nprint('release')\n", b"")
            destination = Path(td) / "helper.py"
            self.dev._materialize_helper("abc123", destination, "published_release")
            self.assertIn(b"release", destination.read_bytes())
        self.assertIn("ref=abc123", run.call_args.args[0][2])

    def test_rollback_uses_previous_endpoint(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = FakeLaunchctl(manifest)
            with mock.patch.object(helper, "wait_for_identity") as wait:
                helper.restore(manifest, launchctl)
            wait.assert_called_once_with(
                manifest, manifest.previous,
                health_url=manifest.previous_health_url,
                health_insecure_tls=manifest.previous_health_insecure_tls,
                health_json=manifest.previous_health_json,
            )

    def test_rollback_plist_is_parsed_before_disruption(self):
        FakeLaunchctl.events = []
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            Path(manifest.rollback_plist).write_text("not a plist")
            Path(manifest.target_binary).write_bytes(b"installed")
            manifest = helper.dataclasses.replace(
                manifest, rollback_plist_sha256=helper.sha256(Path(manifest.rollback_plist))
            )
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl):
                with self.assertRaises(Exception):
                    helper.activate(manifest)
            self.assertEqual([], FakeLaunchctl.events)

    def test_display_url_uses_effective_launchd_port(self):
        self.assertEqual(
            "https://localhost:9555",
            self.dev._prod_display_url({"PHOENIX_TLS": "auto", "PHOENIX_PORT": "9555"}),
        )

    def test_legacy_identity_uses_public_version_and_deployed_sha(self):
        class Response:
            def __enter__(self): return self
            def __exit__(self, *_args): return None
            def read(self): return b"phoenix-ide 0.9.0\n"
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "PROD_SHA_PATH", Path(td) / "deployed.sha"), \
             mock.patch("urllib.request.urlopen", return_value=Response()):
            self.dev.PROD_SHA_PATH.write_text("abc123\n")
            identity, url, insecure = self.dev._legacy_prod_identity({"PHOENIX_PORT": "9123"})
        self.assertEqual(self.dev.RuntimeIdentity("0.9.0", "abc123"), identity)
        self.assertEqual("http://localhost:9123/version", url)
        self.assertFalse(insecure)

    def test_launchd_manifest_defaults_to_bounded_120_second_health_budget(self):
        defaults = {
            field.name: field.default
            for field in helper.dataclasses.fields(helper.Manifest)
        }
        self.assertEqual(30.0, defaults["transition_timeout_secs"])
        self.assertEqual(120.0, defaults["health_timeout_secs"])
        self.assertEqual(
            defaults["transition_timeout_secs"],
            self.dev.LAUNCHD_TRANSITION_TIMEOUT_SECS,
        )
        self.assertEqual(
            defaults["health_timeout_secs"],
            self.dev.LAUNCHD_HEALTH_TIMEOUT_SECS,
        )
        self.assertEqual(
            390.0,
            4 * self.dev.LAUNCHD_TRANSITION_TIMEOUT_SECS
            + 2 * self.dev.LAUNCHD_HEALTH_TIMEOUT_SECS
            + self.dev.LAUNCHD_STALE_HANDOFF_ALLOWANCE_SECS,
        )

    def test_exact_identity_after_30_seconds_within_default_budget_succeeds(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = helper.dataclasses.replace(
                make_manifest(Path(td)),
                health_timeout_secs=120.0,
            )
            clock = FakeClock()
            observations = []

            def fetch(_url, **_kwargs):
                observations.append(clock.now)
                if clock.now < 35.8:
                    raise TimeoutError("HTTPS listener not ready")
                return manifest.expected

            helper.wait_for_identity(
                manifest,
                manifest.expected,
                monotonic=clock.monotonic,
                sleep=clock.sleep,
                fetch=fetch,
            )

        self.assertGreater(clock.now, 30.0)
        self.assertLess(clock.now, 120.0)
        self.assertGreater(len(observations), 1)

    def test_exact_identity_budget_expiry_reports_elapsed_deadline_and_last_error(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = helper.dataclasses.replace(
                make_manifest(Path(td)),
                health_timeout_secs=40.0,
            )
            clock = FakeClock()
            fetch_times = []

            def fetch(_url, **_kwargs):
                fetch_times.append(clock.now)
                raise TimeoutError("still starting")

            with self.assertRaisesRegex(
                helper.ActivationError,
                r"after 40\.0s .*budget=40\.0s, deadline=40\.000 monotonic.*TimeoutError: still starting",
            ):
                helper.wait_for_identity(
                    manifest,
                    manifest.expected,
                    monotonic=clock.monotonic,
                    sleep=clock.sleep,
                    fetch=fetch,
                )

        self.assertTrue(fetch_times)
        self.assertTrue(all(observed < 40.0 for observed in fetch_times))
        self.assertGreaterEqual(clock.now, 40.0)

    def test_exact_identity_mismatch_never_succeeds(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = helper.dataclasses.replace(
                make_manifest(Path(td)),
                health_timeout_secs=5.0,
            )
            clock = FakeClock()
            wrong = helper.Identity(manifest.expected.version, "wrong-full-sha")

            with self.assertRaisesRegex(
                helper.ActivationError,
                r"observed version=2\.0\.0 git_sha=wrong-full-sha",
            ):
                helper.wait_for_identity(
                    manifest,
                    manifest.expected,
                    monotonic=clock.monotonic,
                    sleep=clock.sleep,
                    fetch=lambda _url, **_kwargs: wrong,
                )

    def test_helper_normalizes_legacy_version_before_exact_rollback_comparison(self):
        class Response:
            def __enter__(self): return self
            def __exit__(self, *_args): return None
            def read(self): return b"phoenix-ide 0.9.0\n"
        with mock.patch.object(helper.urllib.request, "urlopen", return_value=Response()):
            identity = helper.fetch_identity(
                "http://localhost:8031/version",
                expected_git_sha="abc123def456",
            )
        self.assertEqual(helper.Identity("0.9.0", "abc123def456"), identity)

    def test_legacy_upgrade_can_use_health_identity_without_binary_probe(self):
        legacy = {"version": "0.9.0", "git_sha": "abc123def456"}
        with mock.patch.object(
            self.dev,
            "_binary_identity",
            side_effect=SystemExit("unsupported --build-identity"),
        ), mock.patch.object(
            self.dev,
            "_current_prod_identity",
            return_value=None,
        ), mock.patch.object(
            self.dev,
            "_legacy_prod_identity",
            return_value=(legacy, "http://localhost:8031/version", False),
        ):
            resolved = self.dev._resolve_rollback_identity(Path("rollback"), {})
        self.assertEqual(
            (legacy, "http://localhost:8031/version", False, False),
            resolved,
        )

    def test_legacy_rollback_verification_uses_plain_version_body(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = helper.dataclasses.replace(make_manifest(Path(td)), previous_health_json=False)
            launchctl = FakeLaunchctl(manifest)
            with mock.patch.object(helper, "fetch_identity", return_value=manifest.previous) as fetch:
                helper.restore(manifest, launchctl)
            self.assertEqual(manifest.previous.git_sha, fetch.call_args.kwargs["expected_git_sha"])

    def test_status_failure_keeps_claim_for_recovery(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            claim = Path(manifest.active_path)
            claim.write_text(manifest.transaction_id + "\n")
            argv = ["helper", "activate", "--manifest", str(Path(td) / "manifest.json"),
                    "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
            Path(argv[3]).write_text(json.dumps(helper.dataclasses.asdict(manifest)))
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "activate", side_effect=helper.ActivationError("status write failed")), \
                 mock.patch.object(helper, "status_is_durable_terminal", return_value=False), \
                 mock.patch.object(helper, "request_helper_bootout"):
                self.assertEqual(1, helper.main())
            self.assertEqual(manifest.transaction_id, claim.read_text().strip())

    def test_precondition_failure_is_terminal_and_releases_claim(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            helper.write_status(manifest, "precondition_failed", failure="bad staged plist")
            self.assertTrue(helper.status_is_durable_terminal(manifest))
            self.assertIn("precondition_failed", helper.TERMINAL_STATES)

    def test_candidate_env_uses_only_repo_environment_file(self):
        with mock.patch.object(
            self.dev,
            "_load_env_file",
            side_effect=lambda env: env.update({"PHOENIX_PASSWORD": "repo-secret", "PHOENIX_PORT": "9443"}),
        ):
            env, _path = self.dev._launchd_candidate_env()
        self.assertEqual({"PHOENIX_PASSWORD": "repo-secret", "PHOENIX_PORT": "9443"}, env)

    def test_prod_override_commands_reject_without_backend_detection(self):
        with mock.patch.object(self.dev, "detect_prod_env") as detect:
            with self.assertRaisesRegex(SystemExit, r"edit \.phoenix-ide\.env directly"):
                self.dev.cmd_prod_override_set("PHOENIX_PORT", "9443")
            with self.assertRaisesRegex(SystemExit, r"edit \.phoenix-ide\.env directly"):
                self.dev.cmd_prod_override_unset("PHOENIX_PORT")
        detect.assert_not_called()

    def test_rollback_endpoint_is_derived_from_rollback_plist(self):
        with tempfile.TemporaryDirectory() as td:
            plist_path = Path(td) / "rollback.plist"
            plist_path.write_bytes(plistlib.dumps({
                "EnvironmentVariables": {"PHOENIX_TLS": "auto"},
                "Sockets": {"Listeners": {"SockServiceName": "9555"}},
            }))
            env = self.dev._launchd_env_from_plist(plist_path)
            url, insecure = self.dev._launchd_health_probe(env)
        self.assertEqual("https://localhost:9555/api/version", url)
        self.assertTrue(insecure)

    def test_initial_status_failure_releases_claim(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", Path(td)), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", Path(td) / "active"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", Path(td) / "claim.lock"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", Path(td) / "restart-active"), \
             mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)), \
             mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
             mock.patch.object(self.dev, "_write_json_atomic", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                self.dev.launchd_prod_deploy()
            self.assertFalse(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())

    def test_prod_stop_refuses_pending_paired_finalization(self):
        with mock.patch.object(self.dev, "_paired_recovery_refusal", return_value="pending finalization"), mock.patch.object(self.dev, "detect_prod_env") as detect, self.assertRaisesRegex(SystemExit, "pending finalization"):
            self.dev.cmd_prod_stop()
        detect.assert_not_called()

    def test_prod_stop_uses_service_target_and_requires_absence(self):
        success = subprocess.CompletedProcess([], 0, "", "")
        with mock.patch.object(self.dev.subprocess, "run", side_effect=[success, success, success]) as run:
            self.dev._launchd_stop_if_loaded()
        self.assertEqual(run.call_args_list[1].args[0], ["launchctl", "bootout", f"gui/{os.getuid()}/{self.dev.LAUNCHD_LABEL}"])
        self.assertIn("--probe-service-absence", run.call_args_list[2].args[0])
        with mock.patch.object(self.dev.subprocess, "run", side_effect=[success, subprocess.CompletedProcess([], 5, "", "")]), self.assertRaisesRegex(SystemExit, "stop failed"):
            self.dev._launchd_stop_if_loaded()

    def test_activation_sqlite_interpreter_probe_failure_prevents_bootstrap(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            candidate = root / "candidate"
            candidate.write_bytes(b"candidate")
            identity = self.dev.RuntimeIdentity("2.0.0", "a" * 40)
            prepared = self.dev.PreparedCandidate(binary=candidate, source_kind=self.dev.ProdSourceKind.LOCAL_HEAD, source_commit=identity.git_sha, identity=identity)

            def run(command, **_kwargs):
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, str(self.dev.LAUNCHD_HANDOFF_PROTOCOL_VERSION) + "\n", "")
                if "-c" in command:
                    self.assertIn("sqlite3", command[-1])
                    return subprocess.CompletedProcess(command, 1, "", "No module named sqlite3")
                return subprocess.CompletedProcess(command, 0, "", "")

            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root / "deploy"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "deploy" / "active"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", root / "deploy" / "status.json"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev, "LAUNCHD_INSTALL_DIR", root / "installed"), mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", root / "installed.plist"), mock.patch.object(self.dev, "_restart_claim_owner", return_value=None), mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({"PHOENIX_PASSWORD": "test-only"}, None)), mock.patch.object(self.dev, "_prepare_local_candidate", return_value=prepared), mock.patch.object(self.dev, "_binary_identity", return_value=identity), mock.patch.object(self.dev, "_materialize_helper", side_effect=lambda *_args: None), mock.patch.object(self.dev, "generate_launchd_plist", return_value=plistlib.dumps({"Label": "example"}).decode()), mock.patch.object(self.dev.subprocess, "run", side_effect=run) as calls:
                with self.assertRaisesRegex(SystemExit, "interpreter cannot run"):
                    self.dev.launchd_prod_deploy()
            self.assertFalse(any("bootstrap" in c.args[0] for c in calls.call_args_list))
            self.assertFalse((root / "deploy" / "active").exists())

    def test_paired_bootstrap_interruption_retains_prepared_claim_without_overwrite(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "deploy" / "transactions" / "interrupted"
            candidate = root / "candidate"
            candidate.write_bytes(b"candidate")
            installed = root / "installed"
            installed.mkdir()
            (installed / "phoenix-ide").write_bytes(b"previous")
            target_plist = root / "installed.plist"
            target_plist.write_bytes(plistlib.dumps({"EnvironmentVariables": {"PATH": "/test", "PHOENIX_PASSWORD": "test-only"}}))
            identity = self.dev.RuntimeIdentity("2.0.0", "a" * 40)
            previous = self.dev.RuntimeIdentity("1.0.0", "b" * 40)
            prepared = self.dev.PreparedCandidate(binary=candidate, source_kind=self.dev.ProdSourceKind.PREPARED_ARTIFACT, source_commit=identity.git_sha, identity=identity)
            controller = self.dev.ProdDeployControllerOptions(transaction_id="interrupted", prepared_artifact=root, expected_full_commit=identity.git_sha, paired_database_upgrade=True)
            def run(command, **_kwargs):
                if "rev-parse" in command:
                    return subprocess.CompletedProcess(command, 0, "d" * 40, "")
                if "bootstrap" in command:
                    raise KeyboardInterrupt("accepted handoff response interrupted")
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, str(self.dev.LAUNCHD_HANDOFF_PROTOCOL_VERSION) + "\n", "")
                return subprocess.CompletedProcess(command, 0, "", "")
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root / "deploy"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "deploy" / "active"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", root / "deploy" / "status.json"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev, "LAUNCHD_INSTALL_DIR", installed), mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", target_plist), mock.patch.object(self.dev, "_restart_claim_owner", return_value=None), mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({"PATH": "/test", "PHOENIX_PASSWORD": "test-only"}, None)), mock.patch.object(self.dev, "_prepare_prepared_artifact", return_value=prepared), mock.patch.object(self.dev, "_binary_identity", side_effect=lambda path: previous if Path(path).name == "rollback.bin" else identity), mock.patch.object(self.dev, "_materialize_helper", side_effect=lambda *_args: None), mock.patch.object(self.dev, "_file_sha256", return_value="c" * 64), mock.patch.object(self.dev, "generate_launchd_plist", return_value=target_plist.read_bytes().decode()), mock.patch.object(self.dev, "_ensure_newsyslog_config"), mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                with self.assertRaises(KeyboardInterrupt):
                    self.dev.launchd_prod_deploy(controller=controller)
            self.assertEqual((root / "deploy" / "active").read_text().strip(), "interrupted")
            self.assertEqual(json.loads((root / "deploy" / "status.json").read_text())["state"], "prepared")

    def test_precondition_failure_records_typed_candidate_identity(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            candidate = root / "built-phoenix"
            candidate.write_bytes(b"candidate")
            active_path = root / "deploy" / "active"
            status_path = root / "deploy" / "status.json"
            commit = "abc123def456" + "0" * 28
            identity = self.dev.RuntimeIdentity("2.0.0", commit)
            prepared = self.dev.PreparedCandidate(
                binary=candidate,
                source_kind=self.dev.ProdSourceKind.LOCAL_HEAD,
                source_commit=commit,
                identity=identity,
            )

            def run(command, **_kwargs):
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, "1\n", "")
                return subprocess.CompletedProcess(command, 0, "", "")

            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root / "deploy"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active_path), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "deploy" / "claim.lock"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_LOCK_PATH", root / "deploy" / "activate.lock"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status_path), \
                 mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart" / "active"), \
                 mock.patch.object(self.dev, "LAUNCHD_INSTALL_DIR", root / "install"), \
                 mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", root / "service.plist"), \
                 mock.patch.object(self.dev, "PROD_SHA_PATH", root / "deployed.sha"), \
                 mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_prepare_local_candidate", return_value=prepared), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=identity), \
                 mock.patch.object(self.dev, "capture_login_shell_path", return_value=("/bin", "test")), \
                 mock.patch.object(self.dev, "print_launchd_path_report"), \
                 mock.patch.object(self.dev, "_materialize_helper"), \
                 mock.patch.object(self.dev, "_ensure_newsyslog_config", side_effect=SystemExit("sudo required")), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                with self.assertRaisesRegex(SystemExit, "sudo required"):
                    self.dev.launchd_prod_deploy()

            status = json.loads(status_path.read_text())
            self.assertEqual("precondition_failed", status["state"])
            self.assertEqual(identity.version, status["expected_version"])
            self.assertEqual(identity.git_sha, status["expected_git_sha"])
            self.assertFalse(active_path.exists())

    def test_staged_identity_mismatch_records_selected_candidate_identity(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            candidate = root / "built-phoenix"
            candidate.write_bytes(b"candidate")
            active_path = root / "deploy" / "active"
            status_path = root / "deploy" / "status.json"
            selected = self.dev.RuntimeIdentity("2.0.0", "abc123def456")
            observed = self.dev.RuntimeIdentity("9.9.9", "bad123def456")
            prepared = self.dev.PreparedCandidate(
                binary=candidate,
                source_kind=self.dev.ProdSourceKind.LOCAL_HEAD,
                source_commit="abc123def456" + "0" * 28,
                identity=selected,
            )

            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root / "deploy"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active_path), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "deploy" / "claim.lock"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status_path), \
                 mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart" / "active"), \
                 mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_prepare_local_candidate", return_value=prepared), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=observed), \
                 mock.patch.object(
                     self.dev.subprocess,
                     "run",
                     return_value=subprocess.CompletedProcess([], 0, "", ""),
                 ):
                with self.assertRaisesRegex(SystemExit, "staged candidate identity changed after signing"):
                    self.dev.launchd_prod_deploy()

            status = json.loads(status_path.read_text())
            self.assertEqual("precondition_failed", status["state"])
            self.assertEqual(selected.version, status["expected_version"])
            self.assertEqual(selected.git_sha, status["expected_git_sha"])
            self.assertNotEqual(observed.version, status["expected_version"])
            self.assertNotEqual(observed.git_sha, status["expected_git_sha"])
            self.assertFalse(active_path.exists())

    def test_rollback_binary_identity_mismatch_is_rejected(self):
        running = {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"}
        rollback = {"version": "1.0.0", "git_sha": "bbbbbbbbbbbb"}
        self.assertFalse(self.dev._rollback_identity_matches(rollback, running, True))

    def test_modern_rollback_rejects_dirty_or_short_prefix_identity(self):
        running = {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"}
        self.assertFalse(self.dev._rollback_identity_matches(
            {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa-dirty"}, running, True
        ))
        self.assertFalse(self.dev._rollback_identity_matches(
            {"version": "1.0.0", "git_sha": "aaaaaa"}, running, True
        ))

    def test_legacy_rollback_allows_exact_twelve_char_commit_prefix(self):
        legacy = {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa" + "b" * 28}
        rollback = {"version": "1.0.0", "git_sha": "aaaaaaaaaaaa"}
        self.assertTrue(self.dev._rollback_identity_matches(rollback, legacy, False))

    def test_candidate_env_snapshot_is_reused(self):
        snapshot = {"PHOENIX_PORT": "9443", "PHOENIX_PASSWORD": "one"}
        with mock.patch.object(self.dev, "_launchd_candidate_env", return_value=(snapshot, Path("env"))) as load:
            first, source = self.dev._launchd_candidate_env()
            staged = dict(first)
        self.assertEqual(snapshot, staged)
        self.assertEqual(Path("env"), source)
        load.assert_called_once()

    def test_helper_rejects_incompatible_manifest_protocol(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            path = Path(td) / "manifest.json"
            value = helper.dataclasses.asdict(manifest)
            value["manifest_version"] = helper.HANDOFF_PROTOCOL_VERSION + 1
            path.write_text(json.dumps(value))
            with self.assertRaisesRegex(helper.ActivationError, "unsupported handoff protocol"):
                helper.Manifest.load(path)

    def test_prod_status_uses_effective_launchd_env(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", Path(td) / "service.plist"), \
             mock.patch.object(self.dev, "_current_prod_identity", return_value=None) as identity, \
             mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "state = running\n", "")), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_PLIST_PATH.write_bytes(plistlib.dumps({
                "EnvironmentVariables": {},
                "Sockets": {"Listeners": {"SockServiceName": "9555"}},
            }))
            self.dev.launchd_prod_status()
        identity.assert_called_once_with({"PHOENIX_PORT": "9555"})
        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("Port: 9555", rendered)
        self.assertIn("URL: http://localhost:9555", rendered)

    def test_prod_status_falls_back_to_legacy_public_version(self):
        legacy_identity = self.dev.RuntimeIdentity(version="0.9.0", git_sha="abc123def456")
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", Path(td) / "service.plist"), \
             mock.patch.object(self.dev, "_current_prod_identity", return_value=None), \
             mock.patch.object(
                 self.dev,
                 "_legacy_prod_identity",
                 return_value=(legacy_identity, "http://localhost:8031/version", False),
             ) as legacy, \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess([], 0, "state = running\n", ""),
             ), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_PLIST_PATH.write_bytes(plistlib.dumps({
                "EnvironmentVariables": {"PHOENIX_PASSWORD": "secret"},
                "Sockets": {"Listeners": {"SockServiceName": "8031"}},
            }))
            self.dev.launchd_prod_status()
        legacy.assert_called_once_with({"PHOENIX_PASSWORD": "secret", "PHOENIX_PORT": "8031"})
        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("Version: 0.9.0 (abc123def456)", rendered)
        self.assertNotIn("Health: not responding", rendered)

    def test_prod_status_reports_durable_state_with_corrupt_plist(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", Path(td) / "service.plist"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch.object(self.dev, "_current_prod_identity", return_value=None), \
             mock.patch.object(self.dev, "_legacy_prod_identity", return_value=None), \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess([], 0, "state = running\n", ""),
             ), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_PLIST_PATH.write_text("not a plist")
            self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps({
                "transaction_id": "tx", "state": "activation_failed_rolled_back",
                "source_kind": "local_head", "expected_version": "1.0.0",
                "expected_git_sha": "abc123def456", "updated_at": "2026-01-01T00:00:00+00:00",
            }))
            self.dev.launchd_prod_status()
        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("Config: unreadable launchd plist", rendered)
        self.assertIn("Last deploy: activation_failed_rolled_back (tx)", rendered)

    def test_release_workflow_lists_all_native_assets_and_checksums(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text()
        for asset in (
            "phoenix_ide-aarch64-apple-darwin",
            "phoenix_ide-x86_64-apple-darwin",
            "phoenix_ide-aarch64-unknown-linux-musl",
            "phoenix_ide-x86_64-unknown-linux-musl",
        ):
            self.assertIn(asset, workflow)
        self.assertIn("SHA256SUMS", workflow)
        self.assertIn('missing required release asset: $asset', workflow)
        self.assertEqual(2, workflow.count("git restore ui/dist/.gitkeep"))
        self.assertEqual(2, workflow.count('test -z "$(git status --porcelain)"'))
        self.assertIn("runner: macos-15-intel", workflow)
        self.assertIn("runner: macos-15", workflow)
        self.assertIn("runner: ubuntu-24.04-arm", workflow)
        self.assertIn("runner: ubuntu-latest", workflow)
        self.assertNotIn("runner: macos-14", workflow)
        self.assertNotIn("runner: macos-13", workflow)

    def test_helper_plist_uses_active_interpreter(self):
        plist = plistlib.loads(self.dev._helper_plist(
            "test.helper", Path("helper.py"), Path("manifest.json"), Path("helper.log"), Path("/opt/python3")
        ))
        self.assertEqual("/opt/python3", plist["ProgramArguments"][0])

    def test_null_source_kind_status_remains_readable(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps({
                "transaction_id": "tx", "state": "preparing", "source_kind": None,
                "release_tag": "v1.2.3", "expected_version": None, "expected_git_sha": None,
                "updated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            }))
            self.dev._print_launchd_deploy_status()
        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("unknown v1.2.3", rendered)
        self.assertNotIn("unreadable status", rendered)

    def test_active_transaction_within_complete_activation_budget_is_not_stale(self):
        active = (
            datetime.datetime.now(datetime.timezone.utc)
            - datetime.timedelta(seconds=360)
        ).isoformat()
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps({
                "transaction_id": "tx", "state": "activating", "source_kind": "local_head",
                "expected_version": "1.0.0", "expected_git_sha": "abc", "updated_at": active,
            }))
            self.dev._print_launchd_deploy_status()
        self.assertFalse(any("STALE:" in str(call) for call in output.call_args_list))

    def test_stale_paired_status_uses_verified_recovery_not_marker_removal(self):
        stale = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=1)).isoformat()
        for state in ("prepared", "activating", "activation_failed_rollback_failed"):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                status = root / "status.json"
                status.write_text(json.dumps({"transaction_id": "tx", "state": state, "source_kind": "prepared_artifact", "updated_at": stale}))
                with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch("builtins.print") as output:
                    self.dev._print_launchd_deploy_status()
                rendered = " ".join(str(c) for c in output.call_args_list)
                self.assertIn("recover-paired", rendered)
                self.assertIn("Do not remove", rendered)
                self.assertNotIn("clearing the active marker", rendered)
                self.assertNotIn("unreadable status", rendered)

    def test_preparing_transaction_reports_stale_recovery(self):
        stale = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=7)).isoformat()
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps({
                "transaction_id": "tx", "state": "preparing", "source_kind": "local_head",
                "expected_version": None, "expected_git_sha": None, "updated_at": stale,
            }))
            self.dev._print_launchd_deploy_status()
        self.assertTrue(any("STALE:" in str(call) for call in output.call_args_list))

    def test_socket_activation_shape_is_preserved(self):
        plist = self.dev.generate_launchd_plist("1.0.0", path_override="/usr/bin")
        self.assertIn("<key>Sockets</key>", plist)
        self.assertIn("<string>IPv4v6</string>", plist)
        self.assertIn("<string>8031</string>", plist)

    def test_launchd_has_no_second_file_writer_for_structured_log(self):
        plist = plistlib.loads(
            self.dev.generate_launchd_plist(
                "1.0.0",
                extra_env={
                    "PHOENIX_LOG_FILE": "",
                    "PHOENIX_FATAL_LOG_FILE": "",
                    "PHOENIX_LOG_STDOUT": "true",
                },
                path_override="/usr/bin",
            ).encode()
        )
        environment = plist["EnvironmentVariables"]
        self.assertEqual(str(self.dev.LAUNCHD_LOG_PATH), environment["PHOENIX_LOG_FILE"])
        self.assertEqual(
            str(self.dev.LAUNCHD_FATAL_LOG_PATH), environment["PHOENIX_FATAL_LOG_FILE"]
        )
        self.assertEqual("false", environment["PHOENIX_LOG_STDOUT"])
        self.assertEqual("/dev/null", plist["StandardOutPath"])
        self.assertEqual(
            str(self.dev.LAUNCHD_STDERR_LOG_PATH), plist["StandardErrorPath"]
        )

    def test_newsyslog_bounds_launchd_pre_main_stderr(self):
        config = self.dev._newsyslog_config("phoenix", "staff")

        self.assertNotIn(str(self.dev.LAUNCHD_LOG_PATH), config)
        self.assertIn(
            f"{self.dev.LAUNCHD_STDERR_LOG_PATH}    phoenix:staff    600  2    64",
            config,
        )
        self.assertIn("64    *  BJN", config)

class PreparedArtifactTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load(ROOT / "dev.py", "devpy_prepared_artifact_test")

    def _valid_artifact(self, root: Path, *, commit: str = "a" * 40, target: str = "aarch64-apple-darwin"):
        version = "1.2.3"
        name = f"phoenix_ide-{target}-prepared-{commit[:12]}"
        binary = root / name
        binary.write_bytes(b"prepared standalone binary")
        receipt = root / f"PREPARATION-RECEIPT-{target}.json"
        receipt.write_text(json.dumps({
            "schema": 1,
            "operation": "prepare-main",
            "target": target,
            "commit": commit,
            "version": version,
            "checks": {
                "developer_id_signature": "verified",
                "hardened_runtime": "verified",
                "notarization": "accepted",
                "stapled_ticket": "validated",
                "gatekeeper": "accepted",
                "embedded_helper_bytes": "identical",
                "notarization_submission_id": "123e4567-e89b-12d3-a456-426614174000",
            },
            "sha256": {name: self.dev._file_sha256(binary)},
        }))
        return binary, receipt, commit, target, version, name

    def _prepare(self, root: Path, commit: str = "a" * 40):
        with mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev.platform, "machine", return_value="arm64"):
            return self.dev._prepare_prepared_artifact(root, commit)

    def _assert_rejected_before_codesign(self, root: Path, message: str, *, commit: str = "a" * 40):
        with mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev.platform, "machine", return_value="arm64"), \
             mock.patch.object(self.dev.subprocess, "run") as run, \
             mock.patch.object(self.dev, "_binary_identity") as identity:
            with self.assertRaisesRegex(SystemExit, message):
                self.dev._prepare_prepared_artifact(root, commit)
        run.assert_not_called()
        identity.assert_not_called()

    def test_valid_receipt_uses_exact_standalone_name_and_separate_codesign_outputs(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary, _receipt, commit, _target, version, name = self._valid_artifact(root)
            verify = subprocess.CompletedProcess([], 0, "", "")
            display = subprocess.CompletedProcess(
                [], 0, "", "Authority=Developer ID Application: Example\n"
                "CodeDirectory v=20500 flags=0x10000(runtime)\nTimestamp=2026-01-01\n"
            )
            with mock.patch.object(self.dev.subprocess, "run", side_effect=[verify, display]) as run, \
                 mock.patch.object(
                     self.dev, "_binary_identity",
                     return_value=self.dev.RuntimeIdentity(version, commit),
                 ) as identity:
                candidate = self._prepare(root)
        self.assertEqual(binary, candidate.binary)
        self.assertEqual(name, binary.name)
        self.assertEqual(self.dev.ProdSourceKind.PREPARED_ARTIFACT, candidate.source_kind)
        self.assertEqual([call.args[0][:3] for call in run.call_args_list], [
            ["codesign", "--verify", "--strict"],
            ["codesign", "--display", "--verbose=4"],
        ])
        identity.assert_called_once_with(binary)

    def test_downloaded_mode644_binary_gets_owner_execute_only_after_validation(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary, *_ = self._valid_artifact(root)
            binary.chmod(0o644)
            def identity(path):
                self.assertTrue(path.stat().st_mode & 0o100)
                return self.dev.RuntimeIdentity("1.2.3", "a" * 40)
            verify = subprocess.CompletedProcess([], 0, "", "")
            display = subprocess.CompletedProcess([], 0, "", "Authority=Developer ID Application: Example\nCodeDirectory v=20500 flags=0x10000(runtime)\nTimestamp=2026-01-01\n")
            with mock.patch.object(self.dev.subprocess, "run", side_effect=[verify, display]), mock.patch.object(self.dev, "_binary_identity", side_effect=identity):
                self._prepare(root)
            self.assertEqual(binary.stat().st_mode & 0o777, 0o744)

    def test_zip_checksum_does_not_count_as_exact_standalone_binary_checksum(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary, receipt, commit, target, _version, _name = self._valid_artifact(Path(td))
            metadata = json.loads(receipt.read_text())
            digest = metadata["sha256"].pop(next(iter(metadata["sha256"])))
            metadata["sha256"][f"phoenix_ide-{target}-prepared-{commit[:12]}.zip"] = digest
            receipt.write_text(json.dumps(metadata))
            self._assert_rejected_before_codesign(root, "exact standalone")
            self.assertTrue(binary.exists())

    def test_invalid_receipt_schema_submission_sha_and_metadata_fail_before_execution(self):
        cases = [
            ("schema", lambda metadata: metadata.update(schema=2), "valid submission UUID"),
            ("submission", lambda metadata: metadata["checks"].update(notarization_submission_id="not-a-uuid"), "valid submission UUID"),
            ("sha", lambda metadata: metadata["sha256"].update({next(iter(metadata["sha256"])): "bad"}), "exact standalone"),
            ("metadata", lambda metadata: metadata["checks"].update(gatekeeper="rejected"), "accepted signing/notarization/Gatekeeper/helper checks"),
        ]
        for _name, mutate, message in cases:
            with self.subTest(case=_name), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                _binary, receipt, _commit, _target, _version, _standalone = self._valid_artifact(root)
                metadata = json.loads(receipt.read_text())
                mutate(metadata)
                receipt.write_text(json.dumps(metadata))
                self._assert_rejected_before_codesign(root, message)

    def test_short_commit_wrong_target_and_binary_symlink_fail_before_execution(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            self._assert_rejected_before_codesign(root, "full 40-character", commit="a" * 12)

        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            _binary, receipt, _commit, _target, _version, _name = self._valid_artifact(root)
            metadata = json.loads(receipt.read_text())
            metadata["target"] = "x86_64-apple-darwin"
            receipt.write_text(json.dumps(metadata))
            self._assert_rejected_before_codesign(root, "this host architecture")

        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary, _receipt, _commit, _target, _version, _name = self._valid_artifact(root)
            real = root / "real-binary"
            binary.rename(real)
            binary.symlink_to(real)
            self._assert_rejected_before_codesign(root, "checksum mismatch")

    def test_identity_mismatch_is_checked_against_receipt_and_expected_commit(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary, _receipt, commit, _target, version, _name = self._valid_artifact(root)
            codesign = subprocess.CompletedProcess(
                [], 0, "Authority=Developer ID Application: Example\n"
                "CodeDirectory v=20500 flags=runtime\nTimestamp=now\n", ""
            )
            with mock.patch.object(self.dev.subprocess, "run", side_effect=[codesign, codesign]), \
                 mock.patch.object(
                     self.dev, "_binary_identity",
                     return_value=self.dev.RuntimeIdentity(version, "b" * 40),
                 ) as identity:
                with self.assertRaisesRegex(SystemExit, "identity does not match"):
                    self._prepare(root)
            identity.assert_called_once_with(binary)

    def test_wrong_platform_is_rejected_before_receipt_execution(self):
        with tempfile.TemporaryDirectory() as td:
            with mock.patch.object(self.dev.sys, "platform", "linux"), \
                 mock.patch.object(self.dev.subprocess, "run") as run:
                with self.assertRaisesRegex(SystemExit, "only on macOS launchd"):
                    self.dev._prepare_prepared_artifact(Path(td), "a" * 40)
            run.assert_not_called()

    def test_cmd_prod_deploy_passes_unenabled_prepared_options_to_launchd(self):
        controller = self.dev.ProdDeployControllerOptions(
            prepared_artifact=Path("prepared"), expected_full_commit="a" * 40,
            paired_database_upgrade=True, transaction_id="tx-123",
        )
        with mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check:
            self.dev.cmd_prod_deploy(controller=controller)
        deploy.assert_called_once_with(None, controller=controller)
        check.assert_not_called()

    def test_cmd_prod_deploy_rejects_incomplete_prepared_artifact_options(self):
        cases = [
            (self.dev.ProdDeployControllerOptions(prepared_artifact=Path("prepared"), paired_database_upgrade=True), "required together"),
            (self.dev.ProdDeployControllerOptions(expected_full_commit="a" * 40), "requires all prepared"),
            (self.dev.ProdDeployControllerOptions(prepared_artifact=Path("prepared")), "required together"),
            (self.dev.ProdDeployControllerOptions(paired_database_upgrade=True), "required together"),
            (self.dev.ProdDeployControllerOptions(prepared_artifact=Path("prepared"), expected_full_commit="a" * 40), "required together"),
            (self.dev.ProdDeployControllerOptions(expected_full_commit="a" * 40, paired_database_upgrade=True), "required together"),
        ]
        for controller, message in cases:
            with self.subTest(message=message):
                with self.assertRaisesRegex(SystemExit, message):
                    self.dev.cmd_prod_deploy(controller=controller)

    def test_failed_prepared_validation_records_requested_source_kind(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            controller = self.dev.ProdDeployControllerOptions(prepared_artifact=root / "artifact", expected_full_commit="a" * 40, paired_database_upgrade=True, transaction_id="failed-receipt")
            with mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, root / "unused.env")), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_claim_launchd_deploy"), \
                 mock.patch.object(self.dev, "_release_launchd_deploy_claim"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), \
                 mock.patch.object(self.dev, "_prepare_prepared_artifact", side_effect=SystemExit("receipt rejected")):
                with self.assertRaisesRegex(SystemExit, "receipt rejected"):
                    self.dev.launchd_prod_deploy(controller=controller)
                observed = json.loads(status.read_text())
                self.assertEqual(observed["state"], "precondition_failed")
                self.assertEqual(observed["source_kind"], "prepared_artifact")

    def test_cmd_prod_deploy_rejects_nonmac_and_release_for_prepared_artifact(self):
        controller = self.dev.ProdDeployControllerOptions(
            prepared_artifact=Path("prepared"), expected_full_commit="a" * 40,
            paired_database_upgrade=True,
        )
        with mock.patch.object(self.dev.sys, "platform", "linux"):
            with self.assertRaisesRegex(SystemExit, "only by macOS launchd"):
                self.dev.cmd_prod_deploy(controller=controller)
        with mock.patch.object(self.dev.sys, "platform", "darwin"):
            with self.assertRaisesRegex(SystemExit, "excludes --release"):
                self.dev.cmd_prod_deploy("v1.2.3", controller=controller)

    def test_prepared_controller_reads_installed_environment_and_preserves_path(self):
        controller = self.dev.ProdDeployControllerOptions(
            prepared_artifact=Path("prepared"), expected_full_commit="a" * 40,
            paired_database_upgrade=True,
        )
        installed = {
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin",
            "PHOENIX_PORT": "9443",
            "PHOENIX_PASSWORD": "installed-secret",
            "PHOENIX_VERSION": "old-version",
        }
        with mock.patch.object(self.dev, "_launchd_env_from_plist", return_value=dict(installed)) as read:
            env, env_file = self.dev._launchd_candidate_env(controller)
        self.assertEqual({key: value for key, value in installed.items() if key != "PHOENIX_VERSION"}, env)
        self.assertEqual(installed["PATH"], env["PATH"])
        self.assertIsNone(env_file)
        read.assert_called_once_with(self.dev.LAUNCHD_PLIST_PATH)


if __name__ == "__main__":
    unittest.main()
