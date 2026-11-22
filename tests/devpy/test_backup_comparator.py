"""Behavioral coverage for SQLite backup equivalence checks."""
import contextlib
import importlib.util
import sqlite3
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]


def load_helper(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


helper = load_helper("backup_comparator_launchd_helper", ROOT / "scripts/launchd_deploy_helper.py")


class BackupComparatorTests(unittest.TestCase):
    def compare_outcomes(self, setup, expected, mutate=None):
        source, backup = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
        source_factory = lambda value: value.decode("utf-8")
        backup_factory = lambda value: value.decode("utf-8")
        source.text_factory, backup.text_factory = source_factory, backup_factory
        diagnostics = {}
        try:
            setup(source)
            setup(backup)
            if mutate is not None:
                mutate(backup)
            if expected == "reject":
                with self.assertRaises(helper.ActivationError):
                    helper._verify_logical_equivalence(
                        source, backup, helper.time.monotonic() + 10, diagnostics,
                    )
            else:
                helper._verify_logical_equivalence(
                    source, backup, helper.time.monotonic() + 10, diagnostics,
                )
            self.assertEqual((source.text_factory, backup.text_factory),
                             (source_factory, backup_factory))
            for timing in diagnostics.get("stage_ms", {}).values():
                self.assertGreaterEqual(timing, 0)
            return diagnostics
        finally:
            source.close()
            backup.close()

    def test_injective_text_representation_preserves_storage_classes_and_bytes(self):
        database = sqlite3.connect(":memory:")
        self.addCleanup(database.close)
        database.text_factory = lambda raw: (raw,)
        cases = (("NULL", type(None), None), ("42", int, 42), ("1.25", float, 1.25),
                 ("CAST(X'ff8000' AS TEXT)", tuple, (b"\xff\x80\x00",)),
                 ("X'ff8000'", bytes, b"\xff\x80\x00"),
                 ("CAST(X'' AS TEXT)", tuple, (b"",)), ("X''", bytes, b""))
        for expression, value_type, value in cases:
            with self.subTest(expression=expression):
                with contextlib.closing(database.execute(f"SELECT {expression}")) as cursor:
                    actual = cursor.fetchone()[0]
                self.assertIs(type(actual), value_type)
                self.assertEqual(actual, value)

    def test_mismatch_error_is_content_free_and_restores_distinct_factories(self):
        source, backup = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
        self.addCleanup(source.close)
        self.addCleanup(backup.close)
        source_factory = lambda raw: raw.decode("utf-8")
        backup_factory = lambda raw: raw
        source.text_factory, backup.text_factory = source_factory, backup_factory
        for database, value in ((source, "private-source"), (backup, "private-backup")):
            database.execute('CREATE TABLE "private-table"("private-column")')
            database.execute('INSERT INTO "private-table" VALUES (?)', (value,))
        with self.assertRaises(helper.ActivationError) as raised:
            helper._verify_logical_equivalence(source, backup, helper.time.monotonic() + 10)
        self.assertEqual(str(raised.exception), "SQLite backup row differs in table ordinal 0")
        self.assertIs(source.text_factory, source_factory)
        self.assertIs(backup.text_factory, backup_factory)

    def test_all_storage_classes(self):
        def setup(db):
            db.execute("CREATE TABLE cells(k INTEGER PRIMARY KEY, v)")
            db.executescript("""INSERT INTO cells VALUES
                (1,NULL),(2,9223372036854775807),(3,1.25),(4,'text'),(5,X'ff00'),
                (6,CAST(X'ff80' AS TEXT));""")
        self.compare_outcomes(setup, "accept")

    def test_equal_nonreal_rows_count_all_cells_and_restore_text_factories(self):
        def setup(db):
            db.execute("CREATE TABLE cells(k INTEGER PRIMARY KEY, n, i, t, invalid, b)")
            db.executescript("""INSERT INTO cells VALUES
                (1,NULL,9223372036854775807,'text',CAST(X'ff80' AS TEXT),X'ff00'),
                (2,NULL,-1,'more',CAST(X'80ff' AS TEXT),X'0001');""")

        diagnostics = self.compare_outcomes(setup, "accept")
        self.assertEqual({key: diagnostics[key] for key in (
            "rows_compared", "cells_compared", "table_rows_compared",
            "table_cells_compared", "tables_completed", "table_ordinal",
        )}, {
            "rows_compared": 3, "cells_compared": 17,
            "table_rows_compared": 1, "table_cells_compared": 5,
            "tables_completed": 2, "table_ordinal": 2,
        })

    def test_equal_real_values_in_declared_and_untyped_columns(self):
        for declaration in ("REAL", ""):
            with self.subTest(declaration=declaration or "untyped"):
                def setup(db):
                    db.execute(f"CREATE TABLE cells(v {declaration})")
                    db.execute("INSERT INTO cells VALUES (1.25)")
                    self.assertEqual(db.execute("SELECT typeof(v) FROM cells").fetchone(), ("real",))

                diagnostics = self.compare_outcomes(setup, "accept")
                self.assertEqual(diagnostics["rows_compared"], 2)
                self.assertEqual(diagnostics["cells_compared"], 6)
                self.assertEqual(diagnostics["tables_completed"], 2)

    def test_equal_nonreal_rows_still_check_deadline(self):
        source, backup = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
        source_factory = lambda value: value.decode("utf-8")
        backup_factory = lambda value: value.decode("utf-8")
        source.text_factory, backup.text_factory = source_factory, backup_factory
        try:
            for db in (source, backup):
                db.execute("CREATE TABLE cells(v)")
                db.executemany("INSERT INTO cells VALUES (?)", ((1,), (2,)))
            diagnostics = {}

            def clock():
                return 101.0 if diagnostics.get("rows_compared", 0) >= 2 else 100.0

            with mock.patch.object(helper.time, "monotonic", side_effect=clock):
                with self.assertRaisesRegex(helper.ActivationError, "deadline exceeded"):
                    helper._verify_logical_equivalence(source, backup, 101.0, diagnostics)
            self.assertEqual(diagnostics["rows_compared"], 2)
            self.assertEqual(diagnostics["cells_compared"], 2)
            self.assertEqual(diagnostics["table_rows_compared"], 2)
            self.assertEqual(diagnostics["table_cells_compared"], 2)
            self.assertEqual(diagnostics["tables_completed"], 0)
            self.assertIs(source.text_factory, source_factory)
            self.assertIs(backup.text_factory, backup_factory)
        finally:
            source.close()
            backup.close()

    def test_batched_late_value_and_rowid_mismatches_keep_rowwise_counters(self):
        boundaries = (16, 17, 32, 33, 40)
        for position in boundaries:
            with self.subTest(kind="value", position=position):
                def setup_value(db):
                    db.execute("CREATE TABLE cells(id INTEGER PRIMARY KEY, value)")
                    db.executemany("INSERT INTO cells VALUES (?, ?)",
                                   ((row, row) for row in range(1, 41)))
                diagnostics = self.compare_outcomes(
                    setup_value, "reject",
                    lambda db, row=position: db.execute(
                        "UPDATE cells SET value=-1 WHERE id=?", (row,)),
                )
                self.assertEqual(diagnostics["table_rows_compared"], position - 1)
                self.assertEqual(diagnostics["table_cells_compared"], position * 2)

            with self.subTest(kind="rowid", position=position):
                def setup_rowid(db):
                    db.execute("CREATE TABLE cells(id INTEGER PRIMARY KEY, value)")
                    db.executemany("INSERT INTO cells VALUES (?, ?)",
                                   ((row, row) for row in range(1, 41)))
                diagnostics = self.compare_outcomes(
                    setup_rowid, "reject",
                    lambda db, row=position: db.execute(
                        "UPDATE cells SET rowid=100000 WHERE id=?", (row,)),
                )
                self.assertEqual(diagnostics["table_rows_compared"], position - 1)
                self.assertEqual(diagnostics["table_cells_compared"], (position - 1) * 2)

            with self.subTest(kind="signed zero", position=position):
                def setup_zero(db):
                    db.execute("CREATE TABLE cells(value)")
                    db.executemany("INSERT INTO cells(rowid,value) VALUES (?,?)",
                                   ((row, -0.0 if row == position else float(row))
                                    for row in range(1, 41)))
                diagnostics = self.compare_outcomes(
                    setup_zero, "reject",
                    lambda db, row=position: db.execute(
                        "UPDATE cells SET value=0.0 WHERE rowid=?", (row,)),
                )
                self.assertEqual(diagnostics["table_rows_compared"], position - 1)
                self.assertEqual(diagnostics["table_cells_compared"], position)

    def test_batched_extra_and_missing_rows_after_each_boundary(self):
        for count in (16, 17, 32, 33, 40):
            for extra in (True, False):
                with self.subTest(count=count, extra=extra):
                    def setup(db):
                        db.execute("CREATE TABLE cells(id INTEGER PRIMARY KEY, value)")
                        db.executemany("INSERT INTO cells VALUES (?,?)",
                                       ((row, row) for row in range(1, count + 1)))
                    def mutate(db):
                        if extra:
                            db.execute("INSERT INTO cells VALUES (?,?)", (count + 1, count + 1))
                        else:
                            db.execute("DELETE FROM cells WHERE id=?", (count,))
                    diagnostics = self.compare_outcomes(setup, "reject", mutate)
                    self.assertEqual(diagnostics["table_rows_compared"], count - (0 if extra else 1))
                    self.assertEqual(diagnostics["table_cells_compared"],
                                     2 * (count - (0 if extra else 1)))

    def test_deadline_after_full_batch_reports_completed_batch_before_failure(self):
        source, backup = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
        try:
            for db in (source, backup):
                db.execute("CREATE TABLE cells(value)")
                db.executemany("INSERT INTO cells VALUES (?)", ((row,) for row in range(40)))
            diagnostics = {}

            def clock():
                return 101.0 if diagnostics.get("rows_compared", 0) >= 16 else 100.0

            with mock.patch.object(helper.time, "monotonic", side_effect=clock):
                with self.assertRaisesRegex(helper.ActivationError, "deadline exceeded"):
                    helper._verify_logical_equivalence(source, backup, 101.0, diagnostics)
            self.assertEqual(diagnostics["table_rows_compared"], 16)
            self.assertEqual(diagnostics["table_cells_compared"], 16)
            self.assertEqual(diagnostics["tables_completed"], 0)
        finally:
            source.close()
            backup.close()

    def test_numeric_type_and_signed_zero_mismatches_at_batch_boundaries(self):
        for position in (15, 16, 17):
            for left, right in ((1, 1.0), (-0.0, 0.0)):
                for reverse in (False, True):
                    source_value, backup_value = (right, left) if reverse else (left, right)
                    with self.subTest(position=position, left=repr(source_value),
                                      right=repr(backup_value)):
                        def setup(db):
                            db.execute("CREATE TABLE cells(v)")
                            db.executemany("INSERT INTO cells VALUES (?)", ((123,) for _ in range(33)))
                            db.execute("UPDATE cells SET v=? WHERE rowid=?", (source_value, position))
                            stored = db.execute("SELECT v FROM cells WHERE rowid=?", (position,)).fetchone()[0]
                            self.assertIs(type(stored), type(source_value))
                            if type(stored) is float:
                                self.assertEqual(struct.pack(">d", stored), struct.pack(">d", source_value))

                        diagnostics = self.compare_outcomes(
                            setup, "reject", lambda db: db.execute(
                                "UPDATE cells SET v=? WHERE rowid=?", (backup_value, position)),
                        )
                        self.assertEqual(diagnostics["table_rows_compared"], position - 1)
                        self.assertEqual(diagnostics["table_cells_compared"], position)
                        self.assertEqual(diagnostics["rows_compared"], position - 1)
                        self.assertEqual(diagnostics["cells_compared"], position)

    def test_text_blob_bytes_at_batch_boundaries_in_both_directions(self):
        for position in (15, 16, 17):
            for payload in ("ff8000", "00", ""):
                text_expression = f"CAST(X'{payload}' AS TEXT)"
                blob_expression = f"X'{payload}'"
                for left, right, outcome in (
                    (text_expression, blob_expression, "reject"),
                    (blob_expression, text_expression, "reject"),
                    (text_expression, text_expression, "accept"),
                    (blob_expression, blob_expression, "accept"),
                ):
                    with self.subTest(position=position, left=left, right=right):
                        def setup(db):
                            db.execute("CREATE TABLE cells(v)")
                            db.executemany("INSERT INTO cells VALUES (?)", ((123,) for _ in range(33)))
                            db.execute(f"UPDATE cells SET v={left} WHERE rowid=?", (position,))

                        diagnostics = self.compare_outcomes(
                            setup, outcome, lambda db: db.execute(
                                f"UPDATE cells SET v={right} WHERE rowid=?", (position,)),
                        )
                        self.assertEqual(diagnostics["table_rows_compared"],
                                         position - 1 if outcome == "reject" else 1)
                        self.assertEqual(diagnostics["rows_compared"],
                                         position - 1 if outcome == "reject" else 34)
                        self.assertEqual(diagnostics["cells_compared"],
                                         position if outcome == "reject" else 38)

    def test_distinct_factories_across_multiple_tables_success_and_late_failure(self):
        def setup(db):
            db.executescript("""CREATE TABLE after_cells(v TEXT);
                INSERT INTO after_cells VALUES ('second table');
                CREATE TABLE cells(v TEXT);
                INSERT INTO cells VALUES (CAST(X'ff8000' AS TEXT));""")

        accepted = self.compare_outcomes(setup, "accept")
        self.assertEqual(accepted["tables_completed"], 3)
        rejected = self.compare_outcomes(setup, "reject", lambda db: db.execute(
            "UPDATE after_cells SET v='different'"))
        self.assertEqual(rejected["tables_completed"], 1)
        self.assertEqual(rejected["table_ordinal"], 2)
        self.assertEqual(rejected["rows_compared"], 1)
        self.assertEqual(rejected["cells_compared"], 2)

    def test_scale_fixture_counters_match_independent_sql_inventory(self):
        fixture = load_helper("backup_comparator_scale_fixture",
                              ROOT / "tests/integration/backup_verification_scale.py")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source.sqlite"
            composition = fixture.create_fixture(
                path, minimum_bytes=1, rows_per_table=17,
                text_bytes=32, blob_bytes=32, extra_columns=1,
            )
            self.assertEqual(composition["documents_rows"], 17)
            self.assertEqual(composition["keyed_records_rows"], 17)
            self.assertTrue(composition["fts5_shadow_rows"])
            with contextlib.closing(sqlite3.connect(path)) as source, \
                    contextlib.closing(sqlite3.connect(":memory:")) as backup:
                source.backup(backup)
                expected = dict.fromkeys(("rows_compared", "cells_compared", "tables_completed",
                                          "table_ordinal", "table_rows_compared", "table_cells_compared"), 0)
                with contextlib.closing(source.execute("PRAGMA main.table_list")) as cursor:
                    tables = cursor.fetchall()
                for schema, name, kind, *_ in tables:
                    if schema != "main":
                        continue
                    expected["table_ordinal"] += 1
                    expected["table_rows_compared"] = expected["table_cells_compared"] = 0
                    if kind == "virtual":
                        continue
                    quoted = '"' + name.replace('"', '""') + '"'
                    with contextlib.closing(source.execute(f"SELECT count(*) FROM {quoted}")) as cursor:
                        count = cursor.fetchone()[0]
                    with contextlib.closing(source.execute(f"PRAGMA table_xinfo({quoted})")) as cursor:
                        width = sum(row[6] not in (1, 2) for row in cursor.fetchall())
                    expected["rows_compared"] += count
                    expected["cells_compared"] += count * width
                    expected["tables_completed"] += 1
                    expected["table_rows_compared"] = count
                    expected["table_cells_compared"] = count * width
                diagnostics = {}
                helper._verify_logical_equivalence(
                    source, backup, helper.time.monotonic() + 10, diagnostics,
                )
                self.assertEqual({key: diagnostics[key] for key in expected}, expected)

    def test_retained_row_cursors_close_before_factories_restore_on_all_exits(self):
        for case in ("success", "mismatch", "leftfetch", "rightfetch", "backup-execute", "deadline"):
            with self.subTest(case=case):
                retained = []
                events = []
                test = self

                class RowCursor:
                    def __init__(self, cursor, owner):
                        self.cursor, self.owner = cursor, owner
                        self.closed = False
                        retained.append(self)

                    def fetchmany(self, count):
                        if case == self.owner.side + "fetch":
                            raise sqlite3.OperationalError("private injected fetch error")
                        return self.cursor.fetchmany(count)

                    def close(self):
                        test.assertIsNot(self.owner.text_factory, bytes)
                        test.assertFalse(self.closed)
                        self.cursor.close()
                        self.closed = True
                        events.append(self.owner.side)

                class ConnectionWitness:
                    def __init__(self, database, side):
                        self.database, self.side = database, side

                    @property
                    def text_factory(self):
                        return self.database.text_factory

                    @text_factory.setter
                    def text_factory(self, factory):
                        if factory is bytes:
                            test.assertTrue(all(cursor.closed for cursor in retained))
                        self.database.text_factory = factory

                    def set_progress_handler(self, *args):
                        return self.database.set_progress_handler(*args)

                    def execute(self, query, *args):
                        row_query = query.startswith("SELECT ") and "ORDER BY" in query and ".sqlite_schema " not in query
                        if row_query and case == "backup-execute" and self.side == "right":
                            raise sqlite3.OperationalError("private injected execute error")
                        cursor = self.database.execute(query, *args)
                        return RowCursor(cursor, self) if row_query else cursor

                with contextlib.closing(sqlite3.connect(":memory:")) as source, \
                        contextlib.closing(sqlite3.connect(":memory:")) as backup:
                    for db, value in ((source, 1), (backup, 2 if case == "mismatch" else 1)):
                        db.execute("CREATE TABLE cells(v)")
                        db.execute("INSERT INTO cells VALUES (?)", (value,))
                    source_factory = lambda raw: raw.decode("utf-8")
                    backup_factory = lambda raw: raw
                    source.text_factory, backup.text_factory = source_factory, backup_factory
                    left, right = ConnectionWitness(source, "left"), ConnectionWitness(backup, "right")

                    def clock():
                        return 101.0 if case == "deadline" and len(retained) >= 2 else 100.0

                    with mock.patch.object(helper.time, "monotonic", side_effect=clock):
                        if case == "success":
                            helper._verify_logical_equivalence(left, right, 101.0)
                        else:
                            message = ("SQLite logical verification deadline exceeded" if case == "deadline"
                                       else "SQLite backup row differs in table ordinal 0" if case == "mismatch"
                                       else "SQLite logical verification database operation failed")
                            with self.assertRaises(helper.ActivationError) as raised:
                                helper._verify_logical_equivalence(left, right, 101.0)
                            self.assertEqual(str(raised.exception), message)
                    self.assertIs(source.text_factory, source_factory)
                    self.assertIs(backup.text_factory, backup_factory)
                    self.assertEqual(len(retained), 4 if case == "success" else 1 if case == "backup-execute" else 2)
                    self.assertEqual(events, ["left"] if case == "backup-execute" else
                                     ["right", "left"] * (len(retained) // 2))
                    for cursor in retained:
                        self.assertTrue(cursor.closed)
                        with self.assertRaises(sqlite3.ProgrammingError):
                            cursor.cursor.fetchone()

    def test_empty_table_and_no_diagnostics(self):
        source, backup = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
        try:
            for db in (source, backup):
                db.execute("CREATE TABLE empty(value)")
                db.execute("CREATE TABLE real_values(value)")
                db.execute("INSERT INTO real_values VALUES (?)", (-0.0,))
            helper._verify_logical_equivalence(
                source, backup, helper.time.monotonic() + 10, None,
            )
        finally:
            source.close()
            backup.close()

    def test_numeric_and_storage_class_matrix(self):
        cases = (
            ("same bytes TEXT versus BLOB", "CAST(X'4142' AS TEXT)", "CAST(X'4142' AS BLOB)", "reject", ("text", "blob")),
            ("different invalid UTF-8 TEXT", "CAST(X'ff' AS TEXT)", "CAST(X'fe' AS TEXT)", "reject", ("text", "text")),
            ("different BLOB", "X'0102'", "X'0103'", "reject", ("blob", "blob")),
            ("INTEGER 1 versus REAL 1", "1", "1.0", "reject", ("integer", "real")),
        )
        for label, source_expr, backup_expr, expected, storage in cases:
            with self.subTest(case=label):
                source, backup = sqlite3.connect(":memory:"), sqlite3.connect(":memory:")
                try:
                    for db, expression in ((source, source_expr), (backup, backup_expr)):
                        db.execute("CREATE TABLE cells(v)")
                        db.execute(f"INSERT INTO cells VALUES ({expression})")
                    self.assertEqual(source.execute("SELECT typeof(v) FROM cells").fetchone()[0], storage[0])
                    self.assertEqual(backup.execute("SELECT typeof(v) FROM cells").fetchone()[0], storage[1])
                finally:
                    source.close()
                    backup.close()
                self.compare_outcomes(
                    lambda db, expr=source_expr: (db.execute("CREATE TABLE cells(v)"),
                                                   db.execute(f"INSERT INTO cells VALUES ({expr})")),
                    expected,
                    lambda db, expr=backup_expr: db.execute(f"UPDATE cells SET v={expr}"),
                )

        def setup_infinities(db):
            db.execute("CREATE TABLE cells(v)")
            db.executemany("INSERT INTO cells VALUES (?)", ((float("inf"),), (float("-inf"),)))
            self.assertEqual(db.execute("SELECT typeof(v),v FROM cells").fetchall(),
                             [("real", float("inf")), ("real", float("-inf"))])
        self.compare_outcomes(setup_infinities, "accept")

        def setup_nan_normalized_to_null(db):
            db.execute("CREATE TABLE cells(v)")
            db.execute("INSERT INTO cells VALUES (?)", (float("nan"),))
            self.assertEqual(db.execute("SELECT typeof(v),v FROM cells").fetchone(), ("null", None))
        self.compare_outcomes(setup_nan_normalized_to_null, "accept")
        self.compare_outcomes(
            setup_nan_normalized_to_null, "accept",
            lambda db: db.execute("UPDATE cells SET v=NULL"),
        )

    def test_binary64_signed_zero_is_not_collapsed(self):
        def setup(db):
            db.execute("CREATE TABLE cells(v)")
            db.execute("INSERT INTO cells VALUES (?)", (-0.0,))
            stored = db.execute("SELECT typeof(v),v FROM cells").fetchone()
            self.assertEqual(stored[0], "real")
            self.assertEqual(struct.pack(">d", stored[1]), struct.pack(">d", -0.0))
        self.compare_outcomes(setup, "accept")
        self.compare_outcomes(setup, "reject", lambda db: db.execute("UPDATE cells SET v=0.0"))

    def test_without_rowid_composite_desc_nocase_payload_difference(self):
        def setup(db):
            db.executescript("""CREATE TABLE keyed(a TEXT COLLATE NOCASE,b INTEGER,payload TEXT,
                PRIMARY KEY(a COLLATE NOCASE DESC,b ASC)) WITHOUT ROWID;
                INSERT INTO keyed VALUES('k',2,'same'),('K',1,'also same');""")
        self.compare_outcomes(setup, "accept")
        self.compare_outcomes(setup, "reject", lambda db: db.execute(
            "UPDATE keyed SET payload='changed' WHERE a='k' AND b=2"))

    def test_fts5_shadow_block_difference_is_rejected(self):
        def setup(db):
            db.executescript("CREATE VIRTUAL TABLE docs USING fts5(body); INSERT INTO docs VALUES('same terms');")
            columns = [row[1] for row in db.execute("PRAGMA table_info(docs_data)")]
            self.assertIn("block", columns)
            self.assertIsNotNone(db.execute("SELECT block FROM docs_data WHERE id=1").fetchone())
        def mutate(db):
            db.execute("UPDATE docs_data SET block=zeroblob(length(block)) WHERE id=1")
        self.compare_outcomes(setup, "accept")
        self.compare_outcomes(setup, "reject", mutate)

    def test_real_index_schema_change_is_rejected(self):
        def setup(db):
            db.execute("CREATE TABLE records(value TEXT)")
            db.execute("INSERT INTO records VALUES ('same')")
        self.compare_outcomes(setup, "reject", lambda db: db.execute(
            "CREATE INDEX records_value_idx ON records(value)"))

    def test_rowid_mutation_is_rejected(self):
        def setup(db):
            db.execute("CREATE TABLE records(value TEXT)")
            db.execute("INSERT INTO records(rowid,value) VALUES(7,'same')")
        self.compare_outcomes(setup, "reject", lambda db: db.execute("UPDATE records SET rowid=8"))

    def test_late_third_column_mismatch_reports_attempts_not_completed_rows(self):
        def setup(db):
            db.execute("CREATE TABLE cells(id INTEGER PRIMARY KEY,a,b,c) WITHOUT ROWID")
            db.execute("INSERT INTO cells VALUES (1,1,2,3)")
        diagnostics = self.compare_outcomes(setup, "reject", lambda db: db.execute(
            "UPDATE cells SET c=99 WHERE id=1"))
        self.assertEqual(diagnostics["cells_compared"], 4)
        self.assertEqual(diagnostics["rows_compared"], 0)
        self.assertEqual(diagnostics["tables_completed"], 0)



if __name__ == "__main__":
    unittest.main()
