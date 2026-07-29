import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_report():
    spec = importlib.util.spec_from_file_location(
        "check_profile_report", ROOT / "scripts" / "check_profile_report.py"
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class CheckProfileReportTests(unittest.TestCase):
    def test_ranks_mixed_runner_records_without_summing_them(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "processes").mkdir()
            (root / "processes" / "step.json").write_text(json.dumps({
                "identity": "step:vitest", "provenance": "exact_process_tree",
                "total_cpu_ms": 20, "wall_ms": 10,
            }))
            (root / "vitest-cpu-worker.jsonl").write_text(json.dumps({
                "full_test_name": "renders", "provenance": "windowed_process",
                "cpu_user_us": 3000, "cpu_system_us": 2000, "wall_time_ms": 7,
            }) + "\n")

            summary = report.render(root, 20)
            markdown = (root / "summary.md").read_text()

        self.assertEqual(2, summary["record_count"])
        self.assertEqual("step:vitest", summary["top_cpu_records"][0]["identity"])
        self.assertEqual(5.0, summary["top_cpu_records"][1]["cpu_ms"])
        self.assertIn("inclusive", markdown)
        self.assertIn("dev.check.test", summary["traceql"]["tests"])

    def test_portable_report_embeds_run_metadata(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "command.json").write_text(json.dumps({
                "identity": "command:dev.py check", "kind": "command",
                "provenance": "exact_process_plus_waited_children",
                "total_cpu_ms": 1, "wall_ms": 2,
                "metadata": {"git_sha": "abc", "active_lanes": ["rust"]},
            }))
            summary = report.render(root, 20, "run")
            markdown = (root / "summary.md").read_text()

        self.assertEqual("abc", summary["run_metadata"]["git_sha"])
        self.assertIn("Run metadata", markdown)

    def test_portable_rows_preserve_retry_outcome_and_overlap_metadata(self):
        report = load_report()
        row = report._normalize(Path("rust-tests/test.json"), {
            "identity": "rust:test", "kind": "rust_test",
            "provenance": "exact_waited_descendants", "total_cpu_ms": 1,
            "attempt": "2", "returncode": 1, "concurrent": True,
            "pid": 42, "binary_id": "crate::bin",
        })

        self.assertEqual("2", row["attempt"])
        self.assertEqual(1, row["returncode"])
        self.assertTrue(row["concurrent"])
        self.assertEqual(42, row["pid"])
        self.assertEqual("crate::bin", row["binary_id"])

    def test_malformed_jsonl_is_reported_without_losing_valid_rows(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "python-test-cpu.jsonl").write_text(
                json.dumps({
                    "identity": "valid", "provenance": "windowed_process",
                    "total_cpu_ms": 1, "wall_ms": 2,
                }) + "\n{truncated\n"
            )
            summary = report.render(root, 20, "run")
            markdown = (root / "summary.md").read_text()

        self.assertEqual("valid", summary["top_cpu_records"][0]["identity"])
        self.assertEqual(2, summary["profile_errors"][0]["line"])
        self.assertIn("Profile record errors", markdown)

    def test_command_total_and_closure_are_exposed(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "command.json").write_text(json.dumps({
                "identity": "command:dev.py check", "kind": "command",
                "provenance": "exact_process_plus_waited_children",
                "total_cpu_ms": 100, "wall_ms": 50,
                "tree_closure": "command_reaped_descendants_unverified",
            }))
            summary = report.render(root, 20, "run")
            markdown = (root / "summary.md").read_text()

        self.assertEqual(100, summary["command"]["cpu_ms"])
        self.assertEqual(1, len(summary["incomplete_tree_records"]))
        self.assertIn("command_reaped_descendants_unverified", markdown)

    def test_traceql_queries_are_scoped_to_profile_run(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            summary = report.render(Path(directory), 20, "run-123")

        self.assertEqual("run-123", summary["profile_run_id"])
        for query in summary["traceql"].values():
            self.assertIn('span.check.profile_run_id = "run-123"', query)

    def test_lane_orchestration_record_is_ranked(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lanes").mkdir()
            (root / "lanes" / "task.json").write_text(json.dumps({
                "identity": "lane:task:in_process_orchestration",
                "kind": "lane_orchestration", "provenance": "exact_thread",
                "total_cpu_ms": 5, "wall_ms": 7,
            }))
            summary = report.render(root, 20, "run")

        self.assertEqual(
            "lane:task:in_process_orchestration",
            summary["top_cpu_records"][0]["identity"],
        )

    def test_vitest_identity_prefers_file_qualified_full_name(self):
        report = load_report()
        row = report._normalize(Path("vitest-cpu-worker.jsonl"), {
            "full_name": "src/a.test.ts > suite > works",
            "full_test_name": "suite > works",
            "file": "src/a.test.ts",
            "cpu_user_us": 1,
            "cpu_system_us": 0,
            "provenance": "windowed_process",
        })

        self.assertEqual("src/a.test.ts > suite > works", row["identity"])

    def test_reconciles_inclusive_parent_with_child_records(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "processes").mkdir()
            (root / "processes" / "vitest-vitest.json").write_text(json.dumps({
                "identity": "step:vitest:vitest", "provenance": "exact_waited_descendants",
                "total_cpu_ms": 100, "wall_ms": 50,
            }))
            (root / "vitest-cpu-worker.jsonl").write_text("\n".join([
                json.dumps({"full_test_name": "one", "provenance": "windowed_process", "cpu_user_us": 30000, "cpu_system_us": 0, "concurrent": True}),
                json.dumps({"full_test_name": "two", "provenance": "windowed_process", "cpu_user_us": 20000, "cpu_system_us": 0}),
            ]) + "\n")

            summary = report.render(root, 20)

        item = summary["reconciliation"][0]
        self.assertEqual(100, item["parent_cpu_ms"])
        self.assertEqual(50, item["attributed_child_cpu_ms"])
        self.assertEqual(50, item["shared_unattributed_cpu_ms"])
        self.assertTrue(item["children_may_overlap"])

    def test_sequential_windowed_children_do_not_claim_overlap(self):
        report = load_report()
        rows = [
            {"source": "vitest-cpu-worker.jsonl", "cpu_ms": 1,
             "provenance": "windowed_process", "concurrent": False},
            {"source": "processes/vitest-vitest.json", "cpu_ms": 2,
             "provenance": "exact_waited_descendants"},
        ]
        item = report._reconciliation(rows)[0]
        self.assertFalse(item["children_may_overlap"])

    def test_unavailable_cpu_is_not_ranked_as_zero(self):
        report = load_report()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "e2e-scenario-cpu.jsonl").write_text(json.dumps({
                "identity": "e2e:server", "provenance": "unavailable",
                "total_cpu_ms": None, "wall_ms": 5,
            }) + "\n")

            summary = report.render(root, 20)
            markdown = (root / "summary.md").read_text()

        self.assertEqual([], summary["top_cpu_records"])
        self.assertEqual("e2e:server", summary["unavailable_cpu_records"][0]["identity"])
        self.assertIn("Unavailable CPU measurements", markdown)



if __name__ == "__main__":
    unittest.main()
