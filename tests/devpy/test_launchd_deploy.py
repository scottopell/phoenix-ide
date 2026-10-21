import datetime
import fcntl
import importlib.util
import contextlib
import dataclasses
import io
import json
import os
import plistlib
import subprocess
import shutil
import sqlite3
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


class ManifestShapeTests(unittest.TestCase):
    def test_load_rejects_malformed_mode_objects_and_field_types(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(root)
            path = root / "manifest.json"
            for mode in ("paired_database_upgrade", "current_schema_upgrade", "ordinary_forward_migration"):
                complete = mode_evidence(root, mode)
                path.write_text(json.dumps(dataclasses.asdict(manifest) | {mode: complete}))
                self.assertIsNotNone(getattr(helper.Manifest.load(path), mode))
                malformed = [{}, [], "", False, 0, "mode"]
                for field in complete:
                    malformed.append({key: value for key, value in complete.items() if key != field})
                    for wrong in (None, False, 1, [], {}, ""):
                        malformed.append(complete | {field: wrong})
                malformed.append(complete | {"unknown": "extra"})
                for value in malformed:
                    with self.subTest(mode=mode, value=value):
                        path.write_text(json.dumps(dataclasses.asdict(manifest) | {mode: value}))
                        with self.assertRaises(helper.ActivationError):
                            helper.Manifest.load(path)

    def test_load_wraps_top_level_and_identity_shape_type_errors(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            payload = dataclasses.asdict(make_manifest(root))
            path = root / "manifest.json"
            malformed = [[], None, {}, payload | {"expected": {}}, payload | {"previous": {}},
                         payload | {"expected": {"version": 2, "git_sha": "a" * 40}},
                         payload | {"uid": "501"}, payload | {"health_insecure_tls": 1}]
            malformed.extend({key: value for key, value in payload.items() if key != missing}
                             for missing in ("expected", "transaction_id", "candidate_binary"))
            for value in malformed:
                with self.subTest(value=value):
                    path.write_text(json.dumps(value))
                    with self.assertRaises(helper.ActivationError):
                        helper.Manifest.load(path)

    def test_helper_admission_rejects_source_and_mode_cross_matrix_without_mutation(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            base = make_manifest(root)
            for mode, record_type in (("paired_database_upgrade", helper.PairedDatabaseUpgrade),
                                      ("current_schema_upgrade", helper.CurrentSchemaUpgrade)):
                for source in (("prepared_artifact",) if mode == "paired_database_upgrade" else
                               ("prepared_artifact", "published_release")):
                    manifest = dataclasses.replace(base, source_kind=source,
                        **{mode: record_type(**mode_evidence(root, mode))})
                    for action, state in (("cleanup_precondition_capacity", "precondition_cleanup_pending"),
                                          ("finalize_paired", "committed"),
                                          ("recover_paired", "activation_failed_rollback_failed")):
                        for status_source in (None, "prepared_artifact", "published_release", "local_head", "invalid"):
                            for prepared_mode in (None, "paired", "current_schema", "invalid"):
                                expected_mode = "paired" if mode == "paired_database_upgrade" else "current_schema"
                                if status_source == source and prepared_mode == expected_mode:
                                    continue
                                with self.subTest(mode=mode, source=source, action=action,
                                                  status_source=status_source, prepared_mode=prepared_mode):
                                    status = {"transaction_id": manifest.transaction_id, "state": state}
                                    if status_source is not None:
                                        status["source_kind"] = status_source
                                    if prepared_mode is not None:
                                        status["prepared_mode"] = prepared_mode
                                    Path(manifest.status_path).write_text(json.dumps(status))
                                    Path(manifest.active_path).write_text(manifest.transaction_id)
                                    before = Path(manifest.status_path).read_bytes()
                                    with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
                                        helper, "validate_manifest_mode"
                                    ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
                                        helper.subprocess, "run"
                                    ) as run, mock.patch.object(helper, "_remove_capacity_files") as remove:
                                        with self.assertRaises(helper.ActivationError):
                                            getattr(helper, action)(manifest, current_schema=mode == "current_schema_upgrade")
                                        launchctl.assert_not_called()
                                        run.assert_not_called()
                                        remove.assert_not_called()
                                    self.assertEqual(Path(manifest.status_path).read_bytes(), before)
                                    self.assertEqual(Path(manifest.active_path).read_text(), manifest.transaction_id)


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


def mode_evidence(transaction: Path, mode="paired_database_upgrade"):
    script = transaction / "retained-helper.py"
    script.write_bytes(b"retained controller helper")
    prepared = {
        "database_path": str(transaction.parent / "prod.db"),
        "backup_path": str(transaction / "backup.sqlite3"),
        "proof_path": str(transaction / "proof.json"),
        "controller_source_commit": "c" * 40,
        "controller_helper_path": str(script),
        "controller_helper_sha256": helper.sha256(script),
    }
    registry = {
        "migration_registry_path": str(transaction / "migrations.rs"),
        "migration_registry_sha256": "d" * 64,
    }
    if mode == "current_schema_upgrade":
        return prepared | registry
    if mode == "ordinary_forward_migration":
        return {"database_path": prepared["database_path"], "helper_path": str(script),
                "helper_sha256": helper.sha256(script)} | registry
    return prepared


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
        label="test.phoenix.server", helper_label="com.phoenix-ide.deploy.tx", uid=os.getuid(), health_url="http://127.0.0.1:1/api/version",
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
    def test_pre_disruption_handoff_is_read_only_and_returns_activation_inputs(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(
                root,
                previous=helper.Identity("1.0.0", "a" * 40),
            )
            before = {path: path.read_bytes() for path in root.iterdir() if path.is_file()}

            validated = helper.validate_pre_disruption_handoff(manifest)

            self.assertEqual(validated.candidate_binary, Path(manifest.candidate_binary))
            self.assertEqual(validated.candidate_plist, Path(manifest.candidate_plist))
            self.assertEqual(
                validated.rollback,
                (Path(manifest.rollback_binary), Path(manifest.rollback_plist)),
            )
            self.assertEqual(
                before,
                {path: path.read_bytes() for path in root.iterdir() if path.is_file()},
            )
            self.assertFalse(Path(manifest.status_path).exists())
            self.assertFalse(Path(manifest.active_path).exists())

    def test_read_only_paired_path_validation_never_creates_transaction_directory(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "missing" / "transaction"
            manifest = dataclasses.replace(
                make_manifest(root),
                paired_database_upgrade=helper.PairedDatabaseUpgrade(
                    database_path=str(root / "prod.db"),
                    backup_path=str(transaction / "backup.sqlite3"),
                    proof_path=str(transaction / "proof.json"),
                    controller_source_commit="c" * 40,
                    controller_helper_sha256="d" * 64,
                    controller_helper_path=str(root / "controller-helper.py"),
                ),
            )

            with self.assertRaisesRegex(helper.ActivationError, "transaction directory is unavailable"):
                helper._paired_path_context(manifest, prepare_transaction=False)
            self.assertFalse(transaction.exists())
            self.assertFalse(transaction.parent.exists())

    def test_exact_prepared_source114_handoff_binds_plist_and_timeouts(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(root)
            target_binary = Path(manifest.target_binary)
            candidate_plist = Path(manifest.candidate_plist)
            rollback_plist = Path(manifest.rollback_plist)
            bound_plist = plistlib.dumps(
                {
                    "Label": manifest.label,
                    "ProgramArguments": [str(target_binary)],
                }
            )
            candidate_plist.write_bytes(bound_plist)
            rollback_plist.write_bytes(bound_plist)
            upgrade = helper.PreparedSource114Upgrade(
                database_path=str(root / "prod.db"),
                backup_path=str(root / "backup.sqlite3"),
                proof_path=str(root / "proof.json"),
                controller_source_commit="c" * 40,
                controller_helper_sha256="d" * 64,
                controller_helper_path=str(root / "controller-helper.py"),
                migration_registry_path=str(root / "migrations.rs"),
                migration_registry_sha256="e" * 64,
                prepared_source_baseline=114,
                candidate_registry_expected_sha256="f" * 64,
            )
            manifest = dataclasses.replace(
                manifest,
                source_kind="prepared_artifact",
                release_tag=None,
                release_commit=None,
                candidate_plist_sha256=helper.sha256(candidate_plist),
                rollback_plist_sha256=helper.sha256(rollback_plist),
                transition_timeout_secs=helper.DEFAULT_TRANSITION_TIMEOUT_SECS,
                health_timeout_secs=helper.DEFAULT_HEALTH_TIMEOUT_SECS,
                current_schema_upgrade=upgrade,
            )
            with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
                helper, "validate_manifest_mode"
            ), mock.patch.object(
                helper,
                "_current_schema_registry",
                side_effect=AssertionError("database registry opened by read-only handoff"),
            ), mock.patch.object(
                helper,
                "validate_current_schema_database",
                side_effect=AssertionError("database opened by read-only handoff"),
            ):
                validated = helper.validate_pre_disruption_handoff(manifest)
            self.assertEqual(validated.candidate_plist, candidate_plist)

            candidate_plist.write_bytes(plistlib.dumps({"Program": str(target_binary)}))
            manifest = dataclasses.replace(
                manifest,
                candidate_plist_sha256=helper.sha256(candidate_plist),
            )
            with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
                helper, "validate_manifest_mode"
            ):
                with self.assertRaisesRegex(helper.ActivationError, "label does not match"):
                    helper.validate_pre_disruption_handoff(manifest)

            manifest = dataclasses.replace(
                manifest,
                transition_timeout_secs=helper.DEFAULT_TRANSITION_TIMEOUT_SECS + 1,
            )
            with self.assertRaisesRegex(helper.ActivationError, "transition_timeout_secs"):
                helper.validate_pre_disruption_handoff(manifest)

    def test_activation_consumes_pre_disruption_handoff_before_preparing_mutation(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = FakeLaunchctl(manifest)
            launchctl.events = []
            with mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(
                helper,
                "validate_pre_disruption_handoff",
                side_effect=helper.ActivationError("read-only admission rejected"),
            ) as validate, mock.patch.object(helper, "prepare_atomic_install") as prepare:
                with self.assertRaisesRegex(helper.ActivationError, "read-only admission rejected"):
                    helper.activate(manifest)
            validate.assert_called_once_with(manifest)
            prepare.assert_not_called()
            self.assertNotIn("stop", launchctl.events)
            self.assertNotIn("start", launchctl.events)

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
            self.assertTrue(Path(manifest.paired_database_upgrade.backup_path).exists())
            self.assertTrue(Path(manifest.paired_database_upgrade.proof_path).exists())
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

    def test_terminal_status_failure_preserves_verified_predecessor_and_retry_writes(self):
        import sqlite3

        for terminal_write in ("before_replace", "after_replace"):
            with self.subTest(terminal_write=terminal_write), tempfile.TemporaryDirectory() as td:
                original_write = helper.write_status

                def fail_terminal(manifest, state, **kwargs):
                    if state == "activation_failed_rolled_back":
                        if terminal_write == "after_replace":
                            original_write(manifest, state, **kwargs)
                        raise OSError("terminal persistence failed")
                    original_write(manifest, state, **kwargs)

                with mock.patch.object(helper, "write_status", side_effect=fail_terminal):
                    result = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate failed"))
                state, manifest, database, *_rest, events, launchctl = result
                self.assertEqual(state, "activation_failed_rollback_failed")
                self.assertEqual(events, ["stop", "start", "stop", "start"])
                self.assertTrue(launchctl.loaded)
                self.assertFalse(helper.status_is_durable_terminal(manifest))
                self.assertEqual(json.loads(Path(manifest.status_path).read_text())["recovery_mode"], "snapshot_restored")
                with sqlite3.connect(database) as conn:
                    conn.execute("UPDATE users SET name = 'accepted-after-recovery'")
                with mock.patch.object(helper, "__file__", manifest.paired_database_upgrade.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "restore_database") as restore_db:
                    with mock.patch.object(helper, "write_status", side_effect=fail_terminal):
                        self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
                    self.assertEqual(events, ["stop", "start", "stop", "start"])
                    self.assertTrue(launchctl.loaded)
                    self.assertEqual(helper.recover_paired(manifest), "activation_failed_rolled_back")
                    restore_db.assert_not_called()
                with sqlite3.connect(database) as conn:
                    self.assertEqual(conn.execute("SELECT name FROM users WHERE id = 1").fetchone(), ("accepted-after-recovery",))
                self.assertTrue(Path(manifest.active_path).exists())

    def test_actual_terminal_rename_fsync_then_fallback_failure_retains_claim(self):
        with tempfile.TemporaryDirectory() as td:
            original_atomic = helper.atomic_write
            injected = False
            fixture = None
            original_fixture = self._full_paired_fixture

            def capture_fixture(root):
                nonlocal fixture
                fixture = original_fixture(root)
                return fixture

            def fail_status_atomic(path, data, mode=0o600):
                nonlocal injected
                if path.name == "status.json":
                    state = json.loads(data)["state"]
                    if state == "activation_failed_rolled_back":
                        injected = True
                        with mock.patch.object(helper, "fsync_dir", side_effect=OSError("status rename durability failed")):
                            return original_atomic(path, data, mode)
                    if injected:
                        raise OSError("unresolved fallback persistence failed")
                return original_atomic(path, data, mode)

            with mock.patch.object(helper, "atomic_write", side_effect=fail_status_atomic), mock.patch.object(self, "_full_paired_fixture", side_effect=capture_fixture):
                with self.assertRaises(helper.VerifiedPredecessorFinalizationError):
                    self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate failed"))
            status = json.loads((Path(td) / "status.json").read_text())
            self.assertEqual(status["state"], "activation_failed_rolled_back")
            self.assertEqual(status["recovery_mode"], "snapshot_restored")
            self.assertTrue((Path(td) / "active").exists())
            manifest, database, _binary, _plist, copied_helper = fixture
            retained = Path(td) / "transactions" / manifest.transaction_id
            retained.mkdir(parents=True)
            (retained / "manifest.json").write_text(
                json.dumps(helper.dataclasses.asdict(manifest))
            )
            dev = load(ROOT / "dev.py", "devpy_fault_reentry_test")
            with mock.patch.object(dev, "LAUNCHD_DEPLOY_DIR", Path(td)), mock.patch.object(dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", Path(manifest.active_path)), mock.patch.object(dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(manifest.status_path)), mock.patch.object(dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", Path(td) / "claim.lock"), mock.patch.object(dev, "LAUNCHD_RESTART_ACTIVE_PATH", Path(td) / "restart-active"), mock.patch.object(dev, "LAUNCHD_RESTART_DIR", Path(td) / "restart"):
                self.assertIn("recover-paired", dev._paired_recovery_refusal(manifest.transaction_id))
                self.assertFalse(dev._status_is_terminal_for_owner(Path(manifest.status_path), manifest.transaction_id, dev._DEPLOY_TERMINAL_STATES))
                for admission in (dev._claim_launchd_deploy, dev._claim_launchd_restart):
                    with self.assertRaises(SystemExit):
                        admission("new-operation")
                    self.assertEqual(Path(manifest.active_path).read_text().strip(), manifest.transaction_id)
            import sqlite3
            with sqlite3.connect(database) as conn:
                conn.execute("UPDATE users SET name = 'after-failed-terminal-fsync'")
            with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", FakeLaunchctl), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "restore_database") as replay, mock.patch.object(FakeLaunchctl, "stop") as stop, mock.patch.object(FakeLaunchctl, "start") as start:
                self.assertEqual(helper.recover_paired(manifest), "activation_failed_rolled_back")
                replay.assert_not_called()
                stop.assert_not_called()
                start.assert_not_called()
            with sqlite3.connect(database) as conn:
                self.assertEqual(conn.execute("SELECT name FROM users WHERE id = 1").fetchone(), ("after-failed-terminal-fsync",))

    def test_main_double_persistence_failure_never_releases_verified_checkpoint_claim(self):
        for command in ("activate", "recover-paired"):
            with self.subTest(command=command), tempfile.TemporaryDirectory() as td:
                manifest, _database, _binary, _plist, _helper = self._full_paired_fixture(Path(td))
                Path(manifest.active_path).write_text(manifest.transaction_id)
                helper.write_status(manifest, "activation_failed_rolled_back", recovery_mode="snapshot_restored")
                argv = ["helper", command, "--manifest", str(Path(td) / "manifest.json"), "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
                with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=manifest), mock.patch.object(helper, "activate", side_effect=helper.VerifiedPredecessorFinalizationError("terminal fsync and fallback write failed")), mock.patch.object(helper, "recover_paired", side_effect=helper.VerifiedPredecessorFinalizationError("terminal fsync and fallback write failed")), mock.patch.object(helper, "write_status", side_effect=OSError("status device still failed")), mock.patch.object(helper, "release_claim") as release, mock.patch.object(helper, "request_helper_bootout"):
                    if command == "recover-paired":
                        with self.assertRaises(helper.VerifiedPredecessorFinalizationError):
                            helper.main()
                    else:
                        self.assertEqual(helper.main(), 1)
                    release.assert_not_called()
                    self.assertTrue(Path(manifest.active_path).exists())

    def test_unchanged_predecessor_finalization_retries_both_capacity_allocations(self):
        import sqlite3

        for entry in ("activation", "fresh_recovery", "checkpoint_retry"):
            for failing_allocation in ("backup", "restore"):
                with self.subTest(entry=entry, allocation=failing_allocation), tempfile.TemporaryDirectory() as td:
                    root = Path(td)
                    manifest, database, _binary, _plist, copied_helper = self._full_paired_fixture(root)
                    Path(manifest.active_path).write_text(manifest.transaction_id)
                    helper.write_status(manifest, "activating", failure="snapshot failed")
                    reserved = helper.reserve_database_capacity(manifest)
                    failing_path = reserved.backup_path if failing_allocation == "backup" else reserved.restore_path
                    backend = FakeLaunchctl(manifest)
                    if entry == "activation":
                        reserved.backup_path.unlink()
                        reserved.restore_path.unlink()
                    else:
                        backend.inspect = mock.Mock(return_value=("running", 101) if entry == "checkpoint_retry" else ("not_loaded", None))
                    if entry == "checkpoint_retry":
                        helper.write_status(manifest, "activation_failed_rolled_back", failure="snapshot failed", recovery_mode="unchanged_predecessor")
                    original_unlink = Path.unlink

                    def fail_allocation(path, *args, **kwargs):
                        if path == failing_path:
                            raise OSError("capacity unlink failed")
                        return original_unlink(path, *args, **kwargs)

                    with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")), mock.patch.object(helper, "restore_database") as replay:
                        if entry == "activation":
                            with mock.patch.object(helper, "_reservation_still_sufficient", side_effect=helper.ActivationError("capacity grew")), mock.patch.object(backend, "inspect", side_effect=lambda: ("not_loaded", None) if backend.events and backend.events[-1] == "stop" else ("running", 101)), mock.patch.object(Path, "unlink", fail_allocation):
                                state = helper.activate(manifest)
                        else:
                            with mock.patch.object(Path, "unlink", fail_allocation):
                                state = helper.recover_paired(manifest)
                        self.assertEqual(state, "activation_failed_rollback_failed")
                        failed = json.loads(Path(manifest.status_path).read_text())
                        self.assertEqual(failed["recovery_mode"], "unchanged_predecessor", failed)
                        self.assertIn("capacity finalization failed", failed["rollback_failure"])
                        self.assertTrue(failing_path.exists())
                        self.assertTrue(Path(manifest.active_path).exists())
                        self.assertFalse(helper.status_is_durable_terminal(manifest))
                        with sqlite3.connect(database) as conn:
                            conn.execute("UPDATE users SET name = 'after-unchanged-resume'")
                        backend.inspect = mock.Mock(return_value=("running", 101))
                        backend.events.clear()
                        argv = ["helper", "recover-paired", "--manifest", str(root / "manifest.json"), "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
                        with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=manifest), mock.patch.object(helper, "request_helper_bootout"):
                            self.assertEqual(helper.main(), 0)
                        replay.assert_not_called()
                        self.assertEqual(backend.events, [])
                    self.assertFalse(reserved.backup_path.exists())
                    self.assertFalse(reserved.restore_path.exists())
                    self.assertFalse(Path(manifest.active_path).exists())
                    self.assertEqual(json.loads(Path(manifest.status_path).read_text())["state"], "activation_failed_rolled_back")
                    with sqlite3.connect(database) as conn:
                        self.assertEqual(conn.execute("SELECT name FROM users WHERE id = 1").fetchone(), ("after-unchanged-resume",))

    def test_unreadable_finalizer_checkpoint_retains_capacity_and_claim(self):
        with tempfile.TemporaryDirectory() as td:
            manifest, _database, _binary, _plist, copied_helper = self._full_paired_fixture(Path(td))
            Path(manifest.active_path).write_text(manifest.transaction_id)
            helper.write_status(manifest, "activation_failed_rolled_back", recovery_mode="unchanged_predecessor")
            reserved = helper.reserve_database_capacity(manifest)
            backend = FakeLaunchctl(manifest)
            original_finalize = helper.finalize_verified_predecessor
            original_read = Path.read_text

            def fail_finalizer_read(current, failure):
                failed = False

                def read(path, *args, **kwargs):
                    nonlocal failed
                    if path == Path(current.status_path) and not failed:
                        failed = True
                        raise OSError("checkpoint read unavailable")
                    return original_read(path, *args, **kwargs)

                with mock.patch.object(Path, "read_text", read):
                    original_finalize(current, failure)

            argv = ["helper", "recover-paired", "--manifest", str(Path(td) / "manifest.json"), "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
            with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "restore_database") as replay, mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=manifest), mock.patch.object(helper, "request_helper_bootout"):
                with mock.patch.object(helper, "finalize_verified_predecessor", side_effect=fail_finalizer_read):
                    self.assertEqual(helper.main(), 1)
                self.assertTrue(reserved.backup_path.exists())
                self.assertTrue(reserved.restore_path.exists())
                self.assertTrue(Path(manifest.active_path).exists())
                status = json.loads(Path(manifest.status_path).read_text())
                self.assertEqual(status["state"], "activation_failed_rollback_failed")
                self.assertEqual(status["recovery_mode"], "unchanged_predecessor")
                self.assertIn("checkpoint read unavailable", status["rollback_failure"])
                self.assertEqual(backend.events, [])
                self.assertEqual(helper.main(), 0)
                replay.assert_not_called()
                self.assertEqual(backend.events, [])
            self.assertFalse(reserved.backup_path.exists())
            self.assertFalse(reserved.restore_path.exists())
            self.assertFalse(Path(manifest.active_path).exists())

    def test_unchanged_cleanup_interruption_and_directory_fsync_are_retryable(self):
        for interruption in (SystemExit("cleanup interrupted"), OSError("cleanup directory fsync failed")):
            with self.subTest(interruption=type(interruption).__name__), tempfile.TemporaryDirectory() as td:
                manifest, _database, _binary, _plist, copied_helper = self._full_paired_fixture(Path(td))
                Path(manifest.active_path).write_text(manifest.transaction_id)
                helper.write_status(manifest, "activation_failed_rolled_back", recovery_mode="unchanged_predecessor")
                reserved = helper.reserve_database_capacity(manifest)
                backend = FakeLaunchctl(manifest)
                original_sync = helper.fsync_dir
                failed = False

                def fail_cleanup_barrier(path):
                    nonlocal failed
                    if not failed and not reserved.backup_path.exists():
                        failed = True
                        raise interruption
                    original_sync(path)

                with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper, "wait_for_identity"), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "restore_database") as replay:
                    with mock.patch.object(helper, "fsync_dir", side_effect=fail_cleanup_barrier):
                        if isinstance(interruption, SystemExit):
                            with self.assertRaises(SystemExit):
                                helper.recover_paired(manifest)
                        else:
                            self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
                    self.assertTrue(Path(manifest.active_path).exists())
                    self.assertFalse(reserved.backup_path.exists())
                    self.assertFalse(reserved.restore_path.exists())
                    self.assertFalse(helper._capacity_observation_path(reserved).exists())
                    backend.events.clear()
                    self.assertEqual(helper.recover_paired(manifest), "activation_failed_rolled_back")
                    self.assertEqual(backend.events, [])
                    replay.assert_not_called()
                self.assertFalse(reserved.backup_path.exists())
                self.assertFalse(reserved.restore_path.exists())

    def test_checkpoint_retry_verification_failure_is_durable_and_never_replays_snapshot(self):
        with tempfile.TemporaryDirectory() as td:
            result = self._activate_full_paired(Path(td), health_failure=helper.ActivationError("candidate failed"))
            _state, manifest, _database, _binary, _plist, *_rest, events, launchctl = result
            copied_helper = manifest.paired_database_upgrade.controller_helper_path
            self.assertEqual(json.loads(Path(manifest.status_path).read_text())["state"], "activation_failed_rolled_back")
            before_events = list(events)
            with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("checkpoint health verification failed")), mock.patch.object(helper, "restore_database") as replay:
                self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
                replay.assert_not_called()
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual(status["state"], "activation_failed_rollback_failed")
            self.assertEqual(status["failure"], "candidate failed")
            self.assertIn("checkpoint health verification failed", status["rollback_failure"])
            self.assertEqual(status["recovery_mode"], "snapshot_restored")
            self.assertEqual(Path(manifest.active_path).read_text().strip(), manifest.transaction_id)
            self.assertEqual(events, before_events + ["stop"])
            self.assertFalse(launchctl.loaded)
            self.assertFalse(helper.status_is_durable_terminal(manifest))
            with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(helper, "require_loaded_plist"), mock.patch.object(helper, "restore_database") as replay:
                self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
                replay.assert_not_called()
            retried = json.loads(Path(manifest.status_path).read_text())
            self.assertIn("checkpoint health verification failed", retried["rollback_failure"])
            self.assertIn("post-restore checkpoint has no running predecessor; refuse snapshot replay", retried["rollback_failure"])
            self.assertEqual(retried["recovery_mode"], "snapshot_restored")
            self.assertEqual(Path(manifest.active_path).read_text().strip(), manifest.transaction_id)

    def test_checkpoint_recovery_lock_loser_cannot_release_retained_claim(self):
        with tempfile.TemporaryDirectory() as td:
            manifest, *_ = self._full_paired_fixture(Path(td))
            Path(manifest.active_path).write_text(manifest.transaction_id)
            helper.write_status(manifest, "activation_failed_rolled_back", recovery_mode="snapshot_restored")
            before = Path(manifest.status_path).read_bytes()
            argv = ["helper", "recover-paired", "--manifest", str(Path(td) / "manifest.json"), "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
            with Path(manifest.lock_path).open("a+") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=manifest), mock.patch.object(helper, "release_claim") as release, mock.patch.object(helper, "request_helper_bootout"):
                    self.assertEqual(helper.main(), 1)
                    release.assert_not_called()
            self.assertEqual(Path(manifest.active_path).read_text(), manifest.transaction_id)
            self.assertEqual(Path(manifest.status_path).read_bytes(), before)

    def test_recovery_terminal_status_failure_after_fresh_restore_preserves_runtime(self):
        with tempfile.TemporaryDirectory() as td:
            manifest, _database, _binary, _plist, copied_helper = self._full_paired_fixture(Path(td))
            Path(manifest.active_path).write_text(manifest.transaction_id)
            helper.write_status(manifest, "activation_failed_rollback_failed")
            launchctl = FakeLaunchctl(manifest)
            original_write = helper.write_status

            def fail_terminal(current, state, **kwargs):
                if state == "activation_failed_rolled_back":
                    raise OSError("terminal persistence failed")
                original_write(current, state, **kwargs)

            with mock.patch.object(helper, "__file__", str(copied_helper)), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(helper, "write_status", side_effect=fail_terminal), mock.patch.object(helper, "restore") as restore, mock.patch.object(launchctl, "stop") as stop:
                self.assertEqual(helper.recover_paired(manifest), "activation_failed_rollback_failed")
                restore.assert_called_once()
                stop.assert_not_called()
                self.assertTrue(Path(manifest.active_path).exists())

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
                if isinstance(exception, helper.ConcurrentDeploy):
                    self.assertEqual(status["rollback_failure"], "prior restore failure")
                else:
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
            original_open = helper.os.open

            def fail_file_creation(path, flags, *args, **kwargs):
                if flags & helper.os.O_CREAT:
                    raise OSError(28, "No space left on device")
                return original_open(path, flags, *args, **kwargs)

            with mock.patch.object(
                helper.os, "open", side_effect=fail_file_creation
            ), self.assertRaisesRegex(
                helper.ActivationError, "reserve paired database capacity"
            ):
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
            self.assertEqual("ordinary_activation_failed_rollback_failed", state)
            self.assertTrue(helper.status_is_durable_terminal(manifest))
            self.assertIn("health timeout", status["failure"])
            self.assertIn("health timeout", status["rollback_failure"])

    def test_ordinary_failure_state_releases_matching_durable_launchd_claim(self):
        for source_kind in ("local_head", "published_release"):
            with self.subTest(source_kind=source_kind), tempfile.TemporaryDirectory() as td:
                manifest = helper.dataclasses.replace(make_manifest(Path(td)), source_kind=source_kind)
                Path(manifest.active_path).write_text(manifest.transaction_id)
                argv = ["helper", "activate", "--manifest", str(Path(td) / "manifest.json"), "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
                with mock.patch.object(sys, "argv", argv), mock.patch.object(helper.Manifest, "load", return_value=manifest), mock.patch.object(helper, "Launchctl", FakeLaunchctl), mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("health timeout")), mock.patch.object(helper, "request_helper_bootout"):
                    self.assertEqual(helper.main(), 1)
                self.assertEqual(json.loads(Path(manifest.status_path).read_text())["state"], "ordinary_activation_failed_rollback_failed")
                self.assertFalse(Path(manifest.active_path).exists())

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

    def test_helper_does_not_bootout_when_manifest_is_malformed(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = Path(td) / "manifest.json"
            manifest.write_text("not-json")
            argv = ["helper", "activate", "--manifest", str(manifest),
                    "--helper-label", "test.helper", "--uid", str(os.getuid())]
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "request_helper_bootout") as bootout:
                self.assertEqual(1, helper.main())
            bootout.assert_not_called()

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
            [sys.executable, str(ROOT / "dev.py"), "prod", "deploy", "v1.2.3"],
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
            transaction = root / "transactions" / "broken-pair"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(root)})
            )
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

    def test_terminal_prepared_claim_requires_manifest_matching_mode(self):
        cases = (
            ("paired_database_upgrade", "paired", "current_schema"),
            ("current_schema_upgrade", "current_schema", "paired"),
        )
        for manifest_key, expected_mode, conflicting_mode in cases:
            for admission_name in ("_claim_launchd_deploy", "_claim_launchd_restart"):
                with self.subTest(
                    expected_mode=expected_mode, admission=admission_name
                ), tempfile.TemporaryDirectory() as td:
                    root = Path(td)
                    owner = f"terminal-{expected_mode}"
                    active = root / "active"
                    active.write_text(owner + "\n")
                    status = root / "status.json"
                    status.write_text(
                        json.dumps(
                            {
                                "transaction_id": owner,
                                "state": "committed",
                                "source_kind": "prepared_artifact",
                                "prepared_mode": conflicting_mode,
                                "finalization_pending": False,
                                "rollback_failure": None,
                            }
                        )
                    )
                    transaction = root / "transactions" / owner
                    transaction.mkdir(parents=True)
                    (transaction / "manifest.json").write_text(
                        json.dumps(
                            {
                                "source_kind": "prepared_artifact",
                                manifest_key: mode_evidence(transaction, manifest_key),
                            }
                        )
                    )

                    with mock.patch.object(
                        self.dev, "LAUNCHD_DEPLOY_DIR", root
                    ), mock.patch.object(
                        self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
                    ), mock.patch.object(
                        self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
                    ), mock.patch.object(
                        self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
                    ), mock.patch.object(
                        self.dev, "LAUNCHD_RESTART_DIR", root / "restart"
                    ), mock.patch.object(
                        self.dev,
                        "LAUNCHD_RESTART_ACTIVE_PATH",
                        root / "restart-active",
                    ):
                        self.assertFalse(
                            self.dev._status_is_terminal_for_owner(
                                status, owner, self.dev._DEPLOY_TERMINAL_STATES
                            )
                        )
                        admission = getattr(self.dev, admission_name)
                        with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                            admission("new-operation")

                    self.assertEqual(active.read_text(), owner + "\n")

    def test_ordinary_rollback_failure_releases_only_its_matching_terminal_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            active.write_text("failed-ordinary\n")
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "failed-ordinary",
                        "state": "ordinary_activation_failed_rollback_failed",
                        "source_kind": "local_head",
                        "finalization_pending": False,
                        "rollback_failure": None,
                    }
                )
            )
            transaction = root / "transactions" / "failed-ordinary"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps({"source_kind": "local_head"})
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ):
                self.dev._claim_launchd_deploy(
                    "next", initial_status={"transaction_id": "next", "state": "preparing"}
                )

            self.assertEqual(active.read_text().strip(), "next")
            self.assertEqual(json.loads(status.read_text())["transaction_id"], "next")

    def test_forward_only_claim_survives_unreadable_retained_manifest(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "forward"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text("invalid-json")
            active = root / "active"
            active.write_text("forward\n")
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "forward",
                        "state": "ordinary_activation_failed_forward_only",
                        "finalization_pending": True,
                    }
                )
            )
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_DIR", root / "restart"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ):
                self.assertFalse(
                    self.dev._status_is_terminal_for_owner(
                        status, "forward", self.dev._DEPLOY_TERMINAL_STATES
                    )
                )
                for admission in (
                    self.dev._claim_launchd_deploy,
                    self.dev._claim_launchd_restart,
                ):
                    with self.subTest(admission=admission.__name__), self.assertRaises(
                        SystemExit
                    ) as error:
                        admission("new-operation")
                    self.assertIn("Do not remove", str(error.exception))
                    self.assertEqual(active.read_text(), "forward\n")

    def test_checkpointed_terminal_claim_public_recovery_remains_publication_only(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "checkpoint"
            transaction.mkdir(parents=True)
            retained = transaction / "helper.py"
            retained.write_text("retained helper")
            payload = {"transaction_id": "checkpoint", "source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(transaction) | {"controller_helper_path": str(retained), "controller_helper_sha256": self.dev._file_sha256(retained)}, "helper_label": "com.phoenix-ide.deploy.checkpoint"}
            (transaction / "manifest.json").write_text(json.dumps(payload))
            active = root / "active"
            active.write_text("checkpoint")
            status = root / "status.json"
            status.write_text(json.dumps({"transaction_id": "checkpoint", "state": "activation_failed_rolled_back", "source_kind": "prepared_artifact", "prepared_mode": "paired", "recovery_mode": "snapshot_restored"}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as backend, mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release:
                self.dev.cmd_prod_recover_paired("checkpoint")
                release.assert_not_called()
                self.assertEqual(active.read_text(), "checkpoint")
                plist = plistlib.loads((transaction / "recovery-helper.plist").read_bytes())
                self.assertEqual(plist["ProgramArguments"][2], "recover-paired")
                self.assertEqual(backend.call_count, 3)

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
            status.write_text(json.dumps({"transaction_id": "early", "source_kind": "prepared_artifact", "prepared_mode": "paired", "state": "preparing", "preparing_pid": 123, "updated_at": "2026-10-04T00:00:00Z"}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 113, "", f'Could not find service "com.phoenix-ide.deploy.early" in domain gui/{os.getuid()}')):
                self.dev.cmd_prod_recover_paired("early")
            self.assertFalse(active.exists())
            terminal = json.loads(status.read_text())
            self.assertEqual(terminal["state"], "precondition_failed")
            self.assertIs(terminal["finalization_pending"], False)
            self.assertIsNone(terminal["rollback_failure"])
            self.assertTrue(self.dev._markerless_status_is_resolved(terminal))
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ):
                self.assertTrue(
                    self.dev._status_is_terminal_for_owner(
                        status, "early", self.dev._DEPLOY_TERMINAL_STATES
                    )
                )

    def test_finalization_controller_uses_retained_helper_without_bootstrap(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "transactions" / "pending"
            staging.mkdir(parents=True)
            script = staging / "launchd_deploy_helper.py"
            script.write_bytes(b"retained helper")
            (staging / "manifest.json").write_text(json.dumps({"transaction_id": "pending", "source_kind": "prepared_artifact", "helper_label": "com.phoenix-ide.deploy.pending", "paired_database_upgrade": mode_evidence(staging) | {"controller_helper_sha256": self.dev._file_sha256(script), "controller_helper_path": str(script)}}))
            active, status = root / "active", root / "status.json"
            active.write_text("pending\n")
            status.write_text(json.dumps({"transaction_id": "pending", "source_kind": "prepared_artifact", "prepared_mode": "paired", "state": "committed", "finalization_pending": True}))
            import builtins
            original_import = builtins.__import__

            def sqlite_unavailable(name, *args, **kwargs):
                if name == "sqlite3":
                    raise ModuleNotFoundError("optional sqlite3 unavailable")
                return original_import(name, *args, **kwargs)

            def execute_probe(command, **_kwargs):
                if "-c" in command:
                    with mock.patch.object(builtins, "__import__", side_effect=sqlite_unavailable):
                        exec(command[-1], {})
                return subprocess.CompletedProcess(command, 0, "", "")

            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev.subprocess, "run", side_effect=execute_probe) as run:
                self.dev.cmd_prod_finalize_paired("pending")
            self.assertEqual(run.call_count, 3)
            self.assertNotIn("sqlite3", run.call_args_list[0].args[0][-1])
            self.assertIn("finalize-paired", run.call_args.args[0])
            self.assertFalse(any("bootstrap" in call.args[0] for call in run.call_args_list))

    def test_committed_recovery_refusal_never_bootstraps_or_rewrites_status(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "transactions" / "committed"
            staging.mkdir(parents=True)
            (staging / "manifest.json").write_text(json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(root)}))
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
            manifest.write_text(json.dumps({"transaction_id": "preparing", "helper_label": "com.phoenix-ide.deploy.preparing", "source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(staging) | {"database_path": str(root / "prod.db")}}))
            active, status = root / "active", root / "status.json"
            active.write_text("preparing\n")
            status.write_text(json.dumps({"transaction_id": "preparing", "source_kind": "prepared_artifact", "prepared_mode": "paired", "state": "preparing", "preparing_pid": 123}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
                self.dev.cmd_prod_recover_paired("preparing")
            self.assertEqual(run.call_count, 1)
            self.assertNotIn("bootstrap", run.call_args.args[0])
            terminal = json.loads(status.read_text())
            self.assertEqual(terminal["state"], "precondition_failed")
            self.assertIs(terminal["finalization_pending"], False)
            self.assertIsNone(terminal["rollback_failure"])
            self.assertTrue(self.dev._markerless_status_is_resolved(terminal))
            self.assertFalse(active.exists())
            self.assertIn("backup_path", json.loads(manifest.read_text())["paired_database_upgrade"])

    def test_prepared_handoff_absent_abandons_without_runtime_mutation(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            staging = root / "transactions" / "prepared"
            staging.mkdir(parents=True)
            (staging / "manifest.json").write_text(json.dumps({"transaction_id": "prepared", "helper_label": "com.phoenix-ide.deploy.prepared", "source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(staging) | {"database_path": str(root / "prod.db")}}))
            active, status = root / "active", root / "status.json"
            active.write_text("prepared\n")
            status.write_text(json.dumps({"transaction_id": "prepared", "source_kind": "prepared_artifact", "prepared_mode": "paired", "state": "prepared", "preparing_pid": 123}))
            backup = staging / "backup.sqlite3"
            reserve = root / ".prod.db.restore-prepared"
            capacity_evidence = staging / "capacity.json"
            backup.write_bytes(b"allocated seed")
            reserve.write_bytes(b"allocated restore")
            capacity_evidence.write_text("capacity evidence")
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
                self.dev.cmd_prod_recover_paired("prepared")
            self.assertEqual(run.call_count, 1)
            self.assertIn("--probe-service-absence", run.call_args.args[0])
            self.assertFalse(active.exists())
            self.assertEqual(json.loads(status.read_text())["state"], "precondition_failed")
            self.assertFalse(backup.exists())
            self.assertFalse(reserve.exists())
            self.assertFalse(capacity_evidence.exists())

    def test_pre_handoff_cleanup_failure_enters_cleanup_pending_and_retains_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction_id = "cleanup-failure"
            staging = root / "transactions" / transaction_id
            staging.mkdir(parents=True)
            database = root / "prod.db"
            backup = staging / "backup.sqlite3"
            proof = staging / "proof.json"
            restore = root / f".prod.db.restore-{transaction_id}"
            capacity = staging / "capacity.json"
            (staging / "manifest.json").write_text(
                json.dumps(
                    {
                        "transaction_id": transaction_id,
                        "helper_label": f"com.phoenix-ide.deploy.{transaction_id}",
                        "source_kind": "prepared_artifact",
                        "paired_database_upgrade": {
                            **mode_evidence(staging),
                            "backup_path": str(backup),
                            "proof_path": str(proof),
                            "database_path": str(database),
                        },
                    }
                )
            )
            for path in (backup, restore, capacity):
                path.write_text("reserved")
            active = root / "active"
            status = root / "status.json"
            active.write_text(transaction_id + "\n")
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": transaction_id,
                        "source_kind": "prepared_artifact",
                        "prepared_mode": "paired",
                        "state": "prepared",
                        "preparing_pid": 123,
                    }
                )
            )
            fsync_calls = 0

            def fail_first_fsync(_fd):
                nonlocal fsync_calls
                fsync_calls += 1
                if fsync_calls == 1:
                    raise OSError("fsync failed")

            with mock.patch.object(
                self.dev.sys, "platform", "darwin"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev.os, "kill", side_effect=ProcessLookupError
            ), mock.patch.object(
                self.dev.os, "fsync", side_effect=fail_first_fsync
            ), mock.patch.object(
                self.dev.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 0, "", ""),
            ):
                with self.assertRaisesRegex(
                    SystemExit, "cleanup is incomplete"
                ):
                    self.dev.cmd_prod_recover_paired(transaction_id)

            self.assertEqual(active.read_text().strip(), transaction_id)
            pending = json.loads(status.read_text())
            self.assertEqual(pending["state"], "precondition_cleanup_pending")
            self.assertIn("fsync failed", pending["cleanup_failure"])

    def test_pre_handoff_abandonment_rejects_wrong_recovery_mode_without_mutation(self):
        cases = (
            ("paired_database_upgrade", True),
            ("current_schema_upgrade", False),
        )
        for retained_mode, request_current in cases:
            with self.subTest(
                retained_mode=retained_mode, request_current=request_current
            ), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                transaction_id = "wrong-mode"
                staging = root / "transactions" / transaction_id
                staging.mkdir(parents=True)
                database = root / "prod.db"
                backup = staging / "backup.sqlite3"
                proof = staging / "proof.json"
                restore = root / f".prod.db.restore-{transaction_id}"
                capacity = staging / "capacity.json"
                (staging / "manifest.json").write_text(
                    json.dumps(
                        {
                            "source_kind": "prepared_artifact",
                            retained_mode: {
                                "backup_path": str(backup),
                                "proof_path": str(proof),
                                "database_path": str(database),
                            },
                        }
                    )
                )
                active = root / "active"
                status = root / "status.json"
                active.write_text(transaction_id + "\n")
                original_status = {
                    "transaction_id": transaction_id,
                    "source_kind": "prepared_artifact",
                    "prepared_mode": (
                        "paired"
                        if retained_mode == "paired_database_upgrade"
                        else "current_schema"
                    ),
                    "state": "prepared",
                    "preparing_pid": 123,
                }
                status.write_text(json.dumps(original_status))
                backup.write_text("backup reservation")
                restore.write_text("restore reservation")
                capacity.write_text("capacity evidence")
                with mock.patch.object(
                    self.dev.sys, "platform", "darwin"
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_DIR", root
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
                ), mock.patch.object(
                    self.dev.os, "kill", side_effect=ProcessLookupError
                ), mock.patch.object(
                    self.dev.subprocess,
                    "run",
                    return_value=subprocess.CompletedProcess([], 0, "", ""),
                ):
                    with self.assertRaisesRegex(
                        SystemExit, "does not (?:prove|match) .* recovery mode"
                    ):
                        self.dev.cmd_prod_recover_paired(
                            transaction_id, current_schema=request_current
                        )

                self.assertEqual(active.read_text().strip(), transaction_id)
                self.assertEqual(json.loads(status.read_text()), original_status)
                self.assertTrue(backup.exists())
                self.assertTrue(restore.exists())
                self.assertTrue(capacity.exists())

    def test_dangling_manifest_refuses_pre_handoff_abandonment_without_mutation(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction_id = "dangling-manifest"
            staging = root / "transactions" / transaction_id
            staging.mkdir(parents=True)
            (staging / "manifest.json").symlink_to(staging / "missing-manifest")
            active = root / "active"
            status = root / "status.json"
            active.write_text(transaction_id + "\n")
            payload = {
                "transaction_id": transaction_id,
                "source_kind": "prepared_artifact",
                "prepared_mode": "paired",
                "state": "preparing",
                "preparing_pid": 123,
            }
            status.write_text(json.dumps(payload))
            with mock.patch.object(
                self.dev.sys, "platform", "darwin"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ):
                with self.assertRaisesRegex(SystemExit, "not a regular file"):
                    self.dev.cmd_prod_recover_paired(transaction_id)
            self.assertEqual(active.read_text().strip(), transaction_id)
            self.assertEqual(json.loads(status.read_text()), payload)

    def test_manifest_absent_abandonment_requires_persisted_exact_mode(self):
        for prepared_mode, request_current in (("paired", True), ("current_schema", False), (None, False)):
            with self.subTest(
                prepared_mode=prepared_mode, request_current=request_current
            ), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                transaction_id = "manifest-absent"
                (root / "transactions" / transaction_id).mkdir(parents=True)
                active = root / "active"
                status = root / "status.json"
                active.write_text(transaction_id + "\n")
                payload = {
                    "transaction_id": transaction_id,
                    "source_kind": "prepared_artifact",
                    "state": "preparing",
                    "preparing_pid": 123,
                }
                if prepared_mode is not None:
                    payload["prepared_mode"] = prepared_mode
                status.write_text(json.dumps(payload))
                with mock.patch.object(
                    self.dev.sys, "platform", "darwin"
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_DIR", root
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
                ), mock.patch.object(
                    self.dev.os, "kill", side_effect=ProcessLookupError
                ), mock.patch.object(
                    self.dev.subprocess,
                    "run",
                    return_value=subprocess.CompletedProcess([], 0, "", ""),
                ):
                    with self.assertRaisesRegex(
                        SystemExit, "does not prove .* recovery mode"
                    ):
                        self.dev.cmd_prod_recover_paired(
                            transaction_id, current_schema=request_current
                        )
                self.assertTrue(active.exists())
                self.assertEqual(json.loads(status.read_text()), payload)

    def test_status_recovers_paired_guidance_from_claim_without_readable_status(self):
        for contents in (None, "{", "[]", "null"):
            with self.subTest(contents=contents), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                staging = root / "transactions" / "lost"
                staging.mkdir(parents=True)
                (staging / "manifest.json").write_text(json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(root)}))
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

    def test_status_always_reports_active_owner_when_last_status_mismatches(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            active.write_text("actual-owner\n")
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "older-terminal",
                        "state": "committed",
                        "source_kind": "local_head",
                    }
                )
            )
            output = io.StringIO()
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), contextlib.redirect_stdout(output):
                self.dev._print_launchd_deploy_status()

            rendered = output.getvalue()
            self.assertIn("Active deploy: status unavailable/mismatched (actual-owner)", rendered)
            self.assertIn("Last deploy: committed (older-terminal)", rendered)

            status.unlink()
            output = io.StringIO()
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), contextlib.redirect_stdout(output):
                self.dev._print_launchd_deploy_status()
            self.assertIn("Active deploy: status unavailable (actual-owner)", output.getvalue())

    def test_active_nonregular_status_is_rejected_before_read(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            active.write_text("actual-owner\n")
            os.mkfifo(status)
            real_read_text = Path.read_text

            def reject_status_read(path, *args, **kwargs):
                if path == status:
                    raise AssertionError("non-regular status must not be read")
                return real_read_text(path, *args, **kwargs)

            output = io.StringIO()
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_DIR", root / "restart"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ), mock.patch.object(
                Path, "read_text", reject_status_read
            ), contextlib.redirect_stdout(output):
                self.dev._print_launchd_deploy_status()
                with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                    self.dev._claim_launchd_restart("next-restart")

            rendered = output.getvalue()
            self.assertIn("Active deploy: status unavailable (actual-owner)", rendered)
            self.assertIn("non-regular or symlinked", rendered)
            self.assertEqual(active.read_text(), "actual-owner\n")
            self.assertTrue(status.exists())

    def test_status_surfaces_symlinked_markerless_evidence_as_unresolved(self):
        for dangling in (False, True):
            with self.subTest(dangling=dangling), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                status = root / "status.json"
                target = root / "status-target.json"
                if not dangling:
                    target.write_text(
                        json.dumps(
                            {
                                "transaction_id": "terminal",
                                "state": "committed",
                                "source_kind": "local_head",
                            }
                        )
                    )
                status.symlink_to(target)
                output = io.StringIO()
                with mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
                ), contextlib.redirect_stdout(output):
                    self.dev._print_launchd_deploy_status()

                rendered = output.getvalue()
                self.assertIn("UNRESOLVED: markerless deployment status", rendered)
                self.assertIn("non-regular or symlinked", rendered)
                self.assertNotIn("Last deploy: committed", rendered)

    def test_pending_paired_commit_blocks_admission_without_authorizing_rollback(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            status.write_text(json.dumps({"transaction_id": "pending", "source_kind": "prepared_artifact", "state": "committed", "finalization_pending": True}))
            transaction = root / "transactions" / "pending"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(root)})
            )
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

    def test_claim_parent_fsync_follows_status_and_marker_file_fsync(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active, status = root / "active", root / "status.json"
            initial = {"transaction_id": "early", "state": "preparing", "source_kind": "prepared_artifact", "preparing_pid": os.getpid()}
            original_fsync = os.fsync
            events = []

            def file_sync(fd):
                if active.exists() and os.fstat(fd).st_ino == active.stat().st_ino:
                    events.append("marker-file")
                original_fsync(fd)

            def directory_sync(path):
                self.assertEqual(json.loads(status.read_text()), initial)
                if not active.exists():
                    return
                self.assertEqual(active.read_text().strip(), "early")
                self.assertEqual(events, ["marker-file"])
                self.assertEqual(path, root)
                events.append("marker-parent")

            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev, "_restart_claim_owner", return_value=None), mock.patch.object(self.dev.os, "fsync", side_effect=file_sync), mock.patch.object(self.dev, "_fsync_directory", side_effect=directory_sync):
                self.dev._claim_launchd_deploy("early", initial_status=initial)
            self.assertEqual(events, ["marker-file", "marker-parent"])

    def test_claim_parent_fsync_failure_never_prepares_or_hands_off(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active, status = root / "active", root / "status.json"
            original_sync = self.dev._fsync_directory

            def fail_marker_parent(path):
                if active.exists():
                    raise OSError("claim directory fsync failed")
                original_sync(path)

            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev, "_restart_claim_owner", return_value=None), mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)), mock.patch.object(self.dev, "_preflight_prod_bind_auth"), mock.patch.object(self.dev, "_fsync_directory", side_effect=fail_marker_parent), mock.patch.object(self.dev, "_prepare_prepared_artifact") as prepare, mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release, mock.patch.object(self.dev.subprocess, "run") as backend:
                controller = self.dev.ProdDeployControllerOptions(transaction_id="early", prepared_artifact=root, paired_database_upgrade=True, expected_full_commit="a" * 40)
                with self.assertRaisesRegex(OSError, "claim directory fsync failed"):
                    self.dev.launchd_prod_deploy(controller=controller)
                prepare.assert_not_called()
                release.assert_not_called()
                backend.assert_not_called()
            self.assertEqual(active.read_text().strip(), "early")
            self.assertEqual(json.loads(status.read_text())["state"], "preparing")

    def test_pre_manifest_recovery_refuses_unknown_helper_or_missing_pid(self):
        for missing_pid in (False, True):
            with self.subTest(missing_pid=missing_pid), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                active, status = root / "active", root / "status.json"
                active.write_text("early\n")
                payload = {"transaction_id": "early", "state": "preparing", "source_kind": "prepared_artifact", "prepared_mode": "paired"}
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
            status.write_text(json.dumps({"transaction_id": "live", "source_kind": "prepared_artifact", "prepared_mode": "paired", "state": "preparing", "preparing_pid": 123}))
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
            payload = {"transaction_id": "failed.pair", "source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(transaction) | {"controller_helper_path": str(retained), "controller_helper_sha256": self.dev._file_sha256(retained)}, "helper_label": "com.phoenix-ide.deploy.failed.pair"}
            (transaction / "manifest.json").write_text(json.dumps(payload))
            active = root / "active"
            active.write_text("failed.pair\n")
            status = root / "status.json"
            status.write_text(json.dumps({"transaction_id": "failed.pair", "source_kind": "prepared_artifact", "prepared_mode": "paired", "state": "activation_failed_rollback_failed"}))
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "_deploy_claim_owner", return_value="failed.pair"), mock.patch.object(self.dev, "_paired_recovery_refusal", return_value="retain claim"), mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")) as run:
                with self.assertRaisesRegex(SystemExit, "active Python interpreter"):
                    self.dev.cmd_prod_recover_paired("failed.pair")
            self.assertFalse((transaction / "recovery-helper.plist").exists())
            self.assertEqual("failed.pair", active.read_text().strip())
            self.assertIn("sqlite3", run.call_args.args[0][-1])

    def test_recover_current_refuses_precondition_cleanup_before_handoff(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "failed.current"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps(
                    {
                        "source_kind": "prepared_artifact",
                        "current_schema_upgrade": mode_evidence(transaction, "current_schema_upgrade"),
                    }
                )
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "failed.current",
                        "state": "precondition_cleanup_pending",
                    }
                )
            )
            with mock.patch.object(
                self.dev.sys, "platform", "darwin"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "_deploy_claim_owner", return_value="failed.current"
            ), mock.patch.object(self.dev.subprocess, "run") as backend:
                with self.assertRaisesRegex(SystemExit, "cleanup"):
                    self.dev.cmd_prod_recover_paired(
                        "failed.current", current_schema=True
                    )

            backend.assert_not_called()

    def test_precondition_cleanup_handoff_uses_cleanup_only_helper_command(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "failed.current"
            transaction.mkdir(parents=True)
            retained = transaction / "helper.py"
            retained.write_text("retained helper")
            payload = {
                "source_kind": "prepared_artifact",
                "transaction_id": "failed.current",
                "current_schema_upgrade": {
                    **mode_evidence(transaction, "current_schema_upgrade"),
                    "controller_helper_path": str(retained),
                    "controller_helper_sha256": self.dev._file_sha256(retained),
                    "proof_path": str(transaction / "proof.json"),
                },
                "helper_label": "com.phoenix-ide.deploy.failed.current",
            }
            (transaction / "manifest.json").write_text(json.dumps(payload))
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "failed.current",
                        "source_kind": "prepared_artifact",
                        "prepared_mode": "current_schema",
                        "state": "precondition_cleanup_pending",
                    }
                )
            )
            completed = subprocess.CompletedProcess([], 0, "", "")
            with mock.patch.object(
                self.dev.sys, "platform", "darwin"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "_deploy_claim_owner", return_value="failed.current"
            ), mock.patch.object(
                self.dev.subprocess, "run", side_effect=[completed, completed, completed]
            ) as backend:
                self.dev.cmd_prod_cleanup_precondition(
                    "failed.current", current_schema=True
                )

            plist = plistlib.loads((transaction / "cleanup-helper.plist").read_bytes())
            self.assertEqual(plist["ProgramArguments"][2], "cleanup-current")
            commands = [call.args[0] for call in backend.call_args_list]
            self.assertFalse(any("recover-current" in command for command in commands))
            self.assertFalse(any("recover-paired" in command for command in commands))
            self.assertEqual(commands[-1][0:2], ["launchctl", "bootstrap"])

    def test_precondition_cleanup_controller_requires_exact_persisted_mode(self):
        for prepared_mode in (None, "paired"):
            with self.subTest(prepared_mode=prepared_mode), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                transaction = root / "transactions" / "failed.current"
                transaction.mkdir(parents=True)
                retained = transaction / "helper.py"
                retained.write_text("retained helper")
                (transaction / "manifest.json").write_text(
                    json.dumps(
                        {
                            "source_kind": "prepared_artifact",
                            "transaction_id": "failed.current",
                            "current_schema_upgrade": {
                                **mode_evidence(transaction, "current_schema_upgrade"),
                                "controller_helper_path": str(retained),
                                "controller_helper_sha256": self.dev._file_sha256(retained),
                                "proof_path": str(transaction / "proof.json"),
                            },
                            "helper_label": "com.phoenix-ide.deploy.failed.current",
                        }
                    )
                )
                status = root / "status.json"
                status_payload = {
                    "transaction_id": "failed.current",
                    "state": "precondition_cleanup_pending",
                }
                if prepared_mode is not None:
                    status_payload["prepared_mode"] = prepared_mode
                status.write_text(json.dumps(status_payload))
                original_status = status.read_bytes()

                with mock.patch.object(
                    self.dev.sys, "platform", "darwin"
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_DIR", root
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
                ), mock.patch.object(
                    self.dev, "_deploy_claim_owner", return_value="failed.current"
                ), mock.patch.object(self.dev.subprocess, "run") as backend:
                    with self.assertRaisesRegex(
                        SystemExit,
                        "matching retained current-schema transaction and claim",
                    ):
                        self.dev.cmd_prod_cleanup_precondition(
                            "failed.current", current_schema=True
                        )

                backend.assert_not_called()
                self.assertEqual(status.read_bytes(), original_status)

    def test_precondition_cleanup_controller_rejects_symlinked_status(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "failed.current"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text("{}")
            status_target = root / "status-target.json"
            status_target.write_text(
                json.dumps(
                    {
                        "transaction_id": "failed.current",
                        "state": "precondition_cleanup_pending",
                    }
                )
            )
            status = root / "status.json"
            status.symlink_to(status_target)

            with mock.patch.object(
                self.dev.sys, "platform", "darwin"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(self.dev.subprocess, "run") as backend:
                with self.assertRaisesRegex(SystemExit, "regular non-symlink status"):
                    self.dev.cmd_prod_cleanup_precondition(
                        "failed.current", current_schema=True
                    )

            backend.assert_not_called()
            self.assertTrue(status.is_symlink())
            self.assertEqual(
                json.loads(status_target.read_text())["state"],
                "precondition_cleanup_pending",
            )

    def test_supported_recovery_handoff_uses_retained_helper_without_clearing_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "failed.pair"
            transaction.mkdir(parents=True)
            retained = transaction / "helper.py"
            retained.write_text("retained helper")
            payload = {"transaction_id": "failed.pair", "source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(transaction) | {"controller_helper_path": str(retained), "controller_helper_sha256": self.dev._file_sha256(retained)}, "helper_label": "com.phoenix-ide.deploy.failed.pair"}
            (transaction / "manifest.json").write_text(json.dumps(payload))
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "failed.pair",
                        "source_kind": "prepared_artifact",
                        "prepared_mode": "paired",
                        "state": "activation_failed_rollback_failed",
                    }
                )
            )
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "_deploy_claim_owner", return_value="failed.pair"), mock.patch.object(self.dev, "_paired_recovery_refusal", return_value="retain claim"), mock.patch.object(self.dev.subprocess, "run", side_effect=[subprocess.CompletedProcess([], 0, "", ""), subprocess.CompletedProcess([], 0, "", ""), subprocess.CompletedProcess([], 0, "", "")]) as backend, mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release:
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
                (ordinary / "manifest.json").write_text(
                    json.dumps(
                        {
                            "source_kind": "local_head",
                            "paired_database_upgrade": None,
                        }
                    )
                )
                os.utime(ordinary, (100 + i, 100 + i))
            for name, content in (
                ("paired", json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": {"backup_path": "private"}})),
                ("current-historical", json.dumps({"source_kind": "prepared_artifact", "current_schema_upgrade": {"backup_path": "private"}})),
                (
                    "forward",
                    json.dumps(
                        {
                            "source_kind": "local_head",
                            "ordinary_forward_migration": {"database_path": "/db"},
                        }
                    ),
                ),
                ("unreadable", "invalid-json"),
                ("unknown", None),
            ):
                transaction = root / name
                transaction.mkdir()
                if content is not None:
                    (transaction / "manifest.json").write_text(content)
                os.utime(transaction, (1, 1))
            self.dev._prune_launchd_deploy_transactions(root, "new-current")
            self.assertEqual(
                sorted(p.name for p in root.iterdir()),
                [
                    "current-historical",
                    "forward",
                    "ordinary-3",
                    "ordinary-4",
                    "ordinary-5",
                    "ordinary-6",
                    "ordinary-7",
                    "paired",
                    "unknown",
                    "unreadable",
                ],
            )

    def test_claim_refuses_markerless_forward_only_status(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "retained",
                        "state": "candidate_may_migrate",
                        "source_kind": "local_head",
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ):
                with self.assertRaisesRegex(
                    self.dev.ConcurrentLaunchdOperation,
                    "markerless forward-only transaction retained",
                ):
                    self.dev._claim_launchd_deploy("new")

            self.assertFalse((root / "active").exists())
            self.assertEqual(
                json.loads(status.read_text())["transaction_id"], "retained"
            )

    def test_markerless_cleanup_pending_fences_deploy_restart_and_stop(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "cleanup",
                        "state": "precondition_cleanup_pending",
                        "source_kind": "prepared_artifact",
                    }
                )
            )
            patches = (
                mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root),
                mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
                ),
                mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status),
                mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
                ),
                mock.patch.object(
                    self.dev, "LAUNCHD_RESTART_DIR", root / "restart"
                ),
                mock.patch.object(
                    self.dev,
                    "LAUNCHD_RESTART_ACTIVE_PATH",
                    root / "restart-active",
                ),
            )
            with patches[0], patches[1], patches[2], patches[3], patches[4], patches[5]:
                with self.assertRaisesRegex(
                    self.dev.ConcurrentLaunchdOperation,
                    "markerless pre-stop cleanup",
                ):
                    self.dev._claim_launchd_deploy("new")
                with self.assertRaisesRegex(
                    self.dev.ConcurrentLaunchdOperation,
                    "markerless pre-stop cleanup",
                ):
                    self.dev._claim_launchd_restart("restart")
                with mock.patch.object(
                    self.dev, "detect_prod_env", return_value="launchd"
                ) as detect_environment:
                    with self.assertRaisesRegex(
                        SystemExit,
                        "markerless pre-stop cleanup",
                    ):
                        self.dev.cmd_prod_stop()
                    detect_environment.assert_called_once_with()

            self.assertFalse((root / "active").exists())
            self.assertFalse((root / "restart-active").exists())

    def test_active_owner_terminal_proof_requires_regular_exact_status(self):
        malformed = (
            {"transaction_id": "owner", "state": "committed", "finalization_pending": []},
            {"transaction_id": "owner", "state": "committed", "rollback_failure": {}},
            {
                "transaction_id": "owner",
                "state": "committed",
                "source_kind": "local_head",
                "finalization_pending": False,
            },
            {
                "transaction_id": "owner",
                "state": "committed",
                "finalization_pending": False,
                "rollback_failure": None,
            },
        )
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ):
                for payload in malformed:
                    with self.subTest(payload=payload):
                        active.write_text("owner\n")
                        status.unlink(missing_ok=True)
                        status.write_text(json.dumps(payload))
                        with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                            self.dev._claim_launchd_deploy("new")
                        self.assertEqual(active.read_text().strip(), "owner")
                external = root / "external-status.json"
                external.write_text(
                    json.dumps({"transaction_id": "owner", "state": "committed"})
                )
                active.write_text("owner\n")
                status.unlink()
                status.symlink_to(external)
                with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                    self.dev._claim_launchd_deploy("new")
                self.assertEqual(active.read_text().strip(), "owner")

    def test_unreadable_active_markers_fence_operations_before_status_mutation(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            deploy_active = root / "active"
            restart_active = root / "restart-active"
            status = root / "status.json"
            original = json.dumps(
                {
                    "transaction_id": "terminal",
                    "state": "committed",
                    "source_kind": "local_head",
                    "finalization_pending": False,
                    "rollback_failure": None,
                }
            )
            status.write_text(original)
            deploy_active.symlink_to(root / "missing-deploy-owner")
            restart_active.symlink_to(root / "missing-restart-owner")
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", deploy_active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_DIR", root / "restart"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", restart_active
            ):
                with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                    self.dev._claim_launchd_deploy("new")
                with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                    self.dev._claim_launchd_restart("new")
                with mock.patch.object(
                    self.dev, "detect_prod_env", return_value="launchd"
                ) as detect_environment:
                    with self.assertRaises(SystemExit):
                        self.dev.cmd_prod_stop()
                    detect_environment.assert_called_once_with()
            self.assertEqual(status.read_text(), original)

    def test_claim_release_fsync_failure_restores_visible_owner(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            active.write_text("owner\n")
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "_fsync_directory",
                side_effect=[OSError("fsync failed"), None],
            ) as fsync_directory:
                with self.assertRaisesRegex(OSError, "fsync failed"):
                    self.dev._release_launchd_deploy_claim("owner")
            self.assertEqual(fsync_directory.call_args_list, [mock.call(active.parent)] * 2)
            self.assertEqual(active.read_text(), "owner\n")
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch.object(
                self.dev, "_fsync_directory",
                side_effect=[OSError("unlink fsync failed"), OSError("restored claim fsync failed")],
            ) as fsync_directory:
                with self.assertRaisesRegex(OSError, "restored claim fsync failed"):
                    self.dev._release_launchd_deploy_claim_unlocked("owner")
            self.assertEqual(fsync_directory.call_args_list, [mock.call(active.parent)] * 2)
            self.assertEqual(active.read_text(), "owner\n")

    def test_symlinked_manifest_is_unknown_and_retained_by_pruning(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transactions = root / "transactions"
            transactions.mkdir()
            external = root / "external-manifest.json"
            external.write_text(json.dumps({"source_kind": "local_head"}))
            retained = transactions / "symlinked"
            retained.mkdir()
            (retained / "manifest.json").symlink_to(external)
            os.utime(retained, (1, 1))
            for index in range(4):
                ordinary = transactions / f"ordinary-{index}"
                ordinary.mkdir()
                (ordinary / "manifest.json").write_text(
                    json.dumps({"source_kind": "local_head"})
                )
                os.utime(ordinary, (100 + index, 100 + index))
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root):
                self.assertIs(
                    self.dev._deploy_manifest_kind(retained),
                    self.dev.DeployManifestKind.UNKNOWN,
                )
                self.dev._prune_launchd_deploy_transactions(transactions, "current")
            self.assertTrue(retained.exists())

    def test_manifest_mode_requires_exact_source_kind(self):
        payloads = (
            {"source_kind": "prepared_artifac", "paired_database_upgrade": {}},
            {"source_kind": "local_head", "paired_database_upgrade": {}},
            {"source_kind": "prepared_artifact", "ordinary_forward_migration": {}},
            {"source_kind": "unknown", "ordinary_forward_migration": {}},
            {"ordinary_forward_migration": {}},
        )
        with tempfile.TemporaryDirectory() as td:
            transaction = Path(td)
            manifest = transaction / "manifest.json"
            for payload in payloads:
                with self.subTest(payload=payload):
                    manifest.write_text(json.dumps(payload))
                    self.assertIs(
                        self.dev._deploy_manifest_kind(transaction),
                        self.dev.DeployManifestKind.UNKNOWN,
                    )

    def test_malformed_mode_payload_is_unknown_and_has_no_recovery_guidance(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps(
                    {
                        "source_kind": "local_head",
                        "ordinary_forward_migration": [],
                    }
                )
            )
            self.assertIs(
                self.dev._deploy_manifest_kind(transaction),
                self.dev.DeployManifestKind.UNKNOWN,
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "candidate_may_migrate",
                        "source_kind": "local_head",
                        "updated_at": datetime.datetime.now(
                            datetime.timezone.utc
                        ).isoformat(),
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("manifest/status mismatch", rendered)
        self.assertNotIn("resume-forward", rendered)
        self.assertNotIn("recover-", rendered)

    def test_forward_only_owner_is_never_terminal_when_manifest_conflicts(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            active.write_text("tx")
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "ordinary_activation_failed_forward_only",
                        "source_kind": "local_head",
                        "finalization_pending": False,
                    }
                )
            )
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps({"source_kind": "local_head"})
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ):
                self.assertFalse(
                    self.dev._status_is_terminal_for_owner(
                        status,
                        "tx",
                        self.dev._DEPLOY_TERMINAL_STATES,
                    )
                )
                with self.assertRaisesRegex(
                    self.dev.ConcurrentLaunchdOperation,
                    "another launchd deployment .* is active or unresolved",
                ):
                    self.dev._claim_launchd_deploy("new")

            self.assertEqual(active.read_text(), "tx")
            self.assertEqual(json.loads(status.read_text())["transaction_id"], "tx")

    def test_non_object_markerless_status_refuses_operations_cleanly(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            status.write_text("[]")
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ):
                with self.assertRaisesRegex(
                    self.dev.ConcurrentLaunchdOperation,
                    "status is unreadable or malformed",
                ):
                    self.dev._claim_launchd_deploy("new")

            self.assertFalse((root / "active").exists())

    def test_markerless_status_partition_fails_closed_except_proven_terminal(self):
        unresolved = (
            ("preparing", False),
            ("prepared", False),
            ("activating", False),
            ("migration_started", False),
            ("activation_failed_database_committed", False),
            ("activation_failed_rollback_failed", False),
            ("candidate_may_migrate", False),
            ("ordinary_activation_failed_forward_only", False),
            ("committed", True),
            ("unknown_state", False),
        )
        resolved = (
            "committed",
            "precondition_failed",
            "activation_failed_rolled_back",
            "ordinary_activation_failed_rollback_failed",
            "rejected_concurrent",
        )
        with tempfile.TemporaryDirectory() as td:
            status_path = Path(td) / "status.json"
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status_path
            ):
                for state, finalization_pending in unresolved:
                    with self.subTest(state=state, finalization_pending=finalization_pending):
                        status_path.write_text(
                            json.dumps(
                                {
                                    "transaction_id": "markerless",
                                    "state": state,
                                    "source_kind": "local_head",
                                    "finalization_pending": finalization_pending,
                                }
                            )
                        )
                        self.assertIsNotNone(self.dev._markerless_operation_refusal())
                for state in resolved:
                    with self.subTest(state=state):
                        status_path.write_text(
                            json.dumps(
                                {
                                    "transaction_id": "markerless",
                                    "state": state,
                                    "source_kind": "local_head",
                                    "finalization_pending": False,
                                }
                            )
                        )
                        self.assertIsNone(self.dev._markerless_operation_refusal())
                status_path.write_text(
                    json.dumps(
                        {
                            "transaction_id": "markerless",
                            "state": "activation_failed_rolled_back",
                            "source_kind": "local_head",
                            "rollback_failure": "not actually resolved",
                        }
                    )
                )
                self.assertIsNotNone(self.dev._markerless_operation_refusal())

    def test_markerless_terminal_source_mode_cross_matrix_refuses_contradictions(self):
        admitted = {
            ("local_head", None),
            ("published_release", None),
            ("published_release", "current_schema"),
            ("prepared_artifact", "paired"),
            ("prepared_artifact", "current_schema"),
        }
        states = ("committed", "precondition_failed", "activation_failed_rolled_back",
                  "ordinary_activation_failed_rollback_failed", "rejected_concurrent")
        with tempfile.TemporaryDirectory() as td, contextlib.ExitStack() as stack:
            root = Path(td)
            status = root / "status.json"
            active = root / "active"
            for name, value in {
                "LAUNCHD_DEPLOY_DIR": root,
                "LAUNCHD_DEPLOY_STATUS_PATH": status,
                "LAUNCHD_DEPLOY_ACTIVE_PATH": active,
                "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH": root / "claim.lock",
                "LAUNCHD_RESTART_ACTIVE_PATH": root / "restart-active",
            }.items():
                stack.enter_context(mock.patch.object(self.dev, name, value))
            stack.enter_context(mock.patch.object(self.dev, "_restart_claim_owner", return_value=None))
            for source in ("local_head", "published_release", "prepared_artifact"):
                for mode in (None, "paired", "current_schema", "invalid"):
                    for state in states:
                        with self.subTest(source=source, mode=mode, state=state):
                            payload = {
                                "transaction_id": "markerless", "source_kind": source,
                                "prepared_mode": mode, "state": state,
                                "finalization_pending": False, "rollback_failure": None,
                            }
                            status.write_text(json.dumps(payload))
                            before = status.read_bytes()
                            coherent = (source, mode) in admitted
                            self.assertEqual(self.dev._markerless_status_is_resolved(payload), coherent)
                            if coherent:
                                self.assertIsNone(self.dev._markerless_operation_refusal())
                            else:
                                self.assertIsNotNone(self.dev._markerless_operation_refusal())
                                with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                                    self.dev._claim_launchd_deploy("replacement", initial_status={"state": "preparing"})
                                with mock.patch("builtins.print") as output:
                                    self.dev._print_launchd_deploy_status()
                                rendered = " ".join(str(call) for call in output.call_args_list)
                                self.assertIn("UNRESOLVED:", rendered)
                                self.assertNotIn("RECOVERY:", rendered)
                            self.assertEqual(status.read_bytes(), before)
                            self.assertFalse(active.exists())

    def test_markerless_terminal_status_requires_exact_field_shapes(self):
        malformed = (
            {
                "transaction_id": "tx",
                "state": "committed",
                "source_kind": "local_head",
                "finalization_pending": [],
            },
            {
                "transaction_id": "tx",
                "state": "committed",
                "source_kind": "local_head",
                "rollback_failure": {},
            },
            {"transaction_id": "tx", "state": ["committed"], "source_kind": "local_head"},
            {"transaction_id": "tx", "state": "committed"},
            {"transaction_id": "tx", "state": "committed", "source_kind": "typo"},
        )
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status_path = root / "status.json"
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status_path
            ):
                for payload in malformed:
                    with self.subTest(payload=payload):
                        status_path.unlink(missing_ok=True)
                        status_path.write_text(json.dumps(payload))
                        self.assertIsNotNone(self.dev._markerless_operation_refusal())
                status_path.unlink()
                status_path.symlink_to(root / "missing-status")
                self.assertIsNotNone(self.dev._markerless_operation_refusal())

    def test_invalid_json_markerless_status_fences_deploy_restart_and_stop(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            status.write_text("{")
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_DIR", root / "restart"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ):
                for admission in (
                    self.dev._claim_launchd_deploy,
                    self.dev._claim_launchd_restart,
                ):
                    with self.subTest(admission=admission.__name__), self.assertRaisesRegex(
                        self.dev.ConcurrentLaunchdOperation,
                        "status is unreadable or malformed",
                    ):
                        admission("new")
                with mock.patch.object(
                    self.dev, "detect_prod_env", return_value="launchd"
                ) as detect_environment:
                    with self.assertRaisesRegex(
                        SystemExit, "status is unreadable or malformed"
                    ):
                        self.dev.cmd_prod_stop()
                    detect_environment.assert_called_once_with()

            self.assertFalse((root / "active").exists())
            self.assertFalse((root / "restart-active").exists())

    def test_prod_stop_ignores_launchd_fences_for_native_backend(self):
        with mock.patch.object(
            self.dev, "detect_prod_env", return_value="native"
        ), mock.patch.object(
            self.dev, "_deploy_claim_owner"
        ) as claim_owner, mock.patch.object(
            self.dev, "_markerless_operation_refusal"
        ) as markerless_refusal, mock.patch.object(
            self.dev, "native_prod_stop"
        ) as native_stop:
            self.dev.cmd_prod_stop()

        claim_owner.assert_not_called()
        markerless_refusal.assert_not_called()
        native_stop.assert_called_once_with()

    def test_unreadable_markerless_status_refuses_operations(self):
        with tempfile.TemporaryDirectory() as td:
            status = Path(td) / "status.json"
            status.write_text("retained")
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                Path, "read_text", side_effect=PermissionError("denied")
            ):
                refusal = self.dev._markerless_operation_refusal()

        self.assertIn("unreadable or malformed", refusal)

    def test_cleanup_controller_rejects_invalid_label_before_subprocess(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "failed.current"
            transaction.mkdir(parents=True)
            retained = transaction / "helper.py"
            retained.write_text("retained helper")
            (transaction / "manifest.json").write_text(
                json.dumps(
                    {
                        "transaction_id": "failed.current",
                        "source_kind": "prepared_artifact",
                        "current_schema_upgrade": {
                            **mode_evidence(transaction, "current_schema_upgrade"),
                            "controller_helper_path": str(retained),
                            "controller_helper_sha256": self.dev._file_sha256(retained),
                            "proof_path": str(transaction / "proof.json"),
                        },
                        "helper_label": "test.phoenix.server",
                    }
                )
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "failed.current",
                        "source_kind": "prepared_artifact",
                        "prepared_mode": "current_schema",
                        "state": "precondition_cleanup_pending",
                    }
                )
            )
            with mock.patch.object(
                self.dev.sys, "platform", "darwin"
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "_deploy_claim_owner", return_value="failed.current"
            ), mock.patch.object(self.dev.subprocess, "run") as backend:
                with self.assertRaisesRegex(SystemExit, "helper label is invalid"):
                    self.dev.cmd_prod_cleanup_precondition(
                        "failed.current", current_schema=True
                    )

            backend.assert_not_called()

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

    def test_precondition_failure_is_classified_as_terminal(self):
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
        with mock.patch.object(
            self.dev, "_paired_recovery_refusal", return_value="pending finalization"
        ), mock.patch.object(
            self.dev, "detect_prod_env", return_value="launchd"
        ) as detect, self.assertRaisesRegex(SystemExit, "pending finalization"):
            self.dev.cmd_prod_stop()
        detect.assert_called_once_with()

    def test_prod_stop_uses_service_target_and_requires_absence(self):
        success = subprocess.CompletedProcess([], 0, "", "")
        with mock.patch.object(self.dev.subprocess, "run", side_effect=[success, success, success]) as run:
            self.dev._launchd_stop_if_loaded()
        self.assertEqual(run.call_args_list[1].args[0], ["launchctl", "bootout", f"gui/{os.getuid()}/{self.dev.LAUNCHD_LABEL}"])
        self.assertIn("--probe-service-absence", run.call_args_list[2].args[0])
        with mock.patch.object(self.dev.subprocess, "run", side_effect=[success, subprocess.CompletedProcess([], 5, "", "")]), self.assertRaisesRegex(SystemExit, "stop failed"):
            self.dev._launchd_stop_if_loaded()

    def test_activation_sqlite_interpreter_probe_failure_prevents_bootstrap(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "PROD_DB_PATH", Path(td) / "prod.db"):
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
            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root / "deploy"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "deploy" / "active"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", root / "deploy" / "status.json"), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"), mock.patch.object(self.dev, "LAUNCHD_INSTALL_DIR", installed), mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", target_plist), mock.patch.object(self.dev, "_restart_claim_owner", return_value=None), mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({"PATH": "/test", "PHOENIX_PASSWORD": "test-only"}, None)), mock.patch.object(self.dev, "_prepare_prepared_artifact", return_value=prepared), mock.patch.object(self.dev, "_current_prod_identity", return_value=previous), mock.patch.object(self.dev, "_binary_identity", side_effect=lambda path: previous if Path(path).name == "rollback-phoenix-ide" else identity), mock.patch.object(self.dev, "_materialize_helper", side_effect=lambda _commit, path, _kind: path.write_text("# retained helper\n")), mock.patch.object(self.dev, "_file_sha256", return_value="c" * 64), mock.patch.object(self.dev, "generate_launchd_plist", return_value=target_plist.read_bytes().decode()), mock.patch.object(self.dev, "_ensure_newsyslog_config"), mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                with self.assertRaises(KeyboardInterrupt):
                    self.dev.launchd_prod_deploy(controller=controller)
            self.assertEqual((root / "deploy" / "active").read_text().strip(), "interrupted")
            status = json.loads((root / "deploy" / "status.json").read_text())
            self.assertEqual(status["state"], "prepared")
            self.assertEqual(status["prepared_mode"], "paired")

    def test_precondition_failure_records_typed_candidate_identity(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "PROD_DB_PATH", Path(td) / "prod.db"):
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

            patches = (
                mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root / "deploy"),
                mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active_path),
                mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "deploy" / "claim.lock"),
                mock.patch.object(self.dev, "LAUNCHD_DEPLOY_LOCK_PATH", root / "deploy" / "activate.lock"),
                mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status_path),
                mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart" / "active"),
                mock.patch.object(self.dev, "LAUNCHD_INSTALL_DIR", root / "install"),
                mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", root / "service.plist"),
                mock.patch.object(self.dev, "PROD_SHA_PATH", root / "deployed.sha"),
                mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)),
                mock.patch.object(self.dev, "_preflight_prod_bind_auth"),
                mock.patch.object(self.dev, "_prepare_local_candidate", return_value=prepared),
                mock.patch.object(self.dev, "_binary_identity", return_value=identity),
                mock.patch.object(self.dev, "capture_login_shell_path", return_value=("/bin", "test")),
                mock.patch.object(self.dev, "print_launchd_path_report"),
                mock.patch.object(
                    self.dev,
                    "_materialize_helper",
                    side_effect=lambda _commit, path, _kind: path.write_text("# retained helper\n"),
                ),
                mock.patch.object(
                    self.dev,
                    "_ensure_newsyslog_config",
                    side_effect=SystemExit("sudo required"),
                ),
                mock.patch.object(self.dev.subprocess, "run", side_effect=run),
            )
            with contextlib.ExitStack() as stack:
                for patcher in patches:
                    stack.enter_context(patcher)
                with self.assertRaisesRegex(SystemExit, "sudo required"):
                    self.dev.launchd_prod_deploy()

            status = json.loads(status_path.read_text())
            self.assertEqual("precondition_failed", status["state"])
            self.assertIs(status["finalization_pending"], False)
            self.assertIsNone(status["rollback_failure"])
            self.assertTrue(self.dev._markerless_status_is_resolved(status))
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
        before, preparation_and_after = workflow.split("\n  build-incident-linux:", 1)
        preparation, after = preparation_and_after.split("\n  build-macos:", 1)
        release_jobs = before + "\n  build-macos:" + after
        self.assertEqual(2, release_jobs.count("git restore ui/dist/.gitkeep"))
        self.assertEqual(4, release_jobs.count('test -z "$(git status --porcelain)"'))
        self.assertEqual(1, preparation.count("git restore ui/dist/.gitkeep"))
        self.assertEqual(1, preparation.count('test -z "$(git status --porcelain)"'))
        self.assertIn("cargo build --locked --profile release-min", preparation)
        self.assertIn('"runner":"macos-15-intel"', workflow)
        self.assertIn('"runner":"macos-15"', workflow)
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
                active = root / "active"
                active.write_text("tx\n")
                status.write_text(json.dumps({"transaction_id": "tx", "state": state, "source_kind": "prepared_artifact", "updated_at": stale}))
                transaction = root / "transactions" / "tx"
                transaction.mkdir(parents=True)
                (transaction / "manifest.json").write_text(
                    json.dumps({"source_kind": "prepared_artifact", "paired_database_upgrade": mode_evidence(root)})
                )
                with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active), mock.patch("builtins.print") as output:
                    self.dev._print_launchd_deploy_status()
                rendered = " ".join(str(c) for c in output.call_args_list)
                self.assertIn("recover-paired", rendered)
                self.assertIn("Do not remove", rendered)
                self.assertNotIn("clearing the active marker", rendered)
                self.assertNotIn("unreadable status", rendered)

    def test_markerless_ordinary_forward_status_preserves_forward_only_guidance(self):
        for state in (
            "candidate_may_migrate",
            "ordinary_activation_failed_forward_only",
        ):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                transaction = root / "transactions" / "tx"
                transaction.mkdir(parents=True)
                (transaction / "manifest.json").write_text(
                    json.dumps({"source_kind": "local_head", "ordinary_forward_migration": mode_evidence(root, "ordinary_forward_migration")})
                )
                status = root / "status.json"
                status.write_text(
                    json.dumps(
                        {
                            "transaction_id": "tx",
                            "state": state,
                            "source_kind": "local_head",
                            "finalization_pending": True,
                            "updated_at": datetime.datetime.now(
                                datetime.timezone.utc
                            ).isoformat(),
                        }
                    )
                )
                with mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_DIR", root
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
                ), mock.patch.object(
                    self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
                ), mock.patch("builtins.print") as output:
                    self.dev._print_launchd_deploy_status()

                rendered = " ".join(str(call) for call in output.call_args_list)
                self.assertIn("UNRESOLVED:", rendered)
                self.assertIn("no active claim owner", rendered)
                self.assertIn("forward-only", rendered)
                self.assertIn("resume-forward tx", rendered)
                self.assertNotIn("recover-paired", rendered)
                self.assertNotIn("recover-current", rendered)

    def test_markerless_forward_state_with_ordinary_manifest_warns_of_conflict(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps({"source_kind": "local_head"})
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "candidate_may_migrate",
                        "source_kind": "local_head",
                        "updated_at": datetime.datetime.now(
                            datetime.timezone.utc
                        ).isoformat(),
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("forward-only status conflicts", rendered)
        self.assertIn("do not redeploy", rendered)
        self.assertNotIn("resume-forward", rendered)
        self.assertNotIn("recover-", rendered)

    def test_markerless_pre_activation_ordinary_status_has_no_forward_recovery_advice(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps({"source_kind": "local_head", "ordinary_forward_migration": mode_evidence(root, "ordinary_forward_migration")})
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "preparing",
                        "source_kind": "local_head",
                        "updated_at": datetime.datetime.now(
                            datetime.timezone.utc
                        ).isoformat(),
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertNotIn("forward-only", rendered)
        self.assertNotIn("resume-forward", rendered)
        self.assertNotIn("recover-", rendered)

    def test_markerless_committed_ordinary_forward_status_is_terminal(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps({"source_kind": "local_head", "ordinary_forward_migration": mode_evidence(root, "ordinary_forward_migration")})
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "committed",
                        "source_kind": "local_head",
                        "finalization_pending": False,
                        "updated_at": datetime.datetime.now(
                            datetime.timezone.utc
                        ).isoformat(),
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertNotIn("UNRESOLVED:", rendered)
        self.assertNotIn("resume-forward", rendered)
        self.assertNotIn("recover-", rendered)

    def test_markerless_unreadable_ordinary_manifest_is_not_classified_as_paired(self):
        stale = (
            datetime.datetime.now(datetime.timezone.utc)
            - datetime.timedelta(minutes=7)
        ).isoformat()
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text("not-json")
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "preparing",
                        "source_kind": "local_head",
                        "updated_at": stale,
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("ordinary transaction manifest", rendered)
        self.assertIn("confirm no helper is running before retrying", rendered)
        self.assertNotIn("prepared transaction", rendered)
        self.assertNotIn("recover-paired", rendered)
        self.assertNotIn("recover-current", rendered)

    def test_precondition_cleanup_pending_advises_cleanup_only(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            helper_path = transaction / "activate.py"
            helper_path.write_text("retained helper")
            (transaction / "manifest.json").write_text(
                json.dumps(
                    {
                        "source_kind": "prepared_artifact",
                        "current_schema_upgrade": {
                            **mode_evidence(transaction, "current_schema_upgrade"),
                            "controller_helper_path": str(helper_path),
                            "controller_helper_sha256": "hash",
                        },
                    }
                )
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "precondition_cleanup_pending",
                        "source_kind": "prepared_artifact",
                        "failure": "reservation failed",
                        "cleanup_failure": "fsync uncertain",
                        "updated_at": datetime.datetime.now(
                            datetime.timezone.utc
                        ).isoformat(),
                    }
                )
            )
            active = root / "active"
            active.write_text("tx")
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("cleanup-current tx", rendered)
        self.assertIn("service was not intentionally stopped", rendered)
        self.assertNotIn("recover-current", rendered)
        self.assertNotIn("recover-paired", rendered)

    def test_markerless_unknown_prepared_status_warns_without_recovery_command(self):
        stale = (
            datetime.datetime.now(datetime.timezone.utc)
            - datetime.timedelta(minutes=7)
        ).isoformat()
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            active = root / "active"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "activating",
                        "source_kind": "prepared_artifact",
                        "updated_at": stale,
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("UNRESOLVED:", rendered)
        self.assertIn("mode cannot be verified", rendered)
        self.assertIn("preserve evidence", rendered.lower())
        self.assertNotIn("prepared transaction", rendered)
        self.assertNotIn("recover-paired", rendered)
        self.assertNotIn("clearing the active marker", rendered)

    def test_markerless_paired_status_warns_without_recovery_command(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "transactions" / "tx"
            transaction.mkdir(parents=True)
            (transaction / "manifest.json").write_text(
                json.dumps(
                    {
                        "source_kind": "prepared_artifact",
                        "paired_database_upgrade": mode_evidence(transaction),
                    }
                )
            )
            status = root / "status.json"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "activating",
                        "source_kind": "prepared_artifact",
                        "updated_at": datetime.datetime.now(
                            datetime.timezone.utc
                        ).isoformat(),
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", root / "active"
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("prepared transaction status has no active claim owner", rendered)
        self.assertNotIn("recover-paired", rendered)
        self.assertNotIn("recover-current", rendered)

    def test_markerless_verified_predecessor_status_does_not_prescribe_recovery(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            active = root / "active"
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "tx",
                        "state": "activation_failed_rolled_back",
                        "source_kind": "prepared_artifact",
                        "recovery_mode": "unchanged_predecessor",
                        "finalization_pending": False,
                        "updated_at": datetime.datetime.now(
                            datetime.timezone.utc
                        ).isoformat(),
                    }
                )
            )
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch("builtins.print") as output:
                self.dev._print_launchd_deploy_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("activation_failed_rolled_back", rendered)
        self.assertNotIn("RECOVERY:", rendered)
        self.assertNotIn("recover-paired", rendered)

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

class ProtectedReleaseRetainedControllerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load(ROOT / "dev.py", "devpy_protected_release_retained_test")

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.staging = self.root / "transactions" / "protected"
        self.staging.mkdir(parents=True)
        self.active = self.root / "active"
        self.active.write_text("protected\n")
        self.status = self.root / "status.json"
        self.script = self.staging / "retained.py"
        self.script.write_bytes(b"pinned controller helper")
        self.upgrade = {
            "controller_helper_path": str(self.script),
            "controller_helper_sha256": self.dev._file_sha256(self.script),
            "controller_source_commit": "c" * 40,
            "database_path": str(self.root / "prod.db"),
            "backup_path": str(self.staging / "backup.db"),
            "proof_path": str(self.staging / "proof.json"),
            "migration_registry_path": str(self.staging / "migrations.rs"),
            "migration_registry_sha256": "d" * 64,
        }
        self.manifest = self.staging / "manifest.json"
        self.payload = {
            "transaction_id": "protected", "helper_label": "com.phoenix-ide.deploy.protected",
            "source_kind": "published_release", "current_schema_upgrade": self.upgrade,
        }
        self.manifest.write_text(json.dumps(self.payload))
        for name, value in {
            "LAUNCHD_DEPLOY_DIR": self.root,
            "LAUNCHD_DEPLOY_ACTIVE_PATH": self.active,
            "LAUNCHD_DEPLOY_STATUS_PATH": self.status,
            "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH": self.root / "claim.lock",
        }.items():
            patcher = mock.patch.object(self.dev, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        patcher = mock.patch.object(self.dev.sys, "platform", "darwin")
        patcher.start()
        self.addCleanup(patcher.stop)

    def write_status(self, state, **fields):
        payload = {
            "transaction_id": "protected", "source_kind": "published_release",
            "prepared_mode": "current_schema", "state": state,
            "finalization_pending": False, "rollback_failure": None,
            "updated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        }
        payload.update(fields)
        self.status.write_text(json.dumps(payload))
        return payload

    def write_exact_source128_completion(self):
        install_dir = self.root / "install"
        install_dir.mkdir(exist_ok=True)
        target_binary = install_dir / "phoenix-ide"
        target_plist = self.root / "Library" / "LaunchAgents" / "phoenix.plist"
        target_plist.parent.mkdir(parents=True, exist_ok=True)
        deployed_sha = self.root / "deployed.sha"
        for name, value in {
            "LAUNCHD_INSTALL_DIR": install_dir,
            "LAUNCHD_PLIST_PATH": target_plist,
            "PROD_SHA_PATH": deployed_sha,
        }.items():
            patcher = mock.patch.object(self.dev, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)

        candidate = self.staging / "candidate-phoenix-ide"
        candidate_plist = self.staging / "candidate.plist"
        rollback = self.staging / "rollback-phoenix-ide"
        rollback_plist = self.staging / "rollback.plist"
        retained = self.staging / "activate.py"
        registry = self.staging / "migrations.rs"
        candidate.write_bytes(b"exact source128 candidate")
        candidate_plist.write_bytes(b"exact source128 plist")
        rollback.write_bytes(b"exact predecessor")
        rollback_plist.write_bytes(b"exact predecessor plist")
        retained.write_bytes(b"exact retained controller")
        registry.write_bytes(b"exact source128 registry")
        target_binary.write_bytes(candidate.read_bytes())
        target_plist.write_bytes(candidate_plist.read_bytes())
        deployed_sha.write_text(self.dev.PREPARED_SOURCE128_COMMIT + "\n")
        created_at = "2026-10-20T00:00:00+00:00"
        commit = self.dev.PREPARED_SOURCE128_COMMIT
        payload = {
            "manifest_version": self.dev.LAUNCHD_HANDOFF_PROTOCOL_VERSION,
            "transaction_id": "protected",
            "source_kind": "prepared_artifact",
            "source_commit": commit,
            "release_tag": None,
            "release_commit": None,
            "expected": {"version": self.dev.PREPARED_SOURCE128_VERSION, "git_sha": commit},
            "previous": {"version": "0.12.9", "git_sha": "b" * 40},
            "previous_deployed_sha": "b" * 40,
            "candidate_binary": str(candidate),
            "candidate_binary_sha256": self.dev._file_sha256(candidate),
            "candidate_plist": str(candidate_plist),
            "candidate_plist_sha256": self.dev._file_sha256(candidate_plist),
            "rollback_binary": str(rollback),
            "rollback_binary_sha256": self.dev._file_sha256(rollback),
            "rollback_plist": str(rollback_plist),
            "rollback_plist_sha256": self.dev._file_sha256(rollback_plist),
            "target_binary": str(target_binary),
            "target_plist": str(target_plist),
            "label": self.dev.LAUNCHD_LABEL,
            "helper_label": f"{self.dev.LAUNCHD_DEPLOY_HELPER_PREFIX}.protected",
            "uid": os.getuid(),
            "health_url": "http://127.0.0.1:8031/api/version",
            "health_insecure_tls": False,
            "previous_health_url": "http://127.0.0.1:8031/api/version",
            "previous_health_insecure_tls": False,
            "previous_health_json": True,
            "active_path": str(self.active),
            "status_path": str(self.status),
            "deployed_sha_path": str(deployed_sha),
            "lock_path": str(self.root / "activate.lock"),
            "claim_lock_path": str(self.root / "claim.lock"),
            "transition_timeout_secs": 90.0,
            "health_timeout_secs": 30.0,
            "paired_database_upgrade": None,
            "current_schema_upgrade": {
                "prepared_source_baseline": 128,
                "candidate_registry_expected_sha256": self.dev.PREPARED_SOURCE128_REGISTRY_SHA256,
                "database_path": str(self.root / "prod.db"),
                "backup_path": str(self.staging / "database-backup.sqlite3"),
                "proof_path": str(self.staging / "database-backup-proof.json"),
                "controller_source_commit": "c" * 40,
                "controller_helper_sha256": self.dev._file_sha256(retained),
                "controller_helper_path": str(retained),
                "migration_registry_sha256": self.dev.PREPARED_SOURCE128_REGISTRY_SHA256,
                "migration_registry_path": str(registry),
            },
            "created_at": created_at,
        }
        status = {
            "transaction_id": "protected",
            "state": "committed",
            "source_kind": "prepared_artifact",
            "source_commit": commit,
            "release_tag": None,
            "release_commit": None,
            "expected_version": self.dev.PREPARED_SOURCE128_VERSION,
            "expected_git_sha": commit,
            "created_at": created_at,
            "updated_at": "2026-10-20T00:01:00+00:00",
            "failure": None,
            "rollback_failure": None,
            "cleanup_failure": None,
            "precondition_cleanup_completed": False,
            "committed_diagnostic": None,
            "finalization_pending": False,
            "recovery_mode": None,
            "prepared_mode": "current_schema",
        }
        self.manifest.write_text(json.dumps(payload))
        self.status.write_text(json.dumps(status))
        self.script = retained
        return payload, status

    def test_classification_keeps_paired_prepared_only_and_current_release_protected(self):
        for source in ("prepared_artifact", "published_release", "local_head", "unknown"):
            for mode, kind in (("paired_database_upgrade", self.dev.DeployManifestKind.PAIRED),
                               ("current_schema_upgrade", self.dev.DeployManifestKind.CURRENT_SCHEMA)):
                with self.subTest(source=source, mode=mode):
                    self.manifest.write_text(json.dumps({"source_kind": source, mode: mode_evidence(self.staging, mode)}))
                    admitted = source == "prepared_artifact" or (source == "published_release" and mode == "current_schema_upgrade")
                    expected = kind if admitted else self.dev.DeployManifestKind.UNKNOWN
                    self.assertIs(self.dev._deploy_manifest_kind(self.staging), expected)
                    self.assertTrue(self.dev._transaction_must_be_retained(self.staging))

    def test_classifier_rejects_empty_missing_and_wrong_type_mode_fields(self):
        for mode, source, kind in (
            ("paired_database_upgrade", "prepared_artifact", self.dev.DeployManifestKind.PAIRED),
            ("current_schema_upgrade", "prepared_artifact", self.dev.DeployManifestKind.CURRENT_SCHEMA),
            ("current_schema_upgrade", "published_release", self.dev.DeployManifestKind.CURRENT_SCHEMA),
            ("ordinary_forward_migration", "published_release", self.dev.DeployManifestKind.ORDINARY_FORWARD),
        ):
            complete = mode_evidence(self.staging, mode)
            self.manifest.write_text(json.dumps({"source_kind": source, mode: complete}))
            self.assertIs(self.dev._deploy_manifest_kind(self.staging), kind)
            loadable = self.staging / "loadable.json"
            loadable.write_text(json.dumps(dataclasses.asdict(make_manifest(self.staging)) | {
                "source_kind": source, mode: complete,
            }))
            loaded = helper.Manifest.load(loadable)
            self.assertEqual(dataclasses.asdict(getattr(loaded, mode)), complete)
            self.write_status("committed", source_kind=source, prepared_mode=(
                kind.value if kind in {self.dev.DeployManifestKind.PAIRED, self.dev.DeployManifestKind.CURRENT_SCHEMA} else None
            ))
            self.assertTrue(self.dev._status_is_terminal_for_owner(
                self.status, "protected", self.dev._DEPLOY_TERMINAL_STATES,
            ))
            malformed = [{}, [], "", False, 0]
            for field in complete:
                malformed.append({key: value for key, value in complete.items() if key != field})
                malformed.extend(complete | {field: wrong} for wrong in (None, False, 1, [], {}, ""))
            malformed.append(complete | {"unexpected": "field"})
            for value in malformed:
                with self.subTest(mode=mode, value=value):
                    self.manifest.write_text(json.dumps({"source_kind": source, mode: value}))
                    self.assertIs(self.dev._deploy_manifest_kind(self.staging), self.dev.DeployManifestKind.UNKNOWN)
                    self.assertTrue(self.dev._transaction_must_be_retained(self.staging))
                    self.assertFalse(self.dev._status_is_terminal_for_owner(
                        self.status, "protected", self.dev._DEPLOY_TERMINAL_STATES,
                    ))
                    for admission in (self.dev._claim_launchd_deploy, self.dev._claim_launchd_restart):
                        with mock.patch.object(self.dev, "LAUNCHD_RESTART_DIR", self.root / "restart"), mock.patch.object(
                            self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", self.root / "restart-active",
                        ), self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                            admission("new-operation")
                        self.assertEqual(self.active.read_text(), "protected\n")

    def test_classifier_accepts_only_complete_typed_prepared_source114_evidence(self):
        qualified = mode_evidence(self.staging, "current_schema_upgrade") | {
            "prepared_source_baseline": 114,
            "candidate_registry_expected_sha256": self.dev.PREPARED_SOURCE114_REGISTRY_SHA256,
        }
        self.manifest.write_text(json.dumps({
            "source_kind": "prepared_artifact",
            "current_schema_upgrade": qualified,
        }))
        self.assertIs(
            self.dev._deploy_manifest_kind(self.staging),
            self.dev.DeployManifestKind.CURRENT_SCHEMA,
        )
        loadable = self.staging / "qualified-loadable.json"
        loadable.write_text(json.dumps(dataclasses.asdict(make_manifest(self.staging)) | {
            "source_kind": "prepared_artifact",
            "current_schema_upgrade": qualified,
        }))
        loaded = helper.Manifest.load(loadable)
        self.assertIsInstance(loaded.current_schema_upgrade, helper.PreparedSource114Upgrade)

        for changed in (
            qualified | {"prepared_source_baseline": 113},
            qualified | {"prepared_source_baseline": "114"},
            qualified | {"candidate_registry_expected_sha256": ""},
            {key: value for key, value in qualified.items() if key != "prepared_source_baseline"},
            qualified | {"unexpected": "field"},
        ):
            with self.subTest(changed=changed):
                self.manifest.write_text(json.dumps({
                    "source_kind": "prepared_artifact",
                    "current_schema_upgrade": changed,
                }))
                self.assertIs(
                    self.dev._deploy_manifest_kind(self.staging),
                    self.dev.DeployManifestKind.UNKNOWN,
                )
                self.assertTrue(self.dev._transaction_must_be_retained(self.staging))

    def test_classifier_accepts_only_exact_prepared_source128_evidence(self):
        commit = self.dev.PREPARED_SOURCE128_COMMIT
        qualified = mode_evidence(self.staging, "current_schema_upgrade") | {
            "prepared_source_baseline": 128,
            "candidate_registry_expected_sha256": self.dev.PREPARED_SOURCE128_REGISTRY_SHA256,
            "migration_registry_sha256": self.dev.PREPARED_SOURCE128_REGISTRY_SHA256,
        }
        payload = {"source_kind": "prepared_artifact", "source_commit": commit,
                   "expected": {"version": self.dev.PREPARED_SOURCE128_VERSION, "git_sha": commit},
                   "current_schema_upgrade": qualified}
        self.manifest.write_text(json.dumps(payload))
        self.assertIs(self.dev._deploy_manifest_kind(self.staging), self.dev.DeployManifestKind.CURRENT_SCHEMA)
        for changed in (
            payload | {"source_kind": "published_release"},
            payload | {"source_commit": "a" * 40},
            payload | {"expected": payload["expected"] | {"version": "0.13.0-rc.4"}},
            payload | {"expected": payload["expected"] | {"git_sha": "a" * 40}},
            payload | {"release_tag": "v0.13.0"},
            payload | {"current_schema_upgrade": qualified | {"prepared_source_baseline": True}},
            payload | {"current_schema_upgrade": qualified | {"candidate_registry_expected_sha256": "d" * 64}},
            payload | {"current_schema_upgrade": qualified | {"migration_registry_sha256": "d" * 64}},
            payload | {"current_schema_upgrade": qualified | {"unexpected": "field"}},
            payload | {"current_schema_upgrade": {k: v for k, v in qualified.items() if k != "prepared_source_baseline"}},
        ):
            with self.subTest(changed=changed):
                self.manifest.write_text(json.dumps(changed))
                self.assertIs(self.dev._deploy_manifest_kind(self.staging), self.dev.DeployManifestKind.UNKNOWN)
                self.assertTrue(self.dev._transaction_must_be_retained(self.staging))

    def test_exact_source128_finalize_admits_ambiguous_commit_and_reports_possible_migration(self):
        _payload, terminal = self.write_exact_source128_completion()
        for state in ("migration_started", "activation_failed_database_committed"):
            with self.subTest(state=state):
                pending = terminal | {
                    "state": state,
                    "failure": "candidate migration interrupted",
                    "committed_diagnostic": "candidate migration may have committed writes",
                    "finalization_pending": True,
                }
                self.status.write_text(json.dumps(pending))
                guidance = self.dev._paired_recovery_refusal("protected")
                self.assertIn("finalize-current protected", guidance)
                self.assertIn("exact retained candidate migration", guidance)
                output = io.StringIO()
                with mock.patch.object(
                    self.dev.subprocess, "run",
                    return_value=subprocess.CompletedProcess([], 0, "", ""),
                ) as run, contextlib.redirect_stdout(output):
                    self.dev.cmd_prod_finalize_paired("protected", current_schema=True)
                self.assertEqual(run.call_count, 3)
                self.assertEqual(
                    run.call_args.args[0][1:3],
                    [str(self.staging / "activate.py"), "finalize-current"],
                )
                self.assertIn("may run only the exact retained candidate migration", output.getvalue())
                self.assertIn("no snapshot restore", output.getvalue())

    def test_exact_source128_claim_release_checkpoint_requires_explicit_strict_finalization(self):
        _payload, status = self.write_exact_source128_completion()
        self.assertTrue(self.dev._exact_source128_published_completion(self.staging))
        self.assertFalse(
            self.dev._status_is_terminal_for_owner(
                self.status, "protected", self.dev._DEPLOY_TERMINAL_STATES,
            )
        )
        guidance = self.dev._paired_recovery_refusal("protected")
        self.assertIn("claim-release checkpoint remains unresolved", guidance)
        self.assertIn("finalize-current protected", guidance)
        with self.assertRaisesRegex(
            self.dev.ConcurrentLaunchdOperation, "claim-release checkpoint"
        ):
            self.dev._claim_launchd_deploy("replacement")
        self.assertEqual(self.active.read_text(), "protected\n")

        output = io.StringIO()
        with mock.patch.object(
            self.dev.subprocess, "run",
            return_value=subprocess.CompletedProcess([], 0, "", ""),
        ) as run, contextlib.redirect_stdout(output):
            self.dev.cmd_prod_finalize_paired("protected", current_schema=True)
        self.assertEqual(run.call_count, 2)
        self.assertFalse(self.active.exists())
        self.assertIn("matching claim release completed", output.getvalue())
        self.assertTrue(self.dev._markerless_status_is_resolved(status))
        self.assertIsNone(self.dev._markerless_operation_refusal())

    def test_exact_source128_resolution_rejects_status_shape_identity_and_published_artifact_drift(self):
        payload, status = self.write_exact_source128_completion()
        malformed_statuses = (
            status | {"unexpected": "field"},
            {key: value for key, value in status.items() if key != "cleanup_failure"},
            status | {"expected_git_sha": "a" * 40},
            status | {"transaction_id": "protecte"},
        )
        for malformed in malformed_statuses:
            with self.subTest(status=malformed):
                self.status.write_text(json.dumps(malformed))
                self.active.write_text("protected\n")
                self.assertFalse(self.dev._exact_source128_published_completion(self.staging))
                self.assertFalse(
                    self.dev._status_is_terminal_for_owner(
                        self.status, "protected", self.dev._DEPLOY_TERMINAL_STATES,
                    )
                )
                with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                    self.dev._claim_launchd_deploy("replacement")
                self.assertEqual(self.active.read_text(), "protected\n")
                self.active.unlink()
                self.assertFalse(self.dev._markerless_status_is_resolved(malformed))
                self.assertIsNotNone(self.dev._markerless_operation_refusal())

        self.status.write_text(json.dumps(status))
        published_paths = (
            Path(payload["target_binary"]),
            Path(payload["target_plist"]),
            Path(payload["deployed_sha_path"]),
        )
        original_bytes = tuple(path.read_bytes() for path in published_paths)
        for path, replacement in zip(
            published_paths,
            (b"wrong binary", b"wrong plist", b"a" * 40 + b"\n"),
        ):
            with self.subTest(path=path):
                path.write_bytes(replacement)
                self.assertFalse(self.dev._exact_source128_published_completion(self.staging))
                self.assertFalse(self.dev._markerless_status_is_resolved(status))
                path.write_bytes(original_bytes[published_paths.index(path)])

        self.manifest.write_text(json.dumps(payload | {"unexpected": "field"}))
        self.assertFalse(self.dev._exact_source128_published_completion(self.staging))
        self.assertFalse(self.dev._markerless_status_is_resolved(status))
        self.manifest.write_text(json.dumps(payload))
        self.status.write_text(json.dumps(status | {"unexpected": "field"}))
        self.active.write_text("protected\n")
        with mock.patch.object(
            self.dev.subprocess, "run",
            return_value=subprocess.CompletedProcess([], 0, "", ""),
        ), self.assertRaisesRegex(SystemExit, "strict terminal status"):
            self.dev.cmd_prod_finalize_paired("protected", current_schema=True)
        self.assertEqual(self.active.read_text(), "protected\n")

    def test_controller_admission_rejects_status_source_mode_cross_matrix(self):
        for mode in ("paired_database_upgrade", "current_schema_upgrade"):
            current = mode == "current_schema_upgrade"
            for source in (("prepared_artifact", "published_release") if current else ("prepared_artifact",)):
                payload = {"transaction_id": "protected", "helper_label": "com.phoenix-ide.deploy.protected",
                           "source_kind": source, mode: mode_evidence(self.staging, mode)}
                self.manifest.write_text(json.dumps(payload))
                for action, state in (("cmd_prod_cleanup_precondition", "precondition_cleanup_pending"),
                                      ("cmd_prod_finalize_paired", "committed"),
                                      ("cmd_prod_recover_paired", "prepared"),
                                      ("cmd_prod_recover_paired", "activation_failed_rollback_failed")):
                    for status_source in (None, "prepared_artifact", "published_release", "local_head", "invalid"):
                        for prepared_mode in (None, "paired", "current_schema", "invalid"):
                            if source == status_source and prepared_mode == ("current_schema" if current else "paired"):
                                continue
                            with self.subTest(mode=mode, source=source, action=action, state=state,
                                              status_source=status_source, prepared_mode=prepared_mode):
                                status = self.write_status(state, source_kind=status_source, prepared_mode=prepared_mode, preparing_pid=123)
                                with mock.patch.object(self.dev.subprocess, "run") as run, mock.patch.object(
                                    self.dev.os, "kill", side_effect=ProcessLookupError
                                ):
                                    with self.assertRaises(SystemExit):
                                        getattr(self.dev, action)("protected", current_schema=current)
                                    run.assert_not_called()
                                self.assertEqual(json.loads(self.status.read_text()), status)
                                self.assertTrue(self.active.exists())

    def test_pre_handoff_recovery_never_uses_dirty_checkout_and_checks_retained_hash(self):
        dirty = self.root / "dirty-checkout"
        (dirty / "scripts").mkdir(parents=True)
        checkout_script = dirty / "scripts/launchd_deploy_helper.py"
        checkout_script.write_text("raise RuntimeError('must never run')")
        for has_manifest in (False, True):
            self.active.write_text("protected\n")
            self.write_status("prepared", preparing_pid=123)
            if has_manifest:
                self.manifest.write_text(json.dumps(self.payload))
            else:
                self.manifest.unlink(missing_ok=True)
            result = (subprocess.CompletedProcess([], 0, "", "") if has_manifest else
                      subprocess.CompletedProcess([], 113, "", f'Could not find service "com.phoenix-ide.deploy.protected" in domain gui/{os.getuid()}'))
            with mock.patch.object(self.dev, "ROOT", dirty), mock.patch.object(
                self.dev.os, "kill", side_effect=ProcessLookupError
            ), mock.patch.object(self.dev.subprocess, "run", return_value=result) as run:
                self.dev.cmd_prod_recover_paired("protected", current_schema=True)
            self.assertNotIn(str(checkout_script), run.call_args.args[0])
            self.assertEqual(run.call_args.args[0][1], str(self.script) if has_manifest else "print")
        self.active.write_text("protected\n")
        self.manifest.write_text(json.dumps(self.payload))
        self.write_status("prepared", preparing_pid=123)
        self.script.write_text("tampered retained helper")
        before = self.status.read_bytes()
        with mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(
            self.dev.subprocess, "run"
        ) as run:
            with self.assertRaisesRegex(SystemExit, "helper binding"):
                self.dev.cmd_prod_recover_paired("protected", current_schema=True)
            run.assert_not_called()
        self.assertEqual(self.status.read_bytes(), before)
        self.assertTrue(self.active.exists())

    def test_manifestless_absence_probe_requires_exact_label_domain_and_failure(self):
        label = "com.phoenix-ide.deploy.protected"
        for returncode, output, admitted in (
            (113, f'Could not find service "{label}" in domain gui/{os.getuid()}', True),
            (113, f'Could not find service "{label}" in domain for user gui: {os.getuid()}', True),
            (0, f'Could not find service "{label}" in domain gui/{os.getuid()}', False),
            (113, f'Could not find service "other" in domain gui/{os.getuid()}', False),
            (113, f'Could not find service "{label}" in domain gui/{os.getuid() + 1}', False),
            (113, "permission denied", False),
        ):
            with self.subTest(output=output, returncode=returncode), mock.patch.object(
                self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], returncode, "", output)
            ):
                self.assertIs(self.dev._probe_deploy_helper_absence(label), admitted)

    def test_terminal_release_status_requires_matching_source_mode_and_no_pending_recovery(self):
        for state in ("committed", "precondition_failed", "activation_failed_rolled_back"):
            self.write_status(state)
            self.assertTrue(self.dev._status_is_terminal_for_owner(self.status, "protected", self.dev._DEPLOY_TERMINAL_STATES))
        for fields in ({"finalization_pending": True}, {"recovery_mode": "unchanged_source"},
                       {"source_kind": "prepared_artifact"}, {"prepared_mode": "paired"}):
            with self.subTest(fields=fields):
                self.write_status("activation_failed_rolled_back", **fields)
                self.assertFalse(self.dev._status_is_terminal_for_owner(self.status, "protected", self.dev._DEPLOY_TERMINAL_STATES))

    def test_pre_handoff_release_abandonment_cleans_allocations_without_runtime_action(self):
        for state, has_manifest in (("preparing", False), ("preparing", True), ("prepared", True)):
            with self.subTest(state=state, has_manifest=has_manifest):
                self.active.write_text("protected\n")
                self.write_status(state, preparing_pid=123)
                if has_manifest:
                    self.manifest.write_text(json.dumps(self.payload))
                else:
                    self.manifest.unlink(missing_ok=True)
                allocations = [Path(self.upgrade["backup_path"]), self.root / ".prod.db.restore-protected", self.staging / "capacity.json"]
                if has_manifest:
                    for path in allocations:
                        path.write_bytes(b"allocation")
                with mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(
                    self.dev.subprocess, "run", return_value=(
                        subprocess.CompletedProcess([], 0, "", "") if has_manifest else
                        subprocess.CompletedProcess([], 113, "", f'Could not find service "com.phoenix-ide.deploy.protected" in domain gui/{os.getuid()}')
                    ),
                ) as run, mock.patch.object(self.dev, "ROOT", self.root / "nonexistent-checkout"):
                    self.dev.cmd_prod_recover_paired("protected", current_schema=True)
                self.assertEqual(run.call_count, 1)
                if has_manifest:
                    self.assertEqual(run.call_args.args[0][1], str(self.script))
                    self.assertIn("--probe-service-absence", run.call_args.args[0])
                else:
                    self.assertEqual(run.call_args.args[0], ["launchctl", "print", f"gui/{os.getuid()}/com.phoenix-ide.deploy.protected"])
                self.assertFalse(self.active.exists())
                self.assertTrue(all(not path.exists() for path in allocations))
                self.assertTrue(self.dev._status_is_terminal_for_owner(self.status, "protected", self.dev._DEPLOY_TERMINAL_STATES))

    def test_pre_handoff_cleanup_enumeration_failure_retains_claim(self):
        for mode, source in (
            ("paired_database_upgrade", "prepared_artifact"),
            ("current_schema_upgrade", "prepared_artifact"),
            ("current_schema_upgrade", "published_release"),
        ):
            with self.subTest(mode=mode, source=source):
                current_schema = mode == "current_schema_upgrade"
                upgrade = mode_evidence(self.staging, mode)
                self.manifest.write_text(json.dumps({
                    "transaction_id": "protected", "helper_label": self.payload["helper_label"],
                    "source_kind": source, mode: upgrade,
                }))
                self.write_status("prepared", source_kind=source,
                                  prepared_mode="current_schema" if current_schema else "paired",
                                  preparing_pid=123)
                backup = Path(upgrade["backup_path"])
                backup.write_bytes(b"allocation")
                with mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(
                    self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", ""),
                ) as run, mock.patch.object(
                    self.dev.os, "scandir", side_effect=PermissionError("enumeration denied"),
                ):
                    with self.assertRaisesRegex(SystemExit, "cleanup is incomplete"):
                        self.dev.cmd_prod_recover_paired("protected", current_schema=current_schema)
                self.assertEqual(run.call_count, 1)
                self.assertIn("--probe-service-absence", run.call_args.args[0])
                self.assertEqual(self.active.read_text(), "protected\n")
                self.assertTrue(backup.exists())
                pending = json.loads(self.status.read_text())
                self.assertEqual(pending["state"], "precondition_cleanup_pending")
                self.assertFalse(pending["precondition_cleanup_completed"])
                self.assertIn("enumeration denied", pending["cleanup_failure"])

    def test_pre_handoff_release_cleanup_failure_retains_claim_and_cleanup_only_guidance(self):
        self.write_status("prepared", preparing_pid=123)
        reserve = self.root / ".prod.db.restore-protected"
        reserve.write_bytes(b"allocation")
        real_unlink = Path.unlink

        def unlink(path, *args, **kwargs):
            if path == reserve:
                raise OSError("injected cleanup failure")
            return real_unlink(path, *args, **kwargs)

        with mock.patch.object(self.dev.os, "kill", side_effect=ProcessLookupError), mock.patch.object(
            self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", ""),
        ), mock.patch.object(Path, "unlink", unlink):
            with self.assertRaisesRegex(SystemExit, "cleanup is incomplete"):
                self.dev.cmd_prod_recover_paired("protected", current_schema=True)
        self.assertTrue(self.active.exists())
        self.assertTrue(reserve.exists())
        self.assertEqual(json.loads(self.status.read_text())["state"], "precondition_cleanup_pending")
        guidance = self.dev._paired_recovery_refusal("protected")
        self.assertIn("cleanup-current", guidance)
        self.assertNotIn("prod recover-current", guidance)

    def test_cleanup_release_uses_retained_helper_only(self):
        self.write_status("precondition_cleanup_pending")
        with mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
            self.dev.cmd_prod_cleanup_precondition("protected", current_schema=True)
        self.assertEqual(run.call_args_list[1].args[0][1], str(self.script))
        plist = plistlib.loads((self.staging / "cleanup-helper.plist").read_bytes())
        self.assertEqual(plist["ProgramArguments"][1:3], [str(self.script), "cleanup-current"])
        self.assertIn("bootstrap", run.call_args.args[0])
        self.assertTrue(self.active.exists())

    def test_finalize_release_uses_retained_helper_without_reselecting_source(self):
        for state in ("committed", "migration_started"):
            with self.subTest(state=state):
                self.write_status(state, finalization_pending=True)
                with mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
                    self.dev.cmd_prod_finalize_paired("protected", current_schema=True)
                self.assertEqual(run.call_args.args[0][1:3], [str(self.script), "finalize-current"])
                self.assertFalse(any("bootstrap" in call.args[0] for call in run.call_args_list))
                self.assertEqual(run.call_count, 3)

    def test_recover_release_uses_retained_helper_and_current_recovery_guidance(self):
        self.write_status("activation_failed_rollback_failed")
        self.assertIn("recover-current", self.dev._paired_recovery_refusal("protected"))
        with mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
            self.dev.cmd_prod_recover_paired("protected", current_schema=True)
        self.assertEqual(run.call_args_list[1].args[0][1], str(self.script))
        plist = plistlib.loads((self.staging / "recovery-helper.plist").read_bytes())
        self.assertEqual(plist["ProgramArguments"][1:3], [str(self.script), "recover-current"])
        self.assertIn("bootstrap", run.call_args.args[0])
        self.assertTrue(self.active.exists())
        self.write_status("committed", finalization_pending=True)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.dev._print_launchd_deploy_status()
        self.assertIn("finalize-current", output.getvalue())
        self.assertNotIn("inconsistent mode", output.getvalue())
        self.write_status("activation_failed_rolled_back", recovery_mode="unchanged_source")
        guidance = self.dev._paired_recovery_refusal("protected")
        self.assertIn("prod recover-current", guidance)
        self.assertNotIn("prod recover-paired", guidance)

    def test_direct_release_paired_entry_refuses_before_claim(self):
        controller = self.dev.ProdDeployControllerOptions(paired_database_upgrade=True)
        with mock.patch.object(self.dev, "_claim_launchd_deploy") as claim:
            with self.assertRaisesRegex(SystemExit, "requires a prepared artifact"):
                self.dev.launchd_prod_deploy("v2.0.0", controller=controller)
        claim.assert_not_called()

    def test_published_paired_mode_refuses_cleanup_finalize_and_recover(self):
        self.payload["paired_database_upgrade"] = self.payload.pop("current_schema_upgrade")
        self.manifest.write_text(json.dumps(self.payload))
        for command, state in ((self.dev.cmd_prod_cleanup_precondition, "precondition_cleanup_pending"),
                               (self.dev.cmd_prod_finalize_paired, "committed"),
                               (self.dev.cmd_prod_recover_paired, "activation_failed_rollback_failed")):
            self.write_status(state, prepared_mode="paired")
            with self.subTest(command=command.__name__), mock.patch.object(self.dev.subprocess, "run") as run:
                with self.assertRaises(SystemExit):
                    command("protected")
                run.assert_not_called()


class LaunchdControllerDurabilityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load(ROOT / "dev.py", "devpy_launchd_controller_durability_test")

    @contextlib.contextmanager
    def deployment(self, *, paired=True, current=False, release=False, public=False, predecessor=True, fail_sync=None, fail_terminal=False, dirty=False, interrupt_handoff=False, candidate_commit="a" * 40, controller_commit="d" * 40, registry_bytes=b"candidate registry bytes\n"):
        with tempfile.TemporaryDirectory() as td, contextlib.ExitStack() as stack:
            root = Path(td)
            deploy = root / "deploy"
            staging = deploy / "transactions" / "durability"
            candidate = root / "candidate"
            candidate.write_bytes(b"candidate")
            installed = root / "installed"
            installed.mkdir()
            target_plist = root / "installed.plist"
            env = {"PATH": "/test", "PHOENIX_PASSWORD": "test-only"}
            plist = plistlib.dumps({"EnvironmentVariables": env})
            if predecessor:
                (root / "prod.db").touch()
                (installed / "phoenix-ide").write_bytes(b"previous")
                target_plist.write_bytes(plist)
            identity = self.dev.RuntimeIdentity("2.0.0", candidate_commit)
            previous = self.dev.RuntimeIdentity("1.0.0", "b" * 40)
            prepared = self.dev.PreparedCandidate(
                binary=candidate,
                source_kind=self.dev.ProdSourceKind.PUBLISHED_RELEASE if release else self.dev.ProdSourceKind.PREPARED_ARTIFACT if paired else self.dev.ProdSourceKind.LOCAL_HEAD,
                source_commit=identity.git_sha, identity=identity,
                release_tag="v2.0.0" if release else None,
                release_commit=identity.git_sha if release else None,
            )
            controller = self.dev.ProdDeployControllerOptions(
                transaction_id="durability", prepared_artifact=root if paired else None,
                expected_full_commit=identity.git_sha if paired else None,
                paired_database_upgrade=paired and not current,
                current_schema_upgrade=current,
            )
            if release:
                controller = dataclasses.replace(
                    controller, enabled=not public, backend=None if public else "launchd",
                    exact_release_tag=None if public else "v2.0.0",
                    expected_full_commit=identity.git_sha,
                    controller_source_commit="c" * 40 if current else None,
                )
            events = []
            real_fsync = os.fsync
            real_sync_dir = self.dev._fsync_directory
            real_write = self.dev._write_json_atomic

            def fsync(fd):
                inode = os.fstat(fd)
                for path in staging.glob("*"):
                    info = path.stat()
                    if path.is_file() and (info.st_dev, info.st_ino) == (inode.st_dev, inode.st_ino):
                        events.append(("file", path.name))
                        if fail_sync == path.name:
                            raise OSError("injected artifact fsync failure")
                        break
                real_fsync(fd)

            def sync_dir(path):
                events.append(("directory", Path(path)))
                if fail_sync == "staging-parent" and Path(path) == staging.parent and staging.exists():
                    raise OSError("injected staging parent fsync failure")
                if fail_sync == "staging" and Path(path) == staging and (staging / "manifest.json").exists() and (staging / "manifest.json").stat().st_mode & 0o777 == 0o400:
                    raise OSError("injected staging fsync failure")
                real_sync_dir(path)

            def write(path, value, **kwargs):
                if value.get("state") == "precondition_failed" and fail_terminal:
                    raise OSError("injected terminal status failure")
                real_write(path, value, **kwargs)
                if "state" in value:
                    events.append(("status", value["state"]))

            def run(command, **_kwargs):
                events.append(("command", tuple(command)))
                if "rev-parse" in command:
                    return subprocess.CompletedProcess(command, 0, controller_commit, "")
                if command[:2] == ["git", "status"] and dirty:
                    return subprocess.CompletedProcess(command, 0, " M dev.py\n", "")
                if command[:2] == ["git", "show"]:
                    return subprocess.CompletedProcess(command, 0, registry_bytes, b"")
                if command[:2] == ["gh", "api"]:
                    contents = b"controller helper bytes\n" if "launchd_deploy_helper.py" in command[2] else b"release registry bytes\n"
                    return subprocess.CompletedProcess(command, 0, contents, b"")
                if "bootstrap" in command and interrupt_handoff:
                    real_write(
                        deploy / "status.json",
                        {
                            "transaction_id": "durability",
                            "state": "candidate_may_migrate",
                            "finalization_pending": True,
                        },
                    )
                    raise KeyboardInterrupt("controller lost accepted handoff")
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, str(self.dev.LAUNCHD_HANDOFF_PROTOCOL_VERSION) + "\n", "")
                return subprocess.CompletedProcess(command, 0, "", "")

            patches = {
                "LAUNCHD_DEPLOY_DIR": deploy,
                "LAUNCHD_DEPLOY_ACTIVE_PATH": deploy / "active",
                "LAUNCHD_DEPLOY_STATUS_PATH": deploy / "status.json",
                "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH": root / "claim.lock",
                "LAUNCHD_INSTALL_DIR": installed,
                "LAUNCHD_PLIST_PATH": target_plist,
                "PROD_SHA_PATH": root / "deployed.sha",
                "PROD_DB_PATH": root / "prod.db",
            }
            for name, value in patches.items():
                stack.enter_context(mock.patch.object(self.dev, name, value))
            mocks = {
                "_restart_claim_owner": {"return_value": None},
                "_launchd_candidate_env": {"return_value": (env, None)},
                "_preflight_prod_bind_auth": {},
                "_prepare_prepared_artifact": {"return_value": prepared},
                "_prepare_local_candidate": {"return_value": prepared},
                "_prepare_release_candidate": {"return_value": prepared},
                "_preflight_ordinary_launchd_policy": {"side_effect": lambda path, staged_helper: events.append(("ordinary_policy", path, staged_helper))},
                "_binary_identity": {"return_value": identity},
                "_resolve_rollback_identity": {"return_value": (previous, "http://localhost:8031/api/version", False, True)},
                "_materialize_helper": {"side_effect": lambda _commit, path, _kind: path.write_text("# retained helper\n")},
                "generate_launchd_plist": {"return_value": plist.decode()},
                "capture_login_shell_path": {"return_value": ("/test", "test")},
                "print_launchd_path_report": {},
                "_ensure_newsyslog_config": {},
                "_report_launchd_handoff": {},
                "_fsync_directory": {"side_effect": sync_dir},
                "_write_json_atomic": {"side_effect": write},
            }
            for name, options in mocks.items():
                stack.enter_context(mock.patch.object(self.dev, name, **options))
            stack.enter_context(mock.patch.object(self.dev.os, "fsync", side_effect=fsync))
            stack.enter_context(mock.patch.object(self.dev.subprocess, "run", side_effect=run))
            yield staging, controller, events

    def test_release_current_schema_fetches_distinct_pinned_helper_and_release_registry(self, public=False):
        materialize = self.dev._materialize_helper
        with self.deployment(paired=False, current=True, release=True, public=public, dirty=True) as (staging, controller, events), \
             mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "_materialize_helper", side_effect=materialize), \
             mock.patch.object(self.dev, "_preflight_ordinary_launchd_policy", side_effect=AssertionError("dirty checkout policy must not run")) as policy, \
             mock.patch.object(self.dev, "ROOT", staging / "nonexistent-checkout"):
            self.dev.launchd_prod_deploy("v2.0.0", controller=controller)
            policy.assert_not_called()
            manifest = json.loads((staging / "manifest.json").read_text())
            self.assertEqual(manifest["source_kind"], "published_release")
            self.assertEqual(manifest["source_commit"], "a" * 40)
            self.assertEqual(manifest["release_commit"], "a" * 40)
            self.assertEqual(manifest["expected"]["git_sha"], "a" * 40)
            current = manifest["current_schema_upgrade"]
            self.assertEqual(current["controller_source_commit"], "c" * 40)
            self.assertEqual(current["controller_helper_sha256"], self.dev._file_sha256(staging / "activate.py"))
            self.assertEqual(current["migration_registry_sha256"], self.dev._file_sha256(staging / "migrations.rs"))
            self.assertEqual((staging / "activate.py").read_bytes(), b"controller helper bytes\n")
            self.assertEqual((staging / "migrations.rs").read_bytes(), b"release registry bytes\n")
            self.assertNotIn("ordinary_forward_migration", manifest)
            requests = [event[1][2] for event in events if event[0] == "command" and event[1][:2] == ("gh", "api")]
            self.assertEqual(requests, [
                "repos/scottopell/phoenix-ide/contents/scripts/launchd_deploy_helper.py?ref=" + "c" * 40,
                "repos/scottopell/phoenix-ide/contents/crates/phoenix-db/src/migrations.rs?ref=" + "a" * 40,
            ])
            self.assertFalse(any(event[0] == "command" and event[1][0] == "git" for event in events))

    def test_public_release_current_schema_preserves_distinct_helper_and_release_provenance(self):
        self.test_release_current_schema_fetches_distinct_pinned_helper_and_release_registry(public=True)

    def test_public_protected_release_uses_installed_environment_without_checkout_env(self):
        read_env = self.dev._launchd_candidate_env
        with self.deployment(paired=False, current=True, release=True, public=True) as (staging, controller, events):
            self.assertFalse(controller.enabled)
            self.assertIsNone(controller.prepared_artifact)
            database = self.dev.PROD_DB_PATH.with_name("installed-custom.db")
            database.touch()
            installed_env = {
                "PATH": "/installed/bin:/usr/bin",
                "PHOENIX_PASSWORD": "installed-test-credential",
                "PHOENIX_DB_PATH": str(database),
                "PHOENIX_VERSION": "1.0.0",
            }
            self.dev.LAUNCHD_PLIST_PATH.write_bytes(plistlib.dumps({"EnvironmentVariables": installed_env}))
            with mock.patch.object(self.dev.sys, "platform", "darwin"), \
                 mock.patch.object(self.dev, "_launchd_candidate_env", side_effect=read_env), \
                 mock.patch.object(self.dev, "_load_env_file", side_effect=AssertionError("checkout environment must not load")) as load_env, \
                 mock.patch.object(self.dev, "capture_login_shell_path", side_effect=AssertionError("installed PATH required")) as login_path:
                self.dev.launchd_prod_deploy("v2.0.0", controller=controller)
                load_env.assert_not_called()
                login_path.assert_not_called()
                expected_env = installed_env | {"PHOENIX_VERSION": "2.0.0"}
                candidate = plistlib.loads((staging / "candidate.plist").read_bytes())
                self.assertEqual(candidate["EnvironmentVariables"], expected_env)
                preflight_env = {key: value for key, value in installed_env.items() if key != "PHOENIX_VERSION"}
                self.assertEqual(self.dev._preflight_prod_bind_auth.call_args_list, [
                    mock.call(preflight_env, socket_activated=True),
                    mock.call(preflight_env, socket_activated=True),
                ])
                self.assertEqual(self.dev.generate_launchd_plist.call_args.kwargs["path_override"], installed_env["PATH"])
            manifest = json.loads((staging / "manifest.json").read_text())
            self.assertEqual(manifest["current_schema_upgrade"]["database_path"], str(database))
            self.assertIn(("status", "prepared"), events)
            self.assertTrue(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_public_protected_release_unreadable_installed_plist_fails_before_claim_or_handoff(self):
        read_env = self.dev._launchd_candidate_env
        for missing in (False, True):
            with self.subTest(missing=missing), self.deployment(paired=False, current=True, release=True, public=True) as (staging, controller, events):
                if missing:
                    self.dev.LAUNCHD_PLIST_PATH.unlink()
                else:
                    self.dev.LAUNCHD_PLIST_PATH.write_bytes(b"not a plist")
                with mock.patch.object(self.dev.sys, "platform", "darwin"), \
                     mock.patch.object(self.dev, "_launchd_candidate_env", side_effect=read_env), \
                     mock.patch.object(self.dev, "_load_env_file", side_effect=AssertionError("checkout environment must not load")) as load_env, \
                     mock.patch.object(self.dev, "_claim_launchd_deploy") as claim:
                    with self.assertRaisesRegex(SystemExit, "installed launchd configuration is unreadable"):
                        self.dev.launchd_prod_deploy("v2.0.0", controller=controller)
                    load_env.assert_not_called()
                    claim.assert_not_called()
                    self.dev._prepare_release_candidate.assert_not_called()
                self.assertFalse(staging.exists())
                self.assertFalse(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
                self.assertFalse(any(event[0] == "command" for event in events))

    def test_ordinary_controller_preflight_failure_prevents_handoff_for_all_sources(self):
        for release in (False, True):
            with self.subTest(release=release), self.deployment(paired=False, release=release) as (staging, controller, events), \
                 mock.patch.object(self.dev.sys, "platform", "darwin"), \
                 mock.patch.object(self.dev, "_preflight_ordinary_launchd_policy", side_effect=RuntimeError("unqualified interval")) as policy:
                with self.assertRaisesRegex(RuntimeError, "unqualified interval"):
                    self.dev.launchd_prod_deploy("v2.0.0" if release else None, controller=controller)
                policy.assert_called_once_with(staging / "manifest.json", staging / "activate.py")
                self.assertIn(("status", "precondition_failed"), events)
                self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_existing_database_without_predecessor_fails_policy_before_handoff(self):
        preflight = self.dev._preflight_ordinary_launchd_policy
        registry = ROOT / "crates/phoenix-db/src/migrations.rs"
        for release in (False, True):
            with self.subTest(release=release), self.deployment(paired=False, release=release, predecessor=False) as (staging, controller, events):
                database = staging.parents[2] / "configured.db"
                with sqlite3.connect(database) as connection:
                    connection.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
                    connection.executemany(
                        "INSERT INTO _migrations VALUES (?, ?)",
                        [
                            entry
                            for entry in helper.migration_registry(
                                registry, qualified_current_target=False,
                            )
                            if entry[0] <= 114
                        ],
                    )
                before = database.read_bytes()
                env = {"PATH": "/test", "PHOENIX_DB_PATH": str(database)}
                plist = plistlib.dumps({"EnvironmentVariables": env}).decode()

                def materialize(_commit, source_path, destination, _kind):
                    self.assertEqual(source_path, "crates/phoenix-db/src/migrations.rs")
                    destination.write_bytes(registry.read_bytes())

                with mock.patch.object(self.dev.sys, "platform", "darwin"), \
                     mock.patch.object(self.dev, "_launchd_candidate_env", return_value=(env, None)), \
                     mock.patch.object(self.dev, "generate_launchd_plist", return_value=plist), \
                     mock.patch.object(self.dev, "_materialize_helper", side_effect=lambda _commit, path, _kind: path.write_bytes(Path(helper.__file__).read_bytes())), \
                     mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize), \
                     mock.patch.object(self.dev, "_preflight_ordinary_launchd_policy", side_effect=preflight) as policy:
                    with self.assertRaisesRegex(RuntimeError, "ordinary pending migration interval is not qualified"):
                        self.dev.launchd_prod_deploy("v2.0.0" if release else None, controller=controller)

                policy.assert_called_once_with(staging / "manifest.json", staging / "activate.py")
                manifest = json.loads((staging / "manifest.json").read_text())
                self.assertEqual(manifest["source_kind"], "published_release" if release else "local_head")
                self.assertIsNone(manifest["previous"])
                ordinary = manifest["ordinary_forward_migration"]
                self.assertEqual(ordinary["database_path"], str(database))
                self.assertEqual(ordinary["migration_registry_sha256"], self.dev._file_sha256(staging / "migrations.rs"))
                self.assertEqual(ordinary["helper_sha256"], self.dev._file_sha256(staging / "activate.py"))
                self.assertEqual(database.read_bytes(), before)
                self.assertFalse(self.dev.PROD_DB_PATH.exists())
                self.assertFalse((self.dev.LAUNCHD_INSTALL_DIR / "phoenix-ide").exists())
                self.assertFalse(self.dev.LAUNCHD_PLIST_PATH.exists())
                self.assertFalse(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
                self.assertIn(("status", "precondition_failed"), events)
                self.assertNotIn(("status", "prepared"), events)
                self.assertFalse(any(event[0] == "command" and event[1][0] == "launchctl" for event in events))

    def test_ordinary_nonprepared_129_over_live128_retains_predecessor_and_rejects_before_stop(self):
        preflight = self.dev._preflight_ordinary_launchd_policy
        registry = ROOT / "crates/phoenix-db/src/migrations.rs"
        for release in (False, True):
            with self.subTest(release=release), self.deployment(paired=False, release=release) as (staging, controller, events):
                database = self.dev.PROD_DB_PATH
                with sqlite3.connect(database) as connection:
                    connection.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
                    connection.executemany("INSERT INTO _migrations VALUES (?, ?)", [
                        item for item in helper.migration_registry(registry, qualified_current_target=False)
                        if item[0] <= 128
                    ])
                before_database = database.read_bytes()
                before_binary = (self.dev.LAUNCHD_INSTALL_DIR / "phoenix-ide").read_bytes()
                before_plist = self.dev.LAUNCHD_PLIST_PATH.read_bytes()
                env = {"PATH": "/test", "PHOENIX_DB_PATH": str(database)}
                plist = plistlib.dumps({"EnvironmentVariables": env}).decode()

                def materialize(_commit, source_path, destination, _kind):
                    self.assertEqual(source_path, "crates/phoenix-db/src/migrations.rs")
                    destination.write_bytes(registry.read_bytes())

                with mock.patch.object(self.dev.sys, "platform", "darwin"), \
                     mock.patch.object(self.dev, "_launchd_candidate_env", return_value=(env, None)), \
                     mock.patch.object(self.dev, "generate_launchd_plist", return_value=plist), \
                     mock.patch.object(self.dev, "_materialize_helper", side_effect=lambda _commit, path, _kind: path.write_bytes(Path(helper.__file__).read_bytes())), \
                     mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize), \
                     mock.patch.object(self.dev, "_preflight_ordinary_launchd_policy", side_effect=preflight) as policy:
                    with self.assertRaisesRegex(RuntimeError, "ordinary pending migration interval is not qualified"):
                        self.dev.launchd_prod_deploy("v2.0.0" if release else None, controller=controller)
                policy.assert_called_once_with(staging / "manifest.json", staging / "activate.py")
                manifest = json.loads((staging / "manifest.json").read_text())
                self.assertIsNone(manifest["paired_database_upgrade"])
                self.assertNotIn("current_schema_upgrade", manifest)
                self.assertIsNotNone(manifest["previous"])
                self.assertEqual(manifest["ordinary_forward_migration"]["database_path"], str(database))
                self.assertEqual(database.read_bytes(), before_database)
                self.assertEqual((self.dev.LAUNCHD_INSTALL_DIR / "phoenix-ide").read_bytes(), before_binary)
                self.assertEqual(self.dev.LAUNCHD_PLIST_PATH.read_bytes(), before_plist)
                self.assertIn(("status", "precondition_failed"), events)
                self.assertNotIn(("status", "prepared"), events)
                self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))
                self.assertFalse(any(event[0] == "command" and event[1][0] == "launchctl" for event in events))

    def test_fresh_database_path_switch_preserves_predecessor_rollback(self):
        for release in (False, True):
            with self.subTest(release=release), self.deployment(paired=False, release=release) as (staging, controller, events):
                old_database = self.dev.PROD_DB_PATH
                old_database.write_bytes(b"predecessor data")
                new_database = old_database.with_name("fresh.db")
                old_env = {"PATH": "/test", "PHOENIX_DB_PATH": str(old_database)}
                self.dev.LAUNCHD_PLIST_PATH.write_bytes(plistlib.dumps({"EnvironmentVariables": old_env}))
                new_env = old_env | {"PHOENIX_DB_PATH": str(new_database)}
                with mock.patch.object(self.dev.sys, "platform", "darwin"), \
                     mock.patch.object(self.dev, "_launchd_candidate_env", return_value=(new_env, None)), \
                     mock.patch.object(self.dev, "generate_launchd_plist", return_value=plistlib.dumps({"EnvironmentVariables": new_env}).decode()), \
                     mock.patch.object(self.dev, "_preflight_ordinary_launchd_policy", side_effect=AssertionError("fresh DB has no migration interval")) as policy:
                    self.dev.launchd_prod_deploy("v2.0.0" if release else None, controller=controller)
                policy.assert_not_called()
                manifest = helper.Manifest.load(staging / "manifest.json")
                self.assertIsNone(manifest.ordinary_forward_migration)
                self.assertFalse((staging / "migrations.rs").exists())
                self.assertFalse(new_database.exists())
                self.assertIsNotNone(manifest.previous)

                def health(manifest_arg, expected, **_kwargs):
                    if expected == manifest.expected:
                        new_database.write_bytes(b"candidate data")
                        raise helper.ActivationError("candidate health failed")
                    self.assertEqual(expected, manifest.previous)
                    restored = plistlib.loads(Path(manifest_arg.target_plist).read_bytes())
                    self.assertEqual(restored["EnvironmentVariables"]["PHOENIX_DB_PATH"], str(old_database))
                    self.assertEqual(old_database.read_bytes(), b"predecessor data")

                with mock.patch.object(helper, "validate_manifest_identities"), \
                     mock.patch.object(helper, "Launchctl") as launchctl, \
                     mock.patch.object(helper, "wait_for_identity", side_effect=health):
                    launchctl.return_value.stop.return_value = 123
                    state = helper.activate(manifest)
                self.assertEqual(state, "activation_failed_rolled_back")
                self.assertEqual(launchctl.return_value.start.call_count, 2)
                self.assertEqual(Path(manifest.target_binary).read_bytes(), b"previous")
                self.assertEqual(new_database.read_bytes(), b"candidate data")
                self.assertEqual(old_database.read_bytes(), b"predecessor data")
                self.assertTrue(helper.status_is_durable_terminal(manifest))
                self.assertTrue(helper.release_durable_terminal_claim(manifest))
                self.assertFalse(Path(manifest.active_path).exists())

    def test_empty_first_install_skips_migration_policy_for_local_and_release(self):
        for release in (False, True):
            with self.subTest(release=release), self.deployment(paired=False, release=release, predecessor=False) as (staging, controller, events), \
                 mock.patch.object(self.dev.sys, "platform", "darwin"), \
                 mock.patch.object(self.dev, "_preflight_ordinary_launchd_policy", side_effect=AssertionError("empty install has no migration interval")) as policy:
                self.assertFalse(self.dev.PROD_DB_PATH.exists())
                self.dev.launchd_prod_deploy("v2.0.0" if release else None, controller=controller)
                policy.assert_not_called()
                manifest = json.loads((staging / "manifest.json").read_text())
                self.assertIsNone(manifest["previous"])
                self.assertNotIn("ordinary_forward_migration", manifest)
                self.assertFalse((staging / "migrations.rs").exists())
                self.assertFalse(self.dev.PROD_DB_PATH.exists())
                self.assertIn(("status", "prepared"), events)
                self.assertTrue(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_ordinary_forward_installer_binds_candidate_registry_and_helper(self):
        with self.deployment(paired=False, current=False) as (staging, controller, events):
            self.dev.launchd_prod_deploy(controller=controller)
            manifest = json.loads((staging / "manifest.json").read_text())
            forward = manifest["ordinary_forward_migration"]
            self.assertEqual(forward["database_path"], str(self.dev.PROD_DB_PATH))
            self.assertEqual(forward["helper_path"], str(staging / "activate.py"))
            self.assertEqual(
                forward["helper_sha256"], self.dev._file_sha256(staging / "activate.py")
            )
            self.assertEqual(forward["migration_registry_path"], str(staging / "migrations.rs"))
            self.assertIn(
                ("command", ("git", "show", "a" * 40 + ":crates/phoenix-db/src/migrations.rs")),
                events,
            )

    def test_ordinary_interrupted_handoff_preserves_forward_boundary_and_claim(self):
        with self.deployment(
            paired=False, current=False, interrupt_handoff=True
        ) as (staging, controller, _events):
            with self.assertRaises(KeyboardInterrupt):
                self.dev.launchd_prod_deploy(controller=controller)
            status = json.loads((staging.parents[1] / "status.json").read_text())
            self.assertEqual(status["state"], "candidate_may_migrate")
            self.assertTrue(status["finalization_pending"])
            self.assertEqual(
                (staging.parents[1] / "active").read_text().strip(), "durability"
            )

    def test_current_schema_direct_launchd_entry_rejects_missing_artifact_without_build(self):
        controller = self.dev.ProdDeployControllerOptions(current_schema_upgrade=True, expected_full_commit="a" * 40)
        with mock.patch.object(self.dev, "_prepare_local_candidate") as build:
            with self.assertRaisesRegex(SystemExit, "requires --prepared-artifact"):
                self.dev.launchd_prod_deploy(controller=controller)
            build.assert_not_called()

    def test_current_schema_stages_candidate_commit_registry_before_handoff(self):
        with self.deployment(current=True) as (staging, controller, events):
            self.dev.launchd_prod_deploy(controller=controller)
            manifest = json.loads((staging / "manifest.json").read_text())
            upgrade = manifest["current_schema_upgrade"]
            registry = staging / "migrations.rs"
            self.assertIsNone(manifest["paired_database_upgrade"])
            self.assertEqual(upgrade["controller_source_commit"], "d" * 40)
            self.assertEqual(upgrade["database_path"], str(self.dev.PROD_DB_PATH))
            self.assertEqual(upgrade["backup_path"], str(staging / "database-backup.sqlite3"))
            self.assertEqual(upgrade["proof_path"], str(staging / "database-backup-proof.json"))
            self.assertEqual(upgrade["controller_helper_path"], str(staging / "activate.py"))
            self.assertEqual(upgrade["controller_helper_sha256"], self.dev._file_sha256(staging / "activate.py"))
            self.assertEqual(upgrade["migration_registry_path"], str(registry))
            self.assertEqual(upgrade["migration_registry_sha256"], self.dev._file_sha256(registry))
            self.assertEqual(registry.read_bytes(), b"candidate registry bytes\n")
            self.assertEqual(registry.stat().st_mode & 0o777, 0o600)
            self.assertIn(("command", ("git", "show", "a" * 40 + ":crates/phoenix-db/src/migrations.rs")), events)
            prepared = events.index(("status", "prepared"))
            self.assertLess(events.index(("file", "migrations.rs")), prepared)
            self.assertLess(events.index(("directory", staging)), prepared)
            self.assertLess(prepared, next(i for i, event in enumerate(events) if event[0] == "command" and "bootstrap" in event[1]))

    def test_prepared114_receipt_candidate_gets_typed_evidence_from_exact_git_object(self):
        commit = self.dev.PREPARED_SOURCE114_COMMIT
        registry_bytes = subprocess.check_output(
            ["git", "show", f"{commit}:crates/phoenix-db/src/migrations.rs"], cwd=ROOT,
        )
        with self.deployment(current=True, candidate_commit=commit, registry_bytes=registry_bytes) as (staging, controller, events), \
             mock.patch.object(self.dev, "ROOT", staging / "different-controller-checkout"):
            self.dev.launchd_prod_deploy(controller=controller)
            manifest = json.loads((staging / "manifest.json").read_text())
            current = manifest["current_schema_upgrade"]
            self.assertEqual(manifest["source_commit"], commit)
            self.assertEqual(manifest["expected"]["git_sha"], commit)
            self.assertEqual(current["prepared_source_baseline"], 114)
            self.assertEqual(current["candidate_registry_expected_sha256"], self.dev.PREPARED_SOURCE114_REGISTRY_SHA256)
            self.assertEqual(current["migration_registry_sha256"], self.dev.PREPARED_SOURCE114_REGISTRY_SHA256)
            self.assertEqual(current["controller_source_commit"], "d" * 40)
            self.assertEqual((staging / "migrations.rs").read_bytes(), registry_bytes)
            self.assertIn(("command", ("git", "show", f"{commit}:crates/phoenix-db/src/migrations.rs")), events)
            self.assertFalse(any(event[0] == "command" and event[1][:2] == ("gh", "api") for event in events))
        with self.deployment(current=True) as (staging, controller, _events):
            self.dev.launchd_prod_deploy(controller=controller)
            current = json.loads((staging / "manifest.json").read_text())["current_schema_upgrade"]
            self.assertNotIn("prepared_source_baseline", current)
            self.assertNotIn("candidate_registry_expected_sha256", current)

    def test_prepared128_controller_manifest_round_trips_through_helper_exact_qualification(self):
        commit = self.dev.PREPARED_SOURCE128_COMMIT
        registry_bytes = subprocess.check_output(
            ["git", "show", f"{commit}:crates/phoenix-db/src/migrations.rs"], cwd=ROOT,
        )
        identity = self.dev.RuntimeIdentity(self.dev.PREPARED_SOURCE128_VERSION, commit)
        with self.deployment(current=True, candidate_commit=commit, registry_bytes=registry_bytes) as (staging, controller, events), \
             mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "ROOT", staging / "different-controller-checkout"), \
             mock.patch.object(self.dev, "_binary_identity", return_value=identity), \
             mock.patch.object(self.dev, "_prepare_prepared_artifact", return_value=self.dev.PreparedCandidate(
                 binary=staging.parents[2] / "candidate", source_kind=self.dev.ProdSourceKind.PREPARED_ARTIFACT,
                 source_commit=commit, identity=identity,
             )):
            self.dev.launchd_prod_deploy(controller=controller)
            manifest_path = staging / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            current = manifest["current_schema_upgrade"]
            self.assertEqual(commit, helper.PREPARED_SOURCE128_COMMIT)
            self.assertEqual(self.dev.PREPARED_SOURCE128_VERSION, helper.PREPARED_SOURCE128_VERSION)
            self.assertEqual(
                self.dev.PREPARED_SOURCE128_REGISTRY_SHA256,
                helper.PREPARED_SOURCE128_REGISTRY_SHA256,
            )
            self.assertEqual(manifest["expected"], identity.as_dict())
            self.assertEqual(manifest["source_kind"], "prepared_artifact")
            self.assertEqual(current["prepared_source_baseline"], 128)
            self.assertEqual(current["candidate_registry_expected_sha256"], self.dev.PREPARED_SOURCE128_REGISTRY_SHA256)
            self.assertEqual(current["migration_registry_sha256"], self.dev.PREPARED_SOURCE128_REGISTRY_SHA256)
            self.assertEqual(current["controller_source_commit"], "d" * 40)
            self.assertIs(self.dev._deploy_manifest_kind(staging), self.dev.DeployManifestKind.CURRENT_SCHEMA)
            loaded = helper.Manifest.load(manifest_path)
            self.assertIsInstance(loaded.current_schema_upgrade, helper.PreparedSource128Upgrade)
            helper.validate_prepared_source128(loaded)
            self.assertIn(("command", ("git", "show", f"{commit}:crates/phoenix-db/src/migrations.rs")), events)
            self.assertFalse(any(event[0] == "command" and event[1][:2] == ("gh", "api") for event in events))

    def test_prepared128_requires_exact_version_registry_and_separate_controller(self):
        commit = self.dev.PREPARED_SOURCE128_COMMIT
        registry_bytes = subprocess.check_output(
            ["git", "show", f"{commit}:crates/phoenix-db/src/migrations.rs"], cwd=ROOT,
        )
        for version, content, controller_commit, error in (
            ("2.0.0", registry_bytes, "d" * 40, "receipt-bound version/commit"),
            (self.dev.PREPARED_SOURCE128_VERSION, b"substituted registry\n", "d" * 40, "registry SHA-256 mismatch"),
            (self.dev.PREPARED_SOURCE128_VERSION, registry_bytes, commit, "different clean controller"),
        ):
            with self.subTest(version=version, controller_commit=controller_commit, content=content[:20]), \
                 self.deployment(current=True, candidate_commit=commit, controller_commit=controller_commit, registry_bytes=content) as (staging, controller, events), \
                 mock.patch.object(self.dev.sys, "platform", "darwin"), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=self.dev.RuntimeIdentity(version, commit)), \
                 mock.patch.object(self.dev, "_prepare_prepared_artifact", return_value=self.dev.PreparedCandidate(
                     binary=staging.parents[2] / "candidate", source_kind=self.dev.ProdSourceKind.PREPARED_ARTIFACT,
                     source_commit=commit, identity=self.dev.RuntimeIdentity(version, commit),
                 )):
                with self.assertRaisesRegex(SystemExit, error):
                    self.dev.launchd_prod_deploy(controller=controller)
                self.assertNotIn(("status", "prepared"), events)
                self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_prepared128_refuses_missing_predecessor_before_handoff(self):
        commit = self.dev.PREPARED_SOURCE128_COMMIT
        identity = self.dev.RuntimeIdentity(self.dev.PREPARED_SOURCE128_VERSION, commit)
        with self.deployment(current=True, candidate_commit=commit, predecessor=False) as (staging, controller, events), \
             mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value=identity), \
             mock.patch.object(self.dev, "_prepare_prepared_artifact", return_value=self.dev.PreparedCandidate(
                 binary=staging.parents[2] / "candidate", source_kind=self.dev.ProdSourceKind.PREPARED_ARTIFACT,
                 source_commit=commit, identity=identity,
             )), \
             self.assertRaisesRegex(SystemExit, "installed launchd predecessor"):
            self.dev.launchd_prod_deploy(controller=controller)
        self.assertNotIn(("status", "prepared"), events)
        self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_public_prepared129_without_protected_mode_refuses_before_dispatch(self):
        options = self.dev.ProdDeployControllerOptions(
            prepared_artifact=Path("prepared"), expected_full_commit=self.dev.PREPARED_SOURCE128_COMMIT,
        )
        with mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check, \
             self.assertRaisesRegex(SystemExit, "required together"):
            self.dev.cmd_prod_deploy(controller=options)
        deploy.assert_not_called()
        check.assert_not_called()

    def test_valid_prepared129_over_live128_without_current_schema_rejects_before_materialization_claim_or_launchctl(self):
        prepare_artifact = self.dev._prepare_prepared_artifact
        commit = self.dev.PREPARED_SOURCE128_COMMIT
        registry = ROOT / "crates/phoenix-db/src/migrations.rs"
        with self.deployment(current=True, candidate_commit=commit) as (staging, controller, events):
            artifact = staging.parents[2] / "prepared129"
            artifact.mkdir()
            target = "aarch64-apple-darwin"
            binary_name = f"phoenix_ide-{target}-prepared-{commit[:12]}"
            binary = artifact / binary_name
            binary.write_bytes(b"valid prepared source128 candidate targeting schema129")
            (artifact / f"PREPARATION-RECEIPT-{target}.json").write_text(json.dumps({
                "schema": 1,
                "operation": "prepare-main",
                "target": target,
                "commit": commit,
                "version": self.dev.PREPARED_SOURCE128_VERSION,
                "checks": {
                    "developer_id_signature": "verified",
                    "hardened_runtime": "verified",
                    "notarization": "accepted",
                    "stapled_ticket": "validated",
                    "gatekeeper": "accepted",
                    "embedded_helper_bytes": "identical",
                    "notarization_submission_id": "123e4567-e89b-12d3-a456-426614174000",
                },
                "sha256": {binary_name: self.dev._file_sha256(binary)},
            }))
            identity = self.dev.RuntimeIdentity(self.dev.PREPARED_SOURCE128_VERSION, commit)
            verify = subprocess.CompletedProcess([], 0, "", "")
            display = subprocess.CompletedProcess(
                [], 0, "Authority=Developer ID Application: Example\n"
                "CodeDirectory v=20500 flags=0x10000(runtime)\nTimestamp=2026-01-01\n", "",
            )
            with mock.patch.object(self.dev.sys, "platform", "darwin"), \
                 mock.patch.object(self.dev.platform, "machine", return_value="arm64"), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=[verify, display]), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=identity):
                validated = prepare_artifact(artifact, commit)
            self.assertEqual(validated.identity, identity)
            self.assertIs(validated.source_kind, self.dev.ProdSourceKind.PREPARED_ARTIFACT)

            database = self.dev.PROD_DB_PATH
            database.unlink()
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)")
                connection.executemany("INSERT INTO _migrations VALUES (?, ?)", [
                    item for item in helper.migration_registry(registry, qualified_current_target=False)
                    if item[0] <= 128
                ])
            before = database.read_bytes()
            for paired in (False, True):
                unprotected = dataclasses.replace(
                    controller,
                    prepared_artifact=artifact,
                    current_schema_upgrade=False,
                    paired_database_upgrade=paired,
                )
                with self.subTest(paired=paired), \
                     mock.patch.object(self.dev.sys, "platform", "darwin"), \
                     mock.patch.object(self.dev, "_claim_launchd_deploy") as claim, \
                     mock.patch.object(self.dev, "_prepare_prepared_artifact") as candidate_prepare, \
                     mock.patch.object(self.dev, "_materialize_helper") as materialize_helper, \
                     mock.patch.object(self.dev.subprocess, "run") as backend, \
                     self.assertRaisesRegex(SystemExit, "requires --current-schema-upgrade"):
                    self.dev.launchd_prod_deploy(controller=unprotected)
                claim.assert_not_called()
                candidate_prepare.assert_not_called()
                materialize_helper.assert_not_called()
                backend.assert_not_called()
            self.assertEqual(database.read_bytes(), before)
            self.assertFalse((staging / "manifest.json").exists())
            self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_prepared114_controller_rejects_bad_registry_same_controller_and_dirty_checkout(self):
        commit = self.dev.PREPARED_SOURCE114_COMMIT
        for options, error in (({}, "registry SHA-256 mismatch"),
                               ({"controller_commit": commit}, "different clean controller"),
                               ({"dirty": True}, "clean controller checkout")):
            with self.subTest(options=options), self.deployment(current=True, candidate_commit=commit, **options) as (staging, controller, events):
                with self.assertRaisesRegex(SystemExit, error):
                    self.dev.launchd_prod_deploy(controller=controller)
                self.assertNotIn(("status", "prepared"), events)
                self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_current_schema_rejects_dirty_controller_before_staging_registry(self):
        with self.deployment(current=True, dirty=True) as (staging, controller, events):
            with self.assertRaisesRegex(SystemExit, "clean controller checkout"):
                self.dev.launchd_prod_deploy(controller=controller)
            self.assertFalse((staging / "migrations.rs").exists())
            self.assertNotIn(("status", "prepared"), events)
            self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_current_schema_registry_fsync_failure_prevents_handoff(self):
        with self.deployment(current=True, fail_sync="migrations.rs") as (staging, controller, events):
            with self.assertRaisesRegex(OSError, "injected artifact fsync failure"):
                self.dev.launchd_prod_deploy(controller=controller)
            self.assertNotIn(("status", "prepared"), events)
            self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_reused_transaction_cannot_mutate_hardlinked_installed_plist(self):
        with self.deployment() as (staging, controller, events):
            self.dev.launchd_prod_deploy(controller=controller)
            target = self.dev.LAUNCHD_PLIST_PATH
            target.unlink()
            os.link(staging / "candidate.plist", target)
            captured = {path.name: path.read_bytes() for path in staging.iterdir() if path.is_file()}
            target_before = target.read_bytes()
            status_before = self.dev.LAUNCHD_DEPLOY_STATUS_PATH.read_bytes()
            self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.unlink()
            events.clear()
            with self.assertRaises(FileExistsError):
                self.dev.launchd_prod_deploy(controller=controller)
            self.assertEqual(target.read_bytes(), target_before)
            self.assertEqual(self.dev.LAUNCHD_DEPLOY_STATUS_PATH.read_bytes(), status_before)
            self.assertEqual(captured, {path.name: path.read_bytes() for path in staging.iterdir() if path.is_file()})
            self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))

    def test_retained_artifacts_are_durable_before_prepared_and_bootstrap(self):
        for paired, predecessor in ((True, True), (False, True), (False, False)):
            with self.subTest(paired=paired, predecessor=predecessor), self.deployment(paired=paired, predecessor=predecessor) as (staging, controller, events):
                self.dev.launchd_prod_deploy(controller=controller)
                retained = {"candidate-phoenix-ide", "candidate.plist", "activate.py", "helper.plist", "manifest.json"}
                if predecessor:
                    retained.update({"rollback-phoenix-ide", "rollback.plist"})
                prepared = events.index(("status", "prepared"))
                bootstrap = next(i for i, event in enumerate(events) if event[0] == "command" and "bootstrap" in event[1])
                barrier = max(i for i, event in enumerate(events[:prepared]) if event == ("directory", staging))
                last_file_sync = max(events.index(("file", name)) for name in retained)
                self.assertLess(last_file_sync, barrier)
                self.assertLess(barrier, prepared)
                self.assertLess(prepared, bootstrap)
                first_file_sync = min(events.index(("file", name)) for name in retained)
                self.assertTrue(any(event == ("directory", staging.parent) for event in events[:first_file_sync]))
                verification = next(i for i, event in enumerate(events) if event[0] == "command" and event[1][:3] == ("codesign", "--verify", "--strict"))
                self.assertLess(verification, events.index(("file", "candidate-phoenix-ide")))
                self.assertEqual((staging / "manifest.json").stat().st_mode & 0o777, 0o400)

    def test_each_artifact_or_staging_fsync_failure_prevents_handoff(self):
        for artifact in ("candidate-phoenix-ide", "rollback-phoenix-ide", "candidate.plist", "rollback.plist", "helper.plist", "activate.py", "manifest.json", "staging", "staging-parent"):
            with self.subTest(artifact=artifact), self.deployment(fail_sync=artifact) as (staging, controller, events):
                with self.assertRaisesRegex(OSError, "injected .*fsync failure"):
                    self.dev.launchd_prod_deploy(controller=controller)
                self.assertNotIn(("status", "prepared"), events)
                self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))
                self.assertEqual(json.loads((staging.parents[1] / "status.json").read_text())["state"], "precondition_failed")
                self.assertFalse((staging.parents[1] / "active").exists())

    def test_paired_preparation_and_terminal_write_failures_retain_preparing_claim(self):
        with self.deployment(fail_terminal=True) as (staging, controller, events):
            with mock.patch.object(self.dev, "_prepare_prepared_artifact", side_effect=SystemExit("preparation failed")), mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release:
                with self.assertRaisesRegex(OSError, "terminal status failure"):
                    self.dev.launchd_prod_deploy(controller=controller)
            release.assert_not_called()
            self.assertEqual((staging.parents[1] / "active").read_text().strip(), controller.transaction_id)
            self.assertEqual(json.loads((staging.parents[1] / "status.json").read_text())["state"], "preparing")
            self.assertFalse(any(event[0] == "command" and "bootstrap" in event[1] for event in events))


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

    def test_public_current_schema_release_command_parses_and_dispatches_without_hidden_flags(self):
        argv = ["dev.py", "prod", "deploy", "--release", "v2.0.0",
                "--expected-full-commit", "a" * 40,
                "--controller-source-commit", "c" * 40, "--current-schema-upgrade"]
        with mock.patch.object(self.dev.sys, "argv", argv), \
             mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev.sys.stdout, "reconfigure"), \
             mock.patch.object(self.dev.sys.stderr, "reconfigure"):
            self.dev.main()
        options = deploy.call_args.kwargs["controller"]
        self.assertEqual(deploy.call_args.args, ("v2.0.0",))
        self.assertFalse(options.enabled)
        self.assertIsNone(options.backend)
        self.assertIsNone(options.exact_release_tag)
        self.assertIsNone(options.expected_asset_name)
        self.assertIsNone(options.expected_asset_sha256)
        self.assertEqual(options.expected_full_commit, "a" * 40)
        self.assertEqual(options.controller_source_commit, "c" * 40)
        self.assertTrue(options.current_schema_upgrade)
        check.assert_not_called()

    def test_public_protected_release_option_matrix(self):
        valid = self.dev.ProdDeployControllerOptions(
            expected_full_commit="a" * 40, controller_source_commit="c" * 40,
            current_schema_upgrade=True,
        )
        invalid = [
            (None, valid), ("latest", valid), ("stable", valid), ("v2", valid),
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit=None)),
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit="c" * 12)),
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit="C" * 40)),
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit="a" * 40)),
            ("v2.0.0", dataclasses.replace(valid, expected_full_commit=None)),
            ("v2.0.0", dataclasses.replace(valid, expected_full_commit="a" * 12)),
            ("v2.0.0", dataclasses.replace(valid, expected_full_commit="A" * 40)),
            ("v2.0.0", dataclasses.replace(valid, current_schema_upgrade=False)),
            ("v2.0.0", dataclasses.replace(valid, paired_database_upgrade=True)),
            ("v2.0.0", dataclasses.replace(valid, prepared_artifact=Path("prepared"))),
        ]
        for release, options in invalid:
            for entry in (self.dev.cmd_prod_deploy, self.dev.launchd_prod_deploy):
                with self.subTest(release=release, options=options, entry=entry.__name__), \
                     mock.patch.object(self.dev.sys, "platform", "darwin"), \
                     mock.patch.object(self.dev, "_claim_launchd_deploy") as claim, \
                     mock.patch.object(self.dev, "cmd_check") as check, self.assertRaises(SystemExit):
                    entry(release, controller=options)
                claim.assert_not_called()
                check.assert_not_called()
        for platform, backend in (("linux", "native"), ("darwin", "daemon")):
            with self.subTest(platform=platform, backend=backend), \
                 mock.patch.object(self.dev.sys, "platform", platform), \
                 mock.patch.object(self.dev, "detect_prod_env", return_value=backend), \
                 mock.patch.object(self.dev, "_claim_launchd_deploy") as claim, \
                 self.assertRaisesRegex(SystemExit, "only by macOS launchd"):
                self.dev.cmd_prod_deploy("v2.0.0", controller=valid)
            claim.assert_not_called()

    def test_protected_release_controller_option_matrix(self):
        valid = self.dev.ProdDeployControllerOptions(
            enabled=True, backend="launchd", exact_release_tag="v2.0.0",
            expected_full_commit="a" * 40, controller_source_commit="c" * 40,
            current_schema_upgrade=True,
        )
        with mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check:
            self.dev.cmd_prod_deploy("v2.0.0", controller=valid)
            deploy.assert_called_once_with("v2.0.0", controller=valid)
            check.assert_not_called()
        invalid = (
            (None, valid),
            ("v2.0.0", dataclasses.replace(valid, enabled=False)),  # hidden bindings without controller mode
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit=None)),
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit="c" * 12)),
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit="C" * 40)),
            ("v2.0.0", dataclasses.replace(valid, controller_source_commit="a" * 40)),
            ("v2.0.0", dataclasses.replace(valid, current_schema_upgrade=False)),
            ("v2.0.0", dataclasses.replace(valid, paired_database_upgrade=True)),
            ("v2.0.0", dataclasses.replace(valid, prepared_artifact=Path("prepared"))),
            ("v2.0.0", dataclasses.replace(valid, backend="systemd")),
            ("v2.0.0", dataclasses.replace(valid, expected_full_commit=None)),
            ("v2.0.0", dataclasses.replace(valid, exact_release_tag=None)),
            ("latest", valid),
        )
        for release, options in invalid:
            for entry in (self.dev.cmd_prod_deploy, self.dev.launchd_prod_deploy):
                with self.subTest(release=release, options=options, entry=entry.__name__), \
                     mock.patch.object(self.dev.sys, "platform", "darwin"), \
                     mock.patch.object(self.dev, "_claim_launchd_deploy") as claim, \
                     mock.patch.object(self.dev, "cmd_check") as check, \
                     self.assertRaises(SystemExit):
                    entry(release, controller=options)
                claim.assert_not_called()
                check.assert_not_called()
        with mock.patch.object(self.dev.sys, "platform", "linux"), self.assertRaisesRegex(SystemExit, "only by macOS launchd"):
            self.dev.cmd_prod_deploy("v2.0.0", controller=valid)

    def test_current_schema_options_require_triad_and_exclude_release(self):
        controller = self.dev.ProdDeployControllerOptions(
            prepared_artifact=Path("prepared"), expected_full_commit="a" * 40,
            current_schema_upgrade=True,
        )
        with mock.patch.object(self.dev.sys, "platform", "darwin"), \
             mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check:
            self.dev.cmd_prod_deploy(controller=controller)
            deploy.assert_called_once_with(None, controller=controller)
            check.assert_not_called()
            with self.assertRaisesRegex(SystemExit, "excludes --release"):
                self.dev.cmd_prod_deploy("v1.2.3", controller=controller)
            with self.assertRaisesRegex(SystemExit, "mutually exclusive"):
                self.dev.cmd_prod_deploy(controller=dataclasses.replace(controller, paired_database_upgrade=True))
        with mock.patch.object(self.dev.sys, "platform", "linux"):
            with self.assertRaisesRegex(SystemExit, "only by macOS launchd"):
                self.dev.cmd_prod_deploy(controller=controller)
        for invalid in (
            self.dev.ProdDeployControllerOptions(current_schema_upgrade=True),
            self.dev.ProdDeployControllerOptions(current_schema_upgrade=True, expected_full_commit="a" * 40),
            self.dev.ProdDeployControllerOptions(current_schema_upgrade=True, prepared_artifact=Path("prepared")),
        ):
            with self.subTest(invalid=invalid), self.assertRaisesRegex(SystemExit, "required together"):
                self.dev.cmd_prod_deploy(controller=invalid)

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

    def test_current_schema_receipt_validation_failure_prevents_handoff(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "status.json"
            controller = self.dev.ProdDeployControllerOptions(
                prepared_artifact=root / "artifact", expected_full_commit="a" * 40,
                current_schema_upgrade=True, transaction_id="failed-current-receipt",
            )
            with mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_claim_launchd_deploy"), \
                 mock.patch.object(self.dev, "_release_launchd_deploy_claim"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status), \
                 mock.patch.object(self.dev, "_prepare_prepared_artifact", side_effect=SystemExit("receipt rejected")) as validate, \
                 mock.patch.object(self.dev.subprocess, "run") as run:
                with self.assertRaisesRegex(SystemExit, "receipt rejected"):
                    self.dev.launchd_prod_deploy(controller=controller)
                validate.assert_called_once_with(root / "artifact", "a" * 40)
                run.assert_not_called()
                self.assertEqual(json.loads(status.read_text())["state"], "precondition_failed")

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
