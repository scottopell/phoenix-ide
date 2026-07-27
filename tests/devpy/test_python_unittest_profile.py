import json
import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "scripts" / "python_unittest_profile.py"


class PythonUnittestProfileTests(unittest.TestCase):
    def test_runner_preserves_normal_unittest_output_and_writes_jsonl(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "test_sample.py").write_text(textwrap.dedent("""
                import sys
                import unittest

                class Sample(unittest.TestCase):
                    def test_ok(self):
                        print('hello-from-test')
                        self.assertTrue(True)

                if __name__ == '__main__':
                    unittest.main()
            """))
            profile_dir = root / "profile"
            env = dict(os.environ)
            env["PHOENIX_CHECK_PROFILE_DIR"] = str(profile_dir)
            result = subprocess.run(
                [sys.executable, str(RUNNER), "discover", "-s", str(root), "-t", str(root)],
                cwd=root,
                capture_output=True,
                text=True,
                env=env,
            )
            records = [json.loads(line) for line in (profile_dir / "python-test-cpu.jsonl").read_text().splitlines()]

        self.assertEqual(0, result.returncode)
        self.assertIn("Ran 1 test", result.stderr)
        self.assertIn("OK", result.stderr)
        self.assertEqual(1, len(records))
        record = records[0]
        self.assertEqual("windowed_process", record["provenance"])
        self.assertEqual("python_unittest:test_sample.Sample.test_ok", record["identity"])
        self.assertEqual("python_unittest_testcase", record["kind"])
        self.assertIn("wall_ms", record)
        self.assertIn("total_cpu_ms", record)

    def test_failing_subtest_marks_parent_record_failed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "test_sample.py").write_text(textwrap.dedent("""
                import unittest

                class Sample(unittest.TestCase):
                    def test_subtests(self):
                        for value in (1, 2):
                            with self.subTest(value=value):
                                self.assertEqual(value, 1)
            """))
            profile_dir = root / "profile"
            env = dict(os.environ)
            env["PHOENIX_CHECK_PROFILE_DIR"] = str(profile_dir)
            result = subprocess.run(
                [sys.executable, str(RUNNER), "discover", "-s", str(root), "-t", str(root)],
                cwd=root, capture_output=True, text=True, env=env,
            )
            record = json.loads(
                (profile_dir / "python-test-cpu.jsonl").read_text().splitlines()[0]
            )

        self.assertNotEqual(0, result.returncode)
        self.assertEqual("failed", record["status"])



if __name__ == "__main__":
    unittest.main()
