"""Disposable helper-only modern migration policy regressions."""
import dataclasses
import importlib.util
import json
import os
import plistlib
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from contextlib import ExitStack
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("modern_migration_helper_test", ROOT / "scripts/launchd_deploy_helper.py")
helper = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = helper
spec.loader.exec_module(helper)


class Backend:
    def __init__(self, manifest):
        self.manifest = manifest
        self.state = ("not_loaded", None)
        self.disruption_started = False
        self.events = []
        self.on_start = None
        self.fail_stop = False
        self.loaded = None
        self.target = f"gui/{manifest.uid}/{manifest.label}"

    def inspect(self):
        return self.state

    def stop(self):
        self.events.append("stop")
        if self.fail_stop:
            raise helper.ActivationError("injected teardown failure")
        self.state = ("not_loaded", None)
        self.disruption_started = True
        return None

    def start(self, old_pid, *, plist_path=None):
        self.events.append(("start", plist_path))
        assert not Path(self.manifest.target_plist).exists(), "unverified auto-load plist published"
        self.loaded = plist_path
        self.state = ("running", 123)
        if self.on_start:
            self.on_start()
        return 123

    def run(self, *args, **kwargs):
        return subprocess.CompletedProcess([], 0, f"path = {self.loaded}\n", "")


def fixture(root, version=112):
    transaction = root / "transaction"
    transaction.mkdir(mode=0o700)
    database = root / "modern.sqlite3"
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
        connection.executemany("INSERT INTO _migrations VALUES (?, ?)", [(number, f'migration-{number}') for number in range(1, version + 1)])
        connection.execute("CREATE TABLE preserved (value TEXT)")
        connection.execute("INSERT INTO preserved VALUES ('original')")
    backup = transaction / "backup.sqlite3"
    rehearsal = transaction / "rehearsal.sqlite3"
    shutil.copy2(database, backup)
    shutil.copy2(backup, rehearsal)
    retained_helper = transaction / "helper.py"
    shutil.copy2(Path(helper.__file__), retained_helper)
    files = {}
    for name, value in {
        "candidate_binary": b"candidate binary",
        "rollback_binary": b"predecessor binary",
        "candidate_plist": plistlib.dumps({"Label": "test.modern", "EnvironmentVariables": {"PHOENIX_DB_PATH": str(database)}, "generation": "candidate"}),
        "rollback_plist": plistlib.dumps({"Label": "test.modern", "EnvironmentVariables": {"PHOENIX_DB_PATH": str(database)}, "generation": "previous"}),
    }.items():
        files[name] = transaction / name
        files[name].write_bytes(value)
    target_binary = root / "installed-binary"
    target_plist = root / "installed.plist"
    shutil.copy2(files["rollback_binary"], target_binary)
    shutil.copy2(files["rollback_plist"], target_plist)
    manifest = helper.Manifest(
        manifest_version=1, transaction_id="modern-tx", source_kind="published_release",
        source_commit="b" * 40, release_tag="v2.0.0", release_commit="b" * 40,
        expected=helper.Identity("2.0.0", "b" * 40), previous=helper.Identity("1.0.0", "a" * 40),
        previous_deployed_sha="a" * 40,
        **{name: str(path) for name, path in files.items()},
        **{name + "_sha256": helper.sha256(path) for name, path in files.items()},
        target_binary=str(target_binary), target_plist=str(target_plist), label="test.modern",
        helper_label="test.modern.activation", uid=os.getuid(), health_url="http://127.0.0.1:1/api/version",
        health_insecure_tls=False, previous_health_url="http://127.0.0.1:2/api/version",
        previous_health_insecure_tls=False, previous_health_json=True,
        active_path=str(root / "active"), status_path=str(transaction / "status.json"),
        deployed_sha_path=str(root / "deployed.sha"), lock_path=str(root / "operation.lock"),
        claim_lock_path=str(root / "claim.lock"), created_at="2026-01-01T00:00:00+00:00",
        ordinary_migration=helper.OrdinaryMigration(
            database_path=str(database), database_sha256=helper.sha256(database),
            backup_path=str(backup), backup_sha256=helper.sha256(backup),
            rehearsal_path=str(rehearsal), rehearsal_sha256=helper.sha256(rehearsal),
            previous_binary_sha256=helper.sha256(files["rollback_binary"]),
            previous_plist_sha256=helper.sha256(files["rollback_plist"]),
            controller_helper_path=str(retained_helper), controller_helper_sha256=helper.sha256(retained_helper),
        ),
    )
    Path(manifest.active_path).write_text(manifest.transaction_id)
    helper.write_status(manifest, "prepared")
    return manifest, Backend(manifest)


class ModernMigrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.manifest, self.backend = fixture(self.root)
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(mock.patch.object(helper, "__file__", self.manifest.ordinary_migration.controller_helper_path))
        self.stack.enter_context(mock.patch.object(helper, "Launchctl", return_value=self.backend))
        self.lsof = self.stack.enter_context(mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")))
        self.health = self.stack.enter_context(mock.patch.object(helper, "wait_for_identity"))

    def test_sparse_modern_ledger_is_not_admitted(self):
        with sqlite3.connect(self.manifest.ordinary_migration.backup_path) as connection:
            connection.execute("DELETE FROM _migrations WHERE version = 45")
        with self.assertRaisesRegex(helper.ActivationError, "complete contiguous"):
            helper.ordinary_database_ledger(Path(self.manifest.ordinary_migration.backup_path))

    def test_prepared_handoff_failure_can_explicitly_resume_matched_predecessor(self):
        self.assertEqual(helper.read_status(self.manifest)["state"], "prepared")
        Path(self.manifest.status_path).write_text(json.dumps({"transaction_id": self.manifest.transaction_id, "state": "prepared", "preparing_pid": 999999}))
        with mock.patch.object(helper.os, "kill", side_effect=ProcessLookupError()):
            self.assertEqual(helper.resume_migration(self.manifest), "migration_resumed")
        self.assertEqual([event for event in self.backend.events if isinstance(event, tuple)], [("start", self.manifest.rollback_plist)])

    def test_live_prepared_controller_prevents_resume_and_any_mutation(self):
        Path(self.manifest.status_path).write_text(json.dumps({"transaction_id": self.manifest.transaction_id, "state": "prepared", "preparing_pid": os.getpid()}))
        before = Path(self.manifest.status_path).read_bytes()
        with self.assertRaisesRegex(helper.ActivationError, "controller still alive"):
            helper.resume_migration(self.manifest)
        self.assertEqual(Path(self.manifest.status_path).read_bytes(), before)
        self.assertEqual(self.backend.events, [])

    def test_same_ledger_wrong_backup_is_rejected_without_activation(self):
        migration = self.manifest.ordinary_migration
        with sqlite3.connect(migration.backup_path) as connection:
            connection.execute("UPDATE preserved SET value = 'wrong private data'")
        shutil.copyfile(migration.backup_path, migration.rehearsal_path)
        digest = helper.sha256(Path(migration.backup_path))
        self.manifest = dataclasses.replace(self.manifest, ordinary_migration=dataclasses.replace(migration, backup_sha256=digest, rehearsal_sha256=digest))
        self.backend.manifest = self.manifest
        with self.assertRaisesRegex(helper.ActivationError, "does not match stopped source contents"):
            helper.activate(self.manifest)
        self.assertFalse(any(isinstance(event, tuple) and event[0] == "start" for event in self.backend.events))
        self.assertTrue(Path(self.manifest.target_plist).exists())

    def test_logically_matching_sqlite_backup_can_differ_in_physical_bytes(self):
        migration = self.manifest.ordinary_migration
        with sqlite3.connect(migration.database_path) as connection:
            connection.execute("PRAGMA user_version = 7")
        with sqlite3.connect(migration.database_path) as source, sqlite3.connect(migration.backup_path) as target:
            source.backup(target)
        with sqlite3.connect(migration.backup_path) as connection:
            connection.execute("VACUUM")
        shutil.copyfile(migration.backup_path, migration.rehearsal_path)
        database_sha = helper.sha256(Path(migration.database_path))
        backup_sha = helper.sha256(Path(migration.backup_path))
        self.manifest = dataclasses.replace(self.manifest, ordinary_migration=dataclasses.replace(migration, database_sha256=database_sha, backup_sha256=backup_sha, rehearsal_sha256=backup_sha))
        self.backend.manifest = self.manifest
        self.assertNotEqual(database_sha, backup_sha)
        helper.validate_ordinary_receipt(self.manifest)

    def mutate(self):
        with sqlite3.connect(self.manifest.ordinary_migration.database_path) as connection:
            connection.execute("UPDATE preserved SET value = 'candidate mutated'")
            connection.execute("INSERT INTO _migrations VALUES (113, 'candidate')")

    def fail_activation(self):
        self.backend.on_start = self.mutate
        self.health.side_effect = helper.ActivationError("candidate health failed")
        self.assertEqual("migration_failed_stopped", helper.activate(self.manifest))
        self.backend.on_start = None
        self.health.side_effect = None

    def restore_manually(self):
        shutil.copyfile(self.manifest.ordinary_migration.backup_path, self.manifest.ordinary_migration.database_path)

    def test_manifest_parses_optional_object_and_rejects_other_shapes(self):
        path = self.root / "manifest.json"
        raw = dataclasses.asdict(self.manifest)
        path.write_text(json.dumps(raw))
        self.assertEqual(self.manifest, helper.Manifest.load(path))
        raw.pop("ordinary_migration")
        path.write_text(json.dumps(raw))
        self.assertIsNone(helper.Manifest.load(path).ordinary_migration)
        for value in (False, [], "migration", {}):
            raw["ordinary_migration"] = value
            path.write_text(json.dumps(raw))
            with self.assertRaises((helper.ActivationError, TypeError)):
                helper.Manifest.load(path)

    def test_candidate_mutation_health_failure_never_starts_predecessor_or_restores_db(self):
        with mock.patch.object(helper, "restore") as restore, mock.patch.object(helper, "restore_database") as restore_db:
            self.fail_activation()
        restore.assert_not_called()
        restore_db.assert_not_called()
        self.assertEqual([("start", self.manifest.candidate_plist), "stop"], self.backend.events)
        self.assertEqual(("not_loaded", None), self.backend.state)
        self.assertEqual(self.manifest.candidate_binary_sha256, helper.sha256(Path(self.manifest.target_binary)))
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue(list(Path(self.manifest.status_path).parent.glob("*.quarantined")))
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertFalse(helper.status_is_durable_terminal(self.manifest))
        self.assertIn("candidate health failed", helper.read_status(self.manifest)["failure"])
        with sqlite3.connect(self.manifest.ordinary_migration.database_path) as connection:
            self.assertEqual(("candidate mutated",), connection.execute("SELECT value FROM preserved").fetchone())

    def test_teardown_failure_is_diagnostic_and_retains_claim(self):
        self.backend.fail_stop = True
        self.fail_activation()
        self.assertIn("teardown unconfirmed", helper.read_status(self.manifest)["rollback_failure"])
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertFalse(Path(self.manifest.target_plist).exists())

    def test_resume_refuses_unrestored_database_without_starting(self):
        self.fail_activation()
        before = Path(self.manifest.ordinary_migration.database_path).read_bytes()
        self.assertEqual("migration_failed_stopped", helper.resume_migration(self.manifest))
        self.assertEqual([("start", self.manifest.candidate_plist), "stop", "stop"], self.backend.events)
        self.assertEqual(before, Path(self.manifest.ordinary_migration.database_path).read_bytes())
        self.assertIn("restored database checksum mismatch", helper.read_status(self.manifest)["failure"])
        self.assertTrue(Path(self.manifest.active_path).exists())

    def test_manual_restore_then_resume_exact_private_predecessor_and_publish(self):
        self.fail_activation()
        self.restore_manually()
        before = Path(self.manifest.ordinary_migration.database_path).read_bytes()
        self.assertEqual("migration_resumed", helper.resume_migration(self.manifest))
        self.assertEqual(("start", self.manifest.rollback_plist), self.backend.events[-1])
        self.assertEqual(before, Path(self.manifest.ordinary_migration.database_path).read_bytes())
        self.assertEqual(self.manifest.rollback_binary_sha256, helper.sha256(Path(self.manifest.target_binary)))
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.rollback_plist))
        self.assertEqual("a" * 40, Path(self.manifest.deployed_sha_path).read_text().strip())
        self.assertTrue(helper.status_is_durable_terminal(self.manifest))
        self.assertTrue(Path(self.manifest.active_path).exists(), "function leaves release to main")
        self.health.assert_called_with(self.manifest, self.manifest.previous, health_url=self.manifest.previous_health_url, health_insecure_tls=False, health_json=True)

    def test_activation_main_retains_claim_after_candidate_mutation(self):
        self.backend.on_start = self.mutate
        self.health.side_effect = helper.ActivationError("candidate unhealthy")
        argv = ["helper", "activate", "--manifest", "unused", "--helper-label", self.manifest.helper_label, "--uid", str(self.manifest.uid)]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=self.manifest), mock.patch.object(helper, "request_helper_bootout"):
            self.assertEqual(1, helper.main())
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertEqual("migration_failed_stopped", helper.read_status(self.manifest)["state"])
        self.assertEqual([("start", self.manifest.candidate_plist), "stop"], self.backend.events)

    def test_resume_main_releases_owned_claim_without_bootout(self):
        self.fail_activation()
        self.restore_manually()
        argv = ["helper", "resume-migration", "--manifest", "unused", "--helper-label", self.manifest.helper_label, "--uid", str(self.manifest.uid)]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=self.manifest), mock.patch.object(helper, "request_helper_bootout") as bootout:
            self.assertEqual(0, helper.main())
        self.assertFalse(Path(self.manifest.active_path).exists())
        bootout.assert_not_called()

    def test_resume_refuses_sidecars_and_does_not_remove_them(self):
        self.fail_activation()
        self.restore_manually()
        for suffix in ("-wal", "-shm", "-journal"):
            with self.subTest(suffix=suffix):
                sidecar = Path(self.manifest.ordinary_migration.database_path + suffix)
                sidecar.write_bytes(b"operator-owned")
                self.assertEqual("migration_failed_stopped", helper.resume_migration(self.manifest))
                self.assertEqual(b"operator-owned", sidecar.read_bytes())
                self.assertIn("sidecars remain", helper.read_status(self.manifest)["failure"])
                sidecar.unlink()
        self.assertEqual(1, sum(isinstance(event, tuple) for event in self.backend.events))

    def test_resume_health_failure_tears_down_and_checkpoint_refuses_replay(self):
        self.fail_activation()
        self.restore_manually()
        self.health.side_effect = helper.ActivationError("predecessor unhealthy")
        self.assertEqual("migration_failed_stopped", helper.resume_migration(self.manifest))
        self.assertEqual(("not_loaded", None), self.backend.state)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertEqual("migration_resume_started", helper.read_status(self.manifest)["recovery_mode"])
        self.health.side_effect = None
        starts = sum(isinstance(event, tuple) for event in self.backend.events)
        self.assertEqual("migration_failed_stopped", helper.resume_migration(self.manifest))
        self.assertEqual(starts, sum(isinstance(event, tuple) for event in self.backend.events))
        self.assertIn("refuse startup replay", helper.read_status(self.manifest)["failure"])

    def test_resume_finalization_failure_stops_verified_predecessor(self):
        self.fail_activation()
        self.restore_manually()
        original = helper.commit_atomic_install
        def fail_publish(source, target):
            if target == Path(self.manifest.target_plist):
                raise OSError("publication failed")
            original(source, target)
        with mock.patch.object(helper, "commit_atomic_install", side_effect=fail_publish):
            self.assertEqual("migration_failed_stopped", helper.resume_migration(self.manifest))
        self.assertEqual(("not_loaded", None), self.backend.state)
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertFalse(Path(self.manifest.target_plist).exists())

    def test_activation_requires_stopped_service_and_exclusive_database(self):
        for state in (("running", 42), ("waiting", None), ("not_loaded", 42)):
            self.backend.state = state
            with self.assertRaisesRegex(helper.ActivationError, "exactly stopped"):
                helper.activate(self.manifest)
            self.assertEqual([], self.backend.events)
        self.backend.state = ("not_loaded", None)
        for result in (subprocess.CompletedProcess([], 0, "42\n", ""), subprocess.CompletedProcess([], 1, "", "denied"), subprocess.CompletedProcess([], 2, "", "")):
            self.lsof.return_value = result
            with self.assertRaises(helper.ActivationError):
                helper.activate(self.manifest)
            self.assertEqual([], self.backend.events)
        self.assertTrue(Path(self.manifest.target_plist).exists())
        self.assertTrue(helper.status_is_durable_terminal(self.manifest))

    def test_receipt_hash_changes_fail_before_disruption(self):
        for name in ("database_path", "backup_path", "rehearsal_path", "controller_helper_path"):
            path = Path(getattr(self.manifest.ordinary_migration, name))
            before = path.read_bytes()
            path.write_bytes(before + b"changed")
            with self.subTest(name=name), self.assertRaisesRegex(helper.ActivationError, "checksum mismatch"):
                helper.activate(self.manifest)
            path.write_bytes(before)
            self.assertEqual([], self.backend.events)
        self.assertTrue(Path(self.manifest.target_plist).exists())

    def test_mode_rejects_paired_source_first_install_wrong_helper_and_database(self):
        migration = self.manifest.ordinary_migration
        cases = [
            dataclasses.replace(self.manifest, paired_database_upgrade=object()),
            dataclasses.replace(self.manifest, source_kind="prepared_artifact"),
            dataclasses.replace(self.manifest, previous=None),
            dataclasses.replace(self.manifest, ordinary_migration=dataclasses.replace(migration, previous_binary_sha256="c" * 64)),
            dataclasses.replace(self.manifest, ordinary_migration=dataclasses.replace(migration, controller_helper_path=str(Path(helper.__file__).parent / "other.py"))),
        ]
        for manifest in cases:
            with self.subTest(manifest=manifest.source_kind), self.assertRaises(helper.ActivationError):
                helper.validate_manifest_mode(manifest)
        with self.assertRaisesRegex(helper.ActivationError, "version <= 69"):
            helper.validate_legacy_database(Path(migration.database_path))

    def test_modern_ledger_gate_and_receipt_ledger_equality(self):
        migration = self.manifest.ordinary_migration
        for version in (69, 70):
            with sqlite3.connect(migration.database_path) as connection:
                connection.execute("DELETE FROM _migrations")
                connection.executemany("INSERT INTO _migrations VALUES (?, ?)", [(number, f'migration-{number}') for number in range(1, version + 1)])
            changed = dataclasses.replace(self.manifest, ordinary_migration=dataclasses.replace(migration, database_sha256=helper.sha256(Path(migration.database_path))))
            with self.subTest(version=version), self.assertRaisesRegex(helper.ActivationError, "modern migration ledger|ledger differs"):
                helper.validate_ordinary_receipt(changed)
        self.assertEqual([], self.backend.events)

    def test_initial_integrity_failure_even_when_hashes_match(self):
        migration = self.manifest.ordinary_migration
        Path(migration.database_path).write_bytes(b"not SQLite")
        changed = dataclasses.replace(self.manifest, ordinary_migration=dataclasses.replace(migration, database_sha256=helper.sha256(Path(migration.database_path))))
        with self.assertRaisesRegex(helper.ActivationError, "integrity/ledger"):
            helper.activate(changed)
        self.assertEqual([], self.backend.events)

    def test_receipts_reject_relative_symlink_and_hardlinked_paths(self):
        migration = self.manifest.ordinary_migration
        relative = dataclasses.replace(self.manifest, ordinary_migration=dataclasses.replace(migration, rehearsal_path="relative.sqlite3"))
        with self.assertRaisesRegex(helper.ActivationError, "absolute non-symlinks"):
            helper.validate_manifest_mode(relative)
        rehearsal = Path(migration.rehearsal_path)
        rehearsal.unlink()
        rehearsal.symlink_to(migration.backup_path)
        with self.assertRaisesRegex(helper.ActivationError, "absolute non-symlinks"):
            helper.validate_manifest_mode(self.manifest)
        rehearsal.unlink()
        os.link(migration.backup_path, rehearsal)
        with self.assertRaisesRegex(helper.ActivationError, "hardlinked"):
            helper.validate_manifest_mode(self.manifest)

    def test_health_failure_without_mutation_still_never_restarts_predecessor(self):
        self.health.side_effect = helper.ActivationError("candidate health failed before migration")
        before = Path(self.manifest.ordinary_migration.database_path).read_bytes()
        self.assertEqual("migration_failed_stopped", helper.activate(self.manifest))
        self.assertEqual(before, Path(self.manifest.ordinary_migration.database_path).read_bytes())
        self.assertEqual([("start", self.manifest.candidate_plist), "stop"], self.backend.events)
        self.assertTrue(Path(self.manifest.active_path).exists())

    def test_precondition_failure_main_releases_only_owned_claim_without_start(self):
        Path(self.manifest.ordinary_migration.rehearsal_path).write_bytes(b"tampered")
        argv = ["helper", "activate", "--manifest", "unused", "--helper-label", self.manifest.helper_label, "--uid", str(self.manifest.uid)]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=self.manifest), mock.patch.object(helper, "request_helper_bootout"):
            self.assertEqual(1, helper.main())
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertEqual([], self.backend.events)
        self.assertTrue(Path(self.manifest.target_plist).exists())

    def test_resume_rejects_changed_captured_or_installed_binary(self):
        self.fail_activation()
        self.restore_manually()
        for name in ("rollback_binary", "rollback_plist", "target_binary"):
            path = Path(getattr(self.manifest, name))
            before = path.read_bytes()
            path.write_bytes(before + b"tampered")
            with self.subTest(name=name):
                self.assertEqual("migration_failed_stopped", helper.resume_migration(self.manifest))
            path.write_bytes(before)
        self.assertEqual(1, sum(isinstance(event, tuple) for event in self.backend.events))
        self.assertTrue(Path(self.manifest.active_path).exists())

    def test_resume_post_terminal_retained_claim_finalizes_without_runtime_or_db_replay(self):
        self.fail_activation()
        self.restore_manually()
        self.assertEqual("migration_resumed", helper.resume_migration(self.manifest))
        self.mutate()
        database = Path(self.manifest.ordinary_migration.database_path)
        before = database.read_bytes()
        status = Path(self.manifest.status_path).read_bytes()
        events = list(self.backend.events)
        argv = ["helper", "resume-migration", "--manifest", "unused", "--helper-label", self.manifest.helper_label, "--uid", str(self.manifest.uid)]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=self.manifest), mock.patch.object(helper, "validate_ordinary_receipt", side_effect=AssertionError("no offline DB validation after terminal success")):
            self.assertEqual(0, helper.main())
        self.assertEqual(events, self.backend.events)
        self.assertEqual(("running", 123), self.backend.state)
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.rollback_plist))
        self.assertEqual(before, database.read_bytes())
        self.assertEqual(status, Path(self.manifest.status_path).read_bytes())
        self.assertFalse(Path(self.manifest.active_path).exists())

    def test_terminal_resume_verification_failure_preserves_success_and_owned_fence(self):
        for defect in ("health", "binary", "private_plist", "published_plist", "loaded_plist", "stopped", "deployed_sha"):
            with self.subTest(defect=defect), tempfile.TemporaryDirectory() as td:
                manifest, backend = fixture(Path(td).resolve())
                helper.write_status(manifest, "migration_failed_stopped")
                with mock.patch.object(helper, "__file__", manifest.ordinary_migration.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=backend):
                    self.assertEqual("migration_resumed", helper.resume_migration(manifest))
                    if defect == "health":
                        self.health.side_effect = helper.ActivationError("identity mismatch")
                    elif defect == "binary":
                        Path(manifest.target_binary).write_bytes(b"wrong binary")
                    elif defect == "private_plist":
                        Path(manifest.rollback_plist).write_bytes(b"wrong private plist")
                    elif defect == "published_plist":
                        data = Path(manifest.target_plist).read_bytes()
                        Path(manifest.target_plist).unlink()
                        Path(manifest.target_plist).write_bytes(data)
                    elif defect == "loaded_plist":
                        backend.loaded = manifest.candidate_plist
                    elif defect == "stopped":
                        backend.state = ("not_loaded", None)
                    else:
                        Path(manifest.deployed_sha_path).write_text("c" * 40)
                    status = Path(manifest.status_path).read_bytes()
                    events = list(backend.events)
                    with self.assertRaises((helper.ActivationError, OSError)):
                        helper.resume_migration(manifest)
                    self.assertEqual(status, Path(manifest.status_path).read_bytes())
                    self.assertEqual(events, backend.events)
                    self.assertTrue(Path(manifest.active_path).exists())
                    self.assertTrue(Path(manifest.target_plist).exists())
                    self.health.side_effect = None

    def test_terminal_retry_preserves_absent_previous_deployed_marker(self):
        self.manifest = dataclasses.replace(self.manifest, previous_deployed_sha=None)
        self.backend.manifest = self.manifest
        self.fail_activation()
        self.restore_manually()
        self.assertEqual("migration_resumed", helper.resume_migration(self.manifest))
        events = list(self.backend.events)
        self.assertEqual("migration_resumed", helper.resume_migration(self.manifest))
        self.assertEqual(events, self.backend.events)
        self.assertFalse(Path(self.manifest.deployed_sha_path).exists())

    def test_terminal_claim_release_failure_can_retry_without_teardown(self):
        self.fail_activation()
        self.restore_manually()
        self.assertEqual("migration_resumed", helper.resume_migration(self.manifest))
        status = Path(self.manifest.status_path).read_bytes()
        events = list(self.backend.events)
        argv = ["helper", "resume-migration", "--manifest", "unused", "--helper-label", self.manifest.helper_label, "--uid", str(self.manifest.uid)]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=self.manifest):
            with mock.patch.object(helper, "release_claim", side_effect=OSError("claim release interrupted")):
                self.assertEqual(1, helper.main())
            self.assertTrue(Path(self.manifest.active_path).exists())
            self.assertEqual(status, Path(self.manifest.status_path).read_bytes())
            self.assertEqual(events, self.backend.events)
            self.assertEqual(0, helper.main())
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertEqual(events, self.backend.events)

    def test_activation_success_publishes_only_after_exact_identity_and_no_db_writes(self):
        before = Path(self.manifest.ordinary_migration.database_path).read_bytes()
        self.health.side_effect = lambda *_: self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertEqual("committed", helper.activate(self.manifest))
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.candidate_plist))
        self.assertEqual(before, Path(self.manifest.ordinary_migration.database_path).read_bytes())
        self.assertTrue(helper.status_is_durable_terminal(self.manifest))

    def test_publication_failure_stops_candidate_even_after_commit_checkpoint(self):
        original = helper.commit_atomic_install
        def fail_publish(source, target):
            if target == Path(self.manifest.target_plist):
                raise OSError("publication failed")
            original(source, target)
        with mock.patch.object(helper, "commit_atomic_install", side_effect=fail_publish):
            self.assertEqual("migration_failed_stopped", helper.activate(self.manifest))
        self.assertEqual(("not_loaded", None), self.backend.state)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertFalse(helper.status_is_durable_terminal(self.manifest))

    def test_resume_wrong_claim_and_wrong_state_have_no_host_effects(self):
        self.fail_activation()
        self.backend.events.clear()
        Path(self.manifest.active_path).write_text("another-tx")
        with self.assertRaisesRegex(helper.ActivationError, "own a retained"):
            helper.resume_migration(self.manifest)
        Path(self.manifest.active_path).write_text(self.manifest.transaction_id)
        helper.write_status(self.manifest, "committed")
        with self.assertRaisesRegex(helper.ActivationError, "own a retained"):
            helper.resume_migration(self.manifest)
        self.assertEqual([], self.backend.events)


if __name__ == "__main__":
    unittest.main()
