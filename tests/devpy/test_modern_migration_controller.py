"""Controller-only offline ordinary migration gates; no host service operations."""
import contextlib
import importlib.util
import json
import io
import os
import shutil
import sqlite3
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("devpy_modern_migration_controller", ROOT / "dev.py")
dev = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = dev
SPEC.loader.exec_module(dev)


class MigrationControllerTests(unittest.TestCase):
    @contextlib.contextmanager
    def deployment(self, *, release=False, capability=True, absent=True, bootstrap=True, fail_sync=False):
        with tempfile.TemporaryDirectory() as td, contextlib.ExitStack() as stack:
            root = Path(td).resolve()
            installed = root / "installed"
            installed.mkdir()
            binary = installed / "phoenix-ide"
            binary.write_bytes(b"previous binary")
            database = root / "database.sqlite3"
            backup = root / "backup.sqlite3"
            rehearsal = root / "rehearsal.sqlite3"
            for path in (database, backup, rehearsal):
                path.write_bytes(b"mock offline database bytes")
            env = {"PATH": "/installed/path", "PHOENIX_DB_PATH": str(database), "PHOENIX_VERSION": "1.0.0"}
            plist = root / "installed.plist"
            plist.write_bytes(plistlib.dumps({"EnvironmentVariables": env}))
            receipt = {
                "schema": 1, "database_path": str(database), "database_sha256": dev._file_sha256(database),
                "backup_path": str(backup), "backup_sha256": dev._file_sha256(backup),
                "rehearsal_path": str(rehearsal), "rehearsal_sha256": dev._file_sha256(rehearsal),
                "previous_binary_sha256": dev._file_sha256(binary), "previous_plist_sha256": dev._file_sha256(plist),
            }
            receipt_path = root / "receipt.json"
            receipt_path.write_text(json.dumps(receipt))
            for path in (backup, rehearsal, receipt_path):
                path.chmod(0o600)
            deploy = root / "deploy"
            staging = deploy / "transactions" / "test-modern"
            paths = {
                "LAUNCHD_DEPLOY_DIR": deploy, "LAUNCHD_DEPLOY_ACTIVE_PATH": deploy / "active",
                "LAUNCHD_DEPLOY_STATUS_PATH": deploy / "status.json", "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH": root / "claim.lock",
                "LAUNCHD_DEPLOY_LOCK_PATH": root / "activate.lock", "LAUNCHD_RESTART_ACTIVE_PATH": root / "restart-active",
                "LAUNCHD_INSTALL_DIR": installed, "LAUNCHD_PLIST_PATH": plist, "PROD_SHA_PATH": root / "deployed.sha",
            }
            for key, value in paths.items():
                stack.enter_context(mock.patch.object(dev, key, value))
            stack.enter_context(mock.patch.object(dev.sys, "platform", "darwin"))
            identity = dev.RuntimeIdentity("2.0.0", "a" * 40)
            candidate = root / "candidate"
            candidate.write_bytes(b"candidate")
            kind = dev.ProdSourceKind.PUBLISHED_RELEASE if release else dev.ProdSourceKind.LOCAL_HEAD
            prepared = dev.PreparedCandidate(binary=candidate, identity=identity, source_kind=kind, source_commit=identity.git_sha,
                                             release_tag="v2.0.0" if release else None, release_commit=identity.git_sha if release else None)
            events = []
            real_write = dev._write_json_atomic
            real_fsync = os.fsync

            def write(path, value, **kwargs):
                real_write(path, value, **kwargs)
                if "state" in value:
                    events.append(("status", value["state"]))

            def fsync(fd):
                info = os.fstat(fd)
                for path in staging.glob("migration-*.sqlite3"):
                    if path.stat().st_ino == info.st_ino:
                        events.append(("fsync", path.name))
                        if fail_sync:
                            raise OSError("injected migration artifact fsync failure")
                real_fsync(fd)

            def run(command, **kwargs):
                events.append(("command", tuple(command)))
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, str(dev.LAUNCHD_HANDOFF_PROTOCOL_VERSION), "")
                if "--supports-ordinary-migration" in command:
                    return subprocess.CompletedProcess(command, 0 if capability else 2, "1" if capability else "", "")
                if "--probe-service-absence" in command:
                    return subprocess.CompletedProcess(command, 0 if absent else 1, "", "")
                if "bootstrap" in command and not bootstrap:
                    return subprocess.CompletedProcess(command, 1, "", "injected bootstrap failure")
                return subprocess.CompletedProcess(command, 0, "", "")

            patches = {
                "_preflight_prod_bind_auth": {}, "_prepare_local_candidate": {"return_value": prepared},
                "_prepare_release_candidate": {"return_value": prepared}, "_binary_identity": {"return_value": identity},
                "_resolve_rollback_identity": {"return_value": (dev.RuntimeIdentity("1.0.0", "b" * 40), "http://localhost:8031/api/version", False, True)},
                "_materialize_helper": {"side_effect": lambda commit, path, source: (events.append(("materialize", commit, source)), path.write_text("# immutable candidate helper\n"))},
                "_materialize_source_file": {"side_effect": lambda commit, source, path, kind: path.write_bytes((ROOT / source).read_bytes())},
                "generate_launchd_plist": {"return_value": plist.read_text()}, "capture_login_shell_path": {},
                "print_launchd_path_report": {}, "_ensure_newsyslog_config": {}, "_report_launchd_handoff": {},
                "_load_env_file": {"side_effect": AssertionError("ambient env must not be loaded")},
                "_write_json_atomic": {"side_effect": write},
            }
            mocks = {name: stack.enter_context(mock.patch.object(dev, name, **options)) for name, options in patches.items()}
            stack.enter_context(mock.patch.object(dev.os, "fsync", side_effect=fsync))
            stack.enter_context(mock.patch.object(dev.subprocess, "run", side_effect=run))
            controller = dev.ProdDeployControllerOptions(transaction_id="test-modern", migration_backup_receipt=receipt_path)
            yield root, staging, controller, receipt, events, mocks

    def test_incomplete_preparation_abandons_only_dead_owner_with_absent_helper(self):
        for alive, absence in ((False, True), (True, True), (False, False)):
            with self.subTest(alive=alive, absence=absence), self.deployment() as (_root, staging, _options, _receipt, _events, _mocks):
                status = {"transaction_id": "test-modern", "state": "preparing", "ordinary_migration": True, "preparing_pid": 999999}
                dev.LAUNCHD_DEPLOY_STATUS_PATH.parent.mkdir(parents=True, exist_ok=True)
                dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps(status))
                dev.LAUNCHD_DEPLOY_ACTIVE_PATH.write_text("test-modern")
                with mock.patch.object(dev.os, "kill", return_value=None if alive else None, side_effect=None if alive else ProcessLookupError()), mock.patch.object(dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0 if absence else 1, "", "")) as backend:
                    if alive or not absence:
                        with self.assertRaises(SystemExit):
                            dev.cmd_prod_resume_migration("test-modern")
                        self.assertEqual(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.read_text(), "test-modern")
                    else:
                        dev.cmd_prod_resume_migration("test-modern")
                        self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
                        self.assertEqual(json.loads(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())["state"], "precondition_failed")
                    self.assertTrue(all("--probe-service-absence" in call.args[0] for call in backend.call_args_list))

    def test_receipt_exact_shape_hashes_and_source_bindings(self):
        with self.deployment() as (root, _, options, receipt, _, _):
            env, source = dev._launchd_candidate_env(options)
            self.assertIsNone(source)
            self.assertNotIn("PHOENIX_VERSION", env)
            self.assertEqual(env["PATH"], "/installed/path")
            mutations = [None, [], {**receipt, "extra": 1}, {**receipt, "schema": True}, {**receipt, "schema": 2},
                         {**receipt, "backup_sha256": "BAD"}, {**receipt, "previous_binary_sha256": "0" * 64},
                         {**receipt, "database_sha256": "0" * 64}, {**receipt, "backup_path": "relative.sqlite3"},
                         {**receipt, "rehearsal_path": receipt["backup_path"]}, {**receipt, "database_path": receipt["backup_path"]}]
            for value in mutations:
                with self.subTest(value=value):
                    options.migration_backup_receipt.write_text(json.dumps(value))
                    with self.assertRaisesRegex(SystemExit, "invalid migration backup receipt"):
                        dev._read_migration_backup_receipt(options.migration_backup_receipt, env)
            options.migration_backup_receipt.write_text("{")
            with self.assertRaises(SystemExit):
                dev.launchd_prod_deploy(controller=options)
            self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())

    def test_nonprivate_original_inputs_rejected_before_staging(self):
        for kind in ("receipt", "backup", "rehearsal"):
            for permission in (0o040, 0o020, 0o010, 0o004, 0o002, 0o001):
                with self.subTest(kind=kind, permission=oct(permission)), self.deployment() as (_, staging, options, receipt, events, _):
                    path = options.migration_backup_receipt if kind == "receipt" else Path(receipt[kind + "_path"])
                    path.chmod(0o600 | permission)
                    with self.assertRaisesRegex(SystemExit, "private"):
                        dev.launchd_prod_deploy(controller=options)
                    self.assertFalse(staging.exists())
                    self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
                    self.assertEqual(path.stat().st_mode & 0o777, 0o600 | permission)
                    self.assertFalse(any(event[0] == "command" for event in events))

    def test_staging_rechecks_original_backup_privacy(self):
        with self.deployment() as (_, staging, _, receipt, _, _):
            staging.mkdir(parents=True)
            retained_helper = staging / "helper.py"
            retained_helper.write_text("helper")
            Path(receipt["backup_path"]).chmod(0o644)
            with self.assertRaisesRegex(ValueError, "private"):
                dev._stage_ordinary_migration(receipt, staging, retained_helper, source_commit="a" * 40, source_kind="published_release")
            self.assertFalse(list(staging.glob("migration-*.sqlite3")))

    def test_receipt_symlinks_and_hardlinks_rejected(self):
        with self.deployment() as (root, _, options, receipt, _, _):
            env, _ = dev._launchd_candidate_env(options)
            for link in ("symbolic", "hard"):
                path = root / link
                if link == "symbolic":
                    path.symlink_to(receipt["backup_path"])
                else:
                    os.link(receipt["backup_path"], path)
                options.migration_backup_receipt.write_text(json.dumps({**receipt, "backup_path": str(path)}))
                with self.assertRaises(SystemExit):
                    dev._read_migration_backup_receipt(options.migration_backup_receipt, env)
                path.unlink()

    def test_policy_scope_gates_before_any_checks(self):
        for platform, kwargs in [("linux", {}), ("darwin", {"enabled": True}), ("darwin", {"paired_database_upgrade": True}),
                                 ("darwin", {"prepared_artifact": Path("artifact")}), ("darwin", {"backend": "systemd"})]:
            with self.subTest(platform=platform, kwargs=kwargs), mock.patch.object(dev.sys, "platform", platform), mock.patch.object(dev, "cmd_check") as checks:
                options = dev.ProdDeployControllerOptions(migration_backup_receipt=Path("receipt"), **kwargs)
                with self.assertRaisesRegex(SystemExit, "ordinary macOS"):
                    dev.cmd_prod_deploy(controller=options)
                checks.assert_not_called()

    def test_command_routes_ordinary_policy_to_launchd_and_preserves_source_checks(self):
        with mock.patch.object(dev.sys, "platform", "darwin"), mock.patch.object(dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(dev, "cmd_check") as checks, mock.patch.object(dev, "launchd_prod_deploy") as deploy:
            options = dev.ProdDeployControllerOptions(migration_backup_receipt=Path("/receipt"))
            dev.cmd_prod_deploy(controller=options)
            checks.assert_called_once()
            deploy.assert_called_once_with(None, controller=options)
            checks.reset_mock()
            deploy.reset_mock()
            dev.cmd_prod_deploy("v2.0.0", controller=options)
            checks.assert_not_called()
            deploy.assert_called_once_with("v2.0.0", controller=options)
        with mock.patch.object(dev.sys, "platform", "darwin"), mock.patch.object(dev, "detect_prod_env", return_value="daemon"), mock.patch.object(dev, "cmd_check") as checks:
            with self.assertRaisesRegex(SystemExit, "requires macOS launchd"):
                dev.cmd_prod_deploy(controller=options)
            checks.assert_not_called()

    def test_success_stages_durable_receipt_and_uses_candidate_commit_helper(self):
        for release in (False, True):
            with self.subTest(release=release), self.deployment(release=release) as (_, staging, options, receipt, events, mocks):
                dev.launchd_prod_deploy("v2.0.0" if release else None, controller=options)
                payload = json.loads((staging / "manifest.json").read_text())
                ordinary = payload["ordinary_migration"]
                self.assertNotIn("schema", ordinary)
                self.assertIsNone(payload["paired_database_upgrade"])
                self.assertEqual(ordinary["database_path"], receipt["database_path"])
                self.assertEqual(ordinary["controller_helper_path"], str(staging / "activate.py"))
                self.assertIn(("materialize", "a" * 40, "published_release" if release else "local_head"), events)
                prepared_index = events.index(("status", "prepared"))
                for kind in ("backup", "rehearsal"):
                    path = Path(ordinary[kind + "_path"])
                    self.assertEqual(path.parent, staging)
                    self.assertNotEqual(str(path), receipt[kind + "_path"])
                    self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                    self.assertLess(events.index(("fsync", path.name)), prepared_index)
                self.assertTrue(json.loads(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())["ordinary_migration"])
                mocks["capture_login_shell_path"].assert_not_called()
                self.assertEqual(plistlib.loads((staging / "candidate.plist").read_bytes())["EnvironmentVariables"]["PATH"], "/installed/path")
                commands = [event[1] for event in events if event[0] == "command"]
                self.assertFalse(any("bootout" in command for command in commands))
                self.assertEqual(sum("bootstrap" in command for command in commands), 1)

    def test_old_helper_and_unconfirmed_absence_refuse_without_disruption(self):
        for capability, absent in ((False, True), (True, False)):
            with self.subTest(capability=capability, absent=absent), self.deployment(capability=capability, absent=absent) as (_, _, options, _, events, _):
                with self.assertRaises(SystemExit):
                    dev.launchd_prod_deploy(controller=options)
                self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
                self.assertEqual(json.loads(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())["state"], "precondition_failed")
                self.assertFalse(any("bootstrap" in event[1] or "bootout" in event[1] for event in events if event[0] == "command"))

    def test_migration_fsync_failure_prevents_prepared_and_handoff(self):
        with self.deployment(fail_sync=True) as (_, staging, options, _, events, _):
            with self.assertRaisesRegex(OSError, "fsync"):
                dev.launchd_prod_deploy(controller=options)
            self.assertNotIn(("status", "prepared"), events)
            self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
            self.assertFalse(staging.exists())

    def test_post_copy_preparation_failure_cleans_private_staging_before_release(self):
        for fault in ("helper_plist", "manifest", "directory_fsync"):
            with self.subTest(fault=fault), self.deployment() as (_, staging, options, receipt, _, _), contextlib.ExitStack() as stack:
                if fault == "helper_plist":
                    stack.enter_context(mock.patch.object(dev, "_helper_plist", side_effect=OSError("helper plist failure")))
                elif fault == "manifest":
                    original = dev._write_json_atomic
                    def write(path, value, **kwargs):
                        if path == staging / "manifest.json":
                            raise OSError("manifest failure")
                        return original(path, value, **kwargs)
                    stack.enter_context(mock.patch.object(dev, "_write_json_atomic", side_effect=write))
                else:
                    original = dev._fsync_directory
                    def sync(path):
                        if path == staging:
                            raise OSError("staging directory fsync failure")
                        return original(path)
                    stack.enter_context(mock.patch.object(dev, "_fsync_directory", side_effect=sync))
                with self.assertRaises(OSError):
                    dev.launchd_prod_deploy(controller=options)
                self.assertFalse(staging.exists())
                self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
                self.assertTrue(Path(receipt["backup_path"]).exists())
                self.assertEqual(json.loads(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())["state"], "precondition_failed")

    def test_cleanup_failure_retains_owner_and_supported_abandon_retries_cleanup(self):
        with self.deployment() as (_, staging, options, receipt, events, _):
            with mock.patch.object(dev, "_helper_plist", side_effect=OSError("preparation failure")), mock.patch.object(dev.shutil, "rmtree", side_effect=OSError("cleanup failure")):
                with self.assertRaisesRegex(OSError, "cleanup failure"):
                    dev.launchd_prod_deploy(controller=options)
            self.assertEqual(dev._deploy_claim_owner(), "test-modern")
            self.assertTrue(list(staging.glob("migration-*.sqlite3")))
            status = json.loads(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())
            self.assertTrue(status["cleanup_pending"])
            events.clear()
            with mock.patch.object(dev.os, "kill", side_effect=ProcessLookupError()):
                dev.cmd_prod_resume_migration("test-modern")
            self.assertFalse(staging.exists())
            self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
            self.assertTrue(Path(receipt["backup_path"]).exists())
            self.assertFalse(any("bootstrap" in event[1] for event in events if event[0] == "command"))

    def test_actual_112_lifecycle_cleanup_failure_manual_restore_and_terminal_retry(self):
        from tests.devpy.test_modern_migration import helper, Backend, actual_migrations
        with self.deployment(release=True) as (_, staging, options, receipt, _, _):
            backend = Backend(SimpleNamespace(uid=os.getuid(), label=dev.LAUNCHD_LABEL))
            backend.state = ("running", 123)
            backend.stop()
            self.assertEqual(backend.state, ("not_loaded", None))
            database = Path(receipt["database_path"])
            database.unlink()
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)")
                connection.executemany("INSERT INTO _migrations VALUES (?, ?, '2026-10-05')", actual_migrations(112))
                connection.execute("CREATE TABLE messages (id TEXT PRIMARY KEY, content TEXT NOT NULL)")
                connection.execute("INSERT INTO messages VALUES ('original-message', 'must survive')")
                self.assertEqual(connection.execute("SELECT count(*) FROM _migrations WHERE version=99").fetchone(), (0,))
                connection.commit()
                self.assertEqual(connection.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()[0], 0)
            Path(receipt["backup_path"]).unlink()
            with sqlite3.connect(database) as source, sqlite3.connect(receipt["backup_path"]) as backup:
                source.backup(backup)
            shutil.copyfile(receipt["backup_path"], receipt["rehearsal_path"])
            for kind in ("database", "backup", "rehearsal"):
                path = Path(receipt[kind + "_path"])
                path.chmod(0o600)
                receipt[kind + "_sha256"] = dev._file_sha256(path)
            options.migration_backup_receipt.write_text(json.dumps(receipt))
            with mock.patch.object(dev, "_helper_plist", side_effect=OSError("post-copy failure")), mock.patch.object(dev.shutil, "rmtree", side_effect=OSError("cleanup interruption")):
                with self.assertRaises(OSError):
                    dev.launchd_prod_deploy("v2.0.0", controller=options)
            self.assertTrue(json.loads(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())["cleanup_pending"])
            with mock.patch.object(dev.os, "kill", side_effect=ProcessLookupError()):
                dev.cmd_prod_resume_migration("test-modern")
            self.assertFalse(staging.exists())
            self.assertIsNone(dev._deploy_claim_owner())
            with mock.patch.object(dev, "_materialize_helper", side_effect=lambda commit, path, source: shutil.copyfile(ROOT / "scripts/launchd_deploy_helper.py", path)):
                dev.launchd_prod_deploy("v2.0.0", controller=options)
            manifest_path = staging / "manifest.json"
            manifest = helper.Manifest.load(manifest_path)
            backend.manifest = manifest
            backend.target = f"gui/{manifest.uid}/{manifest.label}"
            registry = Path(manifest.ordinary_migration.migration_registry_path)
            self.assertEqual(helper.ordinary_database_ledger(database, registry)[-1][:2], actual_migrations(112)[-1])
            def candidate_mutation():
                with sqlite3.connect(database) as connection:
                    connection.execute("INSERT INTO _migrations VALUES (?, ?, '2026-10-05')", actual_migrations(113)[-1])
                    connection.execute("CREATE TABLE candidate_changes (value TEXT)")
            backend.on_start = candidate_mutation
            argv = ["helper", "activate", "--manifest", str(manifest_path), "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
            with mock.patch.object(helper, "__file__", manifest.ordinary_migration.controller_helper_path), mock.patch.object(helper, "Launchctl", return_value=backend), mock.patch.object(helper.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")), mock.patch.object(helper, "request_helper_bootout"), mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("candidate failed")) as health:
                with mock.patch.object(sys, "argv", argv):
                    self.assertEqual(helper.main(), 1)
                self.assertEqual(helper.read_status(manifest)["state"], "migration_failed_stopped")
                failed_projection = helper.read_status(manifest)
                self.assertEqual(backend.state, ("not_loaded", None))
                self.assertEqual(helper.ordinary_database_ledger(database, registry)[-1][0], 113)
                shutil.copyfile(manifest.ordinary_migration.backup_path, database)
                backend.on_start = None
                health.side_effect = None
                argv[1] = "resume-migration"
                with mock.patch.object(sys, "argv", argv), mock.patch.object(helper, "release_claim", side_effect=OSError("release interrupted")):
                    self.assertEqual(helper.main(), 1)
                status = Path(manifest.status_path).read_bytes()
                self.assertEqual(helper.read_status(manifest)["state"], "activation_failed_rolled_back")
                self.assertEqual(helper.read_status(manifest)["recovery_mode"], "migration_resumed")
                self.assertIn("Manual offline matched database restoration verified", helper.read_status(manifest)["failure"])
                for consumer in (ROOT / "crates/phoenix-ide/src/api/release_updates.rs", ROOT / "ui/src/pages/ReleaseUpdatePanel.tsx"):
                    self.assertIn("activation_failed_rolled_back", consumer.read_text())
                with sqlite3.connect(database) as connection:
                    self.assertEqual(connection.execute("SELECT * FROM messages").fetchall(), [("original-message", "must survive")])
                    connection.execute("INSERT INTO messages VALUES ('later', 'accepted after resume')")
                for path in (manifest.ordinary_migration.backup_path, manifest.ordinary_migration.rehearsal_path, manifest.ordinary_migration.migration_registry_path):
                    Path(path).unlink()
                later = database.read_bytes()
                database.unlink()
                events = list(backend.events)
                real_sync = helper.fsync_dir
                failed_after_unlink = False
                def sync_failure(path):
                    nonlocal failed_after_unlink
                    if path == Path(manifest.active_path).parent and not Path(manifest.active_path).exists() and not failed_after_unlink:
                        failed_after_unlink = True
                        raise OSError("claim directory sync failed after unlink")
                    return real_sync(path)
                with mock.patch.object(sys, "argv", argv), mock.patch.object(helper, "fsync_dir", side_effect=sync_failure):
                    self.assertEqual(helper.main(), 1)
                self.assertTrue(failed_after_unlink)
                self.assertEqual(Path(manifest.active_path).read_text().strip(), manifest.transaction_id)
                self.assertEqual(Path(manifest.status_path).read_bytes(), status)
                self.assertEqual(backend.events, events)
                print_output = io.StringIO()
                with contextlib.redirect_stdout(print_output):
                    dev._print_launchd_deploy_status()
                self.assertIn("RECOVERY:", print_output.getvalue())
                sync_events = []
                def sync_success(path):
                    if path == Path(manifest.active_path).parent:
                        sync_events.append(Path(manifest.active_path).exists())
                    return real_sync(path)
                with mock.patch.object(sys, "argv", argv), mock.patch.object(helper, "fsync_dir", side_effect=sync_success):
                    self.assertEqual(helper.main(), 0)
                self.assertIn(False, sync_events)
                with mock.patch.object(sys, "argv", argv):
                    self.assertEqual(helper.main(), 0)
                Path(manifest.active_path).write_text("another-owner")
                with mock.patch.object(sys, "argv", argv):
                    self.assertEqual(helper.main(), 1)
                self.assertEqual(Path(manifest.active_path).read_text(), "another-owner")
                Path(manifest.active_path).unlink()
                print_output = io.StringIO()
                with contextlib.redirect_stdout(print_output):
                    dev._print_launchd_deploy_status()
                self.assertNotIn("RECOVERY:", print_output.getvalue())
                self.assertIn("Manual offline matched database restoration verified", print_output.getvalue())
                if output := os.environ.get("PHOENIX_MIGRATION_TEST_PROJECTIONS"):
                    destination = Path(output).resolve()
                    if not destination.is_relative_to(ROOT):
                        raise AssertionError("projection fixture must stay in test worktree")
                    destination.write_text(json.dumps([failed_projection, helper.read_status(manifest)]))
                self.assertEqual(backend.events, events)
                self.assertEqual(Path(manifest.status_path).read_bytes(), status)
                self.assertFalse(Path(manifest.active_path).exists())
                self.assertFalse(database.exists())
                database.write_bytes(later)
                with sqlite3.connect(database) as connection:
                    self.assertEqual(connection.execute("SELECT count(*) FROM messages").fetchone(), (2,))

    def test_bootstrap_attempt_failure_retains_claim_and_policy(self):
        with self.deployment(bootstrap=False) as (_, _, options, _, _, _):
            with self.assertRaisesRegex(SystemExit, "hand activation"):
                dev.launchd_prod_deploy(controller=options)
            self.assertEqual(dev._deploy_claim_owner(), "test-modern")
            status = json.loads(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())
            self.assertEqual(status["state"], "prepared")
            self.assertTrue(status["ordinary_migration"])
            self.assertIn("resume-migration", dev._paired_recovery_refusal("test-modern"))

    def test_configuration_changed_under_claim_refuses(self):
        with self.deployment() as (_, _, options, _, _, _):
            real_claim = dev._claim_launchd_deploy
            def claim(*args, **kwargs):
                real_claim(*args, **kwargs)
                dev.LAUNCHD_PLIST_PATH.write_bytes(b"changed config")
            with mock.patch.object(dev, "_claim_launchd_deploy", side_effect=claim), self.assertRaisesRegex(SystemExit, "configuration changed"):
                dev.launchd_prod_deploy(controller=options)
            self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())

    def test_receipt_changed_under_claim_refuses(self):
        with self.deployment() as (_, _, options, receipt, _, _):
            real_claim = dev._claim_launchd_deploy
            def claim(*args, **kwargs):
                real_claim(*args, **kwargs)
                Path(receipt["database_path"]).write_bytes(b"concurrent mutation")
            with mock.patch.object(dev, "_claim_launchd_deploy", side_effect=claim), self.assertRaisesRegex(SystemExit, "checksum mismatch"):
                dev.launchd_prod_deploy(controller=options)
            self.assertFalse(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())

    def test_retained_manifest_fences_deploy_restart_stop_even_without_status_marker(self):
        with self.deployment() as (_, _, options, _, _, _):
            dev.launchd_prod_deploy(controller=options)
            for state in ("activating", "migration_failed_stopped", "migration_resumed", "activation_failed_rolled_back", "activation_failed_rollback_failed", "unknown"):
                with self.subTest(state=state):
                    dev._write_json_atomic(dev.LAUNCHD_DEPLOY_STATUS_PATH, {"transaction_id": "test-modern", "state": state})
                    self.assertFalse(dev._status_is_terminal_for_owner(dev.LAUNCHD_DEPLOY_STATUS_PATH, "test-modern", dev._DEPLOY_TERMINAL_STATES))
                    for action in (lambda: dev._claim_launchd_deploy("other"), lambda: dev._claim_launchd_restart("other"), dev.cmd_prod_stop):
                        with self.assertRaisesRegex((SystemExit, dev.ConcurrentLaunchdOperation), "resume-migration"):
                            action()
                    self.assertEqual(dev._deploy_claim_owner(), "test-modern")
            for state in ("committed", "precondition_failed"):
                dev._write_json_atomic(dev.LAUNCHD_DEPLOY_STATUS_PATH, {"transaction_id": "test-modern", "state": state})
                self.assertIsNone(dev._paired_recovery_refusal("test-modern"))
                self.assertTrue(dev._status_is_terminal_for_owner(dev.LAUNCHD_DEPLOY_STATUS_PATH, "test-modern", dev._DEPLOY_TERMINAL_STATES))

    def test_preparing_policy_marker_fences_before_manifest_exists(self):
        with self.deployment() as (_, _, _, _, _, _):
            dev._claim_launchd_deploy("test-modern", initial_status={"transaction_id": "test-modern", "state": "preparing", "ordinary_migration": True})
            with self.assertRaisesRegex(dev.ConcurrentLaunchdOperation, "resume-migration"):
                dev._claim_launchd_restart("other")

    def test_resume_foreground_dispatch_has_only_readonly_preprobes(self):
        with self.deployment() as (_, _, options, _, events, _):
            dev.launchd_prod_deploy(controller=options)
            events.clear()
            before = dev.LAUNCHD_DEPLOY_ACTIVE_PATH.read_bytes()
            with mock.patch.object(dev.os, "kill", side_effect=ProcessLookupError()):
                dev.cmd_prod_resume_migration("test-modern")
            commands = [event[1] for event in events if event[0] == "command"]
            self.assertEqual(len(commands), 3)
            self.assertIn("sqlite3", commands[0][-1])
            self.assertIn("--probe-service-absence", commands[1])
            self.assertEqual(commands[2][2], "resume-migration")
            self.assertNotIn("launchctl", commands[2])
            self.assertEqual(dev.LAUNCHD_DEPLOY_ACTIVE_PATH.read_bytes(), before)

    def test_live_prepared_controller_cannot_race_resume_dispatch(self):
        with self.deployment() as (_root, _staging, options, _receipt, events, _mocks):
            dev.launchd_prod_deploy(controller=options)
            before = dev.LAUNCHD_DEPLOY_STATUS_PATH.read_bytes()
            events.clear()
            with self.assertRaisesRegex(SystemExit, "controller remains alive"):
                dev.cmd_prod_resume_migration("test-modern")
            self.assertEqual(dev.LAUNCHD_DEPLOY_STATUS_PATH.read_bytes(), before)
            self.assertFalse(events)

    def test_resume_invalid_binding_or_claim_never_dispatches(self):
        for corruption in ("hash", "claim", "symlink", "source"):
            with self.subTest(corruption=corruption), self.deployment() as (root, staging, options, _, events, _):
                dev.launchd_prod_deploy(controller=options)
                path = staging / "manifest.json"
                payload = json.loads(path.read_text())
                if corruption == "hash":
                    payload["ordinary_migration"]["controller_helper_sha256"] = "0" * 64
                elif corruption == "claim":
                    dev.LAUNCHD_DEPLOY_ACTIVE_PATH.write_text("other")
                elif corruption == "source":
                    payload["source_kind"] = "prepared_artifact"
                else:
                    link = staging / "symlink.py"
                    link.symlink_to(staging / "activate.py")
                    payload["ordinary_migration"]["controller_helper_path"] = str(link)
                path.chmod(0o600)
                path.write_text(json.dumps(payload))
                events.clear()
                with self.assertRaisesRegex(SystemExit, "resume refused"):
                    dev.cmd_prod_resume_migration("test-modern")
                self.assertFalse(events)

    def test_resume_invalid_platform_or_transaction_id_never_dispatches(self):
        for platform, transaction in (("linux", "tx"), ("darwin", "../tx"), ("darwin", "."), ("darwin", "..")):
            with self.subTest(platform=platform, transaction=transaction), mock.patch.object(dev.sys, "platform", platform), mock.patch.object(dev.subprocess, "run") as run:
                with self.assertRaisesRegex(SystemExit, "requires macOS and a safe"):
                    dev.cmd_prod_resume_migration(transaction)
                run.assert_not_called()

    def test_resume_interpreter_failure_retains_owner_without_dispatch(self):
        with self.deployment() as (_, _, options, _, _, _):
            dev.launchd_prod_deploy(controller=options)
            with mock.patch.object(dev.os, "kill", side_effect=ProcessLookupError()), mock.patch.object(dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)) as run:
                with self.assertRaisesRegex(SystemExit, "interpreter cannot run SQLite"):
                    dev.cmd_prod_resume_migration("test-modern")
                self.assertEqual(run.call_count, 1)
            self.assertEqual(dev._deploy_claim_owner(), "test-modern")

    def test_resume_absence_probe_failure_never_starts_foreground_helper(self):
        with self.deployment() as (_, _, options, _, _, _):
            dev.launchd_prod_deploy(controller=options)
            def run(command, **kwargs):
                return subprocess.CompletedProcess(command, 1 if "--probe-service-absence" in command else 0, "", "")
            with mock.patch.object(dev.os, "kill", side_effect=ProcessLookupError()), mock.patch.object(dev.subprocess, "run", side_effect=run) as commands, self.assertRaisesRegex(SystemExit, "absence is unconfirmed"):
                dev.cmd_prod_resume_migration("test-modern")
            self.assertEqual(commands.call_count, 2)
            self.assertEqual(dev._deploy_claim_owner(), "test-modern")

    def test_cli_options_and_resume_dispatch(self):
        patches = [mock.patch.object(dev, name) for name in ("_bootstrap_dev_tracing", "_start_dev_command_tracing")]
        with contextlib.ExitStack() as stack:
            for patch in patches:
                stack.enter_context(patch)
            stack.enter_context(mock.patch.object(dev.sys, "argv", ["dev.py", "prod", "deploy", "--release", "v2.0.0", "--migration-backup-receipt", "/receipt"]))
            deploy = stack.enter_context(mock.patch.object(dev, "cmd_prod_deploy"))
            dev.main()
            self.assertEqual(deploy.call_args.kwargs["controller"].migration_backup_receipt, Path("/receipt"))
        with mock.patch.object(dev, "_bootstrap_dev_tracing"), mock.patch.object(dev, "_start_dev_command_tracing"), \
             mock.patch.object(dev.sys, "argv", ["dev.py", "prod", "resume-migration", "tx"]), mock.patch.object(dev, "cmd_prod_resume_migration") as resume:
            dev.main()
            resume.assert_called_once_with("tx")


if __name__ == "__main__":
    unittest.main()
