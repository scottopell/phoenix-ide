#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Run landed Close outbox obligations again after the wait schema rebuild."""
import sqlite3
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/tests"))
import test_mandatory_close_outbox as close

SQL = (ROOT / "crates/phoenix-db/src/coordinator_watches/question_wait_outbox.sql").read_text()


def apply(db):
    statement = ""
    for line in SQL.splitlines(keepends=True):
        statement += line
        if sqlite3.complete_statement(statement):
            db.execute(statement)
            statement = ""
    assert not statement.strip()


class WaitMigrationTest(close.MandatoryCloseOutboxTests):
    def setUp(self):
        super().setUp()
        self.db.execute("BEGIN")
        apply(self.db)
        self.db.execute("COMMIT")

    def test_populated_mandatory_pending_and_accepted_survive_rebuild(self):
        self.failure("accepted", ordinal=1)
        self.mandatory("accepted")
        self.accept("accepted", "durable_turns", "accepted-turn")
        self.failure("pending", ordinal=2)
        self.mandatory("pending")
        before = self.db.execute("SELECT * FROM coordinator_watch_events ORDER BY event_id").fetchall()
        triggers = dict(self.db.execute("SELECT name,sql FROM sqlite_schema WHERE type='trigger'"))
        self.db.execute("BEGIN")
        apply(self.db)
        self.db.execute("COMMIT")
        self.assertEqual(before, self.db.execute("SELECT * FROM coordinator_watch_events ORDER BY event_id").fetchall())
        self.assertEqual(triggers, dict(self.db.execute("SELECT name,sql FROM sqlite_schema WHERE type='trigger'")))
        self.assertEqual([], self.db.execute("PRAGMA foreign_key_check").fetchall())
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("DELETE FROM coordinator_watch_events WHERE event_id='pending'")
        self.ordinary("wait", source_occurrence_kind="question_request",
                      terminal_kind="awaiting_user_response", terminal_reason="question_request")
        self.accept("wait", "durable_turns", "wait-turn")
        with self.assertRaises(sqlite3.IntegrityError):
            self.ordinary("bad-wait", source_occurrence_kind="question_request")


if __name__ == "__main__":
    unittest.main()
