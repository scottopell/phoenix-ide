import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WRAPPER = ROOT / "scripts" / "check_profile_command.py"


class CheckProfileCommandTests(unittest.TestCase):
    def run_wrapper(self, command):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        output = Path(temporary.name) / "measurement.json"
        result = subprocess.run(
            [sys.executable, str(WRAPPER), "--output", str(output), "--", *command],
            capture_output=True,
            text=True,
        )
        return result, json.loads(output.read_text())

    def test_preserves_streams_exit_code_and_records_cpu(self):
        result, measurement = self.run_wrapper([
            sys.executable,
            "-c",
            "import sys; print('out'); print('err', file=sys.stderr); "
            "sum(i*i for i in range(500000)); sys.exit(7)",
        ])

        self.assertEqual(7, result.returncode)
        self.assertEqual("out\n", result.stdout)
        self.assertEqual("err\n", result.stderr)
        self.assertEqual(1, measurement["schema_version"])
        self.assertEqual("exact_waited_descendants", measurement["provenance"])
        self.assertEqual(7, measurement["returncode"])
        self.assertGreater(measurement["total_cpu_ms"], 0)
        self.assertAlmostEqual(
            measurement["total_cpu_ms"],
            measurement["user_cpu_ms"] + measurement["system_cpu_ms"],
        )
        self.assertIn("identity", measurement)
        self.assertIn("wall_ms", measurement)

    def test_explicit_step_identity_classifies_record_as_step(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "record.json"
            result = subprocess.run([
                sys.executable, str(WRAPPER), "--output", str(output),
                "--identity", "step:rust:cargo test", "--",
                sys.executable, "-c", "pass",
            ])
            record = json.loads(output.read_text())

        self.assertEqual(0, result.returncode)
        self.assertEqual("step", record["kind"])

    @unittest.skipUnless(hasattr(os, "getpgid"), "requires POSIX process groups")
    def test_wrapped_command_remains_in_wrapper_process_group(self):
        result, measurement = self.run_wrapper([
            sys.executable,
            "-c",
            "import os; print(os.getpgrp())",
        ])

        self.assertEqual(0, result.returncode)
        self.assertEqual(os.getpgrp(), int(result.stdout.strip()))
        self.assertGreater(measurement["pid"], 0)

    def test_long_identity_uses_bounded_hashed_filename(self):
        with tempfile.TemporaryDirectory() as directory:
            identity = "rust:test:" + "very_long_test_name_" * 30
            result = subprocess.run([
                sys.executable, str(WRAPPER), "--output-dir", directory,
                "--identity", identity, "--", sys.executable, "-c", "pass",
            ])
            records = list(Path(directory).glob("*.json"))
            record = json.loads(records[0].read_text())

        self.assertEqual(0, result.returncode)
        self.assertEqual(1, len(records))
        self.assertLess(len(records[0].name.encode()), 180)
        self.assertEqual(identity, record["identity"])

    def test_multibyte_identity_uses_byte_bounded_filename(self):
        with tempfile.TemporaryDirectory() as directory:
            identity = "rust:test:" + "測試" * 200
            result = subprocess.run([
                sys.executable, str(WRAPPER), "--output-dir", directory,
                "--identity", identity, "--", sys.executable, "-c", "pass",
            ])
            records = list(Path(directory).glob("*.json"))
            record = json.loads(records[0].read_text())

        self.assertEqual(0, result.returncode)
        self.assertEqual(1, len(records))
        self.assertLess(len(records[0].name.encode()), 180)
        self.assertTrue(records[0].name.isascii())
        self.assertEqual(identity, record["identity"])

    def test_includes_short_lived_waited_grandchild(self):
        baseline_result, baseline = self.run_wrapper([sys.executable, "-c", "pass"])
        self.assertEqual(0, baseline_result.returncode)
        result, measurement = self.run_wrapper([
            sys.executable,
            "-c",
            "import subprocess,sys; subprocess.run([sys.executable, '-c', "
            "'sum(i*i for i in range(3000000))'], check=True)",
        ])

        self.assertEqual(0, result.returncode)
        self.assertGreater(measurement["total_cpu_ms"], baseline["total_cpu_ms"])
        self.assertEqual(
            "waited_descendants_only_survivors_unverified",
            measurement["tree_closure"],
        )

    def test_output_dir_allocates_a_unique_record(self):
        with tempfile.TemporaryDirectory() as temporary:
            result = subprocess.run([
                sys.executable, str(WRAPPER), "--output-dir", temporary,
                "--", sys.executable, "-c", "pass",
            ])
            records = list(Path(temporary).glob("process-*.json"))

        self.assertEqual(0, result.returncode)
        self.assertEqual(1, len(records))

    def test_output_jsonl_appends_versioned_process_tree_record(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "records.jsonl"
            result = subprocess.run([
                sys.executable,
                str(WRAPPER),
                "--output-jsonl",
                str(output),
                "--identity",
                "python_unittest:demo.test_case",
                "--",
                sys.executable,
                "-c",
                "pass",
            ])
            lines = output.read_text().splitlines()

        self.assertEqual(0, result.returncode)
        self.assertEqual(1, len(lines))
        record = json.loads(lines[0])
        self.assertEqual("exact_waited_descendants", record["provenance"])
        self.assertEqual("python_unittest:demo.test_case", record["identity"])
        self.assertIn("wall_ms", record)


if __name__ == "__main__":
    unittest.main()
