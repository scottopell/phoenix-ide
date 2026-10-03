import importlib.util
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).parents[2]
spec = importlib.util.spec_from_file_location("conversation_search_benchmark", ROOT / "scripts/conversation_search_benchmark.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)

class ConversationSearchBenchmarkTests(unittest.TestCase):
    def test_snapshot_captures_committed_wal_and_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.db"
            conn = sqlite3.connect(source)
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("CREATE TABLE messages(message_id TEXT, content TEXT, display_data TEXT)")
            conn.execute("INSERT INTO messages VALUES ('m1', ?, NULL)", ('{"tool_use_id":"call_w7yaFJY51rJxTjog4DKeE2wo","query":"exact observed"}',))
            conn.commit()
            out = root / "fixture"
            args = type("Args", (), {"source": str(source), "output": "", "artifacts": str(out), "force": False, "retries": 2, "busy_timeout": 1.0})()
            bench.snapshot(args)
            self.assertEqual(sqlite3.connect(out / "captured.db").execute("SELECT content FROM messages").fetchone()[0], '{"tool_use_id":"call_w7yaFJY51rJxTjog4DKeE2wo","query":"exact observed"}')
            manifest = json.loads((out / "capture-manifest.json").read_text())
            self.assertEqual(manifest["integrity_check"], "ok")
            self.assertEqual(manifest["recovered_queries"][0]["query"], "exact observed")

    def test_recovery_matches_exact_tool_block_not_sibling(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute(
            "INSERT INTO messages VALUES (?, ?, ?, NULL, ?)",
            (
                "conv",
                "msg",
                json.dumps({"blocks": [
                    {"tool_use_id": bench.CALL_IDS[0], "query": "the exact query"},
                    {"tool_use_id": "other-call", "query": "must not leak"},
                ]}),
                1,
            ),
        )
        conn.commit()
        self.assertEqual(bench._recover_queries(conn)[0]["query"], "the exact query")

    def test_snapshot_refuses_source_inside_artifacts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            artifacts = root / "nested"
            artifacts.mkdir()
            source = artifacts / "source.db"
            sqlite3.connect(source).close()
            args = type("Args", (), {"source": str(source), "output": "", "artifacts": str(artifacts), "force": False, "retries": 1, "busy_timeout": 1.0})()
            with self.assertRaises(SystemExit):
                bench.snapshot(args)

    def test_compare_refuses_measurement_regime_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            base = {"fixture_sha256": "a", "scenario_digest": "s", "profile": "release", "warmup_runs": 1, "measured_warm_runs": 10, "samples": [{"phase": "warm"}]}
            before.write_text(json.dumps(base))
            base["samples"] = [{"phase": "cold"}]
            after.write_text(json.dumps(base))
            with self.assertRaises(SystemExit):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_snapshot_refuses_missing_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            args = type("Args", (), {"source": str(Path(tmp) / "missing"), "output": "", "artifacts": tmp, "force": False, "retries": 1, "busy_timeout": 1.0})()
            with self.assertRaises(SystemExit): bench.snapshot(args)

    def test_compare_refuses_missing_metadata_and_output_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            metadata = {
                "fixture_sha256": "a", "scenario_digest": "s", "profile": "release",
                "warmup_runs": 1, "measured_warm_runs": 10, "commit": "deadbeef",
                "environment": {}, "sqlite_pragmas": {}, "samples": [
                    {"case_id": "case", "surface": "tool", "phase": "warm", "ok": True, "result_digest": "one", "duration_ms": 1}
                ],
            }
            before.write_text(json.dumps(metadata))
            incomplete = dict(metadata)
            incomplete.pop("commit")
            after.write_text(json.dumps(incomplete))
            with self.assertRaisesRegex(SystemExit, "missing metadata"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())
            after.write_text(json.dumps({**metadata, "samples": [{**metadata["samples"][0], "result_digest": "two"}]}))
            with self.assertRaisesRegex(SystemExit, "output mismatch"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_run_rejects_path_traversal_label(self):
        with self.assertRaisesRegex(SystemExit, "label"):
            bench._label("../escape")

    def test_compare_refuses_fixture_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            before.write_text(json.dumps({"fixture_sha256":"a", "scenario_digest":"s"}))
            after.write_text(json.dumps({"fixture_sha256":"b", "scenario_digest":"s"}))
            with self.assertRaises(SystemExit): bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

if __name__ == "__main__": unittest.main()
