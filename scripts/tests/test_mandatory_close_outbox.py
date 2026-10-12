#!/usr/bin/env python3
"""In-memory MIGRATION_111 -> Close outbox seam regression; stdlib only.

Run: python3 scripts/tests/test_mandatory_close_outbox.py
The fixture deliberately does not register a migration or open a Phoenix DB.
"""

import re
import sqlite3
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MIGRATIONS = (ROOT / "crates/phoenix-db/src/migrations.rs").read_text()
FRAGMENT = (ROOT / "crates/phoenix-db/src/coordinator_watches/mandatory_close_outbox.sql").read_text()
MANDATORY_SOURCE = (ROOT / "crates/phoenix-db/src/coordinator_watches/mandatory_close.rs").read_text()
WATCH_SOURCE = (ROOT / "crates/phoenix-db/src/coordinator_watches.rs").read_text()


def rust_queries(source, function, expected_bindings):
    """Extract the literal SQL and verify the live sqlx binding order, fail closed."""
    signature = re.search(rf"^([ ]*)(?:pub(?:\([^)]*\))? )?async fn {function}\(", source, re.M)
    if not signature:
        raise AssertionError(f"Missing Rust function {function}")
    end = source.index("\n" + signature.group(1) + "}", signature.end())
    body = source[signature.end():end]
    calls = re.findall(
        r'sqlx::(?:query_scalar|query)\(\s*"([^"\\]*)"\s*,?\s*\)(.*?)\.(?:fetch_all|execute)\(',
        body, re.S,
    )
    bindings = [re.findall(r"\.bind\(([^()]*)\)", suffix) for _, suffix in calls]
    if bindings != expected_bindings:
        raise AssertionError(f"Unexpected SQL calls/bindings in {function}: {bindings}")
    return [sql for sql, _ in calls]


ROOT_SQL, APPEND_SQL = rust_queries(
    MANDATORY_SOURCE, "append_mandatory_close_failure_event_tx",
    [["failure_occurrence_id"], ["failure_occurrence_id", "source_transcript_id"]],
)
[PENDING_SQL] = rust_queries(WATCH_SOURCE, "pending_coordinator_watch_events", [["limit"]])
[SUPPRESS_SQL] = rust_queries(WATCH_SOURCE, "suppress_stale_watch_event", [["event_id"]])


def append_from_rust_sql(db, failure_occurrence_id):
    """Execute live SQL; model Rust's exact-one-root error before the INSERT."""
    roots = db.execute(ROOT_SQL, (failure_occurrence_id,)).fetchall()
    if len(roots) != 1:
        raise ValueError("Close failure must have exactly one authoritative product root")
    return db.execute(APPEND_SQL, (failure_occurrence_id, roots[0][0])).rowcount


def pending_from_rust_sql(db, limit=16):
    cursor = db.execute(PENDING_SQL, (limit,))
    columns = [column[0] for column in cursor.description]
    return [dict(zip(columns, row)) for row in cursor.fetchall()]


FK_BARRIER_INSERT = "INSERT INTO mandatory_close_outbox_fk_barrier"


def migration(number):
    match = re.search(rf'const MIGRATION_{number:03}: &str = r"(.*?)";', MIGRATIONS, re.S)
    if not match:
        raise AssertionError(f"Missing actual migration {number}")
    return match.group(1)


def execute_fragment(db, before_statement=None):
    # executescript implicitly commits; individual complete statements do not.
    statement = ""
    for line in FRAGMENT.splitlines(keepends=True):
        statement += line
        if sqlite3.complete_statement(statement):
            if before_statement:
                before_statement(db, statement)
            db.execute(statement)
            statement = ""
    if statement.strip():
        raise AssertionError("Incomplete SQL fragment")


BASE = """
PRAGMA foreign_keys = ON;
CREATE TABLE product_conversations (
    id TEXT PRIMARY KEY, kind TEXT NOT NULL,
    ordinary_lifecycle TEXT
);
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    product_conversation_id TEXT REFERENCES product_conversations(id),
    runtime_role TEXT NOT NULL DEFAULT 'user',
    parent_conversation_id TEXT REFERENCES conversations(id),
    continued_in_conv_id TEXT REFERENCES conversations(id),
    coordinator_head INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE product_creation_jobs (id TEXT PRIMARY KEY);
CREATE TABLE close_obligations (
    product_conversation_id TEXT REFERENCES product_conversations(id) ON DELETE CASCADE,
    phase TEXT,
    attempt_id TEXT PRIMARY KEY
);
CREATE TABLE close_attempt_members (attempt_id TEXT REFERENCES close_obligations(attempt_id) ON DELETE CASCADE);
CREATE TABLE close_attempt_scopes (attempt_id TEXT REFERENCES close_obligations(attempt_id) ON DELETE CASCADE);
CREATE TABLE automatic_continuation_admissions (
    predecessor_conversation_id TEXT, phase TEXT
);
CREATE TABLE completed_continuation_handoffs (predecessor_conversation_id TEXT);
CREATE TABLE messages (
    message_id TEXT PRIMARY KEY, conversation_id TEXT REFERENCES conversations(id)
);
CREATE TABLE steering_messages (
    id TEXT PRIMARY KEY, conversation_id TEXT REFERENCES conversations(id)
);
CREATE TABLE durable_turns (
    id TEXT PRIMARY KEY, conversation_id TEXT REFERENCES conversations(id)
);
CREATE TABLE close_cleanup_failures (
    failure_occurrence_id TEXT PRIMARY KEY NOT NULL,
    attempt_id TEXT NOT NULL,
    cleanup_run_ordinal INTEGER NOT NULL,
    source_product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id),
    scope TEXT NOT NULL, resource_kind TEXT NOT NULL,
    identity_kind TEXT NOT NULL, identity_codec TEXT NOT NULL, identity_value TEXT NOT NULL,
    reason TEXT NOT NULL, detail TEXT NOT NULL,
    stop_certainty TEXT NOT NULL CHECK(stop_certainty IN
        ('conversation_and_processes_stopped', 'shutdown_uncertain')),
    confirmed_at_us INTEGER,
    occurred_at_us INTEGER NOT NULL,
    UNIQUE(attempt_id, cleanup_run_ordinal)
);
-- Close owns this immutable table, independently of the outbox fragment.
CREATE TRIGGER close_failure_immutable BEFORE UPDATE ON close_cleanup_failures
BEGIN SELECT RAISE(ABORT, 'Close failure is immutable'); END;
"""


BASE += """
ALTER TABLE close_cleanup_failures ADD COLUMN authority_kind TEXT NOT NULL DEFAULT 'captured_scopes';
CREATE TABLE close_cleanup_failure_resources (
    failure_occurrence_id TEXT, ordinal INTEGER, scope TEXT, resource_kind TEXT,
    identity_kind TEXT, identity_codec TEXT, identity_value TEXT, disposition TEXT
);
CREATE TABLE close_run_retry_effects (attempt_id TEXT, run_ordinal INTEGER, ordinal INTEGER);
CREATE TABLE close_run_retry_successes (attempt_id TEXT, run_ordinal INTEGER, ordinal INTEGER);
"""

BASE += re.findall(
    r"CREATE TRIGGER close_obligations_require_member_cleanup_before_delete\b.*?END;",
    MIGRATIONS, re.S,
)[0]


class MandatoryCloseOutboxTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:", isolation_level=None)
        self.addCleanup(self.db.close)
        self.db.executescript(BASE + migration(110) + migration(111))
        self.db.executescript("""
            INSERT INTO product_conversations VALUES ('source', 'ordinary', 'open');
            INSERT INTO product_conversations VALUES ('other', 'ordinary', 'open');
            INSERT INTO product_conversations VALUES ('global', 'coordinator', NULL);
            INSERT INTO conversations (id, product_conversation_id) VALUES ('root', 'source');
            INSERT INTO conversations (id, product_conversation_id) VALUES ('other-root', 'other');
            INSERT INTO conversations (id, product_conversation_id, runtime_role, coordinator_head)
                VALUES ('global-current', 'global', 'coordinator', 1);
            INSERT INTO conversations (id, product_conversation_id, runtime_role, continued_in_conv_id)
                VALUES ('global-old', 'global', 'coordinator', 'global-current');
            INSERT INTO conversations (id, product_conversation_id, parent_conversation_id)
                VALUES ('global-child', 'global', 'global-current');
            INSERT INTO coordinator_watches VALUES (1, 'source', 10, NULL);
        """)
        self.ordinary("old-accepted")
        self.accept("old-accepted", "durable_turns", "old-turn")
        self.db.execute("""INSERT INTO steering_messages
            (id, conversation_id, origin_kind, origin_subscription_event_id)
            VALUES ('old-steering', 'global-current', 'unknown_historical', NULL)""")
        # Seed all three referencing tables without triggering a second acceptance.
        self.db.execute("""UPDATE steering_messages SET origin_kind='subscription_event',
            origin_subscription_event_id='old-accepted' WHERE id='old-steering'""")
        self.db.execute("""INSERT INTO messages
            (message_id, conversation_id, origin_kind, origin_subscription_event_id)
            VALUES ('old-message', 'global-current', 'subscription_event', 'old-accepted')""")
        self.ordinary("old-pending")
        self.ordinary("old-awaiting")
        self.db.execute("UPDATE coordinator_watch_events SET continuation_state='awaiting' WHERE event_id='old-awaiting'")
        self.ordinary("old-suppressed")
        self.db.execute("UPDATE coordinator_watch_events SET delivery_state='suppressed' WHERE event_id='old-suppressed'")
        self.before = self.db.execute("SELECT * FROM coordinator_watch_events ORDER BY event_id").fetchall()
        self.before_triggers = dict(self.db.execute("SELECT name, sql FROM sqlite_schema WHERE type='trigger'"))
        self.db.execute("BEGIN")
        execute_fragment(self.db)
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])
        self.db.execute("COMMIT")

    def ordinary(self, event_id, **overrides):
        row = dict(event_id=event_id, watch_id=1, source_occurrence_kind="direct_turn",
                   source_occurrence_id=event_id, source_generation=0, source_transcript_id="root",
                   terminal_kind="completed", occurred_at_us=20)
        row.update(overrides)
        return self.insert("coordinator_watch_events", row)

    def insert(self, table, row):
        return self.db.execute(f"INSERT INTO {table} ({','.join(row)}) VALUES ({','.join('?' for _ in row)})", tuple(row.values()))

    def failure(self, failure_id="failure-1", ordinal=1, certainty="conversation_and_processes_stopped"):
        return self.insert("close_cleanup_failures", dict(
            failure_occurrence_id=failure_id, attempt_id="attempt", cleanup_run_ordinal=ordinal,
            source_product_conversation_id="source", scope="conversation", resource_kind="directory",
            identity_kind="path", identity_codec="utf8", identity_value="/fixture-only",
            reason="directory removal failed", detail="fixture failure", stop_certainty=certainty,
            confirmed_at_us=25 if certainty != "shutdown_uncertain" else None,
            occurred_at_us=30))

    def mandatory(self, failure_id="failure-1", **overrides):
        row = dict(event_id=failure_id, route_kind="mandatory_close_failure", watch_id=None,
                   mandatory_failure_occurrence_id=failure_id, mandatory_source_product_id="source",
                   source_occurrence_kind="close_cleanup_failure", source_occurrence_id=failure_id,
                   source_generation=0, source_transcript_id="root", terminal_kind="cleanup_failed",
                   terminal_reason="directory removal failed", occurred_at_us=30)
        row.update(overrides)
        return self.insert("coordinator_watch_events", row)

    def accept(self, event_id, table="durable_turns", receipt_id="new-receipt", target="global-current"):
        return self.insert(table, dict(id=receipt_id, conversation_id=target,
                           origin_kind="subscription_event", origin_subscription_event_id=event_id))

    def rejected(self, callback, message=None):
        with self.assertRaises(sqlite3.IntegrityError) as raised:
            callback()
        if message:
            self.assertIn(message, str(raised.exception))

    def test_populated_rows_ids_incoming_fks_and_old_triggers_preserved(self):
        columns = "event_id, watch_id, source_occurrence_kind, source_occurrence_id, source_generation, source_transcript_id, terminal_kind, terminal_reason, occurred_at_us, continuation_state, delivery_state, accepted_transcript_id"
        self.assertEqual(self.db.execute(f"SELECT {columns} FROM coordinator_watch_events ORDER BY event_id").fetchall(), self.before)
        self.assertEqual(self.db.execute("SELECT DISTINCT route_kind FROM coordinator_watch_events").fetchall(), [("subscription",)])
        for table in ("messages", "steering_messages", "durable_turns"):
            fks = self.db.execute(f"PRAGMA foreign_key_list({table})").fetchall()
            self.assertIn(("coordinator_watch_events", "origin_subscription_event_id", "event_id"), [(row[2], row[3], row[4]) for row in fks])
            self.assertEqual(self.db.execute(f"SELECT origin_subscription_event_id FROM {table}").fetchall(), [("old-accepted",)])
            if table == "messages":
                self.rejected(lambda: self.insert(table, dict(message_id="bad", conversation_id="global-current", origin_kind="subscription_event", origin_subscription_event_id="absent")))
            else:
                self.rejected(lambda: self.accept("absent", table=table))
        after = dict(self.db.execute("SELECT name, sql FROM sqlite_schema WHERE type='trigger'"))
        for name, sql in self.before_triggers.items():
            if name not in ("watch_event_accept_turn", "watch_event_accept_steering",
                            "close_obligations_require_member_cleanup_before_delete"):
                self.assertEqual(after[name], sql, name)
        self.assertEqual(self.db.execute("PRAGMA foreign_keys").fetchone(), (1,))
        self.assertEqual(self.db.execute("PRAGMA defer_foreign_keys").fetchone(), (0,))
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])
        self.assertEqual(self.db.execute("PRAGMA integrity_check").fetchone(), ("ok",))
        self.assertEqual(self.db.execute("SELECT name FROM sqlite_temp_schema WHERE type='table'").fetchall(), [])

    def test_rust_append_maps_product_to_original_root_excluding_child_and_successor(self):
        self.failure()
        self.db.executescript("""
            INSERT INTO conversations (id, product_conversation_id, parent_conversation_id)
                VALUES ('source-child', 'source', 'root');
            INSERT INTO conversations (id, product_conversation_id)
                VALUES ('source-successor', 'source');
            UPDATE conversations SET continued_in_conv_id='source-successor' WHERE id='root';
        """)
        self.assertNotEqual("source", "root")
        self.assertEqual(self.db.execute(ROOT_SQL, ("failure-1",)).fetchall(), [("root",)])
        self.assertEqual(append_from_rust_sql(self.db, "failure-1"), 1)
        self.assertEqual(self.db.execute("""SELECT mandatory_source_product_id, source_transcript_id,
            terminal_reason, occurred_at_us FROM coordinator_watch_events WHERE event_id='failure-1'""").fetchone(),
            ("source", "root", "directory removal failed", 30))

    def test_rust_append_models_missing_and_ambiguous_root_errors_without_writes(self):
        self.failure()
        cases = [
            ("missing-failure", None, "absent-failure", 0),
            ("missing-root", "UPDATE conversations SET runtime_role='coordinator' WHERE id='root'", "failure-1", 0),
            ("ambiguous-root", "INSERT INTO conversations (id, product_conversation_id) VALUES ('extra-root','source')", "failure-1", 2),
        ]
        before = self.db.execute("SELECT * FROM coordinator_watch_events ORDER BY event_id").fetchall()
        for name, change, failure_id, root_count in cases:
            with self.subTest(case=name):
                self.db.execute("SAVEPOINT invalid_root")
                if change:
                    self.db.execute(change)
                self.assertEqual(len(self.db.execute(ROOT_SQL, (failure_id,)).fetchall()), root_count)
                with self.assertRaisesRegex(ValueError, "exactly one authoritative product root"):
                    append_from_rust_sql(self.db, failure_id)
                self.assertEqual(self.db.execute("SELECT * FROM coordinator_watch_events ORDER BY event_id").fetchall(), before)
                self.db.execute("ROLLBACK TO invalid_root")
                self.db.execute("RELEASE invalid_root")

    def test_actual_pending_query_includes_unwatched_history_failure_filters_ordinary(self):
        self.db.execute("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='other'")
        self.db.execute("INSERT INTO close_obligations (product_conversation_id, phase) VALUES ('other','failed')")
        self.insert("close_cleanup_failures", dict(
            failure_occurrence_id="unwatched-failure", attempt_id="attempt", cleanup_run_ordinal=1,
            source_product_conversation_id="other", scope="conversation", resource_kind="directory",
            identity_kind="path", identity_codec="utf8", identity_value="/fixture-only",
            reason="directory removal failed", detail="failure detail", stop_certainty="shutdown_uncertain",
            confirmed_at_us=None, occurred_at_us=30))
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM coordinator_watches WHERE source_product_conversation_id='other'").fetchone(), (0,))
        self.assertEqual(append_from_rust_sql(self.db, "unwatched-failure"), 1)
        eligible = pending_from_rust_sql(self.db)
        self.assertEqual([row["event_id"] for row in eligible], ["old-pending", "unwatched-failure"])
        self.assertEqual([row["event_id"] for row in pending_from_rust_sql(self.db, 1)], ["old-pending"])
        mandatory = eligible[1]
        self.assertEqual((mandatory["route_kind"], mandatory["source_product_conversation_id"],
                          mandatory["source_transcript_id"], mandatory["scope"], mandatory["detail"],
                          mandatory["stop_certainty"], mandatory["stop_confirmed_at_unix_us"]),
                         ("mandatory_close_failure", "other", "other-root", "conversation", "failure detail",
                          "shutdown_uncertain", None))
        for change in (
            "UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='source'",
            "UPDATE coordinator_watches SET ended_at_us=21 WHERE id=1",
            "INSERT INTO close_obligations (product_conversation_id, phase) VALUES ('source','failed')",
        ):
            with self.subTest(change=change):
                self.db.execute("SAVEPOINT eligibility")
                self.db.execute(change)
                self.ordinary("late-ineligible")
                self.assertEqual(self.db.execute("SELECT delivery_state FROM coordinator_watch_events WHERE event_id='late-ineligible'").fetchone(), ("pending",))
                self.assertEqual([row["event_id"] for row in pending_from_rust_sql(self.db)], ["unwatched-failure"])
                self.db.execute("ROLLBACK TO eligibility")
                self.db.execute("RELEASE eligibility")

    def test_actual_stale_suppression_leaves_mandatory_and_filters_ordinary(self):
        self.failure()
        self.assertEqual(append_from_rust_sql(self.db, "failure-1"), 1)
        self.assertEqual(self.db.execute(SUPPRESS_SQL, ("old-pending",)).rowcount, 0)
        for change in (
            "UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='source'",
            "UPDATE coordinator_watches SET ended_at_us=21 WHERE id=1",
        ):
            with self.subTest(change=change):
                self.db.execute("SAVEPOINT stale")
                self.db.execute(change)
                # History's existing trigger may already suppress old-pending.
                # Insert a late occurrence to exercise the Rust query itself.
                self.ordinary("late-stale")
                self.assertEqual(self.db.execute(SUPPRESS_SQL, ("late-stale",)).rowcount, 1)
                self.assertEqual(self.db.execute(SUPPRESS_SQL, ("late-stale",)).rowcount, 0)
                self.assertEqual(self.db.execute(SUPPRESS_SQL, ("failure-1",)).rowcount, 0)
                self.assertEqual(self.db.execute("SELECT event_id, delivery_state FROM coordinator_watch_events WHERE event_id IN ('failure-1','late-stale') ORDER BY event_id").fetchall(),
                                 [("failure-1", "pending"), ("late-stale", "suppressed")])
                self.db.execute("ROLLBACK TO stale")
                self.db.execute("RELEASE stale")

    def test_mandatory_route_check_requires_product_even_without_source_trigger(self):
        self.failure()
        self.db.execute("SAVEPOINT route_constraint")
        self.db.execute("DROP TRIGGER watch_event_mandatory_source")
        self.rejected(lambda: self.mandatory(mandatory_source_product_id=None), "CHECK constraint failed")
        self.db.execute("ROLLBACK TO route_constraint")
        self.db.execute("RELEASE route_constraint")

    def test_mandatory_bypasses_watch_history_and_active_close(self):
        self.db.execute("UPDATE coordinator_watches SET ended_at_us=21 WHERE id=1")
        self.db.execute("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='source'")
        self.db.execute("INSERT INTO close_obligations (product_conversation_id, phase) VALUES ('source', 'failed')")
        for ordinal, table in enumerate(("durable_turns", "steering_messages"), 1):
            failure_id = f"failure-{ordinal}"
            self.failure(failure_id, ordinal)
            self.assertEqual(append_from_rust_sql(self.db, failure_id), 1)
            self.assertEqual(self.db.execute(SUPPRESS_SQL, (failure_id,)).rowcount, 0)
            self.accept(failure_id, table, f"receipt-{ordinal}")
            self.rejected(lambda: self.accept(failure_id, table, f"duplicate-{ordinal}"), "no longer deliverable")
            other = "steering_messages" if table == "durable_turns" else "durable_turns"
            self.rejected(lambda: self.accept(failure_id, other, f"cross-{ordinal}"), "no longer deliverable")
            self.assertEqual(self.db.execute("SELECT delivery_state,accepted_transcript_id FROM coordinator_watch_events WHERE event_id=?", (failure_id,)).fetchone(), ("accepted", "global-current"))
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_mandatory_without_any_watch_enrollment(self):
        # Source other has never had a watch. Its failure row remains authoritative.
        self.insert("close_cleanup_failures", dict(
            failure_occurrence_id="other-failure", attempt_id="attempt", cleanup_run_ordinal=1,
            source_product_conversation_id="other", scope="conversation", resource_kind="directory",
            identity_kind="path", identity_codec="utf8", identity_value="/fixture-only",
            reason="directory removal failed", detail="failure", stop_certainty="shutdown_uncertain",
            confirmed_at_us=None, occurred_at_us=30))
        self.db.execute("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='other'")
        self.db.execute("INSERT INTO close_obligations (product_conversation_id, phase) VALUES ('other','failed')")
        self.assertEqual(append_from_rust_sql(self.db, "other-failure"), 1)
        for table in ("durable_turns", "steering_messages"):
            self.db.execute("SAVEPOINT unwatched")
            self.accept("other-failure", table)
            self.rejected(lambda: self.accept("other-failure", table, "duplicate"), "no longer deliverable")
            self.db.execute("ROLLBACK TO unwatched")
            self.db.execute("RELEASE unwatched")

    def test_existing_close_suppression_and_continuation_triggers_leave_mandatory_pending(self):
        self.failure()
        self.mandatory()
        self.db.execute("UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='source'")
        self.db.execute("""UPDATE coordinator_watch_events SET delivery_state='suppressed'
            WHERE delivery_state='pending' AND watch_id IN
            (SELECT id FROM coordinator_watches WHERE source_product_conversation_id='source')""")
        self.db.execute("INSERT INTO automatic_continuation_admissions VALUES ('root','pending')")
        self.db.execute("UPDATE automatic_continuation_admissions SET phase='failed' WHERE predecessor_conversation_id='root'")
        self.db.execute("INSERT INTO completed_continuation_handoffs VALUES ('root')")
        self.assertEqual(self.db.execute("SELECT delivery_state,continuation_state FROM coordinator_watch_events WHERE event_id='failure-1'").fetchone(), ("pending", "none"))
        self.accept("failure-1")

    def test_ordinary_all_eligibility_guards_remain(self):
        cases = [
            "UPDATE coordinator_watches SET ended_at_us=21 WHERE id=1",
            "UPDATE product_conversations SET ordinary_lifecycle='history' WHERE id='source'",
            "INSERT INTO close_obligations (product_conversation_id, phase) VALUES ('source','failed')",
            "UPDATE coordinator_watch_events SET continuation_state='awaiting' WHERE event_id='old-pending'",
        ]
        for table in ("durable_turns", "steering_messages"):
            for change in cases:
                with self.subTest(table=table, change=change):
                    self.db.execute("SAVEPOINT guard")
                    self.db.execute(change)
                    self.rejected(lambda: self.accept("old-pending", table), "no longer deliverable")
                    self.db.execute("ROLLBACK TO guard")
                    self.db.execute("RELEASE guard")
            self.db.execute("SAVEPOINT accepted")
            self.accept("old-pending", table)
            self.rejected(lambda: self.accept("old-pending", table, "duplicate"), "no longer deliverable")
            self.db.execute("ROLLBACK TO accepted")
            self.db.execute("RELEASE accepted")

    def test_target_is_current_global_root_with_head_on_both_routes(self):
        self.failure()
        self.mandatory()
        for event_id in ("failure-1", "old-pending"):
            for table in ("durable_turns", "steering_messages"):
                for target in ("global-old", "global-child", "root"):
                    with self.subTest(event=event_id, table=table, target=target):
                        self.rejected(lambda: self.accept(event_id, table, target=target), "no longer deliverable")
                self.db.execute("UPDATE conversations SET coordinator_head=0 WHERE id='global-current'")
                self.rejected(lambda: self.accept(event_id, table), "no longer deliverable")
                self.db.execute("UPDATE conversations SET coordinator_head=1 WHERE id='global-current'")
        self.assertEqual(self.db.execute("SELECT delivery_state FROM coordinator_watch_events WHERE event_id='failure-1'").fetchone(), ("pending",))

    def test_authoritative_payload_route_and_root(self):
        self.failure()
        self.rejected(lambda: self.mandatory("nonexistent-failure"), "authoritative failure")
        self.db.execute("UPDATE conversations SET runtime_role='coordinator' WHERE id='root'")
        self.rejected(lambda: self.mandatory(), "authoritative failure")
        self.db.execute("UPDATE conversations SET runtime_role='user' WHERE id='root'")
        self.db.execute("""INSERT INTO conversations (id,product_conversation_id,parent_conversation_id)
            VALUES ('source-child','source','root')""")
        self.db.execute("""INSERT INTO conversations (id,product_conversation_id) VALUES ('source-successor','source')""")
        self.db.execute("UPDATE conversations SET continued_in_conv_id='source-successor' WHERE id='root'")
        for column, value in [
            ("event_id", "wrong-id"), ("mandatory_source_product_id", "other"),
            ("mandatory_source_product_id", None), ("terminal_reason", "forged"),
            ("occurred_at_us", 31), ("source_transcript_id", "other-root"),
            ("source_transcript_id", "source-child"), ("source_transcript_id", "source-successor"),
            ("source_occurrence_id", "wrong"), ("source_generation", 1),
            ("source_occurrence_kind", "direct_turn"), ("terminal_kind", "failed"),
            ("terminal_reason", None), ("watch_id", 1), ("continuation_state", "awaiting"),
            ("delivery_state", "suppressed"), ("mandatory_failure_occurrence_id", None),
        ]:
            with self.subTest(column=column, value=value):
                self.rejected(lambda: self.mandatory(**{column: value}))
        self.mandatory()
        for column, value in [("route_kind", "mandatory_close_failure"),
                              ("watch_id", None), ("mandatory_failure_occurrence_id", "failure-1"),
                              ("mandatory_source_product_id", "source"), ("terminal_kind", "cleanup_failed"),
                              ("source_occurrence_kind", "close_cleanup_failure"), ("terminal_reason", "extra")]:
            with self.subTest(ordinary_column=column):
                self.rejected(lambda: self.ordinary("invalid", **{column: value}))

    def test_every_payload_field_is_immutable_but_acceptance_can_advance(self):
        self.failure()
        self.mandatory()
        changes = dict(event_id="changed", route_kind="subscription", watch_id=1,
                       mandatory_failure_occurrence_id=None, mandatory_source_product_id="other",
                       source_occurrence_kind="direct_turn", source_occurrence_id="changed",
                       source_generation=1, source_transcript_id="other-root", terminal_kind="failed",
                       terminal_reason="changed", occurred_at_us=31)
        for column, value in changes.items():
            with self.subTest(column=column):
                self.rejected(lambda: self.db.execute(f"UPDATE coordinator_watch_events SET {column}=? WHERE event_id='failure-1'", (value,)), "payload is immutable")
        self.rejected(lambda: self.db.execute("UPDATE coordinator_watch_events SET occurred_at_us=21 WHERE event_id='old-pending'"), "payload is immutable")
        self.rejected(lambda: self.db.execute("UPDATE close_cleanup_failures SET reason='changed'"), "Close failure is immutable")
        self.rejected(lambda: self.db.execute("DELETE FROM close_cleanup_failures WHERE failure_occurrence_id='failure-1'"), "FOREIGN KEY")
        self.rejected(lambda: self.db.execute("DELETE FROM product_conversations WHERE id='source'"), "FOREIGN KEY")
        self.accept("failure-1")

    def test_pending_mandatory_event_pins_failure_product_and_transcript(self):
        self.failure()
        self.mandatory()
        before = pending_from_rust_sql(self.db)
        for statement in (
            "DELETE FROM close_cleanup_failures WHERE failure_occurrence_id='failure-1'",
            "DELETE FROM product_conversations WHERE id='source'",
            "DELETE FROM conversations WHERE id='root'",
        ):
            with self.subTest(statement=statement):
                self.rejected(lambda: self.db.execute(statement), "FOREIGN KEY")
                self.assertEqual(pending_from_rust_sql(self.db), before)
        self.rejected(lambda: self.db.execute(
            "DELETE FROM coordinator_watch_events WHERE event_id='failure-1'"),
            "requires delivery")
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_accepted_mandatory_event_preserves_receipt_without_pinning_source(self):
        for table in ("durable_turns", "steering_messages"):
            with self.subTest(table=table):
                self.db.execute("SAVEPOINT accepted_delete")
                self.failure()
                self.mandatory()
                self.accept("failure-1", table)
                self.db.execute("DELETE FROM conversations WHERE id='root'")
                self.db.execute("DELETE FROM close_cleanup_failures WHERE failure_occurrence_id='failure-1'")
                self.db.execute("DELETE FROM product_conversations WHERE id='source'")
                self.assertEqual(self.db.execute("""SELECT delivery_state, mandatory_failure_occurrence_id,
                    mandatory_source_product_id, source_transcript_id, pending_failure_occurrence_id,
                    pending_source_product_id, pending_source_transcript_id
                    FROM coordinator_watch_events WHERE event_id='failure-1'""").fetchone(),
                    ("accepted", "failure-1", "source", "root", None, None, None))
                self.assertEqual(self.db.execute(f"SELECT origin_subscription_event_id FROM {table} WHERE id='new-receipt'").fetchone(), ("failure-1",))
                self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])
                self.rejected(lambda: self.db.execute("""UPDATE coordinator_watch_events
                    SET delivery_state='pending', accepted_transcript_id=NULL WHERE event_id='failure-1'"""), "FOREIGN KEY")
                self.db.execute("ROLLBACK TO accepted_delete")
                self.db.execute("RELEASE accepted_delete")

    def test_accepted_failure_does_not_unpin_pending_retry(self):
        self.failure()
        self.mandatory()
        self.failure("failure-retry", 2)
        self.mandatory("failure-retry")
        self.accept("failure-1")
        self.rejected(lambda: self.db.execute("DELETE FROM conversations WHERE id='root'"), "FOREIGN KEY")
        self.rejected(lambda: self.db.execute("DELETE FROM close_cleanup_failures"), "FOREIGN KEY")
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM close_cleanup_failures").fetchone(), (2,))
        self.assertIn("failure-retry", [row["event_id"] for row in pending_from_rust_sql(self.db)])
        self.accept("failure-retry", receipt_id="retry-receipt")
        self.db.execute("DELETE FROM conversations WHERE id='root'")
        self.db.execute("DELETE FROM close_cleanup_failures")
        self.db.execute("DELETE FROM product_conversations WHERE id='source'")
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_completed_snapshots_delete_only_with_product(self):
        self.db.execute("INSERT INTO close_obligations VALUES ('other', 'completed', 'completed-attempt')")
        self.db.execute("INSERT INTO close_attempt_members VALUES ('completed-attempt')")
        self.db.execute("INSERT INTO close_attempt_scopes VALUES ('completed-attempt')")
        self.rejected(lambda: self.db.execute("DELETE FROM close_obligations WHERE attempt_id='completed-attempt'"), "remove member snapshots")
        self.db.execute("DELETE FROM conversations WHERE id='other-root'")
        self.db.execute("DELETE FROM product_conversations WHERE id='other'")
        for table in ("close_obligations", "close_attempt_members", "close_attempt_scopes"):
            self.assertEqual(self.db.execute(f"SELECT COUNT(*) FROM {table}").fetchone(), (0,))
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_failure_dedup_retry_and_event_identity(self):
        self.failure()
        self.assertEqual(append_from_rust_sql(self.db, "failure-1"), 1)
        self.rejected(lambda: self.failure(), "UNIQUE")
        self.rejected(lambda: self.failure("same-run-another-id", 1), "UNIQUE")
        self.rejected(lambda: self.mandatory(), "UNIQUE")
        self.rejected(lambda: self.mandatory(event_id="second-event"))
        self.assertEqual(append_from_rust_sql(self.db, "failure-1"), 0)
        self.failure("failure-retry", 2, "shutdown_uncertain")
        self.assertEqual(append_from_rust_sql(self.db, "failure-retry"), 1)
        self.accept("failure-1", receipt_id="first")
        self.assertEqual(append_from_rust_sql(self.db, "failure-1"), 0)
        self.assertEqual(self.db.execute("SELECT delivery_state, accepted_transcript_id FROM coordinator_watch_events WHERE event_id='failure-1'").fetchone(), ("accepted", "global-current"))
        self.accept("failure-retry", table="steering_messages", receipt_id="retry")
        self.assertEqual(append_from_rust_sql(self.db, "failure-retry"), 0)
        self.assertEqual(self.db.execute("SELECT failure_occurrence_id, cleanup_run_ordinal FROM close_cleanup_failures ORDER BY cleanup_run_ordinal").fetchall(), [("failure-1", 1), ("failure-retry", 2)])
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM coordinator_watch_events WHERE route_kind='mandatory_close_failure'").fetchone(), (2,))
        self.rejected(lambda: self.ordinary("duplicate", source_occurrence_id="old-pending"), "UNIQUE")

    def test_failure_event_and_acceptance_rollback_atomically(self):
        self.db.execute("BEGIN")
        self.failure("rolled-back")
        self.mandatory("rolled-back")
        self.accept("rolled-back", receipt_id="rolled-back-turn")
        self.db.execute("ROLLBACK")
        for table, column in [("close_cleanup_failures", "failure_occurrence_id"),
                              ("coordinator_watch_events", "event_id"), ("durable_turns", "id")]:
            value = "rolled-back-turn" if table == "durable_turns" else "rolled-back"
            self.assertEqual(self.db.execute(f"SELECT COUNT(*) FROM {table} WHERE {column}=?", (value,)).fetchone(), (0,))
        self.db.execute("BEGIN")
        self.failure("invalid-event")
        self.rejected(lambda: self.mandatory("invalid-event", terminal_reason="forged"))
        self.db.execute("ROLLBACK")
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM close_cleanup_failures").fetchone(), (0,))
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_rebuild_fk_barrier_rejects_injected_orphan_before_clearing_deferral(self):
        db = sqlite3.connect(":memory:", isolation_level=None)
        self.addCleanup(db.close)
        db.executescript(BASE + migration(110) + migration(111))
        before = db.execute("SELECT type,name,sql FROM sqlite_schema ORDER BY name").fetchall()
        injected = []

        def inject_orphan(connection, statement):
            if statement.lstrip().startswith(FK_BARRIER_INSERT):
                self.assertEqual(connection.execute("PRAGMA defer_foreign_keys").fetchone(), (1,))
                connection.execute("""INSERT INTO messages
                    (message_id, origin_kind, origin_subscription_event_id)
                    VALUES ('orphan', 'subscription_event', 'absent-event')""")
                injected.append(True)

        db.execute("BEGIN")
        with self.assertRaisesRegex(sqlite3.IntegrityError, "CHECK constraint failed"):
            execute_fragment(db, before_statement=inject_orphan)
        self.assertEqual(injected, [True])
        self.assertEqual(db.execute("PRAGMA defer_foreign_keys").fetchone(), (1,))
        violations = db.execute("PRAGMA foreign_key_check").fetchall()
        self.assertEqual([(row[0], row[2]) for row in violations], [("messages", "coordinator_watch_events")])
        db.execute("ROLLBACK")
        self.assertEqual(db.execute("SELECT type,name,sql FROM sqlite_schema ORDER BY name").fetchall(), before)
        self.assertEqual(db.execute("SELECT COUNT(*) FROM messages").fetchone(), (0,))
        self.assertEqual(db.execute("SELECT name FROM sqlite_temp_schema").fetchall(), [])
        self.assertEqual(db.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_rebuild_itself_rolls_back_without_mutating_old_schema_or_rows(self):
        db = sqlite3.connect(":memory:", isolation_level=None)
        self.addCleanup(db.close)
        db.executescript(BASE + migration(110) + migration(111))
        before = db.execute("SELECT type,name,sql FROM sqlite_schema ORDER BY name").fetchall()
        db.execute("BEGIN")
        execute_fragment(db)
        db.execute("ROLLBACK")
        self.assertEqual(db.execute("SELECT type,name,sql FROM sqlite_schema ORDER BY name").fetchall(), before)
        self.assertEqual(db.execute("SELECT name FROM sqlite_temp_schema").fetchall(), [])
        self.assertEqual(db.execute("PRAGMA foreign_key_check").fetchall(), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
