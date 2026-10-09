#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
"""Exercise actual wait migration with incoming FKs and an external trigger."""
import re
import sqlite3
import unittest
from pathlib import Path

SOURCE = (Path(__file__).resolve().parents[1] / "crates/phoenix-db/src/migrations.rs").read_text()


def migration(number):
    return re.search(rf'const MIGRATION_{number}: &str = r"(.*?)";', SOURCE, re.S).group(1)


def apply(db, sql):
    statement = ""
    for line in sql.splitlines(keepends=True):
        statement += line
        if sqlite3.complete_statement(statement):
            db.execute(statement)
            statement = ""
    assert not statement.strip()


class WaitMigrationTest(unittest.TestCase):
    def test_populated_outbox_keeps_trigger_and_incoming_foreign_key(self):
        with sqlite3.connect(":memory:") as db:
            db.execute("PRAGMA foreign_keys=ON")
            db.execute("CREATE TABLE coordinator_watches(id INTEGER PRIMARY KEY)")
            old = migration("111")
            start = old.index("CREATE TABLE coordinator_watch_events (")
            end = old.index("ALTER TABLE messages", start)
            apply(db, old[start:end])
            db.execute("CREATE TABLE receipts(id TEXT PRIMARY KEY, event_id TEXT REFERENCES coordinator_watch_events(event_id))")
            db.execute("CREATE TRIGGER receipt_check BEFORE INSERT ON receipts BEGIN SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM coordinator_watch_events WHERE event_id=NEW.event_id) THEN RAISE(ABORT,'missing event') END; END")
            db.execute("INSERT INTO coordinator_watches VALUES(1)")
            db.execute("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,occurred_at_us) VALUES('old',1,'direct_turn','turn',0,'source','completed',1)")
            db.execute("INSERT INTO receipts VALUES('receipt','old')")
            db.commit()
            db.execute("BEGIN")
            apply(db, migration("118"))
            db.commit()
            self.assertEqual(db.execute("PRAGMA foreign_key_check").fetchall(), [])
            self.assertEqual(db.execute("SELECT event_id FROM receipts").fetchall(), [('old',)])
            db.execute("INSERT INTO coordinator_watch_events(event_id,watch_id,source_occurrence_kind,source_occurrence_id,source_generation,source_transcript_id,terminal_kind,terminal_reason,occurred_at_us) VALUES('wait',1,'question_request','request',0,'source','awaiting_user_response','question_request',2)")
            db.execute("INSERT INTO receipts VALUES('wait-receipt','wait')")
            with self.assertRaises(sqlite3.IntegrityError):
                db.execute("INSERT INTO receipts VALUES('bad','absent')")


if __name__ == "__main__":
    unittest.main()
