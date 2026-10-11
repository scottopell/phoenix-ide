"""Focused offline fixture safety, shape, scale, and consistency regressions."""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "conversation_load_fixture.py"
spec = importlib.util.spec_from_file_location("conversation_load_fixture", SCRIPT)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
SECRET = "PRIVATE_SENTINEL_DO_NOT_RETAIN"
TARGET = fixture.DEFAULT_TARGET


def make_source(path):
    db = sqlite3.connect(path)
    db.executescript("""
        PRAGMA foreign_keys=ON;
        CREATE TABLE product_conversations (id TEXT PRIMARY KEY, kind TEXT NOT NULL,
            ordinary_lifecycle TEXT, auto_continue_on_context_exhaustion INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE projects (id TEXT PRIMARY KEY, title TEXT);
        CREATE TABLE work_scopes (id TEXT PRIMARY KEY, authority_kind TEXT, lifecycle TEXT,
            retired_at TEXT, retired_reason TEXT, created_at TEXT, updated_at TEXT,
            environment_kind TEXT, cwd TEXT, worktree_path TEXT, branch_name TEXT,
            base_branch TEXT, worktree_id TEXT, worktree_fingerprint TEXT);
        CREATE TABLE conversations (id TEXT PRIMARY KEY, slug TEXT UNIQUE, title TEXT,
            parent_conversation_id TEXT REFERENCES conversations(id),
            continued_in_conv_id TEXT REFERENCES conversations(id),
            transcript_generation INTEGER NOT NULL DEFAULT 1,
            product_conversation_id TEXT REFERENCES product_conversations(id),
            work_scope_id TEXT REFERENCES work_scopes(id), project_id TEXT REFERENCES projects(id),
            state TEXT NOT NULL, state_kind TEXT NOT NULL, model TEXT,
            created_at TEXT, user_initiated INTEGER, steering_queue TEXT);
        CREATE TABLE coordinator_watch_events (event_id TEXT PRIMARY KEY, secret TEXT);
        CREATE TABLE messages (message_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL REFERENCES conversations(id), sequence_id INTEGER,
            message_type TEXT, content TEXT, display_data TEXT, usage_data TEXT,
            origin_kind TEXT, origin_subscription_event_id TEXT REFERENCES coordinator_watch_events(event_id));
        CREATE TABLE completed_continuation_handoffs (
            predecessor_conversation_id TEXT PRIMARY KEY REFERENCES conversations(id),
            successor_conversation_id TEXT REFERENCES conversations(id),
            continuation_message_id TEXT REFERENCES messages(message_id),
            accepted_successor_message_id TEXT REFERENCES messages(message_id), opening_authority TEXT);
        CREATE TABLE product_conversation_work_scopes (
            work_scope_id TEXT PRIMARY KEY REFERENCES work_scopes(id),
            product_conversation_id TEXT REFERENCES product_conversations(id));
        CREATE TABLE credentials (secret TEXT);
        CREATE TABLE effects (secret TEXT);
        CREATE TABLE jobs (secret TEXT);
        CREATE TABLE message_files (message_id TEXT REFERENCES messages(message_id), original_name TEXT, stored_path TEXT);
        CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, description TEXT);
        CREATE INDEX message_seq ON messages(conversation_id,sequence_id);
        CREATE VIEW message_counts AS SELECT conversation_id,count(*) FROM messages GROUP BY conversation_id;
        CREATE VIRTUAL TABLE search_index USING fts5(content);
        CREATE TRIGGER unsafe_trigger AFTER INSERT ON messages BEGIN INSERT INTO effects VALUES (NEW.content); END;
    """)
    db.execute("INSERT INTO product_conversations VALUES (?,?,?,0)", (TARGET, "ordinary", "open"))
    db.execute("INSERT INTO product_conversations VALUES (?,?,?,0)", ("another-private-id", "ordinary", "history"))
    db.execute("INSERT INTO projects VALUES (?,?)", (SECRET, SECRET))
    db.execute("INSERT INTO coordinator_watch_events VALUES (?,?)", (SECRET, SECRET))
    db.execute("INSERT INTO work_scopes VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
               ("scope-private", "work", "active", None, None, "2026-01-01", "2026-01-01",
                "allocated_worktree", SECRET, SECRET, SECRET, SECRET, SECRET, SECRET))
    for cid, parent, product, state in [
        ("parent-private", None, TARGET, {"type": "context_exhausted", "summary": SECRET + "\n" + SECRET}),
        ("child-private", "parent-private", TARGET, {"type": "tool_executing", "secret": SECRET}),
        ("unrelated-private", None, "another-private-id", {"type": "idle"}),
    ]:
        db.execute("INSERT INTO conversations (id,slug,title,parent_conversation_id,product_conversation_id,work_scope_id,project_id,state,state_kind,model,created_at,user_initiated,steering_queue) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                   (cid, cid + SECRET, SECRET, parent, product, "scope-private", SECRET,
                    json.dumps(state), state["type"], SECRET, "2026-01-01", 1, json.dumps([SECRET])))
    db.execute("INSERT INTO product_conversation_work_scopes VALUES (?,?)", ("scope-private", TARGET))
    payloads = [
        ("agent", [{"type": "tool_use", "id": SECRET, "name": "bash", "input": {SECRET: {"nested": [SECRET]}}},
                   {"type": "text", "text": SECRET + "\n" + SECRET}]),
        ("tool", {"tool_use_id": SECRET, "content": json.dumps({SECRET: [SECRET]}), "is_error": False,
                  "images": [{"data": SECRET}]}),
        ("continuation", {"summary": SECRET}),
        ("user", {"text": SECRET, "llm_text": SECRET, "is_meta": True}),
    ]
    for i, (kind, content) in enumerate(payloads):
        db.execute("INSERT INTO messages VALUES (?,?,?,?,?,?,?,?,?)",
                   (f"msg-{i}", "parent-private" if i < 3 else "child-private", 7 + i * 3,
                    kind, json.dumps(content), json.dumps({SECRET: [SECRET]}), SECRET, "subscription_event", SECRET))
    db.execute("INSERT INTO messages VALUES (?,?,?,?,?,?,?,?,?)",
               ("global-msg", "unrelated-private", 42, "system", json.dumps({"text": SECRET}), None, None, "user_api", None))
    db.execute("INSERT INTO completed_continuation_handoffs VALUES (?,?,?,?,?)",
               ("parent-private", "child-private", "msg-2", "msg-3", "generated_predecessor_context"))
    for table in ("credentials", "effects", "jobs"):
        db.execute(f"INSERT INTO {table} VALUES (?)", (SECRET,))
    db.execute("INSERT INTO schema_migrations VALUES (1,?)", (SECRET,))
    db.execute("INSERT INTO message_files VALUES (?,?,?)", ("msg-3", SECRET, SECRET))
    db.execute("INSERT INTO search_index VALUES (?)", (SECRET,))
    db.commit()
    db.close()


class SanitizerTests(unittest.TestCase):
    def test_text_preserves_character_bytes_and_newline_positions(self):
        text = SECRET + "\n\r\n" + "ö秘密😀\t" + SECRET
        result = fixture.synthetic(text)
        self.assertEqual(len(text), len(result))
        self.assertEqual(len(text.encode()), len(result.encode()))
        self.assertEqual([(i,c) for i,c in enumerate(text) if c in "\r\n"],
                         [(i,c) for i,c in enumerate(result) if c in "\r\n"])
        self.assertNotIn(SECRET, result)

    def test_arbitrary_nested_opaque_secrets_are_not_round_tripped(self):
        raw = [{"type": "tool_use", "id": SECRET, "name": SECRET,
                "input": {SECRET: {"array": [SECRET, {SECRET: SECRET}]}}},
               {"type": "thinking", "thinking": SECRET, "signature": SECRET},
               {"type": "mcp_tool_use", "id": SECRET, "name": SECRET, "server_name": SECRET, "input": [SECRET]},
               {"type": "web_search_tool_result", "tool_use_id": SECRET, "content": {SECRET: [SECRET]}}]
        result = fixture.sanitize_content("agent", json.dumps(raw))
        self.assertNotIn(SECRET, result)
        blocks = json.loads(result)
        self.assertEqual(blocks[0]["name"], "fixture_tool")
        self.assertEqual(blocks[1]["signature"], "")
        self.assertEqual(set(blocks[0]["input"]), {"fixture_payload"})
        self.assertEqual(blocks[0]["id"], blocks[3]["tool_use_id"])

    def test_input_origin_normalizes_as_one_aggregate(self):
        db = sqlite3.connect(":memory:")
        db.row_factory = sqlite3.Row
        row = db.execute("SELECT 'user' AS message_type, ? AS content, 'subscription_event' AS origin_kind, ? AS origin_product_conversation_id, ? AS origin_transcript_id, ? AS origin_subscription_event_id", (json.dumps({"text": SECRET}), SECRET, SECRET, SECRET)).fetchone()
        result = fixture.sanitize_row("messages", row, fixture.Counter())
        self.assertEqual(result["origin_kind"], "unknown_historical")
        self.assertTrue(all(v is None for k, v in result.items() if k.startswith("origin_") and k != "origin_kind"))
        db.close()

    def test_unknown_content_fields_and_types_fail_closed(self):
        for kind, value in [("user", {"text": SECRET, "unknown": SECRET}),
                            ("agent", [{"type": "new_private_block", "data": SECRET}]),
                            ("tool", {"tool_use_id": SECRET, "content": SECRET, "is_error": False, "unknown": SECRET}),
                            ("new_type", {"text": SECRET})]:
            with self.subTest(kind=kind), self.assertRaises(fixture.FixtureError):
                fixture.sanitize_content(kind, json.dumps(value))

    def test_all_object_message_kinds_are_synthesized(self):
        for kind, payload in [("system", {"text": SECRET}), ("error", {"message": SECRET}),
                              ("continuation", {"summary": SECRET}),
                              ("skill", {"name": SECRET, "body": SECRET, "trigger": SECRET})]:
            self.assertNotIn(SECRET, fixture.sanitize_content(kind, json.dumps(payload)))


class BuilderTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.source = self.root / "source.db"
        make_source(self.source)
        self.output = self.root / "fixture"

    def tearDown(self):
        self.temp.cleanup()

    def test_relations_scale_source_read_only_and_no_secret_rows(self):
        before = fixture.file_hash(self.source)
        manifest = fixture.build_fixture(self.source, self.output)
        self.assertEqual(fixture.file_hash(self.source), before)
        self.assertEqual(manifest["counts"]["conversations"], 3)
        self.assertEqual(manifest["counts"]["product_conversations"], 2)
        self.assertEqual(manifest["counts"]["messages"], 5)
        self.assertEqual(manifest["target"]["transcript_count"], 2)
        self.assertEqual(manifest["target"]["message_count"], 4)
        self.assertEqual(manifest["omissions"]["table_rows:message_files"], 1)
        db_path = self.output / "fixture.db"
        self.assertEqual(db_path.stat().st_mode & 0o777, 0o400)
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o700)
        for p in self.output.iterdir():
            self.assertNotIn(SECRET.encode(), p.read_bytes())
        db = sqlite3.connect(db_path.as_uri() + "?mode=ro", uri=True)
        self.addCleanup(db.close)
        self.assertEqual(db.execute("PRAGMA foreign_key_check").fetchall(), [])
        for t in ("credentials", "effects", "jobs", "projects", "coordinator_watch_events", "schema_migrations", "message_files"):
            self.assertEqual(db.execute(f"SELECT count(*) FROM {t}").fetchone()[0], 0)
        self.assertEqual(db.execute("SELECT count(*) FROM sqlite_master WHERE type='trigger'").fetchone()[0], 0)
        self.assertEqual(db.execute("SELECT count(*) FROM sqlite_master WHERE name LIKE 'search_index%'").fetchone()[0], 0)
        self.assertEqual(db.execute("SELECT sequence_id FROM messages ORDER BY sequence_id").fetchall(), [(7,), (10,), (13,), (16,), (42,)])
        content = [json.loads(r[0]) for r in db.execute("SELECT content FROM messages ORDER BY sequence_id")]
        self.assertEqual(content[0][0]["id"], content[1]["tool_use_id"])
        self.assertEqual(content[0][0]["name"], "bash")
        states = dict(db.execute("SELECT id,state_kind FROM conversations"))
        self.assertEqual(states[fixture.identity("parent-private")], "context_exhausted")
        self.assertEqual(states[fixture.identity("child-private")], "idle")
        aux = json.loads((self.output / "aux.json").read_text())
        self.assertEqual(aux[f"GET /api/global/coordinator/route/{TARGET}"]["body"], {"coordinator_id": None, "is_coordinator": False})
        self.assertEqual(manifest["db_sha256"], fixture.file_hash(db_path))
        self.assertFalse(any(self.output.glob("fixture.db-*")))

    def test_source_connection_rejects_writes(self):
        source = fixture.open_source(self.source)
        self.addCleanup(source.close)
        self.assertTrue(source.in_transaction)
        with self.assertRaises(sqlite3.OperationalError):
            source.execute("DELETE FROM messages")

    def test_existing_output_never_overwritten(self):
        self.output.mkdir()
        marker = self.output / "keep"
        marker.write_text("keep")
        with self.assertRaises(fixture.FixtureError):
            fixture.build_fixture(self.source, self.output)
        self.assertEqual(marker.read_text(), "keep")

    def test_unknown_column_deletes_incomplete_output(self):
        db = sqlite3.connect(self.source)
        db.execute("ALTER TABLE conversations ADD COLUMN unknown_private_field TEXT")
        db.commit()
        db.close()
        with self.assertRaises(fixture.FixtureError):
            fixture.build_fixture(self.source, self.output)
        self.assertFalse(self.output.exists())

    def test_broken_relations_fail_closed(self):
        db = sqlite3.connect(self.source)
        db.execute("UPDATE conversations SET work_scope_id='missing'")
        db.commit()
        db.close()
        with self.assertRaisesRegex(fixture.FixtureError, "foreign_key_check"):
            fixture.build_fixture(self.source, self.output)
        self.assertFalse(self.output.exists())

    def test_single_read_snapshot_while_source_changes(self):
        db = sqlite3.connect(self.source)
        db.execute("PRAGMA journal_mode=WAL")
        db.close()
        real_sanitize = fixture.sanitize_row
        inserted = False
        def change_source(table, row, omissions):
            nonlocal inserted
            if not inserted:
                inserted = True
                writer = sqlite3.connect(self.source)
                writer.execute("INSERT INTO product_conversations VALUES ('late','ordinary','open',0)")
                writer.commit()
                writer.close()
            return real_sanitize(table, row, omissions)
        with patch.object(fixture, "sanitize_row", side_effect=change_source):
            manifest = fixture.build_fixture(self.source, self.output)
        self.assertEqual(manifest["counts"]["product_conversations"], 2)
        db = sqlite3.connect(self.source)
        self.addCleanup(db.close)
        self.assertEqual(db.execute("SELECT count(*) FROM product_conversations").fetchone()[0], 3)

    def test_scenario_uses_fixture_tail_and_existing_harness(self):
        scenario_spec = importlib.util.spec_from_file_location("load_scenario", SCRIPT.parent / "conversation_load_scenario.py")
        scenario_module = importlib.util.module_from_spec(scenario_spec)
        scenario_spec.loader.exec_module(scenario_module)
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "source.db"
            make_source(source)
            output = Path(tmp) / "out"
            fixture.build_fixture(source, output, TARGET)
            args = scenario_module.scenario(output, "http://127.0.0.1:8043/")
            self.assertEqual(args["action"], "run_scenario")
            self.assertEqual(args["steps"][0]["kind"], "wait_selector")
            self.assertEqual(args["steps"][2]["kind"], "click")
            self.assertEqual(args["runs"], 1)
            self.assertEqual(args["reset"]["url"], "http://127.0.0.1:8043/")
            self.assertNotIn(SECRET, json.dumps(args))

    def test_completed_auxiliary_fixture_cap_and_static_init(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "source.db"
            make_source(source)
            output = Path(tmp) / "out"
            manifest = fixture.build_fixture(source, output, TARGET)
            db = sqlite3.connect(output / "fixture.db")
            db.row_factory = sqlite3.Row
            for size in range(240, 257):
                aux = fixture.aux_fixtures(TARGET, [f"member-{i}" for i in range(size)])
                member = fixture.add_current_member_fixtures(db, aux, TARGET)
                self.assertLessEqual(len(aux), 256)
                init = json.loads(aux[f"GET /api/conversations/{member}/stream"]["body"].split("data: ", 1)[1])
                self.assertTrue(all(m["origin"]["kind"] == "unknown_historical" for m in init["messages"]))
                self.assertNotIn(SECRET, json.dumps(aux))
                self.assertEqual(manifest["target"]["latest_member"], member)
            db.close()

    def test_auxiliary_fixture_cap(self):
        aux = fixture.aux_fixtures(TARGET, [str(i) for i in range(303)])
        self.assertLessEqual(len(aux), 256)
        self.assertIn(f"GET /api/global/coordinator/route/{TARGET}", aux)

    def test_cli_diagnostics_do_not_print_private_content(self):
        db = sqlite3.connect(self.source)
        db.execute("UPDATE messages SET content=?", (SECRET,))
        db.commit()
        db.close()
        import subprocess
        result = subprocess.run([os.sys.executable, str(SCRIPT), "--source", str(self.source),
                                 "--output-dir", str(self.output)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(SECRET, result.stdout + result.stderr)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
