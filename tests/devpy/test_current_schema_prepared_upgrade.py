"""Current-schema prepared-artifact installer regressions."""

import dataclasses
import contextlib
import importlib.util
import itertools
import json
import os
import plistlib
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


PRODUCT_CONVERSATION_SCHEMA = """
CREATE TABLE product_conversations (
    id TEXT PRIMARY KEY
);
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    product_conversation_id TEXT REFERENCES product_conversations(id) ON DELETE CASCADE
);
CREATE TABLE product_conversation_sources (
    target_product_conversation_id TEXT PRIMARY KEY
        REFERENCES product_conversations(id) ON DELETE CASCADE,
    source_product_conversation_id TEXT NOT NULL,
    source_conversation_id TEXT NOT NULL,
    relation_kind TEXT NOT NULL,
    relation_key TEXT NOT NULL,
    created_at_us INTEGER NOT NULL
);
"""

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "current_schema_prepared_helper_test",
    ROOT / "scripts/launchd_deploy_helper.py",
)
helper = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = helper
spec.loader.exec_module(helper)


QUALIFIED_TARGET = 128


def compiled_ledger(through: int | None = QUALIFIED_TARGET) -> list[tuple[int, str]]:
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
        self.current_registry = ROOT / "crates/phoenix-db/src/migrations.rs"
        source = self.current_registry.read_text()
        registry_match = re.search(
            r"(const MIGRATIONS: &\[Migration\] = &\[)(.*?)(\n\];)",
            source,
            re.DOTALL,
        )
        self.assertIsNotNone(registry_match)
        entries = registry_match.group(2)
        suffix_boundaries = list(re.finditer(
            rf"^    Migration\s*\{{\s*version:\s*{QUALIFIED_TARGET + 1},",
            entries,
            flags=re.MULTILINE,
        ))
        self.assertEqual(
            len(suffix_boundaries), 1,
            "historical registry fixture requires exactly one successor boundary",
        )
        historical_entries = entries[:suffix_boundaries[0].start()]
        self.registry = self.root / "qualified-128-migrations.rs"
        self.registry.write_text(
            source[:registry_match.start(2)] + historical_entries + source[registry_match.end(2):]
        )
        # Synthetic unit fixture: this registry models the historical v128
        # candidate independently of the exact source-114 release artifact.
        self.prepared_registry_pin = helper.sha256(self.registry)
        registry_pin = mock.patch.object(
            helper, "PREPARED_SOURCE114_REGISTRY_SHA256", self.prepared_registry_pin,
        )
        registry_pin.start()
        self.addCleanup(registry_pin.stop)
        repair_pin = mock.patch.object(
            helper, "CURRENT_REPAIR_REGISTRY_SHA256", self.prepared_registry_pin,
        )
        repair_pin.start()
        self.addCleanup(repair_pin.stop)
        capacity = shutil.disk_usage(self.root)._replace(free=100 * 1024**3)
        disk_usage = mock.patch.object(helper.shutil, "disk_usage", return_value=capacity)
        disk_usage.start()
        self.addCleanup(disk_usage.stop)

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
            connection.executescript(PRODUCT_CONVERSATION_SCHEMA)
        return path

    def manifest(self, database: Path) -> helper.Manifest:
        transaction = self.root / "transaction"
        transaction.mkdir(mode=0o700, exist_ok=True)
        candidate_binary = transaction / "candidate"
        candidate_binary.write_text("#!/bin/sh\nexit 0\n")
        candidate_binary.chmod(0o755)
        rollback_binary = transaction / "rollback"
        rollback_binary.write_text("old")
        candidate_plist = transaction / "candidate.plist"
        rollback_plist = transaction / "rollback.plist"
        plist = plistlib.dumps(
            {
                "Label": "test.phoenix",
                "ProgramArguments": [str(candidate_binary)],
                "EnvironmentVariables": {"PHOENIX_DB_PATH": str(database)},
            }
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
            helper_label="com.phoenix-ide.deploy.current-tx",
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

    def helper_main_argv(self, manifest: helper.Manifest) -> list[str]:
        manifest_path = self.root / "manifest.json"
        if not os.path.lexists(manifest_path):
            manifest_path.write_text(json.dumps(dataclasses.asdict(manifest)))
        return [
            "launchd_deploy_helper.py",
            "activate",
            "--manifest",
            str(manifest_path),
            "--helper-label",
            manifest.helper_label,
            "--uid",
            str(manifest.uid),
        ]

    def write_current_status(
        self, manifest: helper.Manifest, state: str, **fields
    ) -> None:
        Path(manifest.status_path).write_text(
            json.dumps(
                {
                    "transaction_id": manifest.transaction_id,
                    "prepared_mode": "current_schema",
                }
            )
        )
        helper.write_status(manifest, state, **fields)

    def test_helper_status_preserves_initial_prepared_mode(self) -> None:
        manifest = self.manifest(self.database())

        self.write_current_status(manifest, "activating")

        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["prepared_mode"], "current_schema")

    def release_manifest(self, database: Path) -> helper.Manifest:
        base = self.manifest(database)
        return dataclasses.replace(
            base, source_kind="published_release", release_tag="v2.0.0",
            release_commit=base.source_commit,
        )

    def test_source_qualified_prefixes_are_exact_and_distinct(self) -> None:
        for source_kind in ("prepared_artifact", "published_release"):
            for through in range(113, 129):
                with self.subTest(source_kind=source_kind, through=through):
                    database = self.database(through=through)
                    admitted = (123, 128) if source_kind == "prepared_artifact" else (114,)
                    if through in admitted:
                        self.assertEqual(
                            helper.validate_current_schema_database(
                                database, self.registry, source_kind=source_kind,
                            ), tuple(compiled_ledger(through)),
                        )
                    else:
                        if through == 129:
                            with sqlite3.connect(database) as connection:
                                connection.execute("INSERT INTO _migrations VALUES (129, 'future')")
                        with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
                            helper.validate_current_schema_database(
                                database, self.registry, source_kind=source_kind,
                            )

    def prepared114_manifest(self, database: Path) -> helper.Manifest:
        base = self.manifest(database)
        current = base.current_schema_upgrade
        registry = Path(current.migration_registry_path)
        # Use the synthetic historical-v128 registry for this unit fixture;
        # the real source-114 artifact is covered by separate integration tests.
        registry.write_bytes(self.registry.read_bytes())
        qualified = helper.PreparedSource114Upgrade(
            **{**dataclasses.asdict(current), "migration_registry_sha256": helper.sha256(registry)},
            prepared_source_baseline=114,
            candidate_registry_expected_sha256=self.prepared_registry_pin,
        )
        return dataclasses.replace(base, source_commit=helper.PREPARED_SOURCE114_COMMIT,
                                   expected=helper.Identity("2.0.0", helper.PREPARED_SOURCE114_COMMIT),
                                   current_schema_upgrade=qualified)

    def test_exact_prepared114_read_only_handoff_uses_real_mode_without_sqlite(self) -> None:
        manifest = self.prepared114_manifest(self.database(through=114))
        before = {
            path.relative_to(self.root): path.read_bytes()
            for path in self.root.rglob("*")
            if path.is_file()
        }
        with mock.patch.object(
            helper,
            "__file__",
            manifest.current_schema_upgrade.controller_helper_path,
        ), mock.patch.object(
            helper,
            "_readonly_connection",
            side_effect=AssertionError("SQLite opened by read-only handoff"),
        ):
            validated = helper.validate_pre_disruption_handoff(manifest)

        self.assertEqual(validated.candidate_binary, Path(manifest.candidate_binary))
        self.assertEqual(validated.candidate_plist, Path(manifest.candidate_plist))
        self.assertEqual(
            before,
            {
                path.relative_to(self.root): path.read_bytes()
                for path in self.root.rglob("*")
                if path.is_file()
            },
        )
        self.assertFalse(Path(manifest.status_path).exists())
        self.assertFalse(Path(manifest.active_path).exists())

    def test_exact_prepared114_and_target_admission_requires_structural_evidence(self) -> None:
        for through in range(113, 129):
            with self.subTest(through=through):
                database = self.database(through=through)
                manifest = self.prepared114_manifest(database)
                registry = Path(manifest.current_schema_upgrade.migration_registry_path)
                if through in (114, 128):
                    self.assertEqual(helper.validate_current_schema_database(database, registry, manifest=manifest),
                                     tuple(compiled_ledger(through)))
                else:
                    with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
                        helper.validate_current_schema_database(database, registry, manifest=manifest)
                if through == 114:
                    with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
                        helper.validate_current_schema_database(database, registry)

    def test_actual_current129_registry_is_rejected_as_prepared114(self) -> None:
        database = self.database(through=129)
        actual_registry = ROOT / "crates/phoenix-db/src/migrations.rs"
        with self.assertRaises(helper.ActivationError):
            helper.validate_current_schema_database(
                database, actual_registry, source_kind="prepared_artifact",
            )

    def test_prepared114_invalid_binding_rejects_before_stop(self) -> None:
        manifest = self.prepared114_manifest(self.database(through=114))
        current = manifest.current_schema_upgrade
        plain = helper.CurrentSchemaUpgrade(**{
            field.name: getattr(current, field.name) for field in dataclasses.fields(helper.CurrentSchemaUpgrade)
        })
        mutations = [
            dataclasses.replace(manifest, source_commit="e" * 40),
            dataclasses.replace(manifest, expected=helper.Identity("2.0.0", "e" * 40)),
            dataclasses.replace(manifest, source_kind="published_release"),
            dataclasses.replace(manifest, source_kind="local_head"),
            dataclasses.replace(manifest, current_schema_upgrade=plain),
        ]
        mutations += [dataclasses.replace(manifest, current_schema_upgrade=dataclasses.replace(current, **fields))
                      for fields in (
                          {"controller_source_commit": manifest.source_commit},
                          {"prepared_source_baseline": 113},
                          {"prepared_source_baseline": True},
                          {"candidate_registry_expected_sha256": "0" * 64},
                          {"migration_registry_sha256": "0" * 64},
                      )]
        self.write_current_status(manifest, "prepared")
        for changed in mutations:
            with self.subTest(changed=changed), \
                 mock.patch.object(helper, "__file__", current.controller_helper_path), \
                 mock.patch.object(helper, "Launchctl") as launchctl, \
                 mock.patch.object(helper, "reserve_database_capacity", side_effect=AssertionError("capacity before binding")):
                self.write_current_status(changed, "prepared")
                with self.assertRaises(helper.ActivationError):
                    helper.activate(changed)
                launchctl.return_value.stop.assert_not_called()
        registry = Path(current.migration_registry_path)
        registry.write_bytes(registry.read_bytes() + b"\n// altered bytes\n")
        with mock.patch.object(helper, "__file__", current.controller_helper_path), \
             mock.patch.object(helper, "Launchctl") as launchctl:
            self.write_current_status(manifest, "prepared")
            with self.assertRaises(helper.ActivationError):
                helper.activate(manifest)
            launchctl.return_value.stop.assert_not_called()

    def test_prepared114_other_source_prefixes_reject_before_stop(self) -> None:
        for through in (113, *range(115, 128)):
            with self.subTest(through=through):
                manifest = self.prepared114_manifest(self.database(through=through))
                self.write_current_status(manifest, "prepared")
                with mock.patch.object(helper, "__file__", manifest.current_schema_upgrade.controller_helper_path), \
                     mock.patch.object(helper, "Launchctl") as launchctl, \
                     mock.patch.object(helper, "reserve_database_capacity") as reserve:
                    with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
                        helper.activate(manifest)
                    launchctl.assert_not_called()
                    reserve.assert_not_called()

    def test_prepared114_manifest_fields_are_all_required_typed_and_closed(self) -> None:
        manifest = self.prepared114_manifest(self.database(through=114))
        path = self.root / "qualified-manifest.json"
        payload = dataclasses.asdict(manifest)
        path.write_text(json.dumps(payload))
        loaded = helper.Manifest.load(path)
        self.assertIsInstance(loaded.current_schema_upgrade, helper.PreparedSource114Upgrade)
        for key in ("prepared_source_baseline", "candidate_registry_expected_sha256"):
            changed = json.loads(json.dumps(payload))
            del changed["current_schema_upgrade"][key]
            path.write_text(json.dumps(changed))
            with self.assertRaises(helper.ActivationError):
                helper.Manifest.load(path)
        for fields in ({"extra_qualification": True}, {"prepared_source_baseline": "114"},
                       {"prepared_source_baseline": None}, {"candidate_registry_expected_sha256": None}):
            changed = json.loads(json.dumps(payload))
            changed["current_schema_upgrade"].update(fields)
            path.write_text(json.dumps(changed))
            with self.assertRaises(helper.ActivationError):
                helper.Manifest.load(path)

    def test_prepared114_recovery_rejects_altered_candidate_or_qualification_before_start(self) -> None:
        manifest = self.prepared114_manifest(self.database(through=114))
        self.write_snapshot_proof(manifest)
        current = manifest.current_schema_upgrade
        Path(manifest.active_path).write_text(manifest.transaction_id)
        self.write_current_status(manifest, "activation_failed_rollback_failed")
        for changed in (
            dataclasses.replace(manifest, source_commit="e" * 40, expected=helper.Identity("2.0.0", "e" * 40)),
            dataclasses.replace(manifest, current_schema_upgrade=dataclasses.replace(current, candidate_registry_expected_sha256="0" * 64)),
            dataclasses.replace(manifest, current_schema_upgrade=dataclasses.replace(current, controller_source_commit=manifest.source_commit)),
        ):
            with self.subTest(changed=changed), \
                 mock.patch.object(helper, "__file__", current.controller_helper_path), \
                 mock.patch.object(helper, "Launchctl") as launchctl:
                with self.assertRaises(helper.ActivationError):
                    helper.recover_paired(changed, current_schema=True)
                launchctl.assert_not_called()
                self.assertEqual(Path(manifest.active_path).read_text(), manifest.transaction_id)

    def test_prepared114_retained_qualification_tamper_rejected_by_snapshot_proof(self) -> None:
        manifest = self.prepared114_manifest(self.database(through=128))
        self.write_snapshot_proof(manifest)
        current = manifest.current_schema_upgrade
        proof_path = Path(current.proof_path)
        original = proof_path.read_bytes()
        for evidence in (None, {"prepared_source_baseline": 115},
                         {"prepared_source_baseline": 114, "candidate_registry_expected_sha256": "0" * 64}):
            proof = json.loads(original)
            proof["prepared_source_qualification"] = evidence
            proof_path.write_text(json.dumps(proof))
            with self.assertRaisesRegex(helper.ActivationError, "proof context mismatch"):
                helper._snapshot_proof(manifest)
        proof_path.write_bytes(original)
        plain = helper.CurrentSchemaUpgrade(**{
            field.name: getattr(current, field.name) for field in dataclasses.fields(helper.CurrentSchemaUpgrade)
        })
        with self.assertRaisesRegex(helper.ActivationError, "proof context mismatch"):
            helper._snapshot_proof(dataclasses.replace(manifest, current_schema_upgrade=plain))
        Path(current.migration_registry_path).write_text("tampered registry")
        with self.assertRaises(helper.ActivationError):
            helper._snapshot_proof(manifest)

    def test_release_114_rejects_incomplete_mismatched_and_extra_ledger(self) -> None:
        for mutation in (
            "DELETE FROM _migrations WHERE version = 70",
            "UPDATE _migrations SET name = 'wrong' WHERE version = 114",
            "INSERT INTO _migrations VALUES (115, 'unexpected')",
        ):
            with self.subTest(mutation=mutation):
                database = self.database(through=114)
                with sqlite3.connect(database) as connection:
                    connection.execute(mutation)
                with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
                    helper.validate_current_schema_database(
                        database, self.registry, source_kind="published_release",
                    )

    def test_release_mode_requires_exact_candidate_binding_and_retained_helper(self) -> None:
        manifest = self.release_manifest(self.database(through=114))
        current = manifest.current_schema_upgrade
        with mock.patch.object(helper, "__file__", current.controller_helper_path):
            helper.validate_manifest_mode(manifest)
            for fields in (
                {"release_commit": "e" * 40}, {"release_tag": None},
                {"expected": helper.Identity("2.0.0", "e" * 40)},
                {"source_kind": "local_head"},
            ):
                with self.subTest(fields=fields), self.assertRaises(helper.ActivationError):
                    helper.validate_manifest_mode(dataclasses.replace(manifest, **fields))
            same_source = dataclasses.replace(
                manifest,
                current_schema_upgrade=dataclasses.replace(
                    current, controller_source_commit=manifest.source_commit,
                ),
            )
            with self.assertRaisesRegex(helper.ActivationError, "exact release candidate source binding"):
                helper.validate_manifest_mode(same_source)
            Path(current.controller_helper_path).write_text("changed helper")
            with self.assertRaisesRegex(helper.ActivationError, "helper bytes changed"):
                helper.validate_manifest_mode(manifest)

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

    def migration_reservation(
        self,
        manifest: helper.Manifest,
        *,
        capacity_bytes: int | None = None,
    ) -> helper.DatabaseCapacityReservation:
        current = manifest.current_schema_upgrade
        assert current is not None
        database = Path(current.database_path)
        observed = helper._source_capacity_bytes(database)
        capacity = (
            capacity_bytes
            if capacity_bytes is not None
            else observed + 1024 * 1024 * 1024
        )
        reservation = helper.DatabaseCapacityReservation(
            Path(current.backup_path),
            helper._restore_capacity_path(manifest, database),
            capacity,
            ceiling_bytes=capacity,
            observed_source_bytes=observed,
            measured_at=helper.utc_now(),
        )
        helper._write_capacity_observation(manifest, reservation)
        return reservation

    def test_schema_123_is_admitted_only_by_explicit_current_schema_gate(self) -> None:
        database = self.database()

        with self.assertRaisesRegex(helper.ActivationError, "paired upgrade requires"):
            helper.validate_legacy_database(database)

        ledger = helper.validate_current_schema_database(database, self.registry)
        self.assertEqual(ledger[-1], compiled_ledger(123)[-1])
        self.assertEqual(ledger, tuple(compiled_ledger(123)))

    def test_actual_current_129_registry_is_rejected_by_qualified_helper(self) -> None:
        database = self.database(through=129)
        current_registry = self.current_registry

        with self.assertRaisesRegex(helper.ActivationError, "newer than the qualified target"):
            helper.migration_registry(current_registry)
        with self.assertRaisesRegex(helper.ActivationError, "newer than the qualified target"):
            helper.validate_current_schema_database(database, current_registry)

    def test_exact_candidate_target_is_also_an_admitted_prepared_source(self) -> None:
        database = self.database(through=128)

        ledger = helper.validate_current_schema_database(database, self.registry)

        self.assertEqual(ledger, tuple(compiled_ledger()))

    def test_actual_registry135_named_sql_parses_without_qualifying_protected_target(self) -> None:
        actual_registry = ROOT / "crates/phoenix-db/src/migrations.rs"
        ledger = helper.migration_registry(actual_registry, qualified_current_target=False)
        self.assertIn((134, "durable_conversation_goal_user_control"), ledger)
        self.assertEqual(ledger[-1], (135, "local_watch_goal_capture"))
        self.assertEqual(ledger, tuple(compiled_ledger(through=None)))
        with self.assertRaisesRegex(helper.ActivationError, "newer than the qualified target"):
            helper.migration_registry(actual_registry)

    def test_candidate_registry_must_contain_qualified_source_baseline(self) -> None:
        for source_kind, baseline in (("prepared_artifact", 123), ("published_release", 114)):
            for migrated in (False, True):
                with self.subTest(source_kind=source_kind, migrated=migrated):
                    registry = self.root / "missing-baseline.rs"
                    source = self.registry.read_text()
                    registry.write_text(re.sub(
                        rf'    Migration\s*\{{\s*version:\s*{baseline},.*?\n    \}},\n',
                        "", source, count=1, flags=re.S,
                    ))
                    parsed = helper.migration_registry(registry)
                    self.assertNotIn(baseline, [version for version, _name in parsed])
                    database = self.database(through=128)
                    with sqlite3.connect(database) as connection:
                        connection.execute("DELETE FROM _migrations WHERE version = ?", (baseline,))
                    with self.assertRaisesRegex(helper.ActivationError, "missing the qualified source baseline"):
                        helper.validate_current_schema_database(
                            database, registry, source_kind=source_kind, migrated=migrated,
                        )

    def test_partial_target_is_not_an_admitted_prepared_source(self) -> None:
        database = self.database(through=127)

        with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
            helper.validate_current_schema_database(database, self.registry)

    def test_qualified_registry_ends_at_product_creation_stage_migration(self) -> None:
        registry = helper.migration_registry(self.registry)
        self.assertEqual(
            registry[-1],
            (128, "product_creation_provisioning_stage"),
        )
        self.assertEqual(registry, tuple(compiled_ledger()))

    def test_future_registry_fields_are_parsed_in_every_order(self) -> None:
        source = self.registry.read_text()
        registry = self.root / "future-migrations.rs"
        fields = ('version: 129', 'name: "future"', 'sql: MIGRATION_129')
        for ordered_fields in itertools.permutations(fields):
            with self.subTest(fields=ordered_fields):
                entry = "Migration { " + ", ".join(ordered_fields) + " },"
                registry.write_text(source.replace("\n];", f"\n    {entry}\n];", 1))
                self.assertEqual(
                    helper.migration_registry(registry, qualified_current_target=False),
                    tuple(compiled_ledger()) + ((129, "future"),),
                )
                with self.assertRaisesRegex(helper.ActivationError, "newer than the qualified target"):
                    helper.migration_registry(registry)

    def test_malformed_registry_entries_fail_closed(self) -> None:
        source = self.registry.read_text()
        registry = self.root / "malformed-migrations.rs"
        invalid_entries = (
            'Migration { name: "future", sql: MIGRATION_129 },',
            'Migration { version: 129, sql: MIGRATION_129 },',
            'Migration { version: 129, name: "future" },',
            'Migration { version: 129, version: 130, name: "future", sql: MIGRATION_129 },',
            'Migration { version: 129, name: "future", name: "other", sql: MIGRATION_129 },',
            'Migration { version: 129, name: "future", sql: MIGRATION_129, sql: MIGRATION_130 },',
            'Migration { version: 129, name: "future", sql: },',
            'Migration { version: 129, name: "future", sql: MIGRATION_129, extra: 1 },',
            'Migration { version: 129, name: "future", sql: MIGRATION_129 garbage },',
            'Migration { version: 129 name: "future", sql: MIGRATION_129 },',
            'Migration { version: 129, name: "future", sql: MIGRATION_129,',
            'Migration { version: 129, name: "future", sql: MIGRATION_129 }, garbage',
            'garbage, Migration { version: 129, name: "future", sql: MIGRATION_129 },',
            'Migration { version: 129, name: "future", sql: MIGRATION_129 },,',
            'Migration { version: 128, name: "duplicate", sql: MIGRATION_128 },',
            'Migration { version: 127, name: "out_of_order", sql: MIGRATION_127 },',
        )
        for entry in invalid_entries:
            registry.write_text(source.replace("\n];", f"\n    {entry}\n];", 1))
            for qualified in (True, False):
                with self.subTest(entry=entry, qualified=qualified):
                    with self.assertRaisesRegex(helper.ActivationError, "registry is invalid"):
                        helper.migration_registry(registry, qualified_current_target=qualified)

    def test_unknown_newer_and_name_mismatched_ledgers_fail_closed(self) -> None:
        newer = self.database(through=124)
        with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
            helper.validate_current_schema_database(newer, self.registry)

        mismatch = self.database()
        with sqlite3.connect(mismatch) as connection:
            connection.execute(
                "UPDATE _migrations SET name = 'forged' WHERE version = 123"
            )
        with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
            helper.validate_current_schema_database(mismatch, self.registry)

    def test_actual_product_conversation_relationship_is_required(self) -> None:
        for mutation in (
            "DROP TABLE product_conversation_sources",
            "ALTER TABLE conversations DROP COLUMN product_conversation_id",
            "DROP TABLE conversations",
            "DROP TABLE product_conversations",
        ):
            with self.subTest(mutation=mutation):
                database = self.database()
                with sqlite3.connect(database) as connection:
                    connection.execute(mutation)
                with self.assertRaisesRegex(
                    helper.ActivationError, "ProductConversation schema"
                ):
                    helper.validate_current_schema_database(database, self.registry)

    def test_actual_product_conversation_foreign_keys_are_required(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.executescript(
                "DROP TABLE conversations;"
                "CREATE TABLE conversations ("
                "id TEXT PRIMARY KEY, product_conversation_id TEXT);"
            )
        with self.assertRaisesRegex(helper.ActivationError, "foreign key is missing"):
            helper.validate_current_schema_database(database, self.registry)

        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.executescript(
                "DROP TABLE product_conversation_sources;"
                "CREATE TABLE product_conversation_sources ("
                "target_product_conversation_id TEXT PRIMARY KEY,"
                "source_product_conversation_id TEXT NOT NULL,"
                "source_conversation_id TEXT NOT NULL,"
                "relation_kind TEXT NOT NULL,"
                "relation_key TEXT NOT NULL,"
                "created_at_us INTEGER NOT NULL);"
            )
        with self.assertRaisesRegex(helper.ActivationError, "foreign keys do not match"):
            helper.validate_current_schema_database(database, self.registry)

    def test_fictional_members_table_is_not_required(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            members = connection.execute(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' "
                "AND name = 'product_conversation_members'"
            ).fetchone()
        self.assertIsNone(members)
        helper.validate_current_schema_database(database, self.registry)

    def test_foreign_key_violation_fails_before_migration(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.execute("PRAGMA foreign_keys = OFF")
            connection.execute(
                "INSERT INTO conversations (id, product_conversation_id) "
                "VALUES ('orphan', 'missing')"
            )
        with self.assertRaisesRegex(helper.ActivationError, "foreign key"):
            helper.validate_current_schema_database(database, self.registry)

    def test_helper_main_releases_exact_claim_after_durable_precondition_failure(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.execute("DROP TABLE product_conversation_sources")
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        events: list[str] = []
        original_fsync = helper.fsync_dir
        original_release = helper.release_claim

        def observed_fsync(path: Path) -> None:
            original_fsync(path)
            status_path = Path(manifest.status_path)
            if path == status_path.parent and status_path.exists():
                status = json.loads(status_path.read_text())
                if status["state"] == "precondition_failed":
                    events.append("status_fsync")

        def observed_release(current: helper.Manifest) -> bool:
            events.append("claim_release")
            return original_release(current)

        with mock.patch.object(sys, "argv", self.helper_main_argv(manifest)), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "fsync_dir", side_effect=observed_fsync
        ), mock.patch.object(
            helper, "release_claim", side_effect=observed_release
        ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)

        launchctl.assert_not_called()
        self.assertEqual(
            events[-3:], ["status_fsync", "claim_release", "status_fsync"]
        )
        self.assertFalse(claim.exists())
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")
        self.assertIn("ProductConversation schema", status["failure"])

    def test_helper_main_releases_pre_stop_precondition_without_disruption(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        reservation = helper.DatabaseCapacityReservation(
            backup, restore, database.stat().st_size
        )
        backup.touch(mode=0o600)
        restore.touch(mode=0o600)
        helper._write_capacity_observation(manifest, reservation)
        capacity_evidence = helper._capacity_observation_path(reservation)
        validations = 0

        def validate(*_args, **_kwargs):
            nonlocal validations
            validations += 1
            if validations == 2:
                raise helper.ActivationError("database changed before stop")
            return tuple(compiled_ledger(123))

        class NotYetStopped:
            def __init__(self) -> None:
                self.stop_calls = 0
                self.disruption_started = False

            def stop(self) -> int | None:
                self.stop_calls += 1
                raise AssertionError("pre-stop validation should fail first")

        launchctl = NotYetStopped()
        original_release = helper.release_claim
        with mock.patch.object(sys, "argv", self.helper_main_argv(manifest)), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "validate_current_schema_database", side_effect=validate
        ), mock.patch.object(
            helper, "reserve_database_capacity", return_value=reservation
        ), mock.patch.object(
            helper, "Launchctl", return_value=launchctl
        ), mock.patch.object(
            helper, "release_claim", wraps=original_release
        ) as release, mock.patch.object(helper, "request_helper_bootout"):
            self.assertEqual(helper.main(), 1)

        self.assertEqual(validations, 2)
        self.assertEqual(launchctl.stop_calls, 0)
        self.assertFalse(backup.exists())
        self.assertFalse(restore.exists())
        self.assertFalse(capacity_evidence.exists())
        release.assert_called_once_with(manifest)
        self.assertFalse(claim.exists())
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")
        self.assertEqual(status["failure"], "database changed before stop")

    def test_helper_main_retains_precondition_status_and_claim_when_release_fails(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.execute("DROP TABLE product_conversation_sources")
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        release = mock.Mock(side_effect=OSError("injected claim release failure"))

        with mock.patch.object(sys, "argv", self.helper_main_argv(manifest)), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "release_claim", release
        ), mock.patch.object(helper, "request_helper_bootout"):
            self.assertEqual(helper.main(), 1)

        release.assert_called_once_with(manifest)
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertIn("injected claim release failure", status["cleanup_failure"])
        self.assertIn("ProductConversation schema", status["failure"])

    def test_helper_main_does_not_release_a_newer_claim_owner(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.execute("DROP TABLE product_conversation_sources")
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        claim = Path(manifest.active_path)
        claim.write_text("newer-transaction")
        original_release = helper.release_claim
        release_results: list[bool] = []

        def observed_release(current: helper.Manifest) -> bool:
            released = original_release(current)
            release_results.append(released)
            return released

        with mock.patch.object(sys, "argv", self.helper_main_argv(manifest)), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "release_claim", side_effect=observed_release
        ) as release, mock.patch.object(helper, "request_helper_bootout"):
            self.assertEqual(helper.main(), 1)

        release.assert_called_once_with(manifest)
        self.assertEqual(release_results, [False])
        self.assertEqual(claim.read_text(), "newer-transaction")
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")

    def test_helper_main_retains_claim_when_terminal_status_fsync_fails(self) -> None:
        database = self.database()
        with sqlite3.connect(database) as connection:
            connection.execute("DROP TABLE product_conversation_sources")
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        original_fsync = helper.fsync_dir
        precondition_fsyncs = 0

        def fail_release_barrier(path: Path) -> None:
            nonlocal precondition_fsyncs
            status_path = Path(manifest.status_path)
            if path == status_path.parent and status_path.exists():
                status = json.loads(status_path.read_text())
                if status["state"] == "precondition_failed":
                    precondition_fsyncs += 1
                    if precondition_fsyncs == 2:
                        raise OSError("injected terminal status fsync failure")
            original_fsync(path)

        with mock.patch.object(sys, "argv", self.helper_main_argv(manifest)), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "fsync_dir", side_effect=fail_release_barrier
        ), mock.patch.object(helper, "release_claim") as release, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)

        self.assertEqual(precondition_fsyncs, 2)
        release.assert_not_called()
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertIn("terminal status fsync failure", status["cleanup_failure"])
        self.assertIn("ProductConversation schema", status["failure"])

    def test_helper_main_preserves_unresolved_status_on_activate_reentry(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        for state, finalization_pending in (
            ("migration_started", False),
            ("committed", True),
            ("precondition_failed", False),
        ):
            with self.subTest(state=state):
                status = {
                    "transaction_id": manifest.transaction_id,
                    "state": state,
                    "source_kind": manifest.source_kind,
                    "finalization_pending": finalization_pending,
                }
                status_bytes = json.dumps(status, sort_keys=True).encode()
                Path(manifest.status_path).write_bytes(status_bytes)
                claim.write_text(manifest.transaction_id)
                with mock.patch.object(
                    sys, "argv", self.helper_main_argv(manifest)
                ), mock.patch.object(
                    helper.Manifest, "load", return_value=manifest
                ), mock.patch.object(
                    helper, "validate_manifest_identities"
                ) as validate_identities, mock.patch.object(
                    helper, "release_claim"
                ) as release, mock.patch.object(helper, "request_helper_bootout"):
                    self.assertEqual(helper.main(), 1)
                validate_identities.assert_not_called()
                release.assert_not_called()
                self.assertEqual(Path(manifest.status_path).read_bytes(), status_bytes)
                self.assertEqual(claim.read_text(), manifest.transaction_id)

    def test_helper_main_checks_freshness_under_activation_lock(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        events: list[str] = []

        def observe_flock(_lock, operation: int) -> None:
            if operation == helper.fcntl.LOCK_EX | helper.fcntl.LOCK_NB:
                events.append("activation_lock")

        def reject_after_lock(_manifest: helper.Manifest) -> None:
            events.append("freshness_check")
            raise helper.ActivationError("injected stale activation")

        with mock.patch.object(sys, "argv", self.helper_main_argv(manifest)), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper.fcntl, "flock", side_effect=observe_flock), mock.patch.object(
            helper,
            "require_fresh_prepared_activation",
            side_effect=reject_after_lock,
        ), mock.patch.object(helper, "release_claim") as release, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)

        self.assertEqual(events, ["activation_lock", "freshness_check"])
        release.assert_not_called()
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        self.assertEqual(
            json.loads(Path(manifest.status_path).read_text())["state"], "prepared"
        )

    def migration_script(
        self,
        manifest: helper.Manifest,
        *,
        partial: bool = False,
        growth_bytes: int = 0,
    ) -> None:
        target = Path(manifest.target_binary)
        baseline = (
            114
            if manifest.source_kind == "published_release"
            or isinstance(manifest.current_schema_upgrade, helper.PreparedSource114Upgrade)
            else 123
        )
        pending = [item for item in compiled_ledger() if item[0] > baseline]
        migrations = pending[:1] if partial else pending
        growth = (
            "connection.execute('CREATE TABLE migration_growth (payload BLOB NOT NULL)')\n"
            f"connection.execute('INSERT INTO migration_growth VALUES (zeroblob({growth_bytes}))')"
            if growth_bytes
            else ""
        )
        script = f"""#!/usr/bin/env python3
import os, sqlite3, sys
connection = sqlite3.connect(os.environ['PHOENIX_DB_PATH'])
connection.executemany('INSERT INTO _migrations (version, name) VALUES (?, ?)', {migrations!r})
{growth}
connection.commit()
connection.close()
sys.exit({7 if partial else 0})
"""
        target.write_text(script)
        target.chmod(0o755)
        object.__setattr__(manifest, "candidate_binary_sha256", helper.sha256(target))

    def test_release_114_migration_and_startup_failures_never_launch_predecessor(self) -> None:
        self.assert_114_failures_never_launch_predecessor(self.release_manifest)

    def test_prepared_114_migration_and_startup_failures_never_launch_predecessor(self) -> None:
        self.assert_114_failures_never_launch_predecessor(self.prepared114_manifest)

    def assert_114_failures_never_launch_predecessor(self, manifest_factory) -> None:
        for failure in ("migration", "startup"):
            with self.subTest(failure=failure):
                database = self.database(through=114)
                manifest = manifest_factory(database)
                self.migration_script(manifest, partial=failure == "migration")
                current = manifest.current_schema_upgrade
                Path(manifest.target_plist).write_bytes(Path(manifest.rollback_plist).read_bytes())
                Path(manifest.active_path).write_text(manifest.transaction_id)
                helper.write_status(manifest, "prepared")
                reservation = self.migration_reservation(manifest)
                Path(current.backup_path).touch(mode=0o600)

                class CandidateLaunch:
                    disruption_started = False
                    starts = 0
                    started_plists = []

                    def stop(self):
                        self.disruption_started = True
                        return 71

                    def inspect(self):
                        return "not_loaded", None

                    def start(self, old_pid, plist_path=None):
                        self.starts += 1
                        self.started_plists.append(plist_path)
                        raise helper.ActivationError("injected startup failure")

                launchctl = CandidateLaunch()
                real_run = helper.subprocess.run
                invoked = []

                def run(command, **kwargs):
                    if "--migrate-only" in command:
                        self.assertEqual(helper.read_status(manifest)["state"], "migration_started")
                        invoked.append(command)
                    return real_run(command, **kwargs)

                with mock.patch.object(helper, "__file__", current.controller_helper_path), \
                     mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                     mock.patch.object(helper, "reserve_database_capacity", return_value=reservation), \
                     mock.patch.object(helper, "assert_database_exclusive"), \
                     mock.patch.object(helper, "_reservation_still_sufficient"), \
                     mock.patch.object(helper, "_require_current_schema_free_space"), \
                     mock.patch.object(helper, "restore") as restore, \
                     mock.patch.object(helper.subprocess, "run", side_effect=run):
                    state = helper.activate(manifest)
                restore.assert_not_called()
                self.assertEqual(len(invoked), 1)
                self.assertEqual(launchctl.starts, 0 if failure == "migration" else 1)
                self.assertEqual(launchctl.started_plists, [] if failure == "migration" else [manifest.candidate_plist])
                self.assertEqual(state, "activation_failed_database_committed" if failure == "migration" else "committed")
                self.assertEqual(Path(manifest.active_path).read_text(), manifest.transaction_id)
                proof = helper._snapshot_proof(manifest)
                self.assertEqual(proof["source_ledger"], [list(item) for item in compiled_ledger(114)])
                self.assertEqual(helper.database_ledger(database)[-1][0], 115 if failure == "migration" else 128)
                self.assertTrue(helper.read_status(manifest)["finalization_pending"])

    def test_offline_migration_commits_exact_target_ledger_and_proof(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.migration_script(manifest)
        self.write_snapshot_proof(manifest)
        reservation = self.migration_reservation(manifest)
        proof_path = Path(manifest.current_schema_upgrade.proof_path)
        initial_proof = proof_path.read_bytes()
        projected_proof = helper._project_migrated_database_proof(initial_proof)

        with mock.patch.object(helper, "assert_database_exclusive"):
            helper.run_current_schema_migration(manifest, reservation)

        actual_proof = proof_path.read_bytes()
        self.assertEqual(len(actual_proof), len(projected_proof))
        helper._validate_database_proof_lifecycle_budget(
            initial_proof,
            actual_proof,
            max(os.statvfs(proof_path.parent).f_frsize, 4096),
        )
        ledger = helper.validate_current_schema_database(
            database,
            Path(manifest.current_schema_upgrade.migration_registry_path),
            migrated=True,
        )
        proof = json.loads(Path(manifest.current_schema_upgrade.proof_path).read_text())
        self.assertEqual(proof["migrated_ledger"], [list(item) for item in ledger])
        self.assertIn("migration_committed_at", proof)

    def test_exact_target_source_still_invokes_offline_candidate(self) -> None:
        self.assert_exact_target_invokes_offline_candidate(self.manifest)

    def test_qualified_prepared114_target_source_still_invokes_offline_candidate(self) -> None:
        self.assert_exact_target_invokes_offline_candidate(self.prepared114_manifest)

    def test_protected_release_exact_target_fails_before_disruption(self) -> None:
        database = self.database(through=128)
        manifest = self.release_manifest(database)
        self.write_current_status(manifest, "prepared")
        before = database.read_bytes()
        launchctl = mock.Mock()

        with mock.patch.object(helper, "__file__", manifest.current_schema_upgrade.controller_helper_path), \
             mock.patch.object(helper, "Launchctl", return_value=launchctl), \
             mock.patch.object(helper, "reserve_database_capacity") as reserve, \
             mock.patch.object(helper, "run_current_schema_migration") as migrate:
            with self.assertRaisesRegex(helper.ActivationError, "qualified source"):
                helper.activate(manifest)

        launchctl.stop.assert_not_called()
        launchctl.start.assert_not_called()
        reserve.assert_not_called()
        migrate.assert_not_called()
        self.assertEqual(database.read_bytes(), before)
        self.assertFalse(Path(manifest.current_schema_upgrade.backup_path).exists())
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")
        self.assertEqual(status["prepared_mode"], "current_schema")

    def test_migrated_target_is_valid_for_both_current_schema_sources(self) -> None:
        database = self.database(through=128)
        for source_kind in ("prepared_artifact", "published_release"):
            with self.subTest(source_kind=source_kind):
                self.assertEqual(
                    helper.validate_current_schema_database(
                        database, self.registry, source_kind=source_kind, migrated=True,
                    ),
                    tuple(compiled_ledger(128)),
                )

    def assert_exact_target_invokes_offline_candidate(self, manifest_factory) -> None:
        database = self.database(through=128)
        manifest = manifest_factory(database)
        current = manifest.current_schema_upgrade
        assert current is not None
        target = Path(manifest.target_binary)
        target.write_text(
            """#!/usr/bin/env python3
import os, sqlite3
connection = sqlite3.connect(os.environ['PHOENIX_DB_PATH'])
connection.execute('CREATE TABLE same_schema_reconciliation (value TEXT NOT NULL)')
connection.execute("INSERT INTO same_schema_reconciliation VALUES ('invoked')")
connection.commit()
connection.close()
"""
        )
        target.chmod(0o755)
        object.__setattr__(manifest, "candidate_binary_sha256", helper.sha256(target))
        self.write_snapshot_proof(manifest)
        reservation = self.migration_reservation(manifest)

        real_run = helper.subprocess.run
        with mock.patch.object(helper, "assert_database_exclusive"), mock.patch.object(
            helper.subprocess, "run", side_effect=real_run,
        ) as run:
            helper.run_current_schema_migration(manifest, reservation)
        invocations = [call for call in run.call_args_list if "--migrate-only" in call.args[0]]
        self.assertEqual(len(invocations), 1)
        self.assertEqual(invocations[0].args[0], [str(target), "--migrate-only"])
        self.assertEqual(invocations[0].kwargs["env"]["PHOENIX_DB_PATH"], str(database))

        with sqlite3.connect(database) as connection:
            self.assertEqual(
                connection.execute(
                    "SELECT value FROM same_schema_reconciliation"
                ).fetchone(),
                ("invoked",),
            )
        proof = json.loads(Path(current.proof_path).read_text())
        target_ledger = [list(item) for item in compiled_ledger()]
        self.assertEqual(proof["source_ledger"], target_ledger)
        self.assertEqual(proof["target_ledger"], target_ledger)
        self.assertEqual(proof["migrated_ledger"], target_ledger)
        self.assertNotEqual(
            proof["source_logical_digest"], proof["migrated_logical_digest"]
        )

    def test_post_migration_growth_cannot_exceed_reserved_capacity(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.migration_script(manifest, growth_bytes=2 * 1024 * 1024)
        self.write_snapshot_proof(manifest)
        reservation = self.migration_reservation(
            manifest,
            capacity_bytes=helper._source_capacity_bytes(database),
        )

        with mock.patch.object(helper, "assert_database_exclusive"):
            with self.assertRaisesRegex(
                helper.ActivationError,
                "migrated database grew beyond its reserved capacity",
            ):
                helper.run_current_schema_migration(manifest, reservation)

        self.assertFalse(helper.current_schema_database_is_uncommitted(manifest))

    def test_proof_publication_rechecks_free_space_after_logical_digest(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.migration_script(manifest)
        self.write_snapshot_proof(manifest)
        reservation = self.migration_reservation(manifest)
        proof_path = Path(manifest.current_schema_upgrade.proof_path)
        initial_proof = proof_path.read_bytes()
        free_space_lost = False
        checks = 0
        post_check_digests = 0
        real_digest = helper.database_logical_digest

        def digest(path):
            nonlocal free_space_lost, post_check_digests
            value = real_digest(path)
            if checks == 1:
                post_check_digests += 1
                free_space_lost = True
            return value

        def require_free_space(*_args, **_kwargs):
            nonlocal checks
            checks += 1
            if free_space_lost:
                raise helper.ActivationError("free space lost during logical digest")

        with mock.patch.object(helper, "assert_database_exclusive"), mock.patch.object(
            helper, "database_logical_digest", side_effect=digest
        ), mock.patch.object(
            helper,
            "_require_current_schema_free_space",
            side_effect=require_free_space,
        ), mock.patch.object(helper, "atomic_write") as atomic_write:
            with self.assertRaisesRegex(
                helper.ActivationError,
                "free space lost during logical digest",
            ):
                helper.run_current_schema_migration(manifest, reservation)

        self.assertEqual(post_check_digests, 1)
        self.assertEqual(checks, 2)
        atomic_write.assert_not_called()
        self.assertEqual(proof_path.read_bytes(), initial_proof)

    def test_initial_proof_rechecks_free_space_after_logical_digest(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        current = manifest.current_schema_upgrade
        assert current is not None
        backup = Path(current.backup_path)
        backup.touch(mode=0o600)
        reservation = helper.DatabaseCapacityReservation(
            backup, self.root / "restore.sqlite3", database.stat().st_size
        )
        proof_path = Path(current.proof_path)
        digest_calls = 0
        free_space_lost = False
        checks = 0
        real_digest = helper.database_logical_digest

        def digest(path):
            nonlocal digest_calls, free_space_lost
            value = real_digest(path)
            digest_calls += 1
            if digest_calls == 4:
                free_space_lost = True
            return value

        def require_free_space(*_args, **_kwargs):
            nonlocal checks
            checks += 1
            if free_space_lost:
                raise helper.ActivationError("free space lost during snapshot digest")

        with mock.patch.object(helper, "assert_database_exclusive"), mock.patch.object(
            helper, "database_logical_digest", side_effect=digest
        ), mock.patch.object(
            helper,
            "_require_current_schema_free_space",
            side_effect=require_free_space,
        ), mock.patch.object(helper, "atomic_write") as atomic_write:
            with self.assertRaisesRegex(
                helper.ActivationError,
                "free space lost during snapshot digest",
            ):
                helper.create_database_backup(manifest, reservation)

        self.assertEqual(digest_calls, 4)
        self.assertEqual(checks, 1)
        atomic_write.assert_not_called()
        self.assertFalse(proof_path.exists())

    def test_failed_offline_migration_allows_rollback_only_when_unchanged(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        Path(manifest.target_binary).write_text("#!/bin/sh\nexit 9\n")
        Path(manifest.target_binary).chmod(0o755)
        object.__setattr__(
            manifest, "candidate_binary_sha256", helper.sha256(Path(manifest.target_binary))
        )
        self.write_snapshot_proof(manifest)
        reservation = self.migration_reservation(manifest)

        with mock.patch.object(helper, "assert_database_exclusive"):
            with self.assertRaisesRegex(helper.ActivationError, "exit 9"):
                helper.run_current_schema_migration(manifest, reservation)
        self.assertTrue(helper.current_schema_database_is_uncommitted(manifest))

        partial_database = self.database()
        partial_manifest = self.manifest(partial_database)
        self.migration_script(partial_manifest, partial=True)
        self.write_snapshot_proof(partial_manifest)
        partial_reservation = self.migration_reservation(partial_manifest)
        with mock.patch.object(helper, "assert_database_exclusive"):
            with self.assertRaisesRegex(helper.ActivationError, "exit 7"):
                helper.run_current_schema_migration(partial_manifest, partial_reservation)
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

    def test_ordinary_unqualified_pending_interval_rejected_but_future_exact_target_allowed(self) -> None:
        database = self.database(through=122)
        manifest = self.ordinary_manifest(database)
        ordinary = manifest.ordinary_forward_migration
        registry = Path(ordinary.migration_registry_path)
        source = registry.read_text()
        source = source.replace(
            "\n];",
            '\n    Migration { sql: "SELECT 1", name: "future", version: 129 },\n];',
            1,
        )
        registry.write_text(source)
        object.__setattr__(
            ordinary, "migration_registry_sha256", helper.sha256(registry)
        )

        self.assertEqual(
            helper.migration_registry(registry, qualified_current_target=False),
            tuple(compiled_ledger()) + ((129, "future"),),
        )
        with self.assertRaisesRegex(helper.ActivationError, "newer than the qualified target"):
            helper.migration_registry(registry)

        helper.validate_manifest_mode(manifest)
        with self.assertRaisesRegex(helper.ActivationError, "pending migration interval is not qualified"):
            helper.ordinary_forward_policy_applies(manifest)
        with sqlite3.connect(database) as connection:
            connection.execute("DELETE FROM _migrations")
            connection.executemany("INSERT INTO _migrations VALUES (?, ?)", compiled_ledger() + [(129, "future")])
        self.assertFalse(helper.ordinary_forward_policy_applies(manifest))

    def test_helper_rejects_unknown_source_kind_for_every_mode(self) -> None:
        database = self.database()
        for manifest in (
            self.manifest(database),
            self.ordinary_manifest(database),
        ):
            with self.subTest(mode=type(manifest.current_schema_upgrade).__name__):
                object.__setattr__(manifest, "source_kind", "prepared_artifac")
                with self.assertRaisesRegex(
                    helper.ActivationError, "source kind is unsupported"
                ):
                    helper.validate_manifest_mode(manifest)

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

    def test_current_initial_start_rechecks_exclusivity(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        Path(manifest.target_plist).write_bytes(Path(manifest.rollback_plist).read_bytes())
        reservation = helper.DatabaseCapacityReservation(
            Path(manifest.current_schema_upgrade.backup_path),
            self.root / "restore.sqlite3",
            database.stat().st_size,
        )

        class StoppedLaunchctl:
            def __init__(self) -> None:
                self.started = 0
                self.disruption_started = False

            def stop(self) -> int | None:
                self.disruption_started = True
                return 71

            def inspect(self):
                return "not_loaded", None

            def start(self, old_pid, plist_path=None) -> None:
                self.started += 1

        launchctl = StoppedLaunchctl()
        exclusive_calls = 0

        def exclusive(_manifest):
            nonlocal exclusive_calls
            exclusive_calls += 1
            if exclusive_calls == 2:
                raise helper.ActivationError("database reopened before initial start")

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "validate_current_schema_database", return_value=tuple(compiled_ledger(123))
        ), mock.patch.object(
            helper, "reserve_database_capacity", return_value=reservation
        ), mock.patch.object(helper, "create_database_backup"), mock.patch.object(
            helper, "_reservation_still_sufficient"
        ), mock.patch.object(
            helper, "run_current_schema_migration"
        ), mock.patch.object(helper, "assert_database_exclusive", side_effect=exclusive), mock.patch.object(
            helper, "Launchctl", return_value=launchctl
        ):
            state = helper.activate(manifest)

        self.assertEqual(state, "committed")
        self.assertEqual(launchctl.started, 0)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertTrue(status["finalization_pending"])
        self.assertIn("database reopened", status["committed_diagnostic"])

    def test_completed_current_finalization_does_not_restart_stopped_service(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        target_plist = Path(manifest.target_plist)
        target_plist.unlink(missing_ok=True)
        os.link(manifest.candidate_plist, target_plist)
        helper.write_status(manifest, "committed")
        absence = subprocess.CompletedProcess([], 1, "", "service not found")
        proof = {"migration_committed_at": "2026-01-01T00:00:00+00:00"}

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper.subprocess, "run", return_value=absence), mock.patch.object(
            helper, "service_absence_confirmed", return_value=True
        ), mock.patch.object(helper, "_snapshot_proof", return_value=proof), mock.patch.object(
            helper, "validate_current_schema_database"
        ), mock.patch.object(helper, "Launchctl") as launchctl:
            state = helper.finalize_paired(manifest, current_schema=True)

        self.assertEqual(state, "committed")
        launchctl.assert_not_called()

    def test_current_pre_stop_failure_releases_both_capacity_files(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.write_current_status(manifest, "prepared")
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        reservation = helper.DatabaseCapacityReservation(
            backup, restore, database.stat().st_size
        )
        backup.touch(mode=0o600)
        restore.touch(mode=0o600)
        helper._write_capacity_observation(manifest, reservation)
        capacity_evidence = helper._capacity_observation_path(reservation)
        validations = 0

        def validate(*_args, **_kwargs):
            nonlocal validations
            validations += 1
            if validations == 2:
                raise helper.ActivationError("database changed before stop")
            return tuple(compiled_ledger(123))

        class NotYetStopped:
            disruption_started = False

            def stop(self) -> int | None:
                raise AssertionError("pre-stop validation should fail first")

        launchctl = NotYetStopped()
        durable_states = []
        real_write_status = helper.write_status

        def record_status(manifest_arg, state, **fields):
            result = real_write_status(manifest_arg, state, **fields)
            durable_states.append(state)
            return result

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "validate_current_schema_database", side_effect=validate
        ), mock.patch.object(
            helper, "reserve_database_capacity", return_value=reservation
        ), mock.patch.object(helper, "Launchctl", return_value=launchctl), mock.patch.object(
            helper, "write_status", side_effect=record_status
        ):
            with self.assertRaisesRegex(helper.ActivationError, "changed before stop"):
                helper.activate(manifest)

        self.assertEqual(durable_states, ["activating", "precondition_failed"])
        self.assertFalse(backup.exists())
        self.assertFalse(restore.exists())
        self.assertFalse(capacity_evidence.exists())
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")
        self.assertEqual(status["prepared_mode"], "current_schema")

    def test_pre_stop_terminal_status_failure_stays_cleanup_pending(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.write_current_status(manifest, "prepared")
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        reservation = helper.DatabaseCapacityReservation(
            backup, restore, database.stat().st_size
        )
        backup.touch(mode=0o600)
        restore.touch(mode=0o600)
        helper._write_capacity_observation(manifest, reservation)
        validations = 0

        def validate(*_args, **_kwargs):
            nonlocal validations
            validations += 1
            if validations == 2:
                raise helper.ActivationError("database changed before stop")
            return tuple(compiled_ledger(123))

        real_write_status = helper.write_status

        def write_status(manifest_arg, state, **fields):
            if state == "precondition_failed":
                raise OSError("terminal status write failed")
            return real_write_status(manifest_arg, state, **fields)

        launchctl = mock.Mock(disruption_started=False)
        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "validate_current_schema_database", side_effect=validate
        ), mock.patch.object(
            helper, "reserve_database_capacity", return_value=reservation
        ), mock.patch.object(
            helper, "Launchctl", return_value=launchctl
        ), mock.patch.object(helper, "write_status", side_effect=write_status):
            with self.assertRaisesRegex(OSError, "terminal status write failed"):
                helper.activate(manifest)

        self.assertFalse(backup.exists())
        self.assertFalse(restore.exists())
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertEqual(status["failure"], "database changed before stop")
        self.assertIn("terminal status write failed", status["cleanup_failure"])

    def test_pre_stop_abandonment_cleanup_failure_stays_cleanup_pending(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        self.write_current_status(manifest, "prepared")
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        reservation = helper.DatabaseCapacityReservation(
            backup, restore, database.stat().st_size
        )
        backup.touch(mode=0o600)
        restore.touch(mode=0o600)
        helper._write_capacity_observation(manifest, reservation)
        validations = 0

        def validate(*_args, **_kwargs):
            nonlocal validations
            validations += 1
            if validations == 2:
                raise helper.ActivationError("database changed before stop")
            return tuple(compiled_ledger(123))

        launchctl = mock.Mock(disruption_started=False)
        durable_states = []
        real_write_status = helper.write_status

        def record_status(manifest_arg, state, **fields):
            result = real_write_status(manifest_arg, state, **fields)
            durable_states.append(state)
            return result

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "validate_current_schema_database", side_effect=validate
        ), mock.patch.object(
            helper, "reserve_database_capacity", return_value=reservation
        ), mock.patch.object(
            helper, "Launchctl", return_value=launchctl
        ), mock.patch.object(
            helper,
            "_remove_capacity_files",
            side_effect=helper.DatabaseCapacityCleanupFailure("fsync failed"),
        ), mock.patch.object(helper, "write_status", side_effect=record_status):
            with self.assertRaisesRegex(
                helper.DatabaseCapacityCleanupFailure, "fsync failed"
            ):
                helper.activate(manifest)

        self.assertEqual(
            durable_states, ["activating", "precondition_cleanup_pending"]
        )
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertEqual(status["failure"], "database changed before stop")
        self.assertIn("fsync failed", status["cleanup_failure"])
        self.assertTrue(backup.exists())
        self.assertTrue(restore.exists())
        self.assertTrue(helper._capacity_observation_path(reservation).exists())
        self.assertEqual(status["prepared_mode"], "current_schema")

    def test_standalone_controller_policy_uses_staged_helper_and_actual_ledgers(self) -> None:
        import runpy

        standalone = self.root / "standalone-controller"
        standalone.mkdir()
        shutil.copy2(ROOT / "dev.py", standalone / "dev.py")
        self.assertFalse((standalone / "scripts").exists())
        preflight = runpy.run_path(str(standalone / "dev.py"))["_preflight_ordinary_launchd_policy"]
        for through in (114, 123, 128):
            for source_kind in ("local_head", "published_release"):
                with self.subTest(through=through, source_kind=source_kind):
                    manifest = dataclasses.replace(self.ordinary_manifest(self.database(through=through)), source_kind=source_kind)
                    manifest_path = self.root / "manifest.json"
                    manifest_path.write_text(json.dumps(dataclasses.asdict(manifest)))
                    if through == 114:
                        with self.assertRaisesRegex(RuntimeError, "pending migration interval is not qualified"):
                            preflight(manifest_path, Path(manifest.ordinary_forward_migration.helper_path))
                    else:
                        preflight(manifest_path, Path(manifest.ordinary_forward_migration.helper_path))

    def test_controller_policy_verifies_helper_binding_before_execution(self) -> None:
        import runpy

        preflight = runpy.run_path(str(ROOT / "dev.py"))["_preflight_ordinary_launchd_policy"]
        manifest = self.ordinary_manifest(self.database())
        manifest_path = self.root / "manifest.json"
        manifest_path.write_text(json.dumps(dataclasses.asdict(manifest)))
        staged_helper = Path(manifest.ordinary_forward_migration.helper_path)
        replacement = self.root / "unbound-helper.py"
        replacement.write_text("raise AssertionError('unbound helper executed')")
        with self.assertRaisesRegex(SystemExit, "helper does not match"):
            preflight(manifest_path, replacement)
        staged_helper.write_text("raise AssertionError('tampered helper executed')")
        with self.assertRaisesRegex(SystemExit, "helper does not match"):
            preflight(manifest_path, staged_helper)

    def test_ordinary_114_rejected_before_stop_for_release_and_local_sources(self) -> None:
        for source_kind in ("local_head", "published_release"):
            with self.subTest(source_kind=source_kind):
                database = self.database(through=114)
                manifest = dataclasses.replace(self.ordinary_manifest(database), source_kind=source_kind)
                with mock.patch.object(helper, "Launchctl") as launchctl:
                    with self.assertRaisesRegex(helper.ActivationError, "pending migration interval is not qualified"):
                        helper.activate(manifest)
                launchctl.assert_not_called()
                self.assertEqual(helper.read_status(manifest)["state"], "precondition_failed")
                self.assertEqual(helper.database_ledger(database), tuple(compiled_ledger(114)))
                self.assertEqual(Path(manifest.target_binary).read_text(), "old predecessor")

    def test_ordinary_exact_target_stays_ordinary_for_release_and_local_sources(self) -> None:
        for source_kind in ("local_head", "published_release"):
            with self.subTest(source_kind=source_kind):
                manifest = dataclasses.replace(self.ordinary_manifest(self.database(through=128)), source_kind=source_kind)
                helper.validate_manifest_mode(manifest)
                self.assertFalse(helper.ordinary_forward_policy_applies(manifest))

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
        self.assertEqual(helper.database_ledger(database)[-1][0], 128)
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

    def test_wrong_helper_recovery_mode_does_not_rewrite_status(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(manifest, "prepared")
        original_status = Path(manifest.status_path).read_bytes()
        argv = self.helper_main_argv(manifest)
        argv[1] = "recover-paired"

        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)

        launchctl.assert_not_called()
        self.assertEqual(Path(manifest.status_path).read_bytes(), original_status)
        self.assertEqual(Path(manifest.active_path).read_text(), manifest.transaction_id)

    def test_helper_claim_fsync_failure_restores_visible_owner(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id + "\n")
        for restored_fsync in (None, OSError("restored claim fsync failed")):
            with self.subTest(restored_fsync=restored_fsync), mock.patch.object(
                helper, "fsync_dir", side_effect=[OSError("fsync failed"), restored_fsync]
            ) as fsync_dir:
                with self.assertRaisesRegex(OSError, "restored claim fsync failed" if restored_fsync else "fsync failed"):
                    helper.release_claim(manifest)
            self.assertEqual(fsync_dir.call_args_list, [mock.call(claim.parent)] * 2)
            self.assertEqual(claim.read_text(), manifest.transaction_id + "\n")

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

    def test_current_schema_reserves_safe_headroom_for_live_wal_growth(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        observed = helper._source_capacity_bytes(database)
        growth_budget = 4 * 1024 * 1024
        free = (
            helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            + helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
            + 3 * observed
            + 3 * growth_budget
        )
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        capacity_evidence = backup.parent / "capacity.json"

        def available_space(_path: Path):
            reserved = sum(
                path.stat().st_size for path in (backup, restore) if path.exists()
            )
            evidence = (
                capacity_evidence.stat().st_blocks * 512
                if capacity_evidence.exists()
                else 0
            )
            live_growth = max(0, helper._source_capacity_bytes(database) - observed)
            return mock.Mock(free=free - reserved - evidence - live_growth)

        with mock.patch.object(
            helper.shutil, "disk_usage", side_effect=available_space
        ):
            reservation = helper.reserve_database_capacity(manifest)

            self.assertGreaterEqual(
                reservation.capacity_bytes, observed + growth_budget - 4096
            )
            with sqlite3.connect(database) as connection:
                connection.execute("PRAGMA journal_mode = WAL")
                connection.execute(
                    "CREATE TABLE post_sample_growth (payload BLOB NOT NULL)"
                )
                connection.execute(
                    "INSERT INTO post_sample_growth (payload) VALUES (zeroblob(?))",
                    (2 * 1024 * 1024,),
                )
                connection.commit()
                connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")

            helper._reservation_still_sufficient(manifest, reservation)
            self.assertGreaterEqual(
                available_space(database.parent).free,
                helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES,
            )

        observation = json.loads(
            helper._capacity_observation_path(reservation).read_text()
        )
        self.assertEqual(observation["transaction_id"], manifest.transaction_id)
        self.assertEqual(observation["observed_source_bytes"], observed)
        self.assertEqual(
            observation["reserved_capacity_bytes"], reservation.capacity_bytes
        )
        self.assertEqual(
            observation["reservation_ceiling_bytes"], reservation.ceiling_bytes
        )
        self.assertGreater(observation["rechecked_source_bytes"], observed)
        self.assertIsNotNone(observation["measured_at"])
        self.assertIsNotNone(observation["rechecked_at"])
        helper.abandon_capacity_reservation(manifest, reservation)

    def test_high_free_space_uses_bounded_target_not_capacity_ceiling(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        observed = 1024 * 1024 * 1024
        free = 500 * 1024 * 1024 * 1024
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)

        with mock.patch.object(
            helper,
            "_capacity_pool_key",
            return_value=("device", 1),
        ), mock.patch.object(
            helper.shutil, "disk_usage", return_value=mock.Mock(free=free)
        ), mock.patch.object(
            helper.os, "statvfs", return_value=mock.Mock(f_frsize=4096)
        ):
            plan = helper._current_schema_reservation_plan(
                manifest, database, backup, restore, observed
            )

        expected_margin = helper._current_schema_live_growth_margin(observed)
        self.assertEqual(plan.reservation_bytes, observed + expected_margin)
        self.assertGreater(plan.ceiling_bytes, plan.reservation_bytes)
        self.assertLess(plan.reservation_bytes, 10 * 1024 * 1024 * 1024)

    def test_capacity_pool_key_uses_apfs_container_identity(self) -> None:
        path = self.root / "volume" / "db.sqlite"
        path.parent.mkdir()
        path.write_text("db")
        info = plistlib.dumps({"APFSContainerReference": "disk3"})
        result = subprocess.CompletedProcess([], 0, info, b"")

        with mock.patch.object(helper.sys, "platform", "darwin"), mock.patch.object(
            helper.os.path, "ismount", return_value=True
        ), mock.patch.object(helper.subprocess, "run", return_value=result) as diskutil:
            key = helper._capacity_pool_key(path)

        self.assertEqual(key, ("apfs_container", "disk3"))
        self.assertEqual(
            diskutil.call_args.args[0],
            ["/usr/sbin/diskutil", "info", "-plist", str(path.parent.resolve())],
        )

    def test_shared_apfs_container_uses_one_combined_capacity_budget(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        observed = 1024 * 1024 * 1024
        free = 500 * 1024 * 1024 * 1024
        unit = 4096
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)

        with mock.patch.object(
            helper,
            "_capacity_pool_key",
            return_value=("apfs_container", "disk3"),
        ), mock.patch.object(
            helper.shutil, "disk_usage", return_value=mock.Mock(free=free)
        ), mock.patch.object(
            helper.os, "statvfs", return_value=mock.Mock(f_frsize=unit)
        ):
            plan = helper._current_schema_reservation_plan(
                manifest, database, backup, restore, observed
            )

        expected_ceiling = (
            free
            - helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            - helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
        ) // 3
        expected_ceiling -= expected_ceiling % unit
        self.assertEqual(plan.ceiling_bytes, expected_ceiling)
        self.assertEqual(
            plan.reservation_bytes,
            observed + helper._current_schema_live_growth_margin(observed),
        )

    def test_current_schema_proof_size_is_bounded_before_reservation(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        oversized_name = "x" * helper.MAX_DATABASE_PROOF_BYTES

        with mock.patch.object(
            helper,
            "validate_current_schema_database",
            return_value=tuple(compiled_ledger(123)),
        ), mock.patch.object(
            helper,
            "migration_registry",
            return_value=((128, oversized_name),),
        ):
            with self.assertRaisesRegex(helper.ActivationError, "proof exceeds"):
                helper._database_proof_bytes(
                    manifest,
                    database,
                    backup_sha256="0" * 64,
                    source_logical_digest="0" * 64,
                    backup_logical_digest="0" * 64,
                )

    def test_proof_atomic_rewrite_peak_is_bounded_before_reservation(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        near_limit = b"x" * helper.MAX_DATABASE_PROOF_BYTES

        with mock.patch.object(
            helper, "_database_proof_bytes", return_value=near_limit
        ), mock.patch.object(
            helper, "_project_migrated_database_proof", return_value=near_limit
        ), mock.patch.object(
            helper,
            "_current_schema_reservation_plan",
            return_value=helper.DatabaseCapacityPlan(4096, 8192),
        ), mock.patch.object(
            helper.os, "statvfs", return_value=mock.Mock(f_frsize=4096)
        ), mock.patch.object(
            helper, "_allocate_private_sqlite"
        ) as allocate:
            with self.assertRaisesRegex(
                helper.ActivationError, "proof lifecycle exceeds"
            ):
                helper.reserve_database_capacity(manifest)

        allocate.assert_not_called()

    def test_proof_lifecycle_budget_uses_filesystem_allocation_units(self) -> None:
        capacity_unit = 1024 * 1024

        with self.assertRaisesRegex(
            helper.ActivationError, "proof lifecycle exceeds"
        ):
            helper._validate_database_proof_lifecycle_budget(
                b"initial", b"migrated", capacity_unit
            )

    def test_capacity_ceiling_accounts_for_separate_filesystems(self) -> None:
        database_dir = self.root / "database-fs"
        backup_dir = self.root / "backup-fs"
        install_dir = self.root / "install-fs"
        for directory in (database_dir, backup_dir, install_dir):
            directory.mkdir()
        database = database_dir / "prod.db"
        database.touch()
        backup = backup_dir / "backup.sqlite3"
        restore = database_dir / "restore.sqlite3"
        current = dataclasses.replace(
            self.manifest(self.database()).current_schema_upgrade,
            database_path=str(database),
            backup_path=str(backup),
        )
        manifest = dataclasses.replace(
            self.manifest(self.database()),
            target_binary=str(install_dir / "phoenix-ide"),
            target_plist=str(install_dir / "phoenix.plist"),
            current_schema_upgrade=current,
        )
        observed = 1024 * 1024
        floor = helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
        device_by_root = {
            database_dir: 1,
            backup_dir: 2,
            install_dir: 3,
        }

        def device(path: Path) -> int:
            for root, value in device_by_root.items():
                if path == root or root in path.parents:
                    return value
            raise AssertionError(f"unexpected path {path}")

        def fake_stat(path: Path, *args, **kwargs):
            return mock.Mock(st_dev=device(path))

        allocation_unit = 4096
        cases = (
            (floor + observed + 8 * 1024 * 1024, floor + 16 * 1024 * 1024),
            (floor + observed + 32 * 1024 * 1024, floor + 4 * 1024 * 1024),
        )
        for database_free, backup_free in cases:
            with self.subTest(
                database_free=database_free, backup_free=backup_free
            ):
                free_by_device = {
                    1: database_free,
                    2: backup_free,
                    3: floor + 1024 * 1024,
                }

                def disk_usage(path: Path):
                    return mock.Mock(free=free_by_device[device(path)])

                with mock.patch.object(
                    Path, "stat", fake_stat
                ), mock.patch.object(
                    helper,
                    "_capacity_pool_key",
                    side_effect=lambda path: ("device", device(path)),
                ), mock.patch.object(
                    helper.shutil, "disk_usage", side_effect=disk_usage
                ), mock.patch.object(
                    helper.os,
                    "statvfs",
                    return_value=mock.Mock(f_frsize=allocation_unit),
                ):
                    plan = helper._current_schema_reservation_plan(
                        manifest, database, backup, restore, observed
                    )

                database_ceiling = (database_free - floor) // 2
                backup_ceiling = (
                    backup_free
                    - floor
                    - helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
                )
                expected = min(database_ceiling, backup_ceiling)
                expected -= expected % allocation_unit
                self.assertEqual(plan.ceiling_bytes, expected)
                self.assertEqual(plan.reservation_bytes, expected)

        free_by_device = {1: floor + observed, 2: floor + observed, 3: floor - 1}
        with mock.patch.object(Path, "stat", fake_stat), mock.patch.object(
            helper,
            "_capacity_pool_key",
            side_effect=lambda path: ("device", device(path)),
        ), mock.patch.object(
            helper.shutil,
            "disk_usage",
            side_effect=lambda path: mock.Mock(free=free_by_device[device(path)]),
        ), mock.patch.object(
            helper.os, "statvfs", return_value=mock.Mock(f_frsize=allocation_unit)
        ):
            with self.assertRaisesRegex(helper.ActivationError, "20 GiB"):
                helper._current_schema_reservation_plan(
                    manifest, database, backup, restore, observed
                )

    def test_capacity_observation_failure_cleans_both_reservations(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        observed = helper._source_capacity_bytes(database)
        free = (
            helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            + 2 * observed
            + 3 * 1024 * 1024
        )
        backup = Path(manifest.current_schema_upgrade.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        with mock.patch.object(
            helper.shutil, "disk_usage", return_value=mock.Mock(free=free)
        ), mock.patch.object(
            helper,
            "atomic_write",
            side_effect=OSError("injected capacity evidence failure"),
        ):
            with self.assertRaisesRegex(
                helper.ActivationError,
                "could not persist paired capacity observation",
            ):
                helper.reserve_database_capacity(manifest)

        self.assertFalse(backup.exists())
        self.assertFalse(restore.exists())

    def test_first_allocation_cleanup_failure_is_classified_and_retained(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        observed = helper._source_capacity_bytes(database)
        free = (
            helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            + 2 * observed
            + 3 * 1024 * 1024
        )
        backup = Path(manifest.current_schema_upgrade.backup_path)
        original_fsync_dir = helper.fsync_dir
        original_unlink = Path.unlink

        def fail_allocation_fsync(path: Path) -> None:
            if path == backup.parent and backup.exists():
                raise OSError("injected allocation directory fsync failure")
            original_fsync_dir(path)

        def fail_backup_unlink(path: Path, *args, **kwargs):
            if path == backup:
                raise OSError("injected backup unlink failure")
            return original_unlink(path, *args, **kwargs)

        with mock.patch.object(
            helper.shutil, "disk_usage", return_value=mock.Mock(free=free)
        ), mock.patch.object(
            helper, "fsync_dir", side_effect=fail_allocation_fsync
        ), mock.patch.object(
            Path, "unlink", fail_backup_unlink
        ):
            with self.assertRaisesRegex(
                helper.DatabaseCapacityCleanupFailure,
                "injected backup unlink failure",
            ):
                helper.reserve_database_capacity(manifest)

        self.assertTrue(backup.exists())
        backup.unlink()

    def test_capacity_cleanup_failure_retains_claim_and_pending_status(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        helper.write_status(manifest, "prepared")
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        failure = helper.DatabaseCapacityCleanupFailure(
            "injected capacity cleanup failure"
        )
        with mock.patch.object(sys, "argv", self.helper_main_argv(manifest)), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper, "validate_current_schema_database", return_value=tuple(compiled_ledger(123))
        ), mock.patch.object(
            helper, "reserve_database_capacity", side_effect=failure
        ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
            helper, "release_claim"
        ) as release, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)

        launchctl.assert_not_called()
        release.assert_not_called()
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertIsNone(status.get("recovery_mode"))
        self.assertFalse(status["finalization_pending"])
        self.assertIsNone(status["rollback_failure"])
        self.assertIn("capacity cleanup failure", status["cleanup_failure"])

    def test_cleanup_main_rejects_symlinked_manifest_before_loading(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        helper.write_status(manifest, "precondition_cleanup_pending")
        original_status = Path(manifest.status_path).read_bytes()
        manifest_target = self.root / "manifest-target.json"
        manifest_target.write_text(json.dumps(dataclasses.asdict(manifest)))
        manifest_path = self.root / "manifest.json"
        manifest_path.symlink_to(manifest_target)
        argv = self.helper_main_argv(manifest)
        argv[1] = "cleanup-current"

        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            helper, "cleanup_precondition_capacity"
        ) as cleanup, mock.patch.object(
            helper, "request_helper_bootout"
        ) as bootout:
            self.assertEqual(helper.main(), 1)

        cleanup.assert_not_called()
        bootout.assert_not_called()
        self.assertTrue(manifest_path.is_symlink())
        self.assertEqual(Path(manifest.status_path).read_bytes(), original_status)
        self.assertEqual(claim.read_text(), manifest.transaction_id)

    def test_cleanup_precondition_rejects_symlinked_status_without_mutation(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        helper.write_status(manifest, "precondition_cleanup_pending")
        status_path = Path(manifest.status_path)
        status_target = status_path.with_name("status-target.json")
        status_path.rename(status_target)
        status_path.symlink_to(status_target)

        with mock.patch.object(
            helper, "validate_manifest_identities"
        ), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper, "_remove_capacity_files") as remove:
            with self.assertRaisesRegex(
                helper.CleanupPreconditionRejected,
                "regular non-symlink status",
            ):
                helper.cleanup_precondition_capacity(
                    manifest, current_schema=True
                )

        remove.assert_not_called()
        self.assertTrue(status_path.is_symlink())
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        self.assertEqual(
            json.loads(status_target.read_text())["state"],
            "precondition_cleanup_pending",
        )

    def test_cleanup_precondition_terminalizes_without_runtime_mutation(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        current = manifest.current_schema_upgrade
        assert current is not None
        backup = Path(current.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        evidence = backup.parent / "capacity.json"
        leftovers = (
            backup.parent / ".capacity.json.crash",
            backup.parent / f".{Path(current.proof_path).name}.crash",
            backup.with_name(backup.name + "-journal"),
            restore.with_name(restore.name + "-wal"),
        )
        for path in (backup, restore, evidence, *leftovers):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("capacity")
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        self.write_current_status(
            manifest,
            "precondition_cleanup_pending",
            failure="reservation failed",
            cleanup_failure="directory fsync uncertain",
        )

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
            helper, "restore"
        ) as restore_database, mock.patch.object(
            helper, "commit_atomic_install"
        ) as install:
            state = helper.cleanup_precondition_capacity(
                manifest, current_schema=True
            )

        self.assertEqual(state, "precondition_failed")
        launchctl.assert_not_called()
        restore_database.assert_not_called()
        install.assert_not_called()
        for path in (backup, restore, evidence, *leftovers):
            self.assertFalse(path.exists())
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")
        self.assertEqual(status["prepared_mode"], "current_schema")
        self.assertIn("reservation failed", status["failure"])
        self.assertTrue(status["precondition_cleanup_completed"])

    def test_cleanup_precondition_requires_exact_persisted_mode(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        status_path = Path(manifest.status_path)

        for prepared_mode in (None, "paired"):
            with self.subTest(prepared_mode=prepared_mode):
                status = {
                    "transaction_id": manifest.transaction_id,
                    "source_kind": manifest.source_kind,
                    "state": "precondition_cleanup_pending",
                }
                if prepared_mode is not None:
                    status["prepared_mode"] = prepared_mode
                status_path.write_text(json.dumps(status))
                original_status = status_path.read_bytes()

                with mock.patch.object(
                    helper, "validate_manifest_identities"
                ), mock.patch.object(
                    helper, "validate_manifest_mode"
                ), mock.patch.object(helper, "_remove_capacity_files") as remove:
                    with self.assertRaisesRegex(
                        helper.CleanupPreconditionRejected,
                        "status source kind or prepared mode",
                    ):
                        helper.cleanup_precondition_capacity(
                            manifest, current_schema=True
                        )

                remove.assert_not_called()
                self.assertEqual(status_path.read_bytes(), original_status)
                self.assertEqual(claim.read_text(), manifest.transaction_id)

    def test_cleanup_precondition_failure_retains_pending_state_and_claim(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        self.write_current_status(
            manifest,
            "precondition_cleanup_pending",
            failure="reservation failed",
            cleanup_failure="first cleanup failed",
        )

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper,
            "_remove_capacity_files",
            side_effect=helper.DatabaseCapacityCleanupFailure("fsync still uncertain"),
        ), mock.patch.object(helper, "Launchctl") as launchctl:
            with self.assertRaisesRegex(
                helper.DatabaseCapacityCleanupFailure, "fsync still uncertain"
            ):
                helper.cleanup_precondition_capacity(manifest, current_schema=True)

        launchctl.assert_not_called()
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertIn("fsync still uncertain", status["cleanup_failure"])

    def test_cleanup_enumeration_failure_retains_pending_state_and_claim(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        self.write_current_status(
            manifest,
            "precondition_cleanup_pending",
            failure="reservation failed",
        )

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper.os, "scandir", side_effect=PermissionError("enumeration denied")
        ), mock.patch.object(helper, "Launchctl") as launchctl:
            with self.assertRaisesRegex(
                helper.DatabaseCapacityCleanupFailure,
                "could not enumerate atomic evidence temporaries.*enumeration denied",
            ):
                helper.cleanup_precondition_capacity(manifest, current_schema=True)

        launchctl.assert_not_called()
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertIn("enumeration denied", status["cleanup_failure"])

    def test_cleanup_main_preserves_original_failure_when_retry_fails(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        self.write_current_status(
            manifest,
            "precondition_cleanup_pending",
            failure="reservation failed",
            cleanup_failure="first cleanup failed",
        )
        argv = self.helper_main_argv(manifest)
        argv[1] = "cleanup-current"

        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(
            helper, "validate_manifest_identities"
        ), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(
            helper,
            "_remove_capacity_files",
            side_effect=helper.DatabaseCapacityCleanupFailure("fsync still uncertain"),
        ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)

        launchctl.assert_not_called()
        self.assertEqual(claim.read_text(), manifest.transaction_id)
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertEqual(status["failure"], "reservation failed")
        self.assertIn("fsync still uncertain", status["cleanup_failure"])

    def test_recover_current_rejects_precondition_cleanup_without_runtime_stop(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(manifest, "precondition_cleanup_pending")

        with mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper, "Launchctl") as launchctl:
            with self.assertRaisesRegex(
                helper.ActivationError,
                "retained unresolved transaction",
            ):
                helper.recover_paired(manifest, current_schema=True)

        launchctl.assert_not_called()
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertEqual(
            Path(manifest.active_path).read_text(), manifest.transaction_id
        )

    def test_recover_current_main_preserves_cleanup_only_state(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        Path(manifest.active_path).write_text(manifest.transaction_id)
        helper.write_status(manifest, "precondition_cleanup_pending")
        argv = self.helper_main_argv(manifest)
        argv[1] = "recover-current"

        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(helper, "validate_manifest_identities"), mock.patch.object(
            helper, "validate_manifest_mode"
        ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
            helper, "request_helper_bootout"
        ):
            self.assertEqual(helper.main(), 1)

        launchctl.assert_not_called()
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_cleanup_pending")
        self.assertIsNone(status["rollback_failure"])
        self.assertEqual(
            Path(manifest.active_path).read_text(), manifest.transaction_id
        )

    def test_rejected_cleanup_command_preserves_status_and_claim(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)

        for command, state in (
            ("cleanup-paired", "precondition_cleanup_pending"),
            ("cleanup-current", "migration_started"),
        ):
            with self.subTest(command=command, state=state):
                helper.write_status(manifest, state)
                original_status = Path(manifest.status_path).read_bytes()
                argv = self.helper_main_argv(manifest)
                argv[1] = command

                with mock.patch.object(sys, "argv", argv), mock.patch.object(
                    helper.Manifest, "load", return_value=manifest
                ), mock.patch.object(
                    helper, "validate_manifest_identities"
                ), mock.patch.object(
                    helper, "validate_manifest_mode"
                ), mock.patch.object(helper, "Launchctl") as launchctl, mock.patch.object(
                    helper, "request_helper_bootout"
                ):
                    self.assertEqual(helper.main(), 1)

                launchctl.assert_not_called()
                self.assertEqual(
                    Path(manifest.status_path).read_bytes(), original_status
                )
                self.assertEqual(claim.read_text(), manifest.transaction_id)

    def test_invalid_helper_label_cannot_request_bootout(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        helper.write_status(manifest, "migration_started")
        original_status = Path(manifest.status_path).read_bytes()

        invalid_manifest = dataclasses.replace(
            manifest, helper_label=manifest.label
        )
        cases = (
            ("cli-label", manifest, "--helper-label", manifest.label),
            ("cli-uid", manifest, "--uid", "-1"),
            ("manifest-label", invalid_manifest, None, None),
        )
        for name, loaded_manifest, field, replacement in cases:
            with self.subTest(name=name):
                argv = self.helper_main_argv(loaded_manifest)
                argv[1] = "cleanup-current"
                if field is not None and replacement is not None:
                    argv[argv.index(field) + 1] = replacement

                with mock.patch.object(sys, "argv", argv), mock.patch.object(
                    helper.Manifest, "load", return_value=loaded_manifest
                ), mock.patch.object(
                    helper, "cleanup_precondition_capacity"
                ) as cleanup, mock.patch.object(
                    helper, "request_helper_bootout"
                ) as bootout:
                    self.assertEqual(helper.main(), 1)

                cleanup.assert_not_called()
                bootout.assert_not_called()
                self.assertEqual(
                    Path(manifest.status_path).read_bytes(), original_status
                )
                self.assertEqual(claim.read_text(), manifest.transaction_id)

    def test_cleanup_precondition_main_releases_only_matching_terminal_claim(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        claim = Path(manifest.active_path)
        claim.write_text(manifest.transaction_id)
        helper.write_status(manifest, "precondition_cleanup_pending")
        argv = self.helper_main_argv(manifest)
        argv[1] = "cleanup-current"

        def cleanup(loaded, *, current_schema):
            self.assertIs(loaded, manifest)
            self.assertTrue(current_schema)
            helper.write_status(
                manifest,
                "precondition_failed",
                precondition_cleanup_completed=True,
            )
            return "precondition_failed"

        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            helper.Manifest, "load", return_value=manifest
        ), mock.patch.object(
            helper, "cleanup_precondition_capacity", side_effect=cleanup
        ), mock.patch.object(helper, "request_helper_bootout"):
            self.assertEqual(helper.main(), 0)

        self.assertFalse(claim.exists())
        status = json.loads(Path(manifest.status_path).read_text())
        self.assertEqual(status["state"], "precondition_failed")
        self.assertTrue(status["precondition_cleanup_completed"])

    def test_checkpoint_interleaved_sample_uses_reserved_ceiling_not_stale_sample(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        stale_live_sample = 64 * 1024
        stopped_checkpoint_size = 512 * 1024
        free = (
            helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            + helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
            + 2 * 1024 * 1024
        )
        with mock.patch.object(
            helper.shutil, "disk_usage", return_value=mock.Mock(free=free)
        ), mock.patch.object(
            helper,
            "_source_capacity_bytes",
            side_effect=(stale_live_sample, stopped_checkpoint_size),
        ):
            reservation = helper.reserve_database_capacity(manifest)
            helper._reservation_still_sufficient(manifest, reservation)

        self.assertGreater(reservation.capacity_bytes, stopped_checkpoint_size)
        observation = json.loads(
            helper._capacity_observation_path(reservation).read_text()
        )
        self.assertEqual(observation["observed_source_bytes"], stale_live_sample)
        self.assertEqual(
            observation["rechecked_source_bytes"], stopped_checkpoint_size
        )
        helper.abandon_capacity_reservation(manifest, reservation)

    def test_stopped_recheck_rejects_consumed_evidence_allowance(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        observed = helper._source_capacity_bytes(database)
        free = (
            helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            + helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
            + 2 * observed
            + 3 * 1024 * 1024
        )
        with mock.patch.object(
            helper.shutil, "disk_usage", return_value=mock.Mock(free=free)
        ):
            reservation = helper.reserve_database_capacity(manifest)

        resident_evidence = helper._resident_evidence_bytes(manifest, reservation)
        exact_floor = (
            helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            + helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
            - resident_evidence
        )
        below_floor = exact_floor - 1
        with mock.patch.object(
            helper.shutil,
            "disk_usage",
            return_value=mock.Mock(free=below_floor),
        ):
            with self.assertRaisesRegex(
                helper.ActivationError,
                "remaining durable-evidence allowance",
            ):
                helper._reservation_still_sufficient(manifest, reservation)
        with mock.patch.object(
            helper.shutil,
            "disk_usage",
            return_value=mock.Mock(free=exact_floor),
        ):
            helper._reservation_still_sufficient(manifest, reservation)
        helper.abandon_capacity_reservation(manifest, reservation)

    def test_reservation_abort_removes_sqlite_sidecars_and_atomic_evidence_temps(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        current = manifest.current_schema_upgrade
        backup = Path(current.backup_path)
        restore = helper._restore_capacity_path(manifest, database)
        sidecar = backup.with_name(backup.name + "-journal")

        def fail_allocation(path, _size):
            path.touch(mode=0o600)
            sidecar.write_text("journal")
            raise helper.ActivationError("allocation failed")

        with mock.patch.object(
            helper,
            "_current_schema_reservation_plan",
            return_value=helper.DatabaseCapacityPlan(4096, 8192),
        ), mock.patch.object(
            helper, "_allocate_private_sqlite", side_effect=fail_allocation
        ):
            with self.assertRaisesRegex(helper.ActivationError, "allocation failed"):
                helper.reserve_database_capacity(manifest)
        self.assertFalse(backup.exists())
        self.assertFalse(restore.exists())
        self.assertFalse(sidecar.exists())

        atomic_temp = backup.parent / ".capacity.json.crash"

        def fail_observation(*_args, **_kwargs):
            atomic_temp.write_text("temporary evidence")
            raise helper.ActivationError("observation failed")

        with mock.patch.object(
            helper,
            "_current_schema_reservation_plan",
            return_value=helper.DatabaseCapacityPlan(4096, 8192),
        ), mock.patch.object(
            helper, "_require_current_schema_free_space"
        ), mock.patch.object(
            helper, "_write_capacity_observation", side_effect=fail_observation
        ):
            with self.assertRaisesRegex(
                helper.ActivationError, "could not persist paired capacity observation"
            ):
                helper.reserve_database_capacity(manifest)
        self.assertFalse(atomic_temp.exists())

    def test_atomic_evidence_floor_uses_minimum_free_space_for_shared_pool(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        reservation = self.migration_reservation(manifest)
        floor = helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
        backup_parent = reservation.backup_path.parent

        def usage(path):
            free = (
                floor + helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
                if Path(path) == backup_parent
                else floor
            )
            return mock.Mock(free=free)

        with mock.patch.object(
            helper, "_capacity_pool_key", return_value=("apfs", "shared")
        ), mock.patch.object(helper.shutil, "disk_usage", side_effect=usage):
            with self.assertRaisesRegex(
                helper.ActivationError, "remaining durable-evidence allowance"
            ):
                helper._require_current_schema_free_space(manifest, reservation)

    def test_initial_capacity_evidence_is_not_written_before_free_space_proof(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        current = manifest.current_schema_upgrade
        backup = Path(current.backup_path)
        restore = helper._restore_capacity_path(manifest, database)

        with mock.patch.object(
            helper,
            "_current_schema_reservation_plan",
            return_value=helper.DatabaseCapacityPlan(4096, 8192),
        ), mock.patch.object(
            helper,
            "_require_current_schema_free_space",
            side_effect=helper.ActivationError("capacity floor unavailable"),
        ), mock.patch.object(
            helper, "_write_capacity_observation"
        ) as write_observation:
            with self.assertRaisesRegex(
                helper.ActivationError, "could not persist paired capacity observation"
            ):
                helper.reserve_database_capacity(manifest)

        write_observation.assert_not_called()
        self.assertFalse(backup.exists())
        self.assertFalse(restore.exists())

    def test_stopped_capacity_evidence_is_not_rewritten_before_free_space_proof(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        reservation = self.migration_reservation(manifest)

        with mock.patch.object(
            helper,
            "_require_current_schema_free_space",
            side_effect=helper.ActivationError("capacity floor unavailable"),
        ), mock.patch.object(
            helper, "_write_capacity_observation"
        ) as write_observation:
            with self.assertRaisesRegex(
                helper.ActivationError, "capacity floor unavailable"
            ):
                helper._reservation_still_sufficient(manifest, reservation)

        write_observation.assert_not_called()

    def test_current_schema_capacity_rejection_persists_both_measurements(self) -> None:
        database = self.database()
        manifest = self.manifest(database)
        observed = helper._source_capacity_bytes(database)
        free = (
            helper.MINIMUM_FREE_AFTER_RESERVATION_BYTES
            + helper.CURRENT_SCHEMA_EVIDENCE_BUDGET_BYTES
            + 3 * (observed + 1024 * 1024)
        )
        with mock.patch.object(
            helper.shutil, "disk_usage", return_value=mock.Mock(free=free)
        ):
            reservation = helper.reserve_database_capacity(manifest)

        required = reservation.capacity_bytes + 4096
        with mock.patch.object(
            helper, "_source_capacity_bytes", return_value=required
        ):
            with self.assertRaisesRegex(
                helper.ActivationError, "grew beyond its reserved capacity"
            ):
                helper._reservation_still_sufficient(manifest, reservation)

        observation = json.loads(
            helper._capacity_observation_path(reservation).read_text()
        )
        self.assertEqual(observation["observed_source_bytes"], observed)
        self.assertEqual(
            observation["reserved_capacity_bytes"], reservation.capacity_bytes
        )
        self.assertEqual(
            observation["reservation_ceiling_bytes"], reservation.ceiling_bytes
        )
        self.assertEqual(observation["rechecked_source_bytes"], required)
        self.assertIsNotNone(observation["rechecked_at"])
        helper.abandon_capacity_reservation(manifest, reservation)

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
        self.assertEqual(allocations, [])




class CurrentSchemaRepairTests(unittest.TestCase):
    """Exact forward replacement; no production or real launchctl invocation."""

    setUp = CurrentSchemaPreparedUpgradeTests.setUp
    database = CurrentSchemaPreparedUpgradeTests.database

    def sealed(self):
        database = self.database(through=128)
        parent = CurrentSchemaPreparedUpgradeTests.manifest(self, database)
        directory = Path(parent.current_schema_upgrade.proof_path).parent
        helper.atomic_write(directory / "original-candidate", b"original", 0o700)
        plist = plistlib.dumps({"Label": parent.label, "ProgramArguments": [str(self.root / "installed")],
                               "EnvironmentVariables": {"PHOENIX_DB_PATH": str(database), "CAPTURED": "exact"}})
        helper.atomic_write(directory / "candidate.plist", plist)
        current = helper.PreparedSource114Upgrade(**dataclasses.asdict(parent.current_schema_upgrade),
                                                prepared_source_baseline=114,
                                                candidate_registry_expected_sha256=helper.PREPARED_SOURCE114_REGISTRY_SHA256)
        parent = dataclasses.replace(parent, source_commit=helper.PREPARED_SOURCE114_COMMIT,
                                     expected=helper.Identity(helper.CURRENT_REPAIR_VERSION, helper.PREPARED_SOURCE114_COMMIT),
                                     candidate_binary=str(directory / "original-candidate"),
                                     candidate_binary_sha256=helper.sha256(directory / "original-candidate"),
                                     candidate_plist=str(directory / "candidate.plist"), candidate_plist_sha256=helper.sha256(directory / "candidate.plist"),
                                     current_schema_upgrade=current, target_binary=str(self.root / "installed"))
        helper.atomic_write(Path(parent.target_binary), (directory / "original-candidate").read_bytes(), 0o755)
        helper.atomic_write(Path(parent.target_plist), plist)
        for name in ("helper.py", "migrations.rs"):
            (directory / name).chmod(0o600)
        backup = Path(current.backup_path)
        shutil.copyfile(database, backup)
        backup.chmod(0o600)
        proof = json.loads(helper._database_proof_bytes(parent, database, backup_sha256=helper.sha256(backup),
                                                      source_logical_digest=helper.database_logical_digest(database),
                                                      backup_logical_digest=helper.database_logical_digest(backup)))
        proof.update(migration_committed_at="2026-10-18T00:00:00+00:00", migrated_ledger=compiled_ledger(),
                     migrated_logical_digest=helper.database_logical_digest(database))
        helper.atomic_write(Path(current.proof_path), helper._encode_database_proof(proof))
        helper.atomic_write(directory / "manifest.json", json.dumps(dataclasses.asdict(parent)).encode())
        helper.write_status(parent, "committed", finalization_pending=True)
        helper.atomic_write(Path(parent.active_path), (parent.transaction_id + "\n").encode())
        restore = helper._restore_capacity_path(parent, database)
        helper._allocate_private_sqlite(restore, database.stat().st_size * 2)
        child = directory / helper.CURRENT_REPAIR_DIRECTORY
        child.mkdir(mode=0o700)
        helper.atomic_write(child / "candidate", b"repair", 0o700)
        helper.atomic_write(child / "candidate.plist", plist)
        helper.atomic_write(child / "parent-status.json", Path(parent.status_path).read_bytes())
        helper.atomic_write(child / "helper.py", Path(helper.__file__).read_bytes())
        helper.atomic_write(child / "migrations.rs", self.registry.read_bytes())
        target = "aarch64-apple-darwin"
        receipt = {"schema": 1, "operation": "prepare-main", "target": target,
                   "commit": helper.CURRENT_REPAIR_COMMIT, "version": helper.CURRENT_REPAIR_VERSION,
                   "checks": {"developer_id_signature": "verified", "hardened_runtime": "verified", "notarization": "accepted",
                              "stapled_ticket": "validated", "gatekeeper": "accepted", "embedded_helper_bytes": "identical",
                              "notarization_submission_id": "00000000-0000-0000-0000-000000000001"},
                   "sha256": {f"phoenix_ide-{target}-prepared-{helper.CURRENT_REPAIR_COMMIT[:12]}": helper.sha256(child / "candidate")}}
        helper.atomic_write(child / "receipt.json", json.dumps(receipt).encode())
        repair = helper.CurrentSchemaRepairManifest(
            1, str(directory / "manifest.json"), helper.sha256(directory / "manifest.json"),
            helper.sha256(Path(parent.status_path)), helper.sha256(Path(current.proof_path)),
            helper.CURRENT_REPAIR_COMMIT, helper.CURRENT_REPAIR_VERSION, "d" * 40,
            helper.sha256(child / "helper.py"), helper.sha256(child / "candidate"), helper.sha256(child / "migrations.rs"),
            helper.sha256(child / "candidate.plist"), helper.sha256(child / "receipt.json"), f"{parent.helper_label}.current-schema-repair")
        path = child / "manifest.json"
        helper.atomic_write(path, json.dumps(dataclasses.asdict(repair)).encode())
        status = helper.CurrentSchemaRepairStatus(1, helper.sha256(path), "prepared", None, None, False, None)
        helper._write_repair_status(path, status)
        helper.atomic_write(Path(parent.target_binary), b"", 0o600)
        Path(parent.target_plist).unlink()
        self.parent, self.repair, self.path = parent, repair, path
        return path

    def environment(self):
        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        self.events = []
        self.launchctl = mock.Mock()
        self.launchctl.stop.side_effect = lambda: self.events.append("stop")
        self.launchctl.start.side_effect = lambda *a, **kw: self.events.append(("start", kw["plist_path"]))
        expected_label = self.parent.label
        expected_target_plist = Path(self.parent.target_plist)

        def require_persistent_route(parent):
            if parent.label != expected_label or Path(parent.target_plist) != expected_target_plist:
                raise helper.ActivationError("repair parent persistent route differs from the sealed route")

        stack.enter_context(
            mock.patch.object(helper, "_require_current_repair_persistent_route", side_effect=require_persistent_route)
        )
        stack.enter_context(mock.patch.object(helper, "__file__", str(self.path.parent / "helper.py")))
        stack.enter_context(mock.patch.object(helper, "Launchctl", return_value=self.launchctl))
        stack.enter_context(mock.patch.object(helper, "assert_database_exclusive", side_effect=lambda p: self.events.append("exclusive")))
        stack.enter_context(mock.patch.object(helper, "require_launchd_absence", side_effect=lambda *a: self.events.append(("absent", a[0]))))
        stack.enter_context(mock.patch.object(helper, "require_loaded_plist", side_effect=lambda *a: self.events.append("loaded")))
        stack.enter_context(mock.patch.object(helper, "wait_for_identity", side_effect=lambda p, i: self.events.append(("identity", i.git_sha))))

        def run(args, **kwargs):
            self.events.append(tuple(args))
            if args[-1] == "--build-identity":
                return subprocess.CompletedProcess(args, 0, json.dumps({"version": helper.CURRENT_REPAIR_VERSION, "git_sha": helper.CURRENT_REPAIR_COMMIT}), "")
            if args[-1] == "--migrate-only":
                status = helper.CurrentSchemaRepairStatus.load(self.path.parent / "status.json", helper.sha256(self.path))
                self.assertEqual(status.state, "reconciliation_started")
                self.assertEqual(helper.sha256(Path(args[0])), self.repair.candidate_sha256)
                self.assertEqual(kwargs["env"]["CAPTURED"], "exact")
                return subprocess.CompletedProcess(args, 0)
            raise AssertionError(f"unexpected subprocess {args}")
        self.run = stack.enter_context(mock.patch.object(helper.subprocess, "run", side_effect=run))
        self.restore = stack.enter_context(mock.patch.object(helper, "restore", side_effect=AssertionError("predecessor forbidden")))
        self.restore_db = stack.enter_context(mock.patch.object(helper, "restore_database", side_effect=AssertionError("snapshot replay forbidden")))
        return stack

    def status(self):
        return helper.CurrentSchemaRepairStatus.load(self.path.parent / "status.json", helper.sha256(self.path))

    def test_exact_success_parent_immutable_and_terminal_release_last(self):
        self.sealed()
        immutable = {p: p.read_bytes() for p in (Path(self.repair.parent_manifest_path), Path(self.parent.status_path), Path(self.parent.current_schema_upgrade.proof_path))}
        self.environment()
        release = helper.release_claim
        def release_checked(parent):
            self.assertEqual(self.status().state, "committed")
            self.assertFalse(self.status().finalization_pending)
            self.assertEqual(Path(parent.deployed_sha_path).read_text().strip(), helper.CURRENT_REPAIR_COMMIT)
            self.assertEqual(helper.sha256(Path(parent.target_plist)), self.repair.plist_sha256)
            self.assertFalse(helper._restore_capacity_path(parent).exists())
            self.assertTrue((self.path.parent / "proof.json").is_file())
            self.assertTrue(Path(parent.current_schema_upgrade.backup_path).is_file())
            self.events.append("release")
            return release(parent)
        with mock.patch.object(helper, "release_claim", side_effect=release_checked):
            self.assertEqual(helper.activate_current_repair(self.path), "committed")
        self.assertEqual(self.events[-1], "release")
        self.assertFalse(Path(self.parent.active_path).exists())
        for path, content in immutable.items():
            self.assertEqual(path.read_bytes(), content)
        self.assertEqual([e for e in self.events if isinstance(e, tuple) and e[-1] == "--migrate-only"], [(self.parent.target_binary, "--migrate-only")])
        self.assertIn(("start", str(self.path.parent / "candidate.plist")), self.events)
        self.restore.assert_not_called()
        self.restore_db.assert_not_called()

    def test_terminal_resume_is_runtime_and_database_mutation_free(self):
        self.sealed()
        self.environment()
        helper.activate_current_repair(self.path)
        self.events.clear()
        with mock.patch.object(helper, "atomic_install", side_effect=AssertionError("terminal install")), mock.patch.object(helper, "_repair_verify_database", side_effect=AssertionError("terminal database")):
            self.assertEqual(helper.activate_current_repair(self.path), "committed")
        self.launchctl.stop.assert_not_called()
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)

    def test_parent_claim_status_and_proof_admission(self):
        self.sealed()
        parent_path = Path(self.repair.parent_manifest_path)
        helper.validate_current_repair_parent(parent_path)
        for key, value in (("state", "migration_started"), ("finalization_pending", False), ("finalization_pending", 1), ("expected_git_sha", "a" * 40)):
            with self.subTest(key=key, value=value):
                old = Path(self.parent.status_path).read_bytes()
                raw = json.loads(old)
                raw[key] = value
                helper.atomic_write(Path(self.parent.status_path), json.dumps(raw).encode())
                with self.assertRaises(helper.ActivationError):
                    helper.validate_current_repair_parent(parent_path)
                helper.atomic_write(Path(self.parent.status_path), old)
        helper.atomic_write(Path(self.parent.active_path), b"other\n")
        with self.assertRaises(helper.ActivationError):
            helper.validate_current_repair_parent(parent_path)

    def test_sealed_evidence_crosswires_fail_closed(self):
        self.sealed()
        for filename in ("candidate", "helper.py", "receipt.json", "migrations.rs", "candidate.plist", "parent-status.json"):
            with self.subTest(filename=filename):
                path = self.path.parent / filename
                old = path.read_bytes()
                helper.atomic_write(path, b"crosswired")
                with self.assertRaises(helper.ActivationError):
                    helper.validate_current_repair(self.path, running_helper=False)
                helper.atomic_write(path, old, 0o700 if filename == "candidate" else 0o600)
        for filename in ("candidate", "helper.py", "receipt.json"):
            with self.subTest(symlink=filename):
                path = self.path.parent / filename
                old = path.read_bytes()
                path.unlink()
                path.symlink_to(Path(self.parent.candidate_binary))
                with self.assertRaises(helper.ActivationError):
                    helper.validate_current_repair(self.path, running_helper=False)
                path.unlink()
                helper.atomic_write(path, old)

    def test_manifest_closed_types_candidate_and_controller_pin(self):
        self.sealed()
        original = json.loads(self.path.read_text())
        for key, value in (("protocol_version", True), ("extra", "forbidden"), ("source_commit", helper.PREPARED_SOURCE114_COMMIT),
                           ("controller_source_commit", helper.CURRENT_REPAIR_COMMIT), ("registry_sha256", "0" * 64),
                           ("parent_manifest_path", "relative/manifest.json")):
            with self.subTest(key=key):
                raw = dict(original, **{key: value})
                helper.atomic_write(self.path, json.dumps(raw).encode())
                with self.assertRaises(helper.ActivationError):
                    helper.CurrentSchemaRepairManifest.load(self.path)
        helper.atomic_write(self.path, json.dumps(original).encode())

    def test_status_closed_state_consistency(self):
        self.sealed()
        for fields in ({"extra": 1}, {"state": "other"}, {"state": "target_verified"}, {"finalization_pending": True},
                       {"state": "failed_stopped"}, {"resume_from": "prepared"}, {"protocol_version": True},
                       {"manifest_sha256": "0" * 64}, {"proof_sha256": "a" * 64}):
            with self.subTest(fields=fields):
                raw = dict(dataclasses.asdict(self.status()), **fields)
                helper.atomic_write(self.path.parent / "status.json", json.dumps(raw).encode())
                with self.assertRaises(helper.ActivationError):
                    self.status()
                helper._write_repair_status(self.path, helper.CurrentSchemaRepairStatus(1, helper.sha256(self.path), "prepared", None, None, False, None))

    def test_selected_child_blocks_original_finalize_and_recovery(self):
        self.sealed()
        with mock.patch.object(helper, "Launchctl") as launchctl:
            for operation in (helper.finalize_paired, helper.recover_paired):
                with self.assertRaisesRegex(helper.ActivationError, "resume-current-repair"):
                    operation(self.parent, current_schema=True)
            self.assertFalse(helper.status_is_durable_terminal(self.parent))
            launchctl.assert_not_called()
        helper.atomic_write(self.path, b"malformed")
        with self.assertRaises(helper.ActivationError):
            helper.finalize_paired(self.parent, current_schema=True)

    def test_install_failure_stops_retains_and_retries_same_child(self):
        self.sealed()
        self.environment()
        with mock.patch.object(helper, "atomic_install", side_effect=OSError("install failed")):
            with self.assertRaises(OSError):
                helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "prepared"))
        self.assertTrue(Path(self.parent.active_path).exists())
        self.assertEqual(helper.activate_current_repair(self.path), "committed")

    def test_migration_failure_keeps_irreversible_checkpoint(self):
        self.sealed()
        self.environment()
        original_run = self.run.side_effect
        self.run.side_effect = lambda args, **kwargs: subprocess.CompletedProcess(args, 9) if args[-1] == "--migrate-only" else original_run(args, **kwargs)
        with self.assertRaises(helper.ActivationError):
            helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "reconciliation_started"))
        self.assertTrue(Path(self.parent.active_path).exists())
        self.restore.assert_not_called()
        self.restore_db.assert_not_called()

    def test_start_failure_resume_does_not_repeat_reconciliation(self):
        self.sealed()
        self.environment()
        self.launchctl.start.side_effect = helper.ActivationError("start")
        with self.assertRaises(helper.ActivationError):
            helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "start_attempted"))
        self.launchctl.start.side_effect = None
        self.events.clear()
        helper.activate_current_repair(self.path)
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)

    def test_health_and_teardown_uncertainty_retains_claim(self):
        self.sealed()
        self.environment()
        stopped = [False]
        def stop():
            stopped[0] = True
            raise helper.ActivationError("teardown uncertain")
        def absence(label, uid):
            if stopped[0] and label == self.parent.label:
                raise helper.ActivationError("absence uncertain")
        self.launchctl.stop.side_effect = stop
        with mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("health")), \
             mock.patch.object(helper, "require_launchd_absence", side_effect=absence):
            with self.assertRaises(helper.ActivationError):
                helper.activate_current_repair(self.path)
        self.assertEqual(self.status().state, "stop_unconfirmed")
        self.assertEqual(self.status().resume_from, "start_attempted")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_database_target_verification_is_required_after_reconciliation(self):
        self.sealed()
        self.environment()
        original_run = self.run.side_effect
        def run(args, **kwargs):
            result = original_run(args, **kwargs)
            if args[-1] == "--migrate-only":
                with sqlite3.connect(self.parent.current_schema_upgrade.database_path) as connection:
                    connection.execute("DELETE FROM _migrations WHERE version = 128")
            return result
        self.run.side_effect = run
        with self.assertRaises(helper.ActivationError):
            helper.activate_current_repair(self.path)
        self.assertEqual(self.status().resume_from, "reconciliation_started")
        self.launchctl.start.assert_not_called()

    def test_publication_failure_stops_then_resumes_without_reconciliation(self):
        self.sealed()
        self.environment()
        with mock.patch.object(helper, "_publish_current_repair", side_effect=OSError("publication")):
            with self.assertRaises(OSError):
                helper.activate_current_repair(self.path)
        self.assertEqual(self.status().resume_from, "committed")
        self.events.clear()
        helper.activate_current_repair(self.path)
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)

    def test_post_publication_failure_removes_persistent_repair_plist_before_stopped_status(self):
        self.sealed()
        self.environment()
        restore = helper._restore_capacity_path(self.parent)
        real_fsync = helper.fsync_dir
        failed = [False]

        def fsync(directory):
            if directory == restore.parent and not restore.exists() and not failed[0]:
                failed[0] = True
                raise OSError("restore cleanup fsync")
            return real_fsync(directory)

        with mock.patch.object(helper, "fsync_dir", side_effect=fsync):
            with self.assertRaisesRegex(OSError, "restore cleanup fsync"):
                helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "committed"))
        self.assertFalse(Path(self.parent.target_plist).exists())
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_persistence_fence_failure_cannot_report_failed_stopped(self):
        self.sealed()
        self.environment()
        helper.atomic_write(Path(self.parent.target_plist), (self.path.parent / "candidate.plist").read_bytes())
        real_fsync = helper.fsync_dir

        def fsync(directory):
            if directory == Path(self.parent.target_plist).parent:
                raise OSError("plist directory fsync")
            return real_fsync(directory)

        with mock.patch.object(helper, "fsync_dir", side_effect=fsync):
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual((self.status().state, self.status().resume_from), ("stop_unconfirmed", "prepared"))
        self.assertFalse(Path(self.parent.target_plist).exists())
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_persistence_cleanup_isolates_path_before_hashing_and_preserves_swapped_bytes(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)
        helper.atomic_write(target, (self.path.parent / "candidate.plist").read_bytes())
        real_rename = helper.os.rename

        def rename(source, destination):
            target.write_bytes(b"swapped before isolation")
            return real_rename(source, destination)

        with mock.patch.object(helper.os, "rename", side_effect=rename):
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "prepared"))
        self.assertFalse(target.exists())
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 1)
        self.assertEqual(quarantines[0].read_bytes(), b"swapped before isolation")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_interrupted_exact_quarantine_is_admitted_without_deleting_evidence(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)
        helper.atomic_write(target, (self.path.parent / "candidate.plist").read_bytes())
        quarantine = target.parent / f"{helper._current_repair_quarantine_prefix(self.parent)}crash"
        helper.os.rename(target, quarantine)
        helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "prepared"))
        self.assertFalse(target.exists())
        self.assertEqual(helper._current_repair_quarantines(self.parent), [quarantine])
        self.assertTrue(helper._current_repair_persistence_owned(self.repair, self.parent))

    def test_unknown_published_plist_is_quarantined_and_blocks_resume(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)
        helper.atomic_write(target, b"not the repair plist")
        helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "prepared"))
        self.assertFalse(target.exists())
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 1)
        self.assertEqual(quarantines[0].read_bytes(), b"not the repair plist")
        self.assertTrue(Path(self.parent.active_path).exists())

        with self.assertRaisesRegex(helper.ActivationError, "persistent target plist"):
            helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "prepared"))
        self.assertFalse(target.exists())
        self.assertEqual(helper._current_repair_quarantines(self.parent), quarantines)

    def test_quarantine_enumeration_failure_blocks_resume(self):
        self.sealed()
        self.environment()
        with mock.patch.object(helper.os, "scandir", side_effect=PermissionError("quarantine lookup")):
            with self.assertRaisesRegex(helper.ActivationError, "persistent target plist"):
                helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "prepared"))
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_unknown_plist_appearing_during_publication_is_not_overwritten(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)
        real_link = helper.os.link
        calls = [0]

        def link(source, destination):
            result = real_link(source, destination)
            calls[0] += 1
            if calls[0] == 1:
                helper.atomic_write(target, b"appeared during health verification")
            return result

        with mock.patch.object(helper.os, "link", side_effect=link):
            with self.assertRaisesRegex(helper.ActivationError, "unknown bytes during publication"):
                helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "committed"))
        self.assertFalse(target.exists())
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 1)
        self.assertEqual(quarantines[0].read_bytes(), b"appeared during health verification")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_plist_recreated_during_stop_is_quarantined_before_stopped_status(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)

        def stop():
            helper.atomic_write(target, b"recreated during stop")

        self.launchctl.stop.side_effect = stop
        helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "prepared"))
        self.assertFalse(target.exists())
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 1)
        self.assertEqual(quarantines[0].read_bytes(), b"recreated during stop")

    def test_service_reappearance_during_post_stop_quarantine_is_unconfirmed(self):
        self.sealed()
        self.environment()
        absence_checks = [0]

        def require_absence(*_args):
            absence_checks[0] += 1
            if absence_checks[0] == 2:
                raise helper.ActivationError("repair restarted")

        with mock.patch.object(helper, "require_launchd_absence", side_effect=require_absence):
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual(absence_checks[0], 2)
        self.assertEqual(self.status().state, "stop_unconfirmed")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_final_process_absence_probe_overrides_transient_first_probe(self):
        self.sealed()
        self.environment()
        absence_checks = [0]

        def require_absence(*_args):
            absence_checks[0] += 1
            if absence_checks[0] == 1:
                raise helper.ActivationError("transient absence probe failure")

        with mock.patch.object(helper, "require_launchd_absence", side_effect=require_absence):
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual(absence_checks[0], 2)
        self.assertEqual(self.status().state, "failed_stopped")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_target_recreated_during_final_absence_probe_is_quarantined(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)
        absence_checks = [0]

        def require_absence(*_args):
            absence_checks[0] += 1
            if absence_checks[0] == 2:
                helper.atomic_write(target, b"recreated during final absence probe")

        with mock.patch.object(helper, "require_launchd_absence", side_effect=require_absence):
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual(absence_checks[0], 2)
        self.assertEqual(self.status().state, "failed_stopped")
        self.assertFalse(target.exists())
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 1)
        self.assertEqual(quarantines[0].read_bytes(), b"recreated during final absence probe")

    def test_failure_settlement_never_opens_quarantined_evidence(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)
        helper.atomic_write(target, b"untrusted evidence")
        with mock.patch.object(helper, "_regular_file_digest", side_effect=AssertionError("opened evidence")):
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual(self.status().state, "failed_stopped")
        self.assertFalse(target.exists())
        self.assertEqual(helper._current_repair_quarantines(self.parent)[0].read_bytes(), b"untrusted evidence")

    def test_continuous_target_recreation_is_bounded_and_stop_unconfirmed(self):
        self.sealed()
        self.environment()
        target = Path(self.parent.target_plist)
        helper.atomic_write(target, b"first")
        real_rename = helper.os.rename
        rename_count = [0]

        def rename(source, destination):
            real_rename(source, destination)
            rename_count[0] += 1
            helper.atomic_write(target, f"recreated-{rename_count[0]}".encode())

        with mock.patch.object(helper, "CURRENT_REPAIR_QUARANTINE_ATTEMPTS", 2), mock.patch.object(
            helper.os, "rename", side_effect=rename
        ):
            helper._record_current_repair_failure(self.path, self.parent, self.launchctl)
        self.assertEqual(rename_count[0], 6)
        self.assertEqual(self.status().state, "stop_unconfirmed")
        self.assertTrue(target.exists())
        self.assertEqual(len(helper._current_repair_quarantines(self.parent)), 6)
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_quarantine_digest_refuses_symlink_fifo_oversize_and_mutation(self):
        self.sealed()
        target = Path(self.parent.target_plist)
        evidence = target.parent / f"{helper._current_repair_quarantine_prefix(self.parent)}unsafe"
        candidate = self.path.parent / "candidate.plist"

        evidence.symlink_to(candidate)
        self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        evidence.unlink()

        os.mkfifo(evidence)
        self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        evidence.unlink()

        with evidence.open("wb") as stream:
            stream.truncate(helper.CURRENT_REPAIR_PLIST_EVIDENCE_MAX_BYTES + 1)
        self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        evidence.unlink()

        helper.atomic_write(evidence, candidate.read_bytes())
        real_fstat = helper.os.fstat
        fstat_calls = [0]

        def changing_fstat(fd):
            metadata = real_fstat(fd)
            fstat_calls[0] += 1
            if fstat_calls[0] == 2:
                return mock.Mock(
                    st_dev=metadata.st_dev,
                    st_ino=metadata.st_ino,
                    st_mode=metadata.st_mode,
                    st_size=metadata.st_size,
                    st_mtime_ns=metadata.st_mtime_ns + 1,
                    st_ctime_ns=metadata.st_ctime_ns,
                )
            return metadata

        with mock.patch.object(helper.os, "fstat", side_effect=changing_fstat):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))

    def test_quarantine_path_replacement_after_open_blocks_resume(self):
        self.sealed()
        evidence = Path(self.parent.target_plist).parent / f"{helper._current_repair_quarantine_prefix(self.parent)}swap"
        helper.atomic_write(evidence, (self.path.parent / "candidate.plist").read_bytes())
        real_read = helper.os.read
        swapped = [False]

        def read_and_swap(fd, size):
            chunk = real_read(fd, size)
            if chunk and not swapped[0]:
                swapped[0] = True
                helper.atomic_write(evidence, b"replacement evidence")
            return chunk

        with mock.patch.object(helper.os, "read", side_effect=read_and_swap):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        self.assertTrue(swapped[0])
        self.assertEqual(evidence.read_bytes(), b"replacement evidence")

    def test_quarantine_replacement_between_digest_passes_blocks_resume(self):
        self.sealed()
        evidence = Path(self.parent.target_plist).parent / f"{helper._current_repair_quarantine_prefix(self.parent)}swap"
        helper.atomic_write(evidence, (self.path.parent / "candidate.plist").read_bytes())
        real_evidence = helper._regular_file_evidence
        digest_calls = [0]

        def digest_then_swap(path, label):
            digest = real_evidence(path, label)
            if Path(path) == evidence:
                digest_calls[0] += 1
                if digest_calls[0] == 1:
                    helper.atomic_write(evidence, b"replacement evidence")
            return digest

        with mock.patch.object(helper, "_regular_file_evidence", side_effect=digest_then_swap):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        self.assertEqual(digest_calls[0], 2)

    def test_target_replacement_between_admission_passes_blocks_resume(self):
        self.sealed()
        target = Path(self.parent.target_plist)
        real_evidence = helper._regular_file_evidence
        digest_calls = [0]

        def digest_then_publish(path, label):
            digest = real_evidence(path, label)
            if Path(path) == target:
                digest_calls[0] += 1
                if digest_calls[0] == 1:
                    helper.atomic_write(target, b"late unknown target")
            return digest

        with mock.patch.object(helper, "_regular_file_evidence", side_effect=digest_then_publish):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        self.assertEqual(digest_calls[0], 2)
        self.assertEqual(target.read_bytes(), b"late unknown target")

    def test_in_place_quarantine_mutation_during_final_target_check_blocks_resume(self):
        self.sealed()
        evidence = Path(self.parent.target_plist).parent / f"{helper._current_repair_quarantine_prefix(self.parent)}swap"
        original = (self.path.parent / "candidate.plist").read_bytes()
        helper.atomic_write(evidence, original)
        target = Path(self.parent.target_plist)
        real_evidence = helper._regular_file_evidence
        target_checks = [0]

        def target_digest_then_mutate(path, label):
            digest = real_evidence(path, label)
            if Path(path) == target:
                target_checks[0] += 1
                if target_checks[0] == 2:
                    with evidence.open("r+b") as stream:
                        stream.write(b"x" * len(original))
                        stream.flush()
                        os.fsync(stream.fileno())
            return digest

        with mock.patch.object(helper, "_regular_file_evidence", side_effect=target_digest_then_mutate):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        self.assertEqual(target_checks[0], 2)
        self.assertEqual(evidence.read_bytes(), b"x" * len(original))

    def test_quarantine_added_after_enumeration_blocks_resume(self):
        self.sealed()
        target = Path(self.parent.target_plist)
        first = target.parent / f"{helper._current_repair_quarantine_prefix(self.parent)}first"
        added = target.parent / f"{helper._current_repair_quarantine_prefix(self.parent)}added"
        helper.atomic_write(first, (self.path.parent / "candidate.plist").read_bytes())
        real_scandir = helper.os.scandir
        scans = [0]

        def scandir_and_add(path):
            scans[0] += 1
            if scans[0] == 2:
                helper.atomic_write(added, b"added unknown evidence")
            return real_scandir(path)

        with mock.patch.object(helper.os, "scandir", side_effect=scandir_and_add):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))
        self.assertTrue(added.exists())

    def test_quarantine_entry_count_limit_fails_closed_before_admission(self):
        self.sealed()
        directory = Path(self.parent.target_plist).parent
        prefix = helper._current_repair_quarantine_prefix(self.parent)
        for index in range(3):
            helper.atomic_write(directory / f"{prefix}{index}", (self.path.parent / "candidate.plist").read_bytes())
        with mock.patch.object(helper, "CURRENT_REPAIR_QUARANTINE_MAX_ENTRIES", 2):
            with self.assertRaisesRegex(helper.ActivationError, "entry limit exceeded"):
                helper._current_repair_quarantines(self.parent)
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))

    def test_quarantine_admission_reserves_attempt_headroom(self):
        self.sealed()
        directory = Path(self.parent.target_plist).parent
        prefix = helper._current_repair_quarantine_prefix(self.parent)
        plist = (self.path.parent / "candidate.plist").read_bytes()
        helper.atomic_write(directory / f"{prefix}one", plist)
        helper.atomic_write(directory / f"{prefix}two", plist)
        with mock.patch.object(helper, "CURRENT_REPAIR_QUARANTINE_MAX_ENTRIES", 4), mock.patch.object(
            helper, "CURRENT_REPAIR_QUARANTINE_HEADROOM_ENTRIES", 3
        ):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))

    def test_failure_retry_consumes_previously_reserved_headroom(self):
        self.sealed()
        directory = Path(self.parent.target_plist).parent
        prefix = helper._current_repair_quarantine_prefix(self.parent)
        plist = (self.path.parent / "candidate.plist").read_bytes()
        helper.atomic_write(directory / f"{prefix}one", plist)
        helper.atomic_write(directory / f"{prefix}two", plist)
        with mock.patch.object(helper, "CURRENT_REPAIR_QUARANTINE_MAX_ENTRIES", 2), mock.patch.object(
            helper, "CURRENT_REPAIR_QUARANTINE_HEADROOM_ENTRIES", 1
        ), mock.patch.object(helper, "CURRENT_REPAIR_QUARANTINE_MAX_TOTAL_BYTES", len(plist) * 2), mock.patch.object(
            helper, "CURRENT_REPAIR_QUARANTINE_HEADROOM_BYTES", len(plist)
        ):
            self.assertTrue(
                helper._current_repair_persistence_owned(
                    self.repair,
                    self.parent,
                    reserve_attempt_headroom=False,
                )
            )

    def test_quarantine_aggregate_byte_limit_fails_closed(self):
        self.sealed()
        directory = Path(self.parent.target_plist).parent
        prefix = helper._current_repair_quarantine_prefix(self.parent)
        plist = (self.path.parent / "candidate.plist").read_bytes()
        helper.atomic_write(directory / f"{prefix}one", plist)
        helper.atomic_write(directory / f"{prefix}two", plist)
        with mock.patch.object(helper, "CURRENT_REPAIR_QUARANTINE_MAX_TOTAL_BYTES", len(plist) * 2 - 1), mock.patch.object(
            helper, "CURRENT_REPAIR_QUARANTINE_HEADROOM_BYTES", 0
        ):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))

    def test_quarantine_admission_reserves_byte_headroom(self):
        self.sealed()
        directory = Path(self.parent.target_plist).parent
        prefix = helper._current_repair_quarantine_prefix(self.parent)
        plist = (self.path.parent / "candidate.plist").read_bytes()
        helper.atomic_write(directory / f"{prefix}one", plist)
        helper.atomic_write(directory / f"{prefix}two", plist)
        with mock.patch.object(helper, "CURRENT_REPAIR_QUARANTINE_MAX_TOTAL_BYTES", len(plist) * 3), mock.patch.object(
            helper, "CURRENT_REPAIR_QUARANTINE_HEADROOM_BYTES", len(plist) * 2
        ):
            self.assertFalse(helper._current_repair_persistence_owned(self.repair, self.parent))

    def test_publication_is_idempotent_without_replacing_exact_target(self):
        self.sealed()
        runtime = helper._repair_runtime(self.parent, self.repair, self.path.parent)
        target = Path(runtime.target_plist)
        publication = helper._repair_publication_path(self.parent)
        helper.atomic_write(target, Path(runtime.candidate_plist).read_bytes())
        helper.atomic_write(publication, Path(runtime.candidate_plist).read_bytes())
        before = target.stat()
        helper._commit_current_repair_plist(runtime, publication)
        after = target.stat()
        self.assertEqual((after.st_dev, after.st_ino), (before.st_dev, before.st_ino))
        self.assertEqual(helper.sha256(target), self.repair.plist_sha256)
        self.assertFalse(publication.exists())
        retired = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(retired), 1)
        self.assertEqual(helper.sha256(retired[0]), self.repair.plist_sha256)

    def test_unknown_existing_target_is_not_replaced_by_publication(self):
        self.sealed()
        runtime = helper._repair_runtime(self.parent, self.repair, self.path.parent)
        target = Path(runtime.target_plist)
        publication = helper._repair_publication_path(self.parent)
        helper.atomic_write(target, b"unknown existing target")
        helper.atomic_write(publication, Path(runtime.candidate_plist).read_bytes())
        before = target.stat()
        with self.assertRaisesRegex(helper.ActivationError, "unknown bytes during publication"):
            helper._commit_current_repair_plist(runtime, publication)
        after = target.stat()
        self.assertEqual((after.st_dev, after.st_ino), (before.st_dev, before.st_ino))
        self.assertEqual(target.read_bytes(), b"unknown existing target")
        self.assertTrue(publication.exists())

    def test_replaced_publication_is_retained_and_refuses_success(self):
        self.sealed()
        runtime = helper._repair_runtime(self.parent, self.repair, self.path.parent)
        target = Path(runtime.target_plist)
        publication = helper._repair_publication_path(self.parent)
        helper.atomic_write(target, Path(runtime.candidate_plist).read_bytes())
        real_fsync = helper.fsync_dir
        replaced = [False]

        def replace_during_fsync(directory):
            real_fsync(directory)
            if publication.exists() and not replaced[0]:
                replaced[0] = True
                replacement = publication.parent / f".{publication.name}.replacement"
                replacement.write_bytes(b"unknown publication replacement")
                replacement.chmod(0o600)
                os.replace(replacement, publication)

        with mock.patch.object(helper, "fsync_dir", side_effect=replace_during_fsync):
            with self.assertRaisesRegex(helper.ActivationError, "publication changed before retirement"):
                helper._publish_current_repair(runtime, self.parent)
        self.assertTrue(replaced[0])
        self.assertFalse(publication.exists())
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 1)
        self.assertEqual(quarantines[0].read_bytes(), b"unknown publication replacement")
        self.assertEqual(helper.sha256(target), self.repair.plist_sha256)

    def test_persistence_admission_runs_only_after_child_validation(self):
        self.sealed()
        self.environment()
        helper.atomic_write(self.path.parent / "candidate", b"tampered child")
        with mock.patch.object(
            helper, "_current_repair_persistence_owned", side_effect=AssertionError("admission before validation")
        ) as persistence_owned:
            with self.assertRaises(helper.ActivationError):
                helper.activate_current_repair(self.path)
        persistence_owned.assert_not_called()
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_claim_parent_fsync_failure_restores_exact_owner_and_retries_after_cleanup(self):
        self.sealed()
        self.environment()
        real_fsync = helper.fsync_dir
        fail_once = [True]
        def fsync(directory):
            if directory == Path(self.parent.active_path).parent and not Path(self.parent.active_path).exists() and fail_once[0]:
                fail_once[0] = False
                raise OSError("claim-parent fsync")
            return real_fsync(directory)
        with mock.patch.object(helper, "fsync_dir", side_effect=fsync):
            with self.assertRaises(OSError):
                helper.activate_current_repair(self.path)
        self.assertEqual(Path(self.parent.active_path).read_text().strip(), self.parent.transaction_id)
        status = self.status()
        self.assertEqual((status.state, status.resume_from, status.finalization_pending), ("committed", None, False))
        self.assertFalse(helper._restore_capacity_path(self.parent).exists())
        helper.activate_current_repair(self.path)
        self.assertFalse(Path(self.parent.active_path).exists())

    def test_stale_original_helper_never_invokes_candidate(self):
        self.sealed()
        self.environment()
        with mock.patch.object(helper, "require_launchd_absence", side_effect=helper.ActivationError("helper still loaded")):
            with self.assertRaises(helper.ActivationError):
                helper.activate_current_repair(self.path)
        self.run.assert_not_called()
        self.launchctl.start.assert_not_called()
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_captured_plist_drift_refuses_even_if_child_digest_is_resealed(self):
        self.sealed()
        altered = plistlib.loads((self.path.parent / "candidate.plist").read_bytes())
        altered["EnvironmentVariables"]["CAPTURED"] = "ambient"
        helper.atomic_write(self.path.parent / "candidate.plist", plistlib.dumps(altered))
        repair = dataclasses.replace(self.repair, plist_sha256=helper.sha256(self.path.parent / "candidate.plist"))
        helper.atomic_write(self.path, json.dumps(dataclasses.asdict(repair)).encode())
        helper._write_repair_status(self.path, helper.CurrentSchemaRepairStatus(1, helper.sha256(self.path), "prepared", None, None, False, None))
        with self.assertRaisesRegex(helper.ActivationError, "configuration differs"):
            helper.validate_current_repair(self.path, running_helper=False)

    def controller(self):
        spec = importlib.util.spec_from_file_location("repair_devpy_test", ROOT / "dev.py")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module

    def test_stopped_runtime_fence_preserves_private_parent_and_blocks_old_checksum(self):
        self.sealed()
        self.assertEqual(Path(self.parent.candidate_binary).read_bytes(), b"original")
        self.assertEqual(helper.sha256(Path(self.parent.candidate_plist)), self.parent.candidate_plist_sha256)
        self.assertEqual(helper.sha256(Path(self.parent.target_binary)), helper.CURRENT_REPAIR_RUNTIME_FENCE_SHA256)
        self.assertFalse(Path(self.parent.target_binary).stat().st_mode & 0o111)
        self.assertFalse(Path(self.parent.target_plist).exists())
        with self.assertRaises(helper.ActivationError):
            helper.verify_staged(self.parent.target_binary, self.parent.candidate_binary_sha256, "historical finalizer target")

    def test_runtime_fence_write_failure_attempts_exact_service_teardown(self):
        self.sealed()
        helper.atomic_write(Path(self.parent.target_binary), Path(self.parent.candidate_binary).read_bytes(), 0o755)
        helper.atomic_write(Path(self.parent.target_plist), Path(self.parent.candidate_plist).read_bytes())
        launchctl = mock.Mock()
        with mock.patch.object(helper, "require_launchd_absence"), \
             mock.patch.object(helper, "Launchctl", return_value=launchctl), \
             mock.patch.object(helper, "atomic_write", side_effect=OSError("fence write")):
            with self.assertRaisesRegex(OSError, "fence write"):
                helper.install_current_repair_runtime_fence(self.parent)
        launchctl.stop.assert_called_once()

    def test_repeat_repair_command_classifies_child_before_full_admission(self):
        self.sealed()
        dev = self.controller()
        with mock.patch.object(dev, "_current_repair_path", return_value=self.path), \
             mock.patch.object(dev, "_current_repair_module", return_value=helper), \
             mock.patch.object(helper, "validate_current_repair_parent", side_effect=AssertionError("full parent admission")), \
             mock.patch.object(helper, "validate_current_schema_database", side_effect=AssertionError("live database admission")), \
             mock.patch.object(dev, "_prepare_prepared_artifact", side_effect=AssertionError("ambient artifact")):
            with self.assertRaisesRegex(SystemExit, "already sealed"):
                dev.cmd_prod_repair_current(self.parent.transaction_id, self.root / "unused", helper.CURRENT_REPAIR_COMMIT)

    def test_preparation_seals_before_handoff_and_uses_candidate_git_object(self):
        self.sealed()
        dev = self.controller()
        artifact = self.root / "prepared"
        artifact.mkdir()
        shutil.copyfile(self.path.parent / "candidate", artifact / "standalone")
        receipt = artifact / "PREPARATION-RECEIPT-aarch64-apple-darwin.json"
        shutil.copyfile(self.path.parent / "receipt.json", receipt)
        self.path.unlink()  # Unsealed staging has no activation authority.
        (self.path.parent / "status.json").unlink()
        helper.atomic_write(Path(self.parent.target_binary), Path(self.parent.candidate_binary).read_bytes(), 0o755)
        helper.atomic_write(Path(self.parent.target_plist), Path(self.parent.candidate_plist).read_bytes())
        prepared = dev.PreparedCandidate(binary=artifact / "standalone", source_kind=dev.ProdSourceKind.PREPARED_ARTIFACT,
                                         source_commit=helper.CURRENT_REPAIR_COMMIT,
                                         identity=dev.RuntimeIdentity(helper.CURRENT_REPAIR_VERSION, helper.CURRENT_REPAIR_COMMIT))
        events = []
        real_atomic = helper.atomic_write
        def write(path, data, mode=0o600):
            events.append(path.name)
            return real_atomic(path, data, mode)
        def source(commit, name, destination, kind):
            self.assertEqual(commit, helper.CURRENT_REPAIR_COMMIT)
            self.assertEqual(name, "crates/phoenix-db/src/migrations.rs")
            self.assertEqual(kind, "local_head")
            destination.write_bytes(self.registry.read_bytes())
        def handoff(path, **kwargs):
            events.append("handoff-enter")
            self.assertEqual(helper.validate_current_repair(path, running_helper=False)[2].state, "prepared")
            self.assertFalse(kwargs["resume"])
            events.append("handoff")
        real_validate_staging = helper.validate_current_repair_staging
        def validate_staging(*args, **kwargs):
            events.append("validate-staging")
            return real_validate_staging(*args, **kwargs)
        with mock.patch.object(dev, "_current_repair_path", return_value=self.path), \
             mock.patch.object(dev, "_current_repair_module", return_value=helper), \
             mock.patch.object(dev, "_prepare_prepared_artifact", return_value=prepared), \
             mock.patch.object(dev.platform, "machine", return_value="arm64"), \
             mock.patch.object(helper, "require_launchd_absence"), mock.patch.object(helper, "assert_database_exclusive"), \
             mock.patch.object(dev.subprocess, "run", side_effect=lambda args, **kw: subprocess.CompletedProcess(args, 0, "d" * 40 if args[1] == "rev-parse" else "", "")), \
             mock.patch.object(dev, "_materialize_helper", side_effect=lambda commit, destination, kind: destination.write_bytes(Path(helper.__file__).read_bytes())), \
             mock.patch.object(dev, "_materialize_source_file", side_effect=source), \
             mock.patch.object(helper, "validate_current_repair_staging", side_effect=validate_staging), \
             mock.patch.object(helper, "atomic_write", side_effect=write), mock.patch.object(dev, "_handoff_current_repair", side_effect=handoff):
            dev.cmd_prod_repair_current(self.parent.transaction_id, artifact, helper.CURRENT_REPAIR_COMMIT)
        self.assertEqual(events[-1], "handoff")
        self.assertLess(events.index("validate-staging"), events.index("status.json"))
        self.assertLess(events.index("status.json"), events.index("installed"))
        self.assertLess(events.index("installed"), events.index("manifest.json"))
        self.assertLess(events.index("manifest.json"), events.index("handoff-enter"))
        self.assertEqual(Path(self.parent.target_binary).read_bytes(), b"")
        self.assertFalse(Path(self.parent.target_binary).stat().st_mode & 0o111)
        self.assertFalse(Path(self.parent.target_plist).exists())

    def test_unsealed_cleanup_refuses_any_repair_checkpoint_or_unknown_path(self):
        self.sealed()
        dev = self.controller()
        self.path.unlink()
        status = json.loads((self.path.parent / "status.json").read_text())
        status["state"] = "reconciliation_started"
        helper.atomic_write(self.path.parent / "status.json", json.dumps(status).encode())
        with self.assertRaises(SystemExit):
            dev._clean_unsealed_current_repair(self.path.parent, helper)
        self.assertTrue((self.path.parent / "candidate").exists())
        status["state"] = "prepared"
        helper.atomic_write(self.path.parent / "status.json", json.dumps(status).encode())
        helper.atomic_write(self.path.parent / "not-owned", b"keep")
        with self.assertRaises(SystemExit):
            dev._clean_unsealed_current_repair(self.path.parent, helper)
        self.assertTrue((self.path.parent / "not-owned").exists())

    def test_unsealed_partial_has_no_selection_and_malformed_seal_fences_classifiers(self):
        self.sealed()
        dev = self.controller()
        transaction = self.path.parent.parent
        self.path.unlink()
        self.assertIsNone(dev._current_repair_resolution(transaction))
        helper.atomic_write(self.path, b"invalid")
        with mock.patch.object(dev, "LAUNCHD_DEPLOY_DIR", transaction.parent.parent), \
             mock.patch.object(dev, "_current_repair_module", return_value=helper):
            self.assertFalse(dev._current_repair_resolution(transaction))
        with mock.patch.object(dev, "_current_repair_resolution", return_value=False):
            self.assertIn("resume-current-repair", dev._paired_recovery_refusal(self.parent.transaction_id))
            self.assertFalse(dev._markerless_status_is_resolved(json.loads(Path(self.parent.status_path).read_text())))
            self.assertFalse(dev._status_is_terminal_for_owner(dev.LAUNCHD_DEPLOY_STATUS_PATH, self.parent.transaction_id, {"committed"}))

    def test_resume_uses_retained_handoff_without_preparation(self):
        self.sealed()
        dev = self.controller()
        with mock.patch.object(dev, "_current_repair_path", return_value=self.path), \
             mock.patch.object(dev, "_handoff_current_repair") as handoff, \
             mock.patch.object(dev, "_prepare_prepared_artifact", side_effect=AssertionError("ambient artifact")), \
             mock.patch.object(dev, "_materialize_helper", side_effect=AssertionError("ambient helper")), \
             mock.patch.object(dev, "_materialize_source_file", side_effect=AssertionError("ambient registry")):
            dev.cmd_prod_resume_current_repair(self.parent.transaction_id)
        handoff.assert_called_once_with(self.path, resume=True)

    def test_reconciliation_checkpoint_fsync_failure_prevents_candidate_invocation(self):
        self.sealed()
        self.environment()
        real_write = helper._write_repair_status
        def write(path, status):
            if status.state == "reconciliation_started":
                raise OSError("checkpoint durability")
            return real_write(path, status)
        with mock.patch.object(helper, "_write_repair_status", side_effect=write):
            with self.assertRaises(OSError):
                helper.activate_current_repair(self.path)
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)
        self.assertEqual(self.status().resume_from, "prepared")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_database_foreign_key_damage_prevents_install_or_start(self):
        self.sealed()
        self.environment()
        with sqlite3.connect(self.parent.current_schema_upgrade.database_path) as connection:
            connection.execute("INSERT INTO conversations VALUES ('broken', 'missing')")
        with mock.patch.object(helper, "atomic_install") as install:
            with self.assertRaises(helper.ActivationError):
                helper.activate_current_repair(self.path)
        install.assert_not_called()
        self.launchctl.start.assert_not_called()
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_parent_status_and_proof_byte_drift_refuse_after_seal(self):
        self.sealed()
        for path in (Path(self.parent.status_path), Path(self.parent.current_schema_upgrade.proof_path)):
            old = path.read_bytes()
            helper.atomic_write(path, old + b"\n")
            with self.assertRaises(helper.ActivationError):
                helper.validate_current_repair(self.path, running_helper=False)
            helper.atomic_write(path, old)

    def test_proof_incomplete_or_uncommitted_parent_refuses(self):
        self.sealed()
        path = Path(self.parent.current_schema_upgrade.proof_path)
        original = json.loads(path.read_text())
        for key, value in (("migrated_ledger", compiled_ledger(127)), ("migration_committed_at", None), ("target_ledger", compiled_ledger(127))):
            with self.subTest(key=key):
                helper.atomic_write(path, json.dumps(dict(original, **{key: value})).encode())
                with self.assertRaises(helper.ActivationError):
                    helper.validate_current_repair_parent(Path(self.repair.parent_manifest_path))
        helper.atomic_write(path, json.dumps(original).encode())

    def test_capacity_loss_stops_retains_without_reconciliation(self):
        self.sealed()
        self.environment()
        capacity = shutil.disk_usage(self.root)._replace(free=0)
        with mock.patch.object(helper.shutil, "disk_usage", return_value=capacity):
            with self.assertRaises(helper.ActivationError):
                helper.activate_current_repair(self.path)
        self.assertEqual(self.status().state, "failed_stopped")
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_parent_service_absence_is_required_before_first_install(self):
        self.sealed()
        self.environment()
        def absence(label, uid):
            if label == self.parent.label:
                raise helper.ActivationError("service present")
        with mock.patch.object(helper, "require_launchd_absence", side_effect=absence), mock.patch.object(helper, "atomic_install") as install:
            with self.assertRaises(helper.ActivationError):
                helper.activate_current_repair(self.path)
        install.assert_not_called()
        self.assertEqual(self.status().state, "stop_unconfirmed")
        self.assertTrue(Path(self.parent.active_path).exists())

    def reseal(self, **fields):
        self.repair = dataclasses.replace(self.repair, **fields)
        helper.atomic_write(self.path, json.dumps(dataclasses.asdict(self.repair)).encode())
        helper._write_repair_status(self.path, helper.CurrentSchemaRepairStatus(
            1, helper.sha256(self.path), "prepared", None, None, False, None))

    def pending_commit(self):
        with mock.patch.object(helper, "_publish_current_repair", side_effect=KeyboardInterrupt("crash after pending commit")):
            with self.assertRaises(KeyboardInterrupt):
                helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().finalization_pending), ("committed", True))

    def test_pending_running_commit_only_retries_health_publication_and_cleanup(self):
        self.sealed()
        self.environment()
        self.pending_commit()
        self.launchctl.inspect.return_value = ("running", 4321)
        self.launchctl.start.reset_mock()
        self.events.clear()
        proof = (self.path.parent / "proof.json").read_bytes()
        with mock.patch.object(helper, "atomic_install", side_effect=AssertionError("pending install")), \
             mock.patch.object(helper, "_repair_verify_database", side_effect=AssertionError("live database inspection")), \
             mock.patch.object(helper, "_source_capacity_bytes", side_effect=AssertionError("live database capacity inspection")):
            self.assertEqual(helper.activate_current_repair(self.path), "committed")
        self.launchctl.start.assert_not_called()
        self.launchctl.stop.assert_not_called()
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)
        self.assertEqual((self.path.parent / "proof.json").read_bytes(), proof)

    def test_pending_stopped_commit_restarts_only_repair_without_reconciliation(self):
        self.sealed()
        self.environment()
        self.pending_commit()
        self.launchctl.inspect.return_value = ("not_loaded", None)
        self.events.clear()
        self.assertEqual(helper.activate_current_repair(self.path), "committed")
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)
        self.assertEqual(self.status().finalization_pending, False)

    def test_cleanup_fsync_failure_retries_after_restore_reservation_is_gone(self):
        self.sealed()
        self.environment()
        restore = helper._restore_capacity_path(self.parent)
        real_fsync = helper.fsync_dir
        fail = [True]
        def fsync(directory):
            if directory == restore.parent and not restore.exists() and fail[0]:
                fail[0] = False
                raise OSError("restore cleanup fsync")
            return real_fsync(directory)
        with mock.patch.object(helper, "fsync_dir", side_effect=fsync):
            with self.assertRaises(OSError):
                helper.activate_current_repair(self.path)
        self.assertEqual(self.status().resume_from, "committed")
        self.assertTrue(Path(self.parent.active_path).exists())
        self.assertFalse(Path(self.parent.target_plist).exists())
        self.assertFalse(restore.exists())
        self.events.clear()
        self.assertEqual(helper.activate_current_repair(self.path), "committed")
        self.assertNotIn((self.parent.target_binary, "--migrate-only"), self.events)

    def test_ambiguous_publication_temporary_is_quarantined_and_preserved(self):
        self.sealed()
        self.environment()
        publication = helper._repair_publication_path(self.parent)
        helper.atomic_write(publication, b"unrecognized publication")
        with self.assertRaisesRegex(helper.ActivationError, "publication changed before retirement"):
            helper.activate_current_repair(self.path)
        self.assertFalse(publication.exists())
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 1)
        self.assertEqual(quarantines[0].read_bytes(), b"unrecognized publication")
        self.assertEqual(self.status().resume_from, "committed")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_matching_orphan_publication_is_retired_and_published(self):
        self.sealed()
        self.environment()
        publication = helper._repair_publication_path(self.parent)
        os.link(self.path.parent / "candidate.plist", publication)
        self.assertEqual(helper.activate_current_repair(self.path), "committed")
        self.assertFalse(publication.exists())
        self.assertTrue(os.path.samefile(self.path.parent / "candidate.plist", self.parent.target_plist))
        quarantines = helper._current_repair_quarantines(self.parent)
        self.assertEqual(len(quarantines), 2)
        self.assertTrue(all(helper.sha256(path) == self.repair.plist_sha256 for path in quarantines))

    def test_classifier_never_scans_database_or_backup_but_full_admission_does(self):
        self.sealed()
        dev = self.controller()
        real_hash = helper.sha256
        forbidden = {Path(self.parent.current_schema_upgrade.database_path), Path(self.parent.current_schema_upgrade.backup_path)}
        def digest(path):
            if path in forbidden:
                raise AssertionError("classifier scanned database/backup")
            return real_hash(path)
        with mock.patch.object(helper, "sha256", side_effect=digest), \
             mock.patch.object(helper, "_snapshot_proof", side_effect=AssertionError("full parent proof scan")), \
             mock.patch.object(helper, "validate_current_schema_database", side_effect=AssertionError("database validation")), \
             mock.patch.object(helper, "database_logical_digest", side_effect=AssertionError("logical scan")), \
             mock.patch.object(dev, "_current_repair_module", return_value=helper):
            self.assertEqual(helper.classify_current_repair(self.path)[2].state, "prepared")
            self.assertFalse(dev._current_repair_resolution(self.path.parent.parent))
            with self.assertRaisesRegex(AssertionError, "full parent proof scan"):
                helper.validate_current_repair(self.path, running_helper=False)

    def test_terminal_child_remains_unresolved_until_exact_parent_claim_release(self):
        self.sealed()
        status = helper.CurrentSchemaRepairStatus(
            1, helper.sha256(self.path), "committed", None, "a" * 64, False, None
        )
        proof = {
            "protocol_version": 1,
            "manifest_sha256": helper.sha256(self.path),
            "parent_proof_sha256": self.repair.parent_proof_sha256,
            "source_commit": self.repair.source_commit,
            "registry_sha256": self.repair.registry_sha256,
            "target_ledger": compiled_ledger(),
        }
        helper.atomic_write(self.path.parent / "proof.json", helper._encode_database_proof(proof))
        status = dataclasses.replace(status, proof_sha256=helper.sha256(self.path.parent / "proof.json"))
        helper._write_repair_status(self.path, status)
        dev = self.controller()
        with mock.patch.object(dev, "_current_repair_module", return_value=helper):
            self.assertFalse(dev._current_repair_resolution(self.path.parent.parent))
            Path(self.parent.active_path).unlink()
            self.assertTrue(dev._current_repair_resolution(self.path.parent.parent))

    def test_terminal_audit_uses_snapshot_after_global_status_and_runtime_replacement(self):
        self.sealed()
        self.environment()
        helper.activate_current_repair(self.path)
        snapshot = (self.path.parent / "parent-status.json").read_bytes()
        helper.atomic_write(Path(self.parent.status_path), b'{"transaction_id":"later","state":"prepared"}')
        helper.atomic_write(Path(self.parent.target_binary), b"later legitimate deployment", 0o755)
        dev = self.controller()
        with mock.patch.object(helper, "_snapshot_proof", side_effect=AssertionError("historical backup scan")), \
             mock.patch.object(dev, "_current_repair_module", return_value=helper):
            self.assertEqual(helper.classify_current_repair(self.path)[2].state, "committed")
            self.assertTrue(dev._current_repair_resolution(self.path.parent.parent))
        self.assertEqual((self.path.parent / "parent-status.json").read_bytes(), snapshot)
        helper.atomic_write(self.path.parent / "parent-status.json", snapshot + b"\n")
        with self.assertRaises(helper.ActivationError):
            helper.classify_current_repair(self.path)

    def test_nonterminal_classifier_crosschecks_live_global_status(self):
        self.sealed()
        helper.atomic_write(Path(self.parent.status_path), b'{"transaction_id":"later"}')
        with self.assertRaises(helper.ActivationError):
            helper.classify_current_repair(self.path)

    def test_classifier_rejects_rebound_parent_status_or_proof_unknown_fields(self):
        self.sealed()
        for path, digest_field in ((self.path.parent / "parent-status.json", "parent_status_sha256"),
                                   (Path(self.parent.current_schema_upgrade.proof_path), "parent_proof_sha256")):
            with self.subTest(path=path.name):
                original = path.read_bytes()
                record = json.loads(original)
                record["unknown"] = True
                helper.atomic_write(path, json.dumps(record).encode())
                self.reseal(**{digest_field: helper.sha256(path)})
                with self.assertRaises(helper.ActivationError):
                    helper.classify_current_repair(self.path)
                helper.atomic_write(path, original)
                self.reseal(**{digest_field: helper.sha256(path)})

    def test_mutable_child_cannot_rebind_failure_settlement_target(self):
        self.sealed()
        self.environment()
        unrelated = self.root / "unrelated.plist"
        helper.atomic_write(unrelated, b"unrelated evidence")
        parent_path = Path(self.repair.parent_manifest_path)
        parent_record = json.loads(parent_path.read_text())
        parent_record["target_plist"] = str(unrelated)
        helper.atomic_write(parent_path, json.dumps(parent_record).encode())
        repair_record = json.loads(self.path.read_text())
        repair_record["parent_manifest_sha256"] = helper.sha256(parent_path)
        helper.atomic_write(self.path, json.dumps(repair_record).encode())

        with self.assertRaisesRegex(helper.ActivationError, "persistent route differs"):
            helper.activate_current_repair(self.path)
        self.assertEqual(unrelated.read_bytes(), b"unrelated evidence")
        self.launchctl.stop.assert_not_called()
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_reloaded_parent_cannot_bypass_persistent_route_check(self):
        self.sealed()
        self.environment()
        unrelated = self.root / "reloaded-unrelated.plist"
        helper.atomic_write(unrelated, b"reloaded unrelated evidence")
        rebound = dataclasses.replace(self.parent, target_plist=str(unrelated))
        status = self.status()

        with mock.patch.object(helper, "validate_current_repair", return_value=(self.repair, rebound, status)):
            with self.assertRaisesRegex(helper.ActivationError, "persistent route differs"):
                helper.activate_current_repair(self.path)
        self.assertEqual(unrelated.read_bytes(), b"reloaded unrelated evidence")
        self.launchctl.stop.assert_called_once_with()
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_malformed_status_stops_with_hash_bound_routing_without_fabricating_checkpoint(self):
        self.sealed()
        self.environment()
        parent_status = Path(self.parent.status_path).read_bytes()
        child_status = self.path.parent / "status.json"
        helper.atomic_write(child_status, b'{"state":"committed"')
        for stop_confirmed in (True, False):
            with self.subTest(stop_confirmed=stop_confirmed):
                self.launchctl.stop.reset_mock()
                absence = None if stop_confirmed else helper.ActivationError("absence unknown")
                with mock.patch.object(helper, "require_launchd_absence", side_effect=absence), \
                     mock.patch.object(helper, "print", create=True) as output:
                    with self.assertRaises(helper.ActivationError):
                        helper.activate_current_repair(self.path)
                self.launchctl.stop.assert_called_once()
                message = " ".join(str(call) for call in output.call_args_list)
                self.assertIn("failed_stopped" if stop_confirmed else "stop_unconfirmed", message)
                self.assertIn("no terminal/checkpoint fabricated", message)
                self.assertEqual(child_status.read_bytes(), b'{"state":"committed"')
                self.assertEqual(Path(self.parent.status_path).read_bytes(), parent_status)
                self.assertTrue(Path(self.parent.active_path).exists())
                self.launchctl.start.assert_not_called()

    def test_malformed_status_cannot_stop_another_owner(self):
        self.sealed()
        self.environment()
        helper.atomic_write(self.path.parent / "status.json", b"malformed")
        helper.atomic_write(Path(self.parent.active_path), b"other-owner\n")
        with self.assertRaises(helper.ActivationError):
            helper.activate_current_repair(self.path)
        self.launchctl.stop.assert_not_called()
        self.assertEqual(Path(self.parent.active_path).read_bytes(), b"other-owner\n")

    def test_malformed_retained_receipt_shapes_stop_before_install(self):
        self.sealed()
        self.environment()
        receipt = self.path.parent / "receipt.json"
        original = json.loads(receipt.read_text())
        for fields in ({"sha256": []}, {"checks": []}, {"target": []}, {"schema": True}):
            with self.subTest(fields=fields):
                helper.atomic_write(receipt, json.dumps(dict(original, **fields)).encode())
                self.reseal(receipt_sha256=helper.sha256(receipt))
                with mock.patch.object(helper, "atomic_install") as install:
                    with self.assertRaises(helper.ActivationError):
                        helper.activate_current_repair(self.path)
                install.assert_not_called()
                self.assertEqual(self.status().resume_from, "prepared")
                self.assertTrue(Path(self.parent.active_path).exists())

    def test_handoff_import_failure_attempts_stop_and_retains_claim(self):
        self.sealed()
        self.environment()
        dev = self.controller()
        def module(source=None):
            if source is not None:
                raise SyntaxError("retained helper import failed")
            return helper
        with mock.patch.object(dev, "_current_repair_module", side_effect=module):
            with self.assertRaises(SyntaxError):
                dev._handoff_current_repair(self.path, resume=True)
        self.launchctl.stop.assert_called_once()
        self.assertEqual(self.status().resume_from, "prepared")
        self.assertTrue(Path(self.parent.active_path).exists())
        self.launchctl.start.assert_not_called()

    def test_handoff_malformed_status_attempts_stop_without_bootstrap_or_rewrite(self):
        self.sealed()
        self.environment()
        dev = self.controller()
        status_path = self.path.parent / "status.json"
        helper.atomic_write(status_path, b"malformed")
        with mock.patch.object(dev, "_current_repair_module", return_value=helper):
            with self.assertRaises(helper.ActivationError):
                dev._handoff_current_repair(self.path, resume=True)
        self.launchctl.stop.assert_called_once()
        self.run.assert_not_called()
        self.assertEqual(status_path.read_bytes(), b"malformed")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_invalid_parent_routing_hash_never_authorizes_teardown(self):
        self.sealed()
        self.environment()
        parent_path = Path(self.repair.parent_manifest_path)
        raw = json.loads(parent_path.read_text())
        raw["label"] = "another.service"
        helper.atomic_write(parent_path, json.dumps(raw).encode())
        helper.atomic_write(self.path.parent / "status.json", b"malformed")
        with self.assertRaises(helper.ActivationError):
            helper.activate_current_repair(self.path)
        self.launchctl.stop.assert_not_called()
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_retained_restore_capacity_loss_prevents_install(self):
        self.sealed()
        self.environment()
        helper._restore_capacity_path(self.parent).write_bytes(b"")
        with mock.patch.object(helper, "atomic_install") as install:
            with self.assertRaisesRegex(helper.ActivationError, "retained restore capacity"):
                helper.activate_current_repair(self.path)
        install.assert_not_called()
        self.assertEqual(self.status().resume_from, "prepared")
        self.assertTrue(Path(self.parent.active_path).exists())

    def test_helper_import_uses_source_without_bytecode_cache(self):
        dev = self.controller()
        source = self.root / "source-helper.py"
        source.write_text("VALUE = 'source'\n")
        with mock.patch.object(importlib.machinery.SourceFileLoader, "get_code", side_effect=AssertionError("cached loader")):
            self.assertEqual(dev._current_repair_module(source).VALUE, "source")
        self.assertFalse((source.parent / "__pycache__").exists())

    def test_failed_helper_import_restores_previous_module_registration(self):
        dev = self.controller()
        broken = self.root / "broken-helper.py"
        broken.write_text("raise RuntimeError('import failed')\n")
        name = "phoenix_current_repair_controller"
        previous = object()
        with mock.patch.dict(sys.modules, {name: previous}):
            with self.assertRaisesRegex(RuntimeError, "import failed"):
                dev._current_repair_module(broken)
            self.assertIs(sys.modules[name], previous)

    def test_unsealed_cleanup_rejects_status_shape_before_any_unlink(self):
        self.sealed()
        dev = self.controller()
        self.path.unlink()
        status_path = self.path.parent / "status.json"
        raw = json.loads(status_path.read_text())
        helper.atomic_write(status_path, json.dumps(dict(raw, unknown=True)).encode())
        with self.assertRaises(SystemExit):
            dev._clean_unsealed_current_repair(self.path.parent, helper)
        self.assertTrue((self.path.parent / "parent-status.json").is_file())
        self.assertTrue((self.path.parent / "candidate").is_file())

    def test_pending_commit_capacity_loss_stops_and_keeps_committed_resume_boundary(self):
        self.sealed()
        self.environment()
        self.pending_commit()
        self.launchctl.inspect.return_value = ("running", 4321)
        capacity = shutil.disk_usage(self.root)._replace(free=0)
        with mock.patch.object(helper.shutil, "disk_usage", return_value=capacity):
            with self.assertRaises(helper.ActivationError):
                helper.activate_current_repair(self.path)
        self.assertEqual((self.status().state, self.status().resume_from), ("failed_stopped", "committed"))
        self.assertTrue(Path(self.parent.active_path).exists())


if __name__ == "__main__":
    unittest.main()
