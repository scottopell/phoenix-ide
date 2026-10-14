"""Current-schema prepared-artifact installer regressions."""

import importlib.util
import re
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock
import dataclasses
import json
import os
import plistlib
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "current_schema_prepared_helper_test",
    ROOT / "scripts/launchd_deploy_helper.py",
)
helper = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = helper
spec.loader.exec_module(helper)


def compiled_ledger(through: int | None = None) -> list[tuple[int, str]]:
    source = (ROOT / "crates/phoenix-db/src/migrations.rs").read_text()
    registry = source.split("const MIGRATIONS: &[Migration] = &[", 1)[1].split("\n];", 1)[0]
    migrations = [
        (int(version), name)
        for version, name in re.findall(
            r'Migration\s*\{\s*version:\s*(\d+),\s*name:\s*"([^"]+)"',
            registry,
        )
    ]
    return migrations if through is None else [item for item in migrations if item[0] <= through]


class CurrentSchemaPreparedUpgradeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.registry = ROOT / "crates/phoenix-db/src/migrations.rs"

    def database(self, *, through: int = 123) -> Path:
        path = self.root / f"schema-{through}.sqlite3"
        path.unlink(missing_ok=True)
        with sqlite3.connect(path) as connection:
            connection.execute(
                "CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL)"
            )
            connection.executemany(
                "INSERT INTO _migrations (version, name) VALUES (?, ?)",
                compiled_ledger(through),
            )
            connection.execute("CREATE TABLE product_conversations (id TEXT PRIMARY KEY)")
            connection.execute(
                "CREATE TABLE product_conversation_members ("
                "product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id))"
            )
        return path

    def manifest(self, database: Path) -> helper.Manifest:
        transaction = self.root / "transaction"
        transaction.mkdir(mode=0o700, exist_ok=True)
        candidate_binary = self.root / "candidate"
        candidate_binary.write_text("#!/bin/sh\nexit 0\n")
        candidate_binary.chmod(0o755)
        rollback_binary = self.root / "rollback"
        rollback_binary.write_text("old")
        candidate_plist = self.root / "candidate.plist"
        rollback_plist = self.root / "rollback.plist"
        plist = plistlib.dumps(
            {"EnvironmentVariables": {"PHOENIX_DB_PATH": str(database)}}
        )
        candidate_plist.write_bytes(plist)
        rollback_plist.write_bytes(plist)
        registry = transaction / "migrations.rs"
        registry.write_bytes(self.registry.read_bytes())
        copied_helper = transaction / "helper.py"
        copied_helper.write_bytes(Path(helper.__file__).read_bytes())
        current = helper.CurrentSchemaUpgrade(
            database_path=str(database),
            backup_path=str(transaction / "backup.sqlite3"),
            proof_path=str(transaction / "proof.json"),
            controller_source_commit="c" * 40,
            controller_helper_sha256=helper.sha256(copied_helper),
            controller_helper_path=str(copied_helper),
            migration_registry_sha256=helper.sha256(registry),
            migration_registry_path=str(registry),
        )
        return helper.Manifest(
            manifest_version=helper.HANDOFF_PROTOCOL_VERSION,
            transaction_id="current-tx",
            source_kind="prepared_artifact",
            source_commit="b" * 40,
            release_tag=None,
            release_commit=None,
            expected=helper.Identity("2.0.0", "b" * 40),
            previous=helper.Identity("1.0.0", "a" * 40),
            previous_deployed_sha="a" * 40,
            candidate_binary=str(candidate_binary),
            candidate_binary_sha256=helper.sha256(candidate_binary),
            candidate_plist=str(candidate_plist),
            candidate_plist_sha256=helper.sha256(candidate_plist),
            rollback_binary=str(rollback_binary),
            rollback_binary_sha256=helper.sha256(rollback_binary),
            rollback_plist=str(rollback_plist),
            rollback_plist_sha256=helper.sha256(rollback_plist),
            target_binary=str(candidate_binary),
            target_plist=str(self.root / "live.plist"),
            label="test.phoenix",
            helper_label="test.phoenix.deploy",
            uid=os.getuid(),
            health_url="http://127.0.0.1:1/api/version",
            health_insecure_tls=False,
            previous_health_url="http://127.0.0.1:2/api/version",
            previous_health_insecure_tls=False,
            previous_health_json=True,
            active_path=str(self.root / "active"),
            status_path=str(self.root / "status.json"),
            deployed_sha_path=str(self.root / "deployed.sha"),
            lock_path=str(self.root / "lock"),
            claim_lock_path=str(self.root / "claim.lock"),
            created_at="2026-01-01T00:00:00+00:00",
            current_schema_upgrade=current,
        )

    def ordinary_manifest(self, database: Path) -> helper.Manifest:
        base = self.manifest(database)
        current = base.current_schema_upgrade
        assert current is not None
        transaction = Path(current.migration_registry_path).parent
        candidate = transaction / "candidate"
        candidate.write_text("new candidate")
        candidate.chmod(0o755)
        candidate_plist = transaction / "candidate.plist"
        candidate_plist.write_bytes(Path(base.candidate_plist).read_bytes())
        live_binary = self.root / "live-binary"
        live_binary.write_text("old predecessor")
        live_binary.chmod(0o755)
        live_plist = self.root / "live.plist"
        live_plist.write_bytes(Path(base.rollback_plist).read_bytes())
        ordinary = helper.OrdinaryForwardMigration(
            database_path=str(database),
            helper_sha256=current.controller_helper_sha256,
            helper_path=current.controller_helper_path,
            migration_registry_sha256=current.migration_registry_sha256,
            migration_registry_path=current.migration_registry_path,
        )
        return dataclasses.replace(
            base,
            source_kind="local_head",
            candidate_binary=str(candidate),
            candidate_binary_sha256=helper.sha256(candidate),
            candidate_plist=str(candidate_plist),
            candidate_plist_sha256=helper.sha256(candidate_plist),
            rollback_binary=str(live_binary),
            rollback_binary_sha256=helper.sha256(live_binary),
            rollback_plist=str(live_plist),
            rollback_plist_sha256=helper.sha256(live_plist),
            target_binary=str(live_binary),
            target_plist=str(live_plist),
            current_schema_upgrade=None,
            ordinary_forward_migration=ordinary,
        )

    def write_snapshot_proof(self, manifest: helper.Manifest) -> None:
        current = manifest.current_schema_upgrade
        assert current is not None
        database = Path(current.database_path)
        reservation = helper.DatabaseCapacityReservation(
            Path(current.backup_path), self.root / "restore.sqlite3", database.stat().st_size
        )
        Path(current.backup_path).touch(mode=0o600)
        with mock.patch.object(helper, "assert_database_exclusive"):
            helper.create_database_backup(manifest, reservation)

    def test_schema_123_is_admitted_only_by_explicit_current_schema_gate(self) -> None:
        database = self.database()

        with self.assertRaisesRegex(helper.ActivationError, "paired upgrade requires"):
            helper.validate_legacy_database(database)

        ledger = helper.validate_current_schema_database(database, self.registry)
        self.assertEqual(ledger[-1], compiled_ledger(123)[-1])
        self.assertEqual(ledger, tuple(compiled_ledger(123)))

    def test_unknown_newer_and_name_mismatched_ledgers_fail_closed(self) -> None:
        newer = self.database(through=124)
        with self.assertRaisesRegex(helper.ActivationError, "supported current baseline"):
            helper.validate_current_schema_database(newer, self.registry)

        mismatch = self.database()
        with sqlite3.connect(mismatch) as connection:
            connection.execute(
                "UPDATE _migrations SET name = 'forged' WHERE version = 123"
            )
        with self.assertRaisesRegex(helper.ActivationError, "supported current baseline"):
            helper.validate_current_schema_database(mismatch, self.registry)

    def test_foreign_key_violation_fails_before_migration(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.execute("PRAGMA foreign_keys = OFF")
            connection.execute(
                "INSERT INTO product_conversation_members (product_conversation_id) VALUES ('missing')"
            )
        with self.assertRaisesRegex(helper.ActivationError, "foreign key"):
            helper.validate_current_schema_database(database, self.registry)

    def migration_script(self, manifest: helper.Manifest, *, partial: bool = False) -> None:
        target = Path(manifest.target_binary)
        pending = [item for item in compiled_ledger() if item[0] > 123]
        migrations = pending[:1] if partial else pending
        script = f"""#!/usr/bin/env python3
import os, sqlite3, sys
connection = sqlite3.connect(os.environ['PHOENIX_DB_PATH'])
connection.executemany('INSERT INTO _migrations (version, name) VALUES (?, ?)', {migrations!r})
connection.commit()
connection.close()
sys.exit({7 if partial else 0})
"""
        target.write_text(script)
        target.chmod(0o755)
        object.__setattr__(manifest, "candidate_binary_sha256", helper.sha256(target))

    def test_offline_migration_commits_exact_target_ledger_and_proof(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.migration_script(manifest)
        self.write_snapshot_proof(manifest)

        with mock.patch.object(helper, "assert_database_exclusive"):
            helper.run_current_schema_migration(manifest)

        ledger = helper.validate_current_schema_database(
            database,
            Path(manifest.current_schema_upgrade.migration_registry_path),
            migrated=True,
        )
        proof = json.loads(Path(manifest.current_schema_upgrade.proof_path).read_text())
        self.assertEqual(proof["migrated_ledger"], [list(item) for item in ledger])
        self.assertIn("migration_committed_at", proof)

    def test_failed_offline_migration_allows_rollback_only_when_unchanged(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        Path(manifest.target_binary).write_text("#!/bin/sh\nexit 9\n")
        Path(manifest.target_binary).chmod(0o755)
        object.__setattr__(
            manifest, "candidate_binary_sha256", helper.sha256(Path(manifest.target_binary))
        )
        self.write_snapshot_proof(manifest)

        with mock.patch.object(helper, "assert_database_exclusive"):
            with self.assertRaisesRegex(helper.ActivationError, "exit 9"):
                helper.run_current_schema_migration(manifest)
        self.assertTrue(helper.current_schema_database_is_uncommitted(manifest))

        partial_database = self.database()
        partial_manifest = self.manifest(partial_database)
        self.migration_script(partial_manifest, partial=True)
        self.write_snapshot_proof(partial_manifest)
        with mock.patch.object(helper, "assert_database_exclusive"):
            with self.assertRaisesRegex(helper.ActivationError, "exit 7"):
                helper.run_current_schema_migration(partial_manifest)
        self.assertFalse(helper.current_schema_database_is_uncommitted(partial_manifest))

    def test_snapshot_binds_integrity_ledger_hash_and_logical_contents(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        current = manifest.current_schema_upgrade
        reservation = helper.DatabaseCapacityReservation(
            Path(current.backup_path), self.root / "restore.sqlite3", database.stat().st_size
        )
        Path(current.backup_path).touch(mode=0o600)

        with mock.patch.object(helper, "assert_database_exclusive"):
            helper.create_database_backup(manifest, reservation)

        proof = json.loads(Path(current.proof_path).read_text())
        self.assertEqual(proof["database_mode"], "current_schema")
        self.assertEqual(proof["source_ledger"], [list(item) for item in compiled_ledger(123)])
        self.assertEqual(proof["target_ledger"], [list(item) for item in compiled_ledger()])
        self.assertEqual(proof["source_logical_digest"], proof["backup_logical_digest"])
        self.assertEqual(proof["sha256"], helper.sha256(Path(current.backup_path)))

    def test_ordinary_nonqualified_schema_does_not_impose_127_ceiling(self) -> None:
        database = self.database(through=122)
        manifest = self.ordinary_manifest(database)
        ordinary = manifest.ordinary_forward_migration
        registry = Path(ordinary.migration_registry_path)
        source = registry.read_text()
        source = source.replace(
            "\n];",
            '\n    Migration { version: 128, name: "future", sql: "SELECT 1" },\n];',
            1,
        )
        registry.write_text(source)
        object.__setattr__(
            ordinary, "migration_registry_sha256", helper.sha256(registry)
        )

        helper.validate_manifest_mode(manifest)
        self.assertFalse(helper.ordinary_forward_policy_applies(manifest))

    def test_current_finalization_verifies_commit_proof_before_candidate_start(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(manifest, "committed", finalization_pending=True)
        Path(manifest.current_schema_upgrade.proof_path).unlink(missing_ok=True)

        class NoStartBeforeProof:
            def __init__(self) -> None:
                self.started = 0

            def inspect(self):
                return "not_loaded", None

            def start(self, old_pid, plist_path=None) -> None:
                self.started += 1

        launchctl = NoStartBeforeProof()
        absence = subprocess.CompletedProcess([], 1, "", "service not found")
        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper.subprocess, "run", return_value=absence), mock.patch.object(
            helper, "service_absence_confirmed", return_value=True
        ), mock.patch.object(helper, "Launchctl", return_value=launchctl):
            with self.assertRaisesRegex(helper.ActivationError, "proof"):
                helper.finalize_paired(manifest, current_schema=True)
        self.assertEqual(launchctl.started, 0)

    def test_current_finalization_rejects_mismatched_proof_context(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.write_snapshot_proof(manifest)
        proof_path = Path(manifest.current_schema_upgrade.proof_path)
        proof = json.loads(proof_path.read_text())
        proof["source_commit"] = "0" * 40
        proof_path.write_text(json.dumps(proof))
        proof_path.chmod(0o600)

        with self.assertRaisesRegex(helper.ActivationError, "context mismatch"):
            helper._snapshot_proof(manifest)

    def test_unreadable_ordinary_ledger_fails_before_candidate_start(self) -> None:
        database = self.database()
        manifest = self.ordinary_manifest(database)
        with mock.patch.object(
            helper,
            "database_ledger",
            side_effect=helper.ActivationError("ledger unreadable"),
        ), mock.patch.object(helper, "Launchctl") as launchctl:
            with self.assertRaisesRegex(helper.ActivationError, "ledger unreadable"):
                helper.activate(manifest)

        launchctl.assert_not_called()
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")

    def test_current_finalization_requires_exclusivity_before_start(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(manifest, "committed", finalization_pending=True)

        class StoppedCandidate:
            def __init__(self) -> None:
                self.started = 0

            def inspect(self):
                return "not_loaded", None

            def start(self, old_pid, plist_path=None) -> None:
                self.started += 1

        launchctl = StoppedCandidate()
        absence = subprocess.CompletedProcess([], 1, "", "service not found")
        proof = {"migration_committed_at": "2026-01-01T00:00:00+00:00"}
        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper.subprocess, "run", return_value=absence), mock.patch.object(
            helper, "service_absence_confirmed", return_value=True
        ), mock.patch.object(helper, "_snapshot_proof", return_value=proof), mock.patch.object(
            helper, "validate_current_schema_database"
        ), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(
            helper,
            "assert_database_exclusive",
            side_effect=helper.ActivationError("database owner remains"),
        ):
            with self.assertRaisesRegex(helper.ActivationError, "database owner remains"):
                helper.finalize_paired(manifest, current_schema=True)

        self.assertEqual(launchctl.started, 0)

    def test_ordinary_forward_failure_never_launches_incompatible_predecessor(self) -> None:
        database = self.database()
        manifest = self.ordinary_manifest(database)

        class FailingCandidateLaunch:
            def __init__(self) -> None:
                self.start_calls = 0
                self.stop_calls = 0

            def stop(self) -> int | None:
                self.stop_calls += 1
                return 41

            def start(self, old_pid, plist_path=None) -> None:
                self.start_calls += 1
                raise helper.ActivationError("candidate failed after it may have migrated")

            def inspect(self):
                return "not_loaded", None

        launchctl = FailingCandidateLaunch()
        with mock.patch.object(helper, "Launchctl", return_value=launchctl):
            state = helper.activate(manifest)

        self.assertEqual(state, "ordinary_activation_failed_forward_only")
        self.assertEqual(launchctl.start_calls, 1)
        self.assertGreaterEqual(launchctl.stop_calls, 2)
        self.assertEqual(
            helper.sha256(Path(manifest.target_binary)),
            manifest.candidate_binary_sha256,
        )
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertTrue(status["finalization_pending"])
        self.assertIn("predecessor launch is forbidden", status["committed_diagnostic"])

    def test_ordinary_failure_after_126_commit_retains_candidate_and_database(self) -> None:
        database = self.database()
        manifest = self.ordinary_manifest(database)
        pending = [item for item in compiled_ledger() if 123 < item[0] <= 126]

        class FailsBefore127:
            def __init__(self) -> None:
                self.start_calls = 0

            def stop(self) -> int | None:
                return 51

            def start(self, old_pid, plist_path=None) -> None:
                self.start_calls += 1
                with sqlite3.connect(database) as connection:
                    connection.executemany(
                        "INSERT INTO _migrations (version, name) VALUES (?, ?)", pending
                    )
                raise helper.ActivationError("failed before migration 127")

            def inspect(self):
                return "not_loaded", None

        launchctl = FailsBefore127()
        with mock.patch.object(helper, "Launchctl", return_value=launchctl):
            state = helper.activate(manifest)

        self.assertEqual(state, "ordinary_activation_failed_forward_only")
        self.assertEqual(helper.database_ledger(database)[-1][0], 126)
        self.assertEqual(launchctl.start_calls, 1)
        self.assertEqual(
            helper.sha256(Path(manifest.target_binary)), manifest.candidate_binary_sha256
        )

    def test_ordinary_identity_failure_after_home_commit_preserves_forward_state(self) -> None:
        database = self.database()
        manifest = self.ordinary_manifest(database)
        pending = [item for item in compiled_ledger() if item[0] > 123]

        class CommitsHomeBeforeIdentityFailure:
            def __init__(self) -> None:
                self.start_calls = 0

            def stop(self) -> int | None:
                return 52

            def start(self, old_pid, plist_path=None) -> None:
                self.start_calls += 1
                with sqlite3.connect(database) as connection:
                    connection.executemany(
                        "INSERT INTO _migrations (version, name) VALUES (?, ?)", pending
                    )
                    connection.execute(
                        "CREATE TABLE coordinator_home_test (environment_kind TEXT NOT NULL)"
                    )
                    connection.execute(
                        "INSERT INTO coordinator_home_test VALUES ('coordinator_home')"
                    )

            def inspect(self):
                return "not_loaded", None

        launchctl = CommitsHomeBeforeIdentityFailure()
        with mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(
            helper,
            "wait_for_identity",
            side_effect=helper.ActivationError("identity health failed"),
        ):
            state = helper.activate(manifest)

        self.assertEqual(state, "ordinary_activation_failed_forward_only")
        self.assertEqual(helper.database_ledger(database)[-1][0], 127)
        with sqlite3.connect(database) as connection:
            self.assertEqual(
                connection.execute(
                    "SELECT environment_kind FROM coordinator_home_test"
                ).fetchone(),
                ("coordinator_home",),
            )
        self.assertEqual(launchctl.start_calls, 1)

    def test_repeated_recover_current_preserves_migration_started_fence(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.write_snapshot_proof(manifest)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(
            manifest,
            "migration_started",
            finalization_pending=True,
            committed_diagnostic="offline migration invoked",
        )
        original_status = Path(manifest.status_path).read_text()
        argv = [
            "launchd_deploy_helper.py",
            "recover-current",
            "--manifest",
            str(self.root / "manifest.json"),
            "--helper-label",
            manifest.helper_label,
            "--uid",
            str(manifest.uid),
        ]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper, "restore") as restore, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)
            self.assertEqual(helper.main(), 1)

        restore.assert_not_called()
        self.assertEqual(Path(manifest.status_path).read_text(), original_status)
        self.assertEqual(Path(manifest.active_path).read_text(), manifest.transaction_id)

    def test_current_recovery_rechecks_unchanged_source_after_teardown(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.write_snapshot_proof(manifest)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(manifest, "activation_failed_rollback_failed", failure="crash")

        class WriterDuringTeardown:
            def __init__(self) -> None:
                self.started = 0

            def stop(self) -> int | None:
                with sqlite3.connect(database) as connection:
                    connection.execute(
                        "INSERT INTO _migrations (version, name) VALUES (?, ?)",
                        compiled_ledger(124)[-1],
                    )
                return None

            def inspect(self):
                return "not_loaded", None

            def start(self, old_pid, plist_path=None) -> None:
                self.started += 1

        launchctl = WriterDuringTeardown()
        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(
            helper, "assert_database_exclusive"
        ):
            state = helper.recover_paired(manifest, current_schema=True)

        self.assertEqual(state, "activation_failed_rollback_failed")
        self.assertIn(
            "changed before exclusive snapshot replay",
            json.loads(Path(manifest.status_path).read_text())["rollback_failure"],
        )
        self.assertEqual(helper.database_ledger(database)[-1][0], 124)
        self.assertEqual(launchctl.started, 0)

    def test_concurrent_forward_resume_preserves_status_and_claim(self) -> None:
        database = self.database()
        manifest = self.ordinary_manifest(database)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(
            manifest,
            "ordinary_activation_failed_forward_only",
            finalization_pending=True,
        )
        original_status = Path(manifest.status_path).read_text()
        argv = [
            "launchd_deploy_helper.py",
            "resume-forward",
            "--manifest",
            str(self.root / "manifest.json"),
            "--helper-label",
            manifest.helper_label,
            "--uid",
            str(manifest.uid),
        ]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(
            helper, "resume_forward", side_effect=helper.ConcurrentDeploy("busy")
        ), mock.patch.object(helper, "request_helper_bootout"):
            self.assertEqual(helper.main(), 1)

        self.assertEqual(Path(manifest.status_path).read_text(), original_status)
        self.assertEqual(Path(manifest.active_path).read_text(), manifest.transaction_id)

    def test_forward_recovery_can_start_only_retained_candidate(self) -> None:
        database = self.database()
        manifest = self.ordinary_manifest(database)
        Path(manifest.target_binary).write_bytes(Path(manifest.candidate_binary).read_bytes())
        Path(manifest.target_binary).chmod(0o755)
        Path(manifest.target_plist).write_bytes(Path(manifest.candidate_plist).read_bytes())
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(
            manifest,
            "ordinary_activation_failed_forward_only",
            finalization_pending=True,
        )

        class ForwardLaunch:
            def __init__(self) -> None:
                self.started = 0

            def inspect(self):
                return "not_loaded", None

            def start(self, old_pid, plist_path=None) -> None:
                self.started += 1

            def stop(self) -> int | None:
                return None

        launchctl = ForwardLaunch()
        absence = subprocess.CompletedProcess([], 1, "", "service not found")
        with mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(
            helper, "wait_for_identity"
        ), mock.patch.object(helper.subprocess, "run", return_value=absence), mock.patch.object(
            helper, "service_absence_confirmed", return_value=True
        ):
            state = helper.resume_forward(manifest)

        self.assertEqual(state, "committed")
        self.assertEqual(launchctl.started, 1)
        self.assertEqual(
            Path(manifest.deployed_sha_path).read_text().strip(), manifest.source_commit
        )

    def test_capacity_requires_twice_actual_database_and_20_gib_floor(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        allocations = []

        def allocate(path: Path, size: int) -> None:
            allocations.append((path, size))
            path.touch(mode=0o600)

        with mock.patch.object(helper, "_allocate_private_sqlite", side_effect=allocate), mock.patch.object(
            helper.shutil,
            "disk_usage",
            return_value=shutil._ntuple_diskusage(100, 80, helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES - 1),
        ):
            with self.assertRaisesRegex(helper.ActivationError, "20 GiB"):
                helper.reserve_database_capacity(manifest)
        self.assertEqual(len(allocations), 2)
        self.assertEqual(allocations[0][1], allocations[1][1])
        self.assertGreaterEqual(allocations[0][1], database.stat().st_size)


if __name__ == "__main__":
    unittest.main()
