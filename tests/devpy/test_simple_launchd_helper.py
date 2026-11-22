"""Ordinary deploy failures stop for manual recovery without inspecting SQLite."""
import contextlib
import dataclasses
import fcntl
import importlib.util
import io
import json
import os
import plistlib
import sqlite3
import subprocess
import sys
import tempfile
import time
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
        self.target = f"gui/{manifest.uid}/{manifest.label}"
        self.domain = f"gui/{manifest.uid}"
        self.pid = 41
        self.starts = []
        self.stops = 0

    def inspect(self):
        return ("running", self.pid) if self.pid is not None else ("not_loaded", None)

    def stop(self, *, expected_pid=None):
        if expected_pid is not None and self.pid != expected_pid:
            raise helper.ActivationError("recovery target changed before bootout")
        self.stops += 1
        old, self.pid = self.pid, None
        with (Path(self.manifest.candidate_binary).parent / "events").open("a") as stream:
            stream.write("stop\n")
        return old

    def run(self, command, **kwargs):
        if command[:2] == ["launchctl", "bootout"]:
            self.stop()
        return subprocess.CompletedProcess(command, 0, "", "")

    def wait(self, predicate, _deadline, _description):
        observed = self.inspect()
        if not predicate(*observed):
            raise AssertionError(f"unexpected launchd state: {observed}")
        return observed

    def start(self, old_pid, *, plist_path=None):
        self.starts.append(plist_path)
        self.pid = 99
        with (Path(self.manifest.candidate_binary).parent / "events").open("a") as stream:
            stream.write("start\n")
        return self.pid


class SQLiteUriPathTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="phoenix-sqlite-uri-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.parent = self.root / "parent # ? % \x01"
        self.parent.mkdir()
        self.database = self.parent / "real # ? % \x02.sqlite"
        with contextlib.closing(sqlite3.connect(self.database)) as connection, connection:
            connection.execute("CREATE TABLE entries(value TEXT NOT NULL)")
            connection.executemany("INSERT INTO entries VALUES (?)", [("one",), ("two %?#",)])
        self.expected = [("one",), ("two %?#",)]

    def test_database_size_connection_closes_after_success(self):
        connection = mock.Mock()
        connection.execute.return_value.fetchone.side_effect = [(3,), (4096,)]
        with mock.patch.object(sqlite3, "connect", return_value=connection):
            self.assertEqual(helper._database_size_bytes(str(self.database)), 3 * 4096)
        connection.close.assert_called_once_with()

    def test_database_size_connection_closes_after_sqlite_error(self):
        connection = mock.Mock()
        connection.execute.side_effect = sqlite3.OperationalError("database unavailable")
        with mock.patch.object(sqlite3, "connect", return_value=connection):
            with self.assertRaisesRegex(helper.ActivationError, "cannot inspect production database capacity"):
                helper._database_size_bytes(str(self.database))
        connection.close.assert_called_once_with()

    def test_capacity_and_verified_backup_use_actual_special_character_source(self):
        before = set(self.parent.iterdir())
        with contextlib.closing(sqlite3.connect(self.database)) as connection:
            expected_size = connection.execute("PRAGMA page_count").fetchone()[0] * connection.execute("PRAGMA page_size").fetchone()[0]
        self.assertEqual(helper._database_size_bytes(str(self.database)), expected_size)
        with mock.patch.object(helper.shutil, "disk_usage", return_value=type("Usage", (), {"free": 1 << 30})()):
            self.assertEqual(helper._preflight_backup_capacity(str(self.database), self.root), expected_size + 16 * 1024 * 1024)
        backup = helper.create_verified_database_backup(str(self.database), self.root)
        with contextlib.closing(sqlite3.connect(backup)) as connection:
            self.assertEqual(connection.execute("SELECT value FROM entries ORDER BY rowid").fetchall(), self.expected)
        self.assertGreater(backup.stat().st_size, 0)
        self.assertEqual(set(self.parent.iterdir()), before)
        self.assertEqual(set(self.root.iterdir()), {self.parent, backup})

    def test_writer_guard_fences_real_special_character_database(self):
        before = set(self.parent.iterdir())
        with helper.sqlite_writer_guard(str(self.database), timeout=0.1):
            with contextlib.closing(sqlite3.connect(self.database, timeout=0.05)) as competitor:
                with self.assertRaisesRegex(sqlite3.OperationalError, "locked"):
                    competitor.execute("BEGIN IMMEDIATE")
        with contextlib.closing(sqlite3.connect(self.database, timeout=0.05)) as competitor:
            competitor.execute("BEGIN IMMEDIATE")
            competitor.rollback()
        self.assertEqual(set(self.parent.iterdir()), before)

    def test_missing_special_character_source_is_never_created(self):
        missing = self.parent / "absent # ? % \x03.sqlite"
        before = set(self.parent.iterdir())
        with self.assertRaises(helper.ActivationError):
            helper._database_size_bytes(str(missing))
        with self.assertRaises(sqlite3.OperationalError):
            with helper.sqlite_writer_guard(str(missing), timeout=0.05):
                self.fail("missing source acquired a writer slot")
        with self.assertRaises(helper.ActivationError):
            helper.create_verified_database_backup(str(missing), self.root)
        self.assertEqual(set(self.parent.iterdir()), before)
        self.assertEqual(set(self.root.iterdir()), {self.parent})


class SQLiteLogicalEquivalenceTests(unittest.TestCase):
    def compare(self, source_sql, backup_sql, *, expect_difference=False):
        source = sqlite3.connect(":memory:")
        backup = sqlite3.connect(":memory:")
        self.addCleanup(source.close)
        self.addCleanup(backup.close)
        source.executescript(source_sql)
        backup.executescript(backup_sql)
        if expect_difference:
            with self.assertRaises(helper.ActivationError):
                helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)
        else:
            helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)

    def test_compares_all_sqlite_storage_classes_and_preserves_text_factories(self):
        source = sqlite3.connect(":memory:")
        backup = sqlite3.connect(":memory:")
        self.addCleanup(source.close)
        self.addCleanup(backup.close)
        for database in (source, backup):
            database.execute("CREATE TABLE values_table (id INTEGER PRIMARY KEY, value)")
        source.executemany("INSERT INTO values_table(value) VALUES (?)", [(1,), (1.25,), ("same",), (b"same",), (None,)])
        backup.executemany("INSERT INTO values_table(value) VALUES (?)", [(1,), (1.25,), ("same",), (b"same",), (None,)])
        factories = source.text_factory, backup.text_factory
        helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)
        self.assertEqual((source.text_factory, backup.text_factory), factories)
        backup.execute("UPDATE values_table SET value=1.0 WHERE id=1")
        with self.assertRaisesRegex(helper.ActivationError, "row differs"):
            helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)

    def test_compares_rowid_and_without_rowid_primary_key_identity(self):
        self.compare("CREATE TABLE t (value TEXT); INSERT INTO t(rowid,value) VALUES (8,'x');",
                     "CREATE TABLE t (value TEXT); INSERT INTO t(rowid,value) VALUES (9,'x');",
                     expect_difference=True)
        self.compare("CREATE TABLE t (a TEXT, b INTEGER, PRIMARY KEY (b,a)) WITHOUT ROWID; INSERT INTO t VALUES ('x',1);",
                     "CREATE TABLE t (a TEXT, b INTEGER, PRIMARY KEY (b,a)) WITHOUT ROWID; INSERT INTO t VALUES ('x',1);")
        self.compare("CREATE TABLE t (a TEXT, b INTEGER, PRIMARY KEY (b,a)) WITHOUT ROWID; INSERT INTO t VALUES ('x',1);",
                     "CREATE TABLE t (a TEXT, b INTEGER, PRIMARY KEY (b,a)) WITHOUT ROWID; INSERT INTO t VALUES ('x',2);",
                     expect_difference=True)

    def test_compares_schema_and_database_metadata(self):
        self.compare("CREATE TABLE t (value); PRAGMA user_version=4;",
                     "CREATE TABLE t (value); PRAGMA user_version=5;", expect_difference=True)
        self.compare("CREATE TABLE t (value); PRAGMA application_id=41;",
                     "CREATE TABLE t (value); PRAGMA application_id=42;", expect_difference=True)
        self.compare("CREATE TABLE t (value); CREATE INDEX i ON t(value);",
                     "CREATE TABLE t (value); CREATE INDEX i ON t(value);")
        self.compare("CREATE TABLE t (value); CREATE INDEX i ON t(value);",
                     "CREATE TABLE t (value); CREATE INDEX i ON t(value COLLATE NOCASE);", expect_difference=True)

    def test_fts5_virtual_contents_are_verified_through_persisted_shadow_tables(self):
        try:
            self.compare("CREATE VIRTUAL TABLE docs USING fts5(body); INSERT INTO docs VALUES ('source token');",
                         "CREATE VIRTUAL TABLE docs USING fts5(body); INSERT INTO docs VALUES ('backup token');",
                         expect_difference=True)
        except sqlite3.OperationalError as exc:
            if "fts5" in str(exc).lower():
                self.skipTest("SQLite was built without FTS5")
            raise
        self.compare("CREATE VIRTUAL TABLE docs USING fts5(body); INSERT INTO docs VALUES ('same token');",
                     "CREATE VIRTUAL TABLE docs USING fts5(body); INSERT INTO docs VALUES ('same token');")

    def test_rtree_module_is_not_spoofed_by_quoted_table_name(self):
        statements = (
            'CREATE VIRTUAL TABLE "USING fts5" USING rtree(id, min_x, max_x);',
            'CREATE VIRTUAL TABLE spatial /* USING fts5 */ USING rtree(id, min_x, max_x);',
            'CREATE VIRTUAL TABLE spatial USING rtree(id, min_x, max_x);',
        )
        try:
            for sql in statements:
                with self.subTest(sql=sql):
                    self.compare(sql, sql, expect_difference=True)
        except sqlite3.OperationalError as exc:
            if "no such module" in str(exc).lower():
                self.skipTest("SQLite was built without RTree")
            raise

    def test_fts5_module_parsing_ignores_names_and_comments_and_accepts_quotes(self):
        sql = ('ATTACH DATABASE \':memory:\' AS "odd schema"; '
               'CREATE VIRTUAL TABLE "odd schema"."USING rtree" '
               '/* misleading USING rtree */ USING "fts5"(body); '
               "INSERT INTO \"odd schema\".\"USING rtree\" VALUES ('same token');")
        try:
            self.compare(sql, sql)
        except sqlite3.OperationalError as exc:
            if "fts5" in str(exc).lower():
                self.skipTest("SQLite was built without FTS5")
            raise

    def test_single_quoted_fts5_identifiers_are_supported(self):
        for sql in ("CREATE VIRTUAL TABLE docs USING 'fts5'(body);",
                    "CREATE VIRTUAL TABLE 'do''cs' USING fts5(body);"):
            with self.subTest(sql=sql):
                self.compare(sql, sql)

    def test_without_rowid_uses_primary_key_collation_and_direction_without_sort(self):
        for key in ('a DESC,b ASC', 'a COLLATE NOCASE,b'):
            with self.subTest(key=key):
                source, backup = sqlite3.connect(':memory:'), sqlite3.connect(':memory:')
                self.addCleanup(source.close)
                self.addCleanup(backup.close)
                for database in (source, backup):
                    database.executescript(f'CREATE TABLE t(a TEXT,b INTEGER,p BLOB, PRIMARY KEY({key})) WITHOUT ROWID;'
                                           "INSERT INTO t VALUES ('Z',1,X'00'),('a',2,X'01');")
                queries = []
                source.set_trace_callback(queries.append)
                helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)
                query = next(sql for sql in queries if sql.startswith('SELECT ') and 'FROM "main"."t"' in sql)
                plan = source.execute('EXPLAIN QUERY PLAN ' + query).fetchall()
                self.assertFalse(any('TEMP B-TREE' in row[3] for row in plan), plan)
                backup.execute("UPDATE t SET p=X'02' WHERE a='Z'")
                with self.assertRaisesRegex(helper.ActivationError, 'row differs'):
                    helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)

    def test_fails_closed_for_unrecognized_virtual_table_modules(self):
        source = sqlite3.connect(":memory:")
        backup = sqlite3.connect(":memory:")
        self.addCleanup(source.close)
        self.addCleanup(backup.close)
        try:
            for database in (source, backup):
                database.execute("CREATE VIRTUAL TABLE spatial USING rtree(id, min_x, max_x)")
        except sqlite3.OperationalError as exc:
            if "no such module" in str(exc).lower():
                self.skipTest("SQLite was built without RTree")
            raise
        with self.assertRaisesRegex(helper.ActivationError, "unsupported SQLite virtual table module"):
            helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)

    def test_virtual_generated_columns_still_shadow_rowid_aliases(self):
        for alias in ("rowid", "RoWiD", "_rowid_", "OID"):
            actual = "oid" if alias.lower() != "oid" else "_rowid_"
            schema = f'CREATE TABLE t (value TEXT, "{alias}" GENERATED ALWAYS AS (42) VIRTUAL);'
            self.compare(schema + f"INSERT INTO t({actual},value) VALUES (8,'x');",
                         schema + f"INSERT INTO t({actual},value) VALUES (9,'x');",
                         expect_difference=True)
        schema = ('CREATE TABLE t (value TEXT, rowid AS (1), '
                  '_rowid_ AS (2), oid AS (3));')
        self.compare(schema, schema, expect_difference=True)

    def test_fails_closed_when_all_rowid_aliases_are_shadowed(self):
        self.compare("CREATE TABLE t (ROWID, _RowId_, OID);",
                     "CREATE TABLE t (ROWID, _RowId_, OID);", expect_difference=True)


class SimpleLaunchdHelperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        binary = self.root / "candidate"
        binary.write_text(f'''#!{sys.executable}
import contextlib, json, os, sqlite3, sys
from pathlib import Path
if sys.argv[1:] == ["--build-identity"]:
    print(json.dumps({{"version": "1.0", "git_sha": "{'a' * 40}"}}))
    raise SystemExit(0)
assert sys.argv[1:] == ["--migrate-only"]
status = json.loads(Path(os.environ["TEST_STATUS_PATH"]).read_text())
assert status["state"] == "migration_started"
with Path(os.environ["TEST_EVENTS"]).open("a") as stream:
    stream.write("migrate\\n")
with contextlib.closing(sqlite3.connect(os.environ["PHOENIX_DB_PATH"])) as db, db:
    db.execute("CREATE TABLE IF NOT EXISTS changes(phase TEXT)")
    db.execute("INSERT INTO changes VALUES (?)", (status["state"],))
if os.environ.get("FAIL_MIGRATION"):
    print("actual migration failure detail", file=sys.stderr, flush=True)
    raise SystemExit(7)
''')
        binary.chmod(0o755)
        self.database = self.root / "application.db"
        with contextlib.closing(sqlite3.connect(self.database)) as db, db:
            db.execute("CREATE TABLE changes(phase TEXT)")
        candidate_plist = self.root / "candidate.plist"
        self.environment = {
            "PHOENIX_DB_PATH": str(self.database),
            "TEST_STATUS_PATH": str(self.root / "status.json"),
            "TEST_EVENTS": str(self.root / "events"),
        }
        self.write_plist(candidate_plist)
        self.manifest = helper.Manifest(
            manifest_version=helper.HANDOFF_PROTOCOL_VERSION, transaction_id="unit-test", source_kind="local_head",
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
        self.previous_binary = f'''#!{sys.executable}
import json, sys
if sys.argv[1:] == ["--build-identity"]:
    print(json.dumps({{"version": "0.9", "git_sha": "{'b' * 40}"}}))
'''.encode()
        Path(self.manifest.target_binary).write_bytes(self.previous_binary)
        Path(self.manifest.target_binary).chmod(0o755)
        Path(self.manifest.target_plist).write_bytes(Path(self.manifest.candidate_plist).read_bytes())
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
        ), mock.patch.object(
            helper, "fetch_identity", return_value=helper.Identity("0.9", "b" * 40),
        ), mock.patch.object(helper, "_confirm_pid_gone", return_value=True):
            return helper.activate(self.manifest)

    def status(self):
        return json.loads(Path(self.manifest.status_path).read_text())

    def changes(self):
        with contextlib.closing(sqlite3.connect(self.database)) as db:
            return db.execute("SELECT phase FROM changes").fetchall()

    def test_success_invokes_migration_once_after_durable_checkpoint_and_stop(self):
        previous_binary = self.root / "previous-phoenix-ide"
        previous_binary.write_bytes(b"previous binary")
        self.assertEqual(self.activate(), "committed")
        backups = list(self.root.glob("database-backup-*.sqlite"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].stat().st_mode & 0o777, 0o600)
        with contextlib.closing(sqlite3.connect(backups[0])) as backup:
            self.assertEqual(backup.execute("PRAGMA integrity_check").fetchone(), ("ok",))
        self.assertEqual(self.changes(), [("migration_started",)])
        self.assertEqual((self.root / "events").read_text().splitlines(), ["stop", "migrate", "start"])
        self.assertEqual(self.launch.starts, [self.manifest.candidate_plist])
        self.assertEqual((self.root / "previous.plist").read_bytes(), Path(self.manifest.candidate_plist).read_bytes())
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), Path(self.manifest.candidate_plist).read_bytes())
        self.assertTrue(Path(self.manifest.target_plist).samefile(self.manifest.candidate_plist))
        self.assertEqual(Path(self.manifest.target_plist).stat().st_mode & 0o777, 0o600)
        self.assertEqual(Path(self.manifest.deployed_sha_path).read_text(), "a" * 40 + "\n")
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertFalse(Path(self.manifest.candidate_binary).exists())
        self.assertFalse(previous_binary.exists())
        self.assertEqual(self.status()["state"], "committed")

    def first_install(self):
        self.database.unlink()
        Path(self.manifest.target_binary).unlink()
        Path(self.manifest.target_plist).unlink()
        self.launch.pid = None

    def test_first_install_missing_database_runs_candidate_without_backup(self):
        self.first_install()
        self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.changes(), [("migration_started",)])
        self.assertEqual((self.root / "events").read_text().splitlines(), ["migrate", "start"])
        self.assertEqual(list(self.root.glob("database-backup-*.sqlite")), [])
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertEqual(proof["database_source"], "absent")
        self.assertIsNone(proof["backup"])
        self.assertEqual(proof["boundary"], "crossed")
        self.assertFalse(proof["recovery_eligible"])

    def test_missing_database_with_predecessor_rejects_before_disruption(self):
        self.database.unlink()
        self.assertEqual(self.activate(), "precondition_failed")
        self.assertIn("existing predecessor", self.status()["failure"])
        self.assertEqual(self.launch.stops, 0)
        self.assertEqual(self.launch.starts, [])
        self.assertFalse((self.root / "events").exists())
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), self.previous_binary)

    def test_missing_database_with_installed_artifacts_rejects_even_if_unloaded(self):
        self.database.unlink()
        self.launch.pid = None
        self.assertEqual(self.activate(), "precondition_failed")
        self.assertIn("existing predecessor or installation", self.status()["failure"])
        self.assertFalse((self.root / "events").exists())

    def test_symlink_database_cannot_be_treated_as_first_install(self):
        self.first_install()
        self.database.symlink_to(self.root / "missing-target.db")
        self.assertEqual(self.activate(), "precondition_failed")
        self.assertIn("existing regular file", self.status()["failure"])
        self.assertFalse((self.root / "events").exists())
        self.assertTrue(self.database.is_symlink())

    def test_first_install_database_appearing_before_boundary_fails_without_candidate(self):
        self.first_install()
        check = helper._first_install_capacity
        calls = 0

        def appear_before_boundary(database_path):
            nonlocal calls
            calls += 1
            if calls == 3:
                self.database.write_bytes(b"unexpected database")
            return check(database_path)

        with mock.patch.object(helper, "_first_install_capacity", side_effect=appear_before_boundary):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(calls, 3)
        self.assertIn("must still be absent", self.status()["failure"])
        self.assertEqual(list(self.root.glob("database-backup-*.sqlite")), [])
        self.assertFalse((self.root / "events").exists())
        self.assertFalse(Path(self.manifest.target_binary).exists())
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertEqual(proof["database_source"], "absent")
        self.assertIsNone(proof["backup"])
        self.assertEqual(proof["boundary"], "not_crossed")

    def test_first_install_space_reserve_uses_database_parent(self):
        self.first_install()
        usage = type("Usage", (), {"free": 16 * 1024 * 1024})()
        with mock.patch.object(helper.shutil, "disk_usage", return_value=usage) as disk_usage:
            self.assertEqual(helper._first_install_capacity(str(self.database)), 16 * 1024 * 1024)
        disk_usage.assert_called_once_with(self.database.parent)

    def test_first_install_insufficient_space_refuses_before_disruption(self):
        self.first_install()
        usage = type("Usage", (), {"free": 16 * 1024 * 1024 - 1})()
        with mock.patch.object(helper.shutil, "disk_usage", return_value=usage):
            self.assertEqual(self.activate(), "precondition_failed")
        self.assertIn("insufficient space", self.status()["failure"])
        self.assertFalse((self.root / "events").exists())

    def test_first_install_requires_durable_boundary_before_candidate(self):
        self.first_install()
        actual = helper.write_status

        def fail_boundary(manifest, state, **kwargs):
            if state == "migration_started":
                raise OSError("boundary status not durable")
            return actual(manifest, state, **kwargs)

        with mock.patch.object(helper, "write_status", side_effect=fail_boundary):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual((self.root / "events").read_text().splitlines(), ["stop"])
        self.assertFalse(Path(self.manifest.target_binary).exists())
        self.assertTrue(Path(self.manifest.active_path).exists())
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertEqual(proof["database_source"], "absent")
        self.assertIsNone(proof["backup"])
        self.assertEqual(proof["boundary"], "attempted")

    def test_native_backup_includes_committed_wal_and_matches_logical_contents(self):
        database = sqlite3.connect(self.database)
        self.addCleanup(database.close)
        with database:
            database.execute("PRAGMA journal_mode=WAL")
            database.execute("INSERT INTO changes VALUES ('committed-in-wal')")
        wal = Path(str(self.database) + "-wal")
        self.assertTrue(wal.exists())
        backup = helper.create_verified_database_backup(str(self.database), self.root)
        with contextlib.closing(sqlite3.connect(backup)) as database:
            self.assertEqual(database.execute("PRAGMA integrity_check").fetchone(), ("ok",))
            self.assertEqual(database.execute("SELECT phase FROM changes").fetchall(), [("committed-in-wal",)])

    def test_verifier_diagnostics_are_bounded_content_free_and_cover_all_stages(self):
        diagnostics = {}
        backup = helper.create_verified_database_backup(
            str(self.database), self.root, diagnostics=diagnostics,
        )
        deadline = helper.time.monotonic() + 10
        proof = helper._backup_proof(backup, deadline, diagnostics)
        captured = io.StringIO()
        with mock.patch.object(helper.sys, "stderr", captured):
            helper._emit_verifier_diagnostics(diagnostics, "verified")
        record = json.loads(captured.getvalue())
        self.assertEqual(record["outcome"], "verified")
        self.assertEqual(set(record["stage_ms"]), {
            "native_backup", "integrity_check", "metadata", "table_comparisons", "fsync", "proof_hash",
        })
        self.assertGreater(record["rows_compared"], 0)
        self.assertGreater(record["cells_compared"], 0)
        self.assertGreater(record["table_ordinal"], 0)
        self.assertNotIn("entries", captured.getvalue())
        self.assertNotIn("one", captured.getvalue())
        self.assertNotIn(str(backup), captured.getvalue())
        self.assertEqual(proof["backup_bytes"], backup.stat().st_size)

    def test_failed_row_comparison_diagnostics_report_only_ordinal_and_progress(self):
        source, backup = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
        self.addCleanup(source.close)
        self.addCleanup(backup.close)
        for database, value in ((source, "secret-source"), (backup, "secret-backup")):
            database.execute('CREATE TABLE "private table name" (payload TEXT)')
            database.execute('INSERT INTO "private table name" VALUES (?)', (value,))
        diagnostics = {}
        with self.assertRaises(helper.ActivationError):
            helper._verify_logical_equivalence(
                source, backup, helper.time.monotonic() + 10, diagnostics,
            )
        captured = io.StringIO()
        with mock.patch.object(helper.sys, "stderr", captured):
            helper._emit_verifier_diagnostics(diagnostics, "ActivationError")
        record = json.loads(captured.getvalue())
        self.assertEqual(record["table_ordinal"], 1)
        self.assertEqual((record["rows_compared"], record["cells_compared"]), (0, 1))
        for secret in ("private table name", "secret-source", "secret-backup", "payload"):
            self.assertNotIn(secret, captured.getvalue())

    def test_diagnostic_output_failure_does_not_replace_verifier_failure(self):
        captured = io.StringIO()
        captured.write = mock.Mock(side_effect=OSError("stderr unavailable"))
        with mock.patch.object(helper.sys, "stderr", captured):
            helper._emit_verifier_diagnostics({}, "ActivationError")

    def test_integrity_error_does_not_expose_database_names(self):
        with contextlib.closing(sqlite3.connect(self.database)) as connection, connection:
            connection.execute('CREATE TABLE "SECRET_TABLE" (value CHECK(value > 0))')
            connection.execute('PRAGMA ignore_check_constraints=ON')
            connection.execute('INSERT INTO "SECRET_TABLE" VALUES (-1)')
        diagnostics = {}
        (self.database.parent / "integrity").mkdir()
        with self.assertRaises(helper.ActivationError) as caught:
            helper.create_verified_database_backup(str(self.database), self.database.parent / "integrity", diagnostics=diagnostics)
        self.assertEqual(str(caught.exception), "SQLite backup integrity check failed")
        self.assertNotIn("SECRET", json.dumps(diagnostics))
        self.assertEqual(diagnostics["current_stage"], "integrity_check")

    def test_verifier_counts_attempted_cells(self):
        with contextlib.closing(sqlite3.connect(":memory:")) as left, left, contextlib.closing(sqlite3.connect(":memory:")) as right, right:
            for connection in (left, right):
                connection.execute("CREATE TABLE a (x, y, z)")
                connection.execute("INSERT INTO a VALUES (1, 2, 3)")
            right.execute("UPDATE a SET z=4")
            diagnostics = {}
            with self.assertRaises(helper.ActivationError):
                helper._verify_logical_equivalence(left, right, time.monotonic() + 10, diagnostics)
            self.assertEqual(diagnostics["cells_compared"], 3)
            self.assertEqual(diagnostics["rows_compared"], 0)
            self.assertEqual(diagnostics["tables_completed"], 0)

    def test_verifier_timeout_records_stage_without_resetting_deadline(self):
        with contextlib.closing(sqlite3.connect(":memory:")) as left, left, contextlib.closing(sqlite3.connect(":memory:")) as right, right:
            deadline = time.monotonic() - 1
            diagnostics = {}
            with self.assertRaisesRegex(helper.ActivationError, "deadline exceeded"):
                helper._verify_logical_equivalence(left, right, deadline, diagnostics)
            self.assertEqual(diagnostics["deadline"], deadline)
            self.assertEqual(diagnostics["current_stage"], "metadata")
            self.assertEqual(diagnostics["stage_budget_ms"], 0)
            self.assertIn("metadata", diagnostics["stage_ms"])

    def test_large_database_backup_uses_streaming_logical_verification(self):
        payload = b"z" * (8 * 1024 * 1024)
        with contextlib.closing(sqlite3.connect(self.database)) as database, database:
            database.execute("CREATE TABLE large_payloads (id INTEGER PRIMARY KEY, data BLOB NOT NULL)")
            database.executemany("INSERT INTO large_payloads(data) VALUES (?)", [(payload,) for _ in range(4)])
        backup = helper.create_verified_database_backup(str(self.database), self.root, timeout=30)
        with contextlib.closing(sqlite3.connect(backup)) as database:
            self.assertEqual(database.execute("SELECT count(*) FROM large_payloads").fetchone(), (4,))
            self.assertEqual(database.execute("PRAGMA integrity_check").fetchone(), ("ok",))

    def test_capacity_reserve_is_db_sized_not_an_arbitrary_large_floor(self):
        size = helper._database_size_bytes(str(self.database))
        available = 17 * 1024 * 1024
        usage = type("Usage", (), {"free": available})()
        with mock.patch.object(helper.shutil, "disk_usage", return_value=usage):
            required = helper._preflight_backup_capacity(str(self.database), self.root)
        self.assertEqual(required, size + 16 * 1024 * 1024)
        self.assertLess(required, 1024 * 1024 * 1024)

    def test_backup_and_verification_share_a_bounded_deadline(self):
        with contextlib.closing(sqlite3.connect(self.database)) as database, database:
            database.execute("INSERT INTO changes VALUES (?)", ("x" * 2_000_000,))
        started = helper.time.monotonic()
        with self.assertRaisesRegex(helper.ActivationError, "deadline"):
            helper.create_verified_database_backup(str(self.database), self.root, timeout=0.000001)
        self.assertLess(helper.time.monotonic() - started, 1.0)
        self.assertEqual(list(self.root.glob("database-backup-*.sqlite")), [])

    def test_backup_busy_wait_obeys_deadline_and_leaves_no_partial_copy(self):
        blocker = sqlite3.connect(self.database, timeout=0.1)
        self.addCleanup(blocker.close)
        blocker.execute("BEGIN EXCLUSIVE")
        started = helper.time.monotonic()
        try:
            with self.assertRaises((helper.ActivationError, sqlite3.OperationalError)):
                helper.create_verified_database_backup(str(self.database), self.root, timeout=0.15)
        finally:
            blocker.rollback()
        self.assertLess(helper.time.monotonic() - started, 0.75)
        self.assertEqual(list(self.root.glob("database-backup-*.sqlite")), [])

    def test_backup_proof_hashing_obeys_the_backup_deadline(self):
        backup = self.root / "large-backup.sqlite"
        backup.write_bytes(b"z" * (4 * 1024 * 1024))
        real_clock = helper.time.monotonic
        ticks = iter((real_clock(), real_clock(), real_clock() + 10))
        with mock.patch.object(helper.time, "monotonic", side_effect=lambda: next(ticks)):
            with self.assertRaisesRegex(helper.ActivationError, "deadline exceeded during proof hashing"):
                helper._backup_proof(backup, real_clock() + 1)

    def test_backup_default_deadline_uses_named_budget(self):
        diagnostics = {}
        with mock.patch.object(helper.time, "monotonic", return_value=123.0):
            backup = helper.create_verified_database_backup(
                str(self.database), self.root, diagnostics=diagnostics,
            )
        self.assertEqual(diagnostics["deadline"], 123.0 + helper.BACKUP_VERIFICATION_TIMEOUT_SECS)
        self.assertTrue(backup.exists())

    def test_activation_backup_and_proof_share_one_deterministic_deadline(self):
        clock = [321.0]
        captured = {}
        real_proof = helper._backup_proof
        real_backup = helper.create_verified_database_backup

        def capture_backup(database_path, directory, *, timeout, diagnostics=None):
            captured["backup_timeout"] = timeout
            backup = real_backup(database_path, directory, timeout=timeout, diagnostics=diagnostics)
            clock[0] += 100.0
            return backup

        def capture_proof(backup, deadline, diagnostics=None):
            captured["deadline"] = deadline
            return real_proof(backup, deadline, diagnostics)

        with mock.patch.object(helper.time, "monotonic", side_effect=lambda: clock[0]), \
             mock.patch.object(helper, "create_verified_database_backup", side_effect=capture_backup), \
             mock.patch.object(helper, "_backup_proof", side_effect=capture_proof):
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(captured["deadline"], 321.0 + helper.BACKUP_VERIFICATION_TIMEOUT_SECS)
        self.assertEqual(captured["backup_timeout"], helper.BACKUP_VERIFICATION_TIMEOUT_SECS)
        self.assertEqual(clock[0], captured["deadline"] - helper.BACKUP_VERIFICATION_TIMEOUT_SECS + 100.0)

    def test_activation_proof_expiry_after_backup_recovers_and_retains_backup(self):
        clock = [321.0]
        real_backup = helper.create_verified_database_backup
        real_proof = helper._backup_proof
        captured = {}
        database_path = self.database
        writer_succeeded = []

        def backup_then_expire(database_path_arg, directory, *, timeout, diagnostics=None):
            backup = real_backup(database_path_arg, directory, timeout=timeout, diagnostics=diagnostics)
            captured["backup"] = backup
            captured["deadline"] = clock[0] + helper.BACKUP_VERIFICATION_TIMEOUT_SECS
            clock[0] = captured["deadline"]
            return backup

        def actual_proof_at_expiry(backup, deadline, diagnostics=None):
            captured["proof_deadline"] = deadline
            return real_proof(backup, deadline, diagnostics)

        with mock.patch.object(helper.time, "monotonic", side_effect=lambda: clock[0]), \
             mock.patch.object(helper, "create_verified_database_backup", side_effect=backup_then_expire), \
             mock.patch.object(helper, "_backup_proof", side_effect=actual_proof_at_expiry):
            self.assertEqual(self.activate(), "failed")

        self.assertEqual(captured["proof_deadline"], captured["deadline"])
        self.assertTrue(captured["backup"].exists())
        self.assertIn(captured["backup"], list(self.root.glob("database-backup-*.sqlite")))
        with contextlib.closing(sqlite3.connect(database_path, timeout=0.05)) as writer, writer:
            writer.execute("INSERT INTO changes VALUES ('guard released')")
            writer_succeeded.append(True)
        self.assertEqual(writer_succeeded, [True])
        self.assertEqual(self.launch.starts, [self.manifest.target_plist])
        self.assertNotIn("migrate", (self.root / "events").read_text())
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), self.previous_binary)
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertEqual(proof["boundary"], "not_crossed")
        self.assertEqual(proof["recovery_outcome"], "predecessor_recovered_verified")

    def test_backup_timeout_before_boundary_recovers_unchanged_predecessor(self):
        clock = [100.0]
        real_restore = helper.restore_predecessor
        recovery_writer_succeeded = []

        def monotonic():
            return clock[0]

        def timeout_backup(database_path, directory, *, timeout, diagnostics=None):
            self.assertEqual(timeout, helper.BACKUP_VERIFICATION_TIMEOUT_SECS)
            self.assertEqual(clock[0], 100.0)
            with contextlib.closing(sqlite3.connect(database_path, timeout=0.05)) as writer, writer:
                with self.assertRaisesRegex(sqlite3.OperationalError, "locked"):
                    writer.execute("INSERT INTO changes VALUES ('competing writer')")
            clock[0] += helper.BACKUP_VERIFICATION_TIMEOUT_SECS + 0.001
            raise helper.ActivationError("SQLite backup and verification deadline exceeded")

        def restore_after_release(manifest, launchctl, captured, previous_plist):
            with contextlib.closing(sqlite3.connect(self.database, timeout=0.05)) as writer, writer:
                writer.execute("INSERT INTO changes VALUES ('recovery writer')")
            recovery_writer_succeeded.append(True)
            return real_restore(manifest, launchctl, captured, previous_plist)

        before_binary = Path(self.manifest.target_binary).read_bytes()
        before_plist = Path(self.manifest.target_plist).read_bytes()
        with mock.patch.object(helper.time, "monotonic", side_effect=monotonic), \
             mock.patch.object(helper, "create_verified_database_backup", side_effect=timeout_backup), \
             mock.patch.object(helper, "restore_predecessor", side_effect=restore_after_release):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(recovery_writer_succeeded, [True])
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), before_binary)
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), before_plist)
        self.assertEqual(self.changes(), [("recovery writer",)])
        self.assertEqual((self.root / "events").read_text().splitlines(), ["stop", "start"])
        self.assertFalse(list(self.root.glob("database-backup-*.sqlite")))
        self.assertNotIn("migrate", (self.root / "events").read_text())
        self.assertIn("unchanged predecessor restored and exact identity verified", self.status()["failure"])
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertEqual(proof["boundary"], "not_crossed")
        self.assertEqual(proof["recovery_outcome"], "predecessor_recovered_verified")
        self.assertEqual(proof["outcome"], "failed")
        self.assertEqual(helper.MIGRATION_TIMEOUT_SECS, 600)

    def test_writer_guard_blocks_competing_writer_until_migration_handoff(self):
        original = helper._backup_proof
        attempted = []

        def compete(backup, deadline, diagnostics=None):
            with contextlib.closing(sqlite3.connect(self.database, timeout=0.05)) as writer, writer:
                writer.execute("PRAGMA busy_timeout=50")
                with self.assertRaisesRegex(sqlite3.OperationalError, "locked"):
                    writer.execute("INSERT INTO changes VALUES ('competing writer')")
            attempted.append(True)
            return original(backup, deadline, diagnostics)

        with mock.patch.object(helper, "_backup_proof", side_effect=compete):
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(attempted, [True])
        self.assertEqual(self.changes(), [("migration_started",)])

    def test_writer_guard_acquisition_is_bounded_before_boundary(self):
        blocker = sqlite3.connect(self.database, timeout=0.05)
        self.addCleanup(blocker.close)
        blocker.execute("BEGIN IMMEDIATE")
        started = helper.time.monotonic()
        try:
            with self.assertRaises(sqlite3.OperationalError):
                with helper.sqlite_writer_guard(str(self.database), timeout=0.05):
                    self.fail("writer guard acquired despite competing reservation")
        finally:
            blocker.rollback()
        self.assertLess(helper.time.monotonic() - started, 0.75)

    def test_writer_guard_released_before_predecessor_recovery(self):
        def failed_backup(*_args, **_kwargs):
            with contextlib.closing(sqlite3.connect(self.database, timeout=0.05)) as writer, writer:
                with self.assertRaises(sqlite3.OperationalError):
                    writer.execute("INSERT INTO changes VALUES ('blocked')")
            raise helper.ActivationError("backup failed")

        def recover(_manifest, _launchctl, _captured, _previous_plist):
            with contextlib.closing(sqlite3.connect(self.database, timeout=0.05)) as writer, writer:
                writer.execute("INSERT INTO changes VALUES ('after guard release')")
            return 99

        with mock.patch.object(helper, "create_verified_database_backup", side_effect=failed_backup), \
             mock.patch.object(helper, "restore_predecessor", side_effect=recover):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.changes(), [("after guard release",)])

    def test_deploy_manifest_protocol_three_accepts_and_two_rejects(self):
        raw = dataclasses.asdict(self.manifest)
        raw["expected"] = dataclasses.asdict(self.manifest.expected)
        path = self.root / "manifest.json"
        raw["manifest_version"] = 3
        path.write_text(json.dumps(raw))
        self.assertEqual(helper.Manifest.load(path).manifest_version, 3)
        raw["manifest_version"] = 2
        path.write_text(json.dumps(raw))
        with self.assertRaisesRegex(helper.ActivationError, "unsupported deployment manifest"):
            helper.Manifest.load(path)

    def test_success_records_durable_backup_proof(self):
        self.assertEqual(self.activate(), "committed")
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertEqual(proof["outcome"], "candidate_verified")
        self.assertEqual(proof["backup"]["backup_sha256"], helper.sha256(Path(proof["backup"]["backup_path"])))
        self.assertEqual(proof["boundary"], "crossed")

    def test_unhealthy_predecessor_without_captured_identity_does_not_block_forward_repair(self):
        with mock.patch.object(helper, "capture_predecessor", return_value=None):
            self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.launch.pid, 99)
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertFalse(proof["recovery_eligible"])
        self.assertIsNone(proof["predecessor"])

    def test_pre_stop_capacity_failure_preserves_predecessor_without_quarantine(self):
        with mock.patch.object(helper, "_preflight_backup_capacity", side_effect=helper.ActivationError("no capacity")):
            self.assertEqual(self.activate(), "precondition_failed")
        self.assertEqual(self.launch.stops, 0)
        self.assertEqual(self.launch.starts, [])
        self.assertEqual(self.launch.pid, 41)
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), self.previous_binary)
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), Path(self.manifest.candidate_plist).read_bytes())

    def test_post_stop_capacity_failure_restores_only_verified_absent_predecessor(self):
        capacity = helper._preflight_backup_capacity
        calls = 0
        def fail_after_stop(database_path, directory):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise helper.ActivationError("capacity changed after stop")
            return capacity(database_path, directory)
        with mock.patch.object(helper, "_preflight_backup_capacity", side_effect=fail_after_stop):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.launch.starts, [self.manifest.target_plist])
        self.assertEqual(self.launch.pid, 99)
        self.assertTrue(Path(self.manifest.target_plist).exists())
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), self.previous_binary)
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertIn("unchanged predecessor restored and exact identity verified", self.status()["failure"])

    def test_recovered_canonical_plist_passes_actual_dev_restart_preflight(self):
        dev_spec = importlib.util.spec_from_file_location("devpy_recovery_preflight_test", ROOT / "dev.py")
        dev = importlib.util.module_from_spec(dev_spec)
        sys.modules[dev_spec.name] = dev
        dev_spec.loader.exec_module(dev)
        plist = Path(self.manifest.candidate_plist)
        data = plistlib.loads(plist.read_bytes())
        installed_binary = self.root / "phoenix-ide"
        Path(self.manifest.target_binary).rename(installed_binary)
        self.manifest = dataclasses.replace(self.manifest, target_binary=str(installed_binary))
        data["ProgramArguments"] = [str(installed_binary)]
        data.update({"KeepAlive": True, "Sockets": {"Listeners": {"SockServiceName": "18111", "SockType": "stream"}}})
        plist.write_bytes(plistlib.dumps(data))
        Path(self.manifest.target_plist).write_bytes(plist.read_bytes())
        self.manifest = dataclasses.replace(self.manifest, candidate_plist_sha256=helper.sha256(plist))
        Path(self.manifest.deployed_sha_path).write_text("b" * 40 + "\n")
        self.launch.manifest = self.manifest
        capacity = helper._preflight_backup_capacity
        calls = 0
        def fail_after_stop(database_path, directory):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise helper.ActivationError("capacity changed after stop")
            return capacity(database_path, directory)
        def start(old_pid, *, plist_path=None):
            self.assertEqual(plist_path, self.manifest.target_plist)
            self.assertTrue(Path(plist_path).is_file())
            return FakeLaunchctl.start(self.launch, old_pid, plist_path=plist_path)
        with mock.patch.object(helper, "_preflight_backup_capacity", side_effect=fail_after_stop), \
             mock.patch.object(self.launch, "start", side_effect=start):
            self.assertEqual(self.activate(), "failed")
        loaded_path = self.launch.starts[-1]
        inspection = dev.LoadedLaunchdJob("running", self.launch.pid, True, loaded_path,
                                          self.manifest.target_binary, ("18111",))
        identity = dev.RuntimeIdentity("0.9", "b" * 40)
        with mock.patch.object(dev, "LAUNCHD_INSTALL_DIR", self.root), \
             mock.patch.object(dev, "LAUNCHD_LABEL", self.manifest.label), \
             mock.patch.object(dev, "LAUNCHD_PLIST_PATH", Path(self.manifest.target_plist)), \
             mock.patch.object(dev, "PROD_SHA_PATH", Path(self.manifest.deployed_sha_path)), \
             mock.patch.object(dev, "_inspect_launchd_job", return_value=inspection), \
             mock.patch.object(dev, "_current_prod_identity", return_value=identity), \
             mock.patch.object(dev, "_binary_identity", return_value=identity):
            installed = dev._installed_launchd_runtime_for_restart()
        self.assertEqual(installed.pid, self.launch.pid)
        self.assertTrue(Path(loaded_path).samefile(self.manifest.target_plist))
        self.assertFalse((self.root / "previous.plist").exists())

    def test_recovery_start_failure_requarantines_published_canonical(self):
        def fail_start(_old_pid, *, plist_path=None):
            self.assertEqual(plist_path, self.manifest.target_plist)
            self.assertTrue(Path(plist_path).is_file())
            self.launch.starts.append(plist_path)
            self.launch.pid = 99
            raise helper.ActivationError("bootstrap failed after loading")
        with mock.patch.object(helper, "_reserve_before_boundary", side_effect=helper.ActivationError("no reserve")), \
             mock.patch.object(self.launch, "start", side_effect=fail_start):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.launch.starts, [self.manifest.target_plist])
        self.assertEqual(self.launch.pid, 99)
        self.assertEqual(self.launch.stops, 1)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue((self.root / "previous.plist").exists())
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertIn("teardown uncertain", self.status()["failure"])

    def test_recovery_changed_pid_is_not_stopped(self):
        def changed_pid(_manifest, _identity):
            self.launch.pid = 12345
            raise helper.ActivationError("another runtime appeared")
        with mock.patch.object(helper, "_reserve_before_boundary", side_effect=helper.ActivationError("no reserve")):
            self.assertEqual(self.activate(changed_pid), "failed")
        self.assertEqual(self.launch.pid, 12345)
        self.assertEqual(self.launch.stops, 1)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue((self.root / "previous.plist").exists())
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertIn("teardown uncertain", self.status()["failure"])

    def test_real_launchctl_stop_rechecks_expected_recovery_pid(self):
        launch = helper.Launchctl(self.manifest)
        with mock.patch.object(launch, "inspect", return_value=("running", 12345)), \
             mock.patch.object(launch, "run") as run:
            with self.assertRaisesRegex(helper.ActivationError, "changed before bootout"):
                launch.stop(expected_pid=99)
        run.assert_not_called()

    def test_recovery_health_failure_requarantines_published_canonical(self):
        with mock.patch.object(helper, "_reserve_before_boundary", side_effect=helper.ActivationError("no reserve")):
            self.assertEqual(self.activate(helper.ActivationError("predecessor health failed")), "failed")
        self.assertEqual(self.launch.starts, [self.manifest.target_plist])
        self.assertIsNone(self.launch.pid)
        self.assertEqual(self.launch.stops, 2)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue((self.root / "previous.plist").exists())
        self.assertTrue(Path(self.manifest.active_path).exists())

    def test_recovery_publication_ambiguous_failure_requarantines_canonical(self):
        original = helper.atomic_write
        def ambiguous(path, data, mode=0o600):
            result = original(path, data, mode)
            if path == Path(self.manifest.target_plist):
                raise OSError("publication fsync uncertain")
            return result
        with mock.patch.object(helper, "_reserve_before_boundary", side_effect=helper.ActivationError("no reserve")), \
             mock.patch.object(helper, "atomic_write", side_effect=ambiguous):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.launch.starts, [])
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue((self.root / "previous.plist").exists())
        self.assertTrue(Path(self.manifest.active_path).exists())

    def test_recovery_publication_failure_never_starts_and_requarantines(self):
        capacity = helper._preflight_backup_capacity
        original_write = helper.atomic_write
        calls = 0

        def fail_after_stop(database_path, directory):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise helper.ActivationError("capacity changed after stop")
            return capacity(database_path, directory)

        def fail_canonical(path, data, mode=0o600):
            if path == Path(self.manifest.target_plist):
                raise OSError("cannot publish predecessor")
            return original_write(path, data, mode)

        with mock.patch.object(helper, "_preflight_backup_capacity", side_effect=fail_after_stop), mock.patch.object(
            helper, "atomic_write", side_effect=fail_canonical,
        ):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.launch.starts, [])
        self.assertIsNone(self.launch.pid)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue((self.root / "previous.plist").exists())
        self.assertTrue(Path(self.manifest.active_path).exists())
        proof = json.loads((self.root / "status.proof.json").read_text())
        self.assertNotEqual(proof.get("recovery_outcome"), "predecessor_recovered_verified")
        self.assertIn("recovery uncertainty", self.status()["failure"])

    def test_untrusted_predecessor_does_not_block_forward_repair(self):
        Path(self.manifest.target_binary).write_bytes(b"untrusted artifact")
        self.assertEqual(self.activate(), "committed")
        self.assertEqual(self.launch.stops, 1)
        self.assertEqual(self.changes(), [("migration_started",)])
        self.assertIsNone(json.loads((self.root / "status.proof.json").read_text())["predecessor"])

    def test_boundary_write_ambiguity_never_starts_or_restores_predecessor(self):
        actual = helper.write_status
        def ambiguous(manifest, state, **kwargs):
            if state == "migration_started":
                actual(manifest, state, **kwargs)
                raise OSError("directory fsync result ambiguous")
            return actual(manifest, state, **kwargs)
        with mock.patch.object(helper, "write_status", side_effect=ambiguous):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.launch.starts, [])
        self.assertIsNone(self.launch.pid)
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), self.previous_binary)
        self.assertTrue(Path(self.manifest.active_path).exists())

    def test_poststart_failure_proof_write_error_does_not_prevent_teardown(self):
        original = helper.write_proof

        def failed_proof(manifest, **fields):
            if fields.get("outcome") == "failed":
                raise OSError(28, "No space left on device")
            return original(manifest, **fields)

        with mock.patch.object(helper, "write_proof", side_effect=failed_proof):
            self.assertEqual(self.activate(helper.ActivationError("unhealthy candidate")), "failed")
        self.assertIsNone(self.launch.pid)
        self.assertEqual(self.launch.stops, 2)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertIn("failure proof persistence failed", self.status()["failure"])

    def test_poststart_terminal_status_failure_keeps_claim_after_candidate_stop(self):
        actual = helper.write_status

        def failed_status(manifest, state, **kwargs):
            if state == "failed":
                raise OSError(28, "No space left on device")
            return actual(manifest, state, **kwargs)

        with mock.patch.object(helper, "write_status", side_effect=failed_status):
            self.assertEqual(self.activate(helper.ActivationError("unhealthy candidate")), "failed")
        self.assertIsNone(self.launch.pid)
        self.assertEqual(self.launch.stops, 2)
        self.assertFalse(Path(self.manifest.target_plist).exists())
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertEqual(self.status()["state"], "verifying")

    def test_terminal_status_write_error_retains_claim_after_safe_recovery(self):
        actual = helper.write_status

        def failed_status(manifest, state, **kwargs):
            if state == "failed":
                raise OSError(28, "No space left on device")
            return actual(manifest, state, **kwargs)

        with mock.patch.object(helper, "_reserve_before_boundary", side_effect=helper.ActivationError("no reserve")), \
             mock.patch.object(helper, "write_status", side_effect=failed_status):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.launch.starts, [self.manifest.target_plist])
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertEqual(self.status()["state"], "stopping")

    def test_ambiguous_boundary_and_failure_persistence_errors_still_stop_candidate(self):
        actual_status = helper.write_status
        actual_proof = helper.write_proof

        def ambiguous(manifest, state, **kwargs):
            if state == "migration_started":
                actual_status(manifest, state, **kwargs)
                raise OSError("directory fsync result ambiguous")
            if state == "failed":
                raise OSError(28, "No space left on device")
            return actual_status(manifest, state, **kwargs)

        def failed_proof(manifest, **fields):
            if fields.get("outcome") == "failed":
                raise OSError(28, "No space left on device")
            return actual_proof(manifest, **fields)

        with mock.patch.object(helper, "write_status", side_effect=ambiguous), \
             mock.patch.object(helper, "write_proof", side_effect=failed_proof):
            self.assertEqual(self.activate(), "failed")
        self.assertEqual(self.launch.starts, [])
        self.assertIsNone(self.launch.pid)
        self.assertTrue(Path(self.manifest.active_path).exists())
        self.assertEqual(self.status()["state"], "migration_started")

    def test_staged_cleanup_failure_does_not_override_failure_or_claim(self):
        unlink = Path.unlink

        def fail_staged_cleanup(path, *args, **kwargs):
            if path.name.startswith(".installed.") and not path.name.startswith(".installed.plist."):
                raise OSError("staged binary busy")
            return unlink(path, *args, **kwargs)

        with mock.patch.object(Path, "unlink", autospec=True, side_effect=fail_staged_cleanup):
            self.assertEqual(self.activate(helper.ActivationError("unhealthy candidate")), "failed")
        self.assertIsNone(self.launch.pid)
        self.assertEqual(self.status()["state"], "failed")
        self.assertTrue(Path(self.manifest.active_path).exists())

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
    def __init__(self, manifest):
        self.manifest = manifest; self.pid = 41
        self.target = f"gui/{manifest.uid}/{manifest.label}"
        self.domain = f"gui/{manifest.uid}"
    def inspect(self):
        return ("running", self.pid) if self.pid is not None else ("not_loaded", None)
    def wait(self, predicate, _deadline, _description):
        state = self.inspect()
        if not predicate(*state): raise RuntimeError(f"unexpected fake state: {state}")
        return state
    def run(self, command, **kwargs):
        if command[:2] == ["launchctl", "bootout"]: self.pid = None
        return h.subprocess.CompletedProcess(command, 0, "", "")
    def stop(self):
        old, self.pid = self.pid, None
        return old
h.Launchctl = Launch
h.fetch_identity = lambda *args, **kwargs: h.Identity("0.9", "b" * 40)
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
        self.assertEqual(Path(self.manifest.target_binary).read_bytes(), self.previous_binary)
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), Path(self.manifest.candidate_plist).read_bytes())
        self.assertFalse(Path(self.manifest.active_path).exists())
        self.assertEqual(self.changes(), [])

    def test_installer_staging_failure_happens_before_stop(self):
        with mock.patch.object(helper, "prepare_atomic_install", side_effect=OSError("no space")):
            self.assertEqual(self.activate(), "precondition_failed")
        self.assertEqual(self.launch.stops, 0)
        self.assertEqual(Path(self.manifest.target_plist).read_bytes(), Path(self.manifest.candidate_plist).read_bytes())

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
             mock.patch.object(helper, "wait_for_identity", side_effect=health), \
             mock.patch.object(helper, "fetch_identity", return_value=helper.Identity("0.9", "b" * 40)):
            self.assertEqual(helper.activate(self.manifest), "failed")
        self.assertIsNone(self.launch.pid)
        self.assertIn("PID changed", self.status()["failure"])
        self.assertFalse(Path(self.manifest.target_plist).exists())

    def test_migration_timeout_stops_without_starting_any_runtime(self):
        run = helper.subprocess.run
        def execute(command, **kwargs):
            if command[-1] == "--migrate-only":
                self.assertEqual(kwargs["timeout"], 600)
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
