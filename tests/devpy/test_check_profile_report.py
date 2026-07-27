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
