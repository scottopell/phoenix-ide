import importlib.util
import json
import sqlite3
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock
ROOT = Path(__file__).parents[2]
spec = importlib.util.spec_from_file_location("conversation_search_benchmark", ROOT / "scripts/conversation_search_benchmark.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class ConversationSearchBenchmarkTests(unittest.TestCase):
    @staticmethod
    def _complete_run(**overrides):
        samples = [
            {"case_id": "case", "surface": "tool", "phase": "first_use_fresh_pool_os_cache_uncontrolled", "iteration": 0,
             "ok": True, "result_count":1, "result_identity":["id"], "result_digest": "one", "duration_ms": 100},
            {"case_id": "case", "surface": "tool", "phase": "warmup_discarded", "iteration": 0,
             "ok": True, "result_count":1, "result_identity":["id"], "result_digest": "one", "duration_ms": 100},
            *[
                {"case_id": "case", "surface": "tool", "phase": "warm", "iteration": i,
                 "ok": True, "result_count":1, "result_identity":["id"], "result_digest": "one", "duration_ms": i + 1}
                for i in range(10)
            ],
        ]
        value = {
            "fixture_sha256": "a", "schema_digest": "schema", "migration_ledger": [],
            "scenario_digest": "s", "profile": "release",
            "warmup_runs": 1, "measured_warm_runs": 10, "commit": "deadbeef",
            "environment": {"host": "host"}, "sqlite_pragmas": {"read_only": True},
            "runtime": {"worker_threads": 2}, "explain_enabled": False,
            "build_configuration": {"rustc_version_verbose": "rustc", "cargo_version": "cargo", "target": "host", "profile": "release", "features": [], "environment": {}},
            "expected_case_surface_set": [["case", "tool"]],
            "case_policies": [{"case_id":"case","surface":"tool","policy":{"limit":20}}],
            "run_uuid": __import__("uuid").uuid4().hex, "started_at_unix":1.0, "completed_at_unix":2.0,
            "explain_plans": [], "samples": samples,
            "fixture_validation":{"transcript_count":1,"freshness_batch_size":64,"locator_orphans":0,"missing_physical_rows":0,"unlocated_physical_rows":0}, "tool_oracle_regime":"none historical", "measurement_regimes":["first_use_fresh_pool_os_cache_uncontrolled", "warm"],
        }
        value.update(overrides)
        return value

    def test_staged_files_require_root_ignore(self):
        with tempfile.TemporaryDirectory() as d:
            repo = Path(d).resolve()
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            (repo / ".gitignore").write_text("private/*\n")
            with mock.patch.object(bench, "__file__", str(repo / "scripts" / "helper.py")):
                bench._ensure_ignored_artifacts(repo / "private")
                (repo / ".gitignore").write_text("private/captured.db\nprivate/capture-manifest.json\nprivate/scenarios.json\nprivate/report.md\nprivate/runs/\n")
                with self.assertRaisesRegex(SystemExit, "unignored"):
                    bench._ensure_ignored_artifacts(repo / "private")

    def test_exact_freshness_and_target_flags(self):
        for evidence in [{"index_fresh": True}, {**self._complete_run()["fixture_validation"], "locator_orphans":1}]:
            with self.assertRaisesRegex(SystemExit, "freshness"):
                bench._validate_run(self._complete_run(fixture_validation=evidence), "bad")
        with mock.patch.dict("os.environ", {"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS":"-C target-cpu=native", "CARGO_REGISTRY_TOKEN":"secret"}), mock.patch.object(bench.subprocess, "check_output", return_value="version"):
            env = bench._build_configuration()["environment"]
            self.assertIn("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS", env)
            self.assertNotIn("CARGO_REGISTRY_TOKEN", env)

    def test_label_reservation_refuses_second_writer(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "capture-manifest.json").write_text("{}")
            args = type("Args", (), {"artifacts":str(root), "label":"same"})()
            def nested(_):
                with self.assertRaisesRegex(SystemExit, "reserved"):
                    bench.run(args)
                return 0
            with mock.patch.object(bench, "_run_reserved", side_effect=nested):
                self.assertEqual(bench.run(args), 0)
            self.assertFalse((root / "runs" / ".same.reserved").exists())

    def test_snapshot_captures_committed_wal_and_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.db"
            conn = sqlite3.connect(source)
            conn.execute("PRAGMA journal_mode=WAL")
            conn.execute("CREATE TABLE messages(message_id TEXT, content TEXT, display_data TEXT)")
            conn.execute("INSERT INTO messages VALUES ('m1', ?, NULL)",
                         ('{"tool_use_id":"call_w7yaFJY51rJxTjog4DKeE2wo","query":"exact observed"}',))
            conn.commit()
            out = root / "fixture"
            args = type("Args", (), {"source": str(source), "artifacts": str(out),
                                      "force": False, "retries": 2, "busy_timeout": 1.0})()
            bench.snapshot(args)
            self.assertEqual(sqlite3.connect(out / "captured.db").execute(
                "SELECT content FROM messages").fetchone()[0],
                '{"tool_use_id":"call_w7yaFJY51rJxTjog4DKeE2wo","query":"exact observed"}')
            manifest = json.loads((out / "capture-manifest.json").read_text())
            self.assertEqual(manifest["integrity_check"], "ok")
            self.assertEqual(len(manifest["schema_digest"]), 64)
            self.assertIsNone(manifest["migration_ledger"])
            self.assertEqual(manifest["recovered_queries"][0]["query"], "exact observed")
            source_conn = sqlite3.connect(source)
            self.assertEqual(
                manifest["logical_size_bytes"],
                source_conn.execute("PRAGMA page_count").fetchone()[0]
                * source_conn.execute("PRAGMA page_size").fetchone()[0],
            )
            source_conn.close()

    def test_recovery_uses_configured_replacement_after_partial_known_calls(self):
        replacement = "call_replacement_123"
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "9bc3b72d-extra", "known", json.dumps({"tool_use_id": bench.CALL_IDS[0], "query": "known query"}), 1))
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "replacement", "replacement-message", json.dumps({"tool_use_id": replacement, "query": "replacement query"}), 2))
        conn.commit()
        with mock.patch.dict("os.environ", {"PHOENIX_SEARCH_CALL_IDS": replacement, "PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT":"replacement"}):
            recovered = bench._recover_queries(conn)
        self.assertEqual([item["query"] for item in recovered], ["known query", "replacement query"])

    def test_recovery_prefix_predicate_is_indexable(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("CREATE INDEX messages_conversation ON messages(conversation_id)")
        plan = conn.execute(
            "EXPLAIN QUERY PLAN SELECT 1 FROM messages WHERE conversation_id GLOB ? LIMIT 1",
            ("9bc3*",),
        ).fetchall()
        self.assertIn("USING COVERING INDEX messages_conversation", plan[0][3])

    def test_recovery_does_not_full_scan_production_schema_without_configured_ids(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE conversations(id TEXT)")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "conv", "msg", json.dumps({"tool_use_id": bench.CALL_IDS[0], "query": "must not scan"}), 1))
        conn.commit()
        with mock.patch.dict("os.environ", {}, clear=True):
            self.assertEqual(bench._recover_queries(conn), [])

    def test_schema_evidence_captures_digest_and_existing_migration_ledger(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE _migrations(version INTEGER, name TEXT)")
        conn.execute("INSERT INTO _migrations VALUES (1, 'initial')")
        conn.commit()
        digest, ledger = bench._schema_evidence(conn)
        self.assertEqual(len(digest), 64)
        self.assertEqual(ledger, [{"version": 1, "name": "initial"}])

    def test_prepare_rejects_duplicate_normalized_observed_queries(self):
        self.assertEqual(bench._normalize_query("  Same   Query "), "same query")
        self.assertEqual(bench._normalize_query("same query"), "same query")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "kind": "conversation-search-fixture", "snapshot_path": str(db),
                "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "source_path": str(root / "source.db"),
            }))
            args = type("Args", (), {"artifacts": str(root), "force": True})()
            with mock.patch.object(bench, "_recover_queries", return_value=[
                {"query": "Same   Query", "source_call_id": "one", "message_id": "m1"},
                {"query": "same query", "source_call_id": "two", "message_id": "m2"},
            ]), mock.patch.object(bench.sqlite3, "connect") as connect:
                connection = connect.return_value
                connection.execute.return_value.fetchone.return_value = None
                with self.assertRaisesRegex(SystemExit, "duplicate normalized"):
                    connect.return_value.execute.return_value.fetchone.side_effect = [(1,), (1000,), (0,)]
                    bench.prepare(args)

    def test_scenarios_are_bound_to_fixture_hash_at_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "snapshot_path": str(db), "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "schema_digest": "schema", "migration_ledger": [],
                "source_path": str(root / "source.db"),
            }))
            (root / "scenarios.json").write_text(json.dumps({
                "version": 1, "fixture_sha256": "different", "scenarios": [],
            }))
            with self.assertRaisesRegex(SystemExit, "does not match"):
                bench.run(type("Args", (), {"artifacts": str(root), "label": "suite", "force": False, "timeout": 1})())

    def test_force_run_removes_stale_result_and_publishes_temp_atomically(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "snapshot_path": str(db), "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "schema_digest": "schema", "migration_ledger": [],
                "source_path": str(root / "source.db"),
            }))
            (root / "scenarios.json").write_text(json.dumps({
                "version": 1, "fixture_sha256": fixture_hash, "expected_case_surface_set": [], "scenarios": [],
            }))
            runs = root / "runs"
            runs.mkdir()
            stale = runs / "suite.json"
            stale.write_text("stale")
            failure = runs / "failures" / "suite.json"
            failure.parent.mkdir()
            failure.write_text("old failure")

            class CompletedProcess:
                returncode = 0

                def wait(self, timeout=None):
                    return None

            def fake_popen(*args, **kwargs):
                self.assertIn("PHOENIX_SEARCH_BENCH_SCHEMA_DIGEST", kwargs["env"])
                self.assertIn("PHOENIX_SEARCH_BENCH_MIGRATION_LEDGER", kwargs["env"])
                Path(kwargs["env"]["PHOENIX_SEARCH_BENCH_OUT"]).write_text("fresh")
                return CompletedProcess()

            with mock.patch.object(bench, "_ensure_ignored_artifacts"), mock.patch.object(bench, "_ensure_clean_source"), mock.patch.object(
                bench, "_build_configuration", return_value={"rustc_version_verbose": "rustc", "cargo_version": "cargo", "target": "host", "profile": "release", "features": [], "environment": {}}
            ), mock.patch.object(bench, "_git_commit", return_value="commit"), mock.patch.object(bench.platform, "platform", return_value="platform"), mock.patch.object(
                bench.platform, "processor", return_value="processor"
            ), mock.patch.object(bench.subprocess, "Popen", side_effect=fake_popen):
                bench.run(type("Args", (), {"artifacts": str(root), "label": "suite", "force": True, "timeout": 1})())
            self.assertEqual(stale.read_text(), "fresh")
            self.assertTrue(failure.exists())
            self.assertEqual(failure.read_text(), "old failure")
            self.assertEqual(list(runs.glob(".*.tmp")), [])

    def test_run_refuses_dirty_source_tree(self):
        with mock.patch.object(bench.subprocess, "check_output", return_value=" M scripts/example.py\n"):
            with self.assertRaisesRegex(SystemExit, "dirty source"):
                bench._ensure_clean_source()

    def test_snapshot_cli_does_not_offer_custom_output(self):
        completed = subprocess.run(
            ["python", str(ROOT / "scripts/conversation_search_benchmark.py"), "snapshot", "--help"],
            capture_output=True, text=True, check=True,
        )
        self.assertNotIn("--output", completed.stdout)

    def test_recovery_matches_exact_tool_block_not_sibling(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "conv", "msg", json.dumps({"blocks": [
                {"tool_use_id": bench.CALL_IDS[0], "query": "the exact query"},
                {"tool_use_id": "other-call", "query": "must not leak"},
            ]}), 1))
        conn.commit()
        with mock.patch.dict("os.environ", {"PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT":"conv"}):
            self.assertEqual(bench._recover_queries(conn)[0]["query"], "the exact query")

    def test_recovery_decodes_serialized_arguments(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "conv", "msg", json.dumps({"tool_use_id": bench.CALL_IDS[0],
                                        "arguments": json.dumps({"query": "decoded query"})}), 1))
        conn.commit()
        with mock.patch.dict("os.environ", {"PHOENIX_SEARCH_REPLACEMENT_TRANSCRIPT":"conv"}):
            self.assertEqual(bench._recover_queries(conn)[0]["query"], "decoded query")

    def test_recovery_known_transcript_uses_bounded_conversation_lookup(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE TABLE messages(conversation_id TEXT, message_id TEXT, content TEXT, display_data TEXT, created_at INTEGER)")
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            "9bc3b72d-extra", "msg", json.dumps({"tool_use_id": bench.CALL_IDS[0], "query": "indexed query"}), 1))
        conn.execute("INSERT INTO messages VALUES (?, ?, ?, NULL, ?)", (
            bench.KNOWN_CALLS[1]["conversation_id"], "msg2",
            json.dumps({"tool_use_id": bench.CALL_IDS[1], "query": "indexed query two"}), 2))
        conn.commit()
        self.assertEqual([item["query"] for item in bench._recover_queries(conn)],
                         ["indexed query", "indexed query two"])

    def test_snapshot_refuses_source_inside_artifacts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            artifacts = root / "nested"
            artifacts.mkdir()
            source = artifacts / "source.db"
            sqlite3.connect(source).close()
            args = type("Args", (), {"source": str(source), "artifacts": str(artifacts),
                                      "force": False, "retries": 1, "busy_timeout": 1.0})()
            with self.assertRaises(SystemExit):
                bench.snapshot(args)

    def test_snapshot_refuses_missing_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            args = type("Args", (), {"source": str(Path(tmp) / "missing"),
                                      "artifacts": tmp, "force": False, "retries": 1,
                                      "busy_timeout": 1.0})()
            with self.assertRaises(SystemExit):
                bench.snapshot(args)

    def test_snapshot_uses_logical_capacity_guard(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.db"
            conn = sqlite3.connect(source)
            conn.execute("CREATE TABLE payload(value TEXT)")
            conn.execute("INSERT INTO payload VALUES (?)", ("x" * 4096,))
            conn.commit()
            conn.close()
            out = root / "fixture"
            args = type("Args", (), {"source": str(source), "artifacts": str(out),
                                      "force": False, "retries": 1, "busy_timeout": 1.0,
                                      "deadline": 1.0})()
            logical_size = sqlite3.connect(source).execute("PRAGMA page_count").fetchone()[0] * sqlite3.connect(source).execute("PRAGMA page_size").fetchone()[0]
            with mock.patch.object(bench.shutil, "disk_usage", return_value=type("Usage", (), {"free": logical_size * 2 - 1})()):
                with self.assertRaisesRegex(SystemExit, "logical snapshot"):
                    bench.snapshot(args)

    def test_report_separates_runs_surfaces_and_discards_successful_warmup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "capture-manifest.json").write_text("{}")
            runs = root / "runs"
            runs.mkdir()
            samples = self._complete_run()["samples"]
            samples.append({"case_id": "case", "surface": "retriever", "phase": "warmup_discarded",
                            "iteration": 0, "ok": True, "result_count":1, "result_identity":["id"], "result_digest": "one", "duration_ms": 999})
            (runs / "suite-1.json").write_text(json.dumps(self._complete_run(samples=samples)))
            bench.report(type("Args", (), {"artifacts": str(root)})())
            report = (root / "report.md").read_text()
            self.assertNotIn("999.000", report)
            self.assertIn("suite-1.json — case — tool — warm", report)
            self.assertIn("fixture_sha256: a", report)
            self.assertIn("scenario_digest: s", report)

    def test_artifacts_inside_repo_must_be_git_ignored(self):
        with mock.patch.object(bench.subprocess, "check_output", return_value=""), mock.patch.object(bench.subprocess, "run", return_value=type("Result", (), {"returncode": 1})()):
            with self.assertRaisesRegex(SystemExit, "unignored"):
                bench._ensure_ignored_artifacts(ROOT / "not-ignored")

    def test_uri_percent_encodes_reserved_filename_characters(self):
        path = Path(tempfile.gettempdir()) / "db?name#fragment.sqlite"
        self.assertIn("%3Fname%23fragment.sqlite", bench._uri(path))

    def test_snapshot_validation_removes_temp_on_exception(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "source.db"
            sqlite3.connect(source).close()
            args = type("Args", (), {"source": str(source), "artifacts": str(root / "fixture"),
                                      "force": False, "retries": 1, "busy_timeout": 1.0, "deadline": 1.0})()
            original = bench._counts
            try:
                bench._counts = mock.Mock(side_effect=RuntimeError("validation interrupted"))
                with self.assertRaisesRegex(RuntimeError, "validation interrupted"):
                    bench.snapshot(args)
                self.assertEqual(list((root / "fixture").glob(".captured.db.*.tmp")), [])
            finally:
                bench._counts = original

    def test_build_configuration_records_compiler_and_forwarded_environment(self):
        with mock.patch.object(bench.subprocess, "check_output", side_effect=["rustc\nhost: x86_64-test\n", "cargo 1"]):
            with mock.patch.dict("os.environ", {"RUSTFLAGS": "-C opt-level=3", "CARGO_BUILD_TARGET": "wasm32"}, clear=True):
                config = bench._build_configuration()
        self.assertEqual(config["target"], "wasm32")
        self.assertEqual(config["environment"]["RUSTFLAGS"], "-C opt-level=3")
        self.assertEqual(config["rustc_version_verbose"], "rustc\nhost: x86_64-test")

    def test_selective_term_requires_bounded_nonzero_match(self):
        conn = sqlite3.connect(":memory:")
        conn.execute("CREATE VIRTUAL TABLE message_fts USING fts5(body)")
        conn.executemany("INSERT INTO message_fts(body) VALUES (?)", [("rareterm",), ("rareterm",)])
        self.assertEqual(bench._selective_term(conn, "rareterm common"), "rareterm")

    def test_prepare_uses_single_alphanumeric_no_hit_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / "captured.db"
            db.write_bytes(b"fixture")
            fixture_hash = bench._hash(db)
            (root / "capture-manifest.json").write_text(json.dumps({
                "kind": "conversation-search-fixture", "snapshot_path": str(db),
                "sha256": fixture_hash, "size_bytes": db.stat().st_size,
                "source_path": str(root / "source.db"),
            }))
            args = type("Args", (), {"artifacts": str(root), "force": True})()
            with mock.patch.object(bench, "_recover_queries", return_value=[
                {"query": "first query", "source_call_id": "one", "message_id": "m1"},
                {"query": "second query", "source_call_id": "two", "message_id": "m2"},
            ]), mock.patch.object(bench.sqlite3, "connect") as connect:
                connect.return_value.execute.return_value.fetchone.return_value = ("conv",)
                connect.return_value.execute.return_value.fetchall.return_value = [(1,)]
                with mock.patch.object(bench, "_selective_term", return_value="verifiedterm"):
                    connect.return_value.execute.return_value.fetchone.side_effect = [(1,), (1000,), (0,)]
                    bench.prepare(args)
            scenarios = json.loads((root / "scenarios.json").read_text())["scenarios"]
            no_hit = next(item for item in scenarios if item["id"] == "verified-no-hit")
            self.assertRegex(no_hit["query"], r"^[A-Za-z0-9]+$")

    def test_compare_refuses_measurement_regime_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            base = self._complete_run()
            before.write_text(json.dumps(base))
            after.write_text(json.dumps({**base, "run_uuid":"other", "runtime": {"worker_threads": 3}}))
            with self.assertRaisesRegex(SystemExit, "regime"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_compare_refuses_missing_metadata_and_output_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            metadata = self._complete_run()
            before.write_text(json.dumps(metadata))
            incomplete = dict(metadata)
            incomplete.pop("commit")
            after.write_text(json.dumps(incomplete))
            with self.assertRaisesRegex(SystemExit, "missing metadata"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())
            changed = [dict(sample) for sample in metadata["samples"]]
            changed[0]["result_digest"] = "two"
            after.write_text(json.dumps({**metadata, "samples": changed}))
            with self.assertRaisesRegex(SystemExit, "output mismatch"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_compare_refuses_incomplete_warm_counts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            run = self._complete_run(samples=self._complete_run()["samples"][:-1])
            before.write_text(json.dumps(run))
            after.write_text(json.dumps(self._complete_run()))
            with self.assertRaisesRegex(SystemExit, "incomplete warm"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())

    def test_run_rejects_path_traversal_label(self):
        with self.assertRaisesRegex(SystemExit, "label"):
            bench._label("../escape")

    def test_compare_refuses_fixture_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            before, after = root / "a.json", root / "b.json"
            before.write_text(json.dumps(self._complete_run()))
            after.write_text(json.dumps(self._complete_run(fixture_sha256="b")))
            with self.assertRaisesRegex(SystemExit, "regime"):
                bench.compare(type("Args", (), {"before": str(before), "after": str(after)})())


if __name__ == "__main__":
    unittest.main()
