import io
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_runner():
    spec = importlib.util.spec_from_file_location("run_devpy_tests_under_test", ROOT / "scripts" / "run_devpy_tests.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class RunDevpyTestsTests(unittest.TestCase):
    def setUp(self):
        self.runner = load_runner()

    def test_discovery_matches_unittest_discover_module_set(self):
        discovered = set(self.runner.discover_test_modules(ROOT / "tests" / "devpy"))
        suite = unittest.defaultTestLoader.discover(str(ROOT / "tests" / "devpy"))
        expected: set[str] = set()

        def walk(node):
            if isinstance(node, unittest.TestSuite):
                for child in node:
                    walk(child)
            else:
                expected.add(node.__class__.__module__)

        walk(suite)
        self.assertEqual(expected, discovered)

    def test_parallel_runner_aggregates_passes_and_propagates_failures(self):
        with tempfile.TemporaryDirectory() as tmp:
            tests_dir = Path(tmp)
            (tests_dir / "test_alpha.py").write_text(
                "import unittest\n"
                "class A(unittest.TestCase):\n"
                "    def test_ok(self): self.assertEqual(1, 1)\n"
                "    @unittest.skip('expected')\n"
                "    def test_skip(self): pass\n"
            )
            (tests_dir / "test_beta.py").write_text(
                "import unittest\n"
                "class B(unittest.TestCase):\n"
                "    def test_ok(self): self.assertTrue(True)\n"
            )
            out = io.StringIO()
            with mock.patch("sys.stdout", out):
                rc = self.runner.main(["--workers", "2", "--tests-dir", str(tests_dir)])
            self.assertEqual(0, rc)
            self.assertIn("Ran 3 tests across 2 modules", out.getvalue())
            self.assertIn("OK (skipped=1)", out.getvalue())

            (tests_dir / "test_gamma.py").write_text(
                "import unittest\n"
                "class C(unittest.TestCase):\n"
                "    def test_fail(self): self.assertEqual(1, 2, 'boom-sentinel')\n"
            )
            out_fail = io.StringIO()
            with mock.patch("sys.stdout", out_fail):
                rc_fail = self.runner.main(["--workers", "2", "--tests-dir", str(tests_dir)])
            self.assertEqual(1, rc_fail)
            self.assertIn("FAILED: test_gamma", out_fail.getvalue())
            self.assertIn("boom-sentinel", out_fail.getvalue())


if __name__ == "__main__":
    unittest.main()
