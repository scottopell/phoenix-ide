import os
import select
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class OutputBufferingTests(unittest.TestCase):
    def test_progress_is_visible_before_command_exits(self):
        source = textwrap.dedent("""
            import importlib.util
            import sys

            spec = importlib.util.spec_from_file_location("devpy", sys.argv[1])
            dev = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(dev)

            def command(args):
                print("Starting command")
                dev._PlainReporter().step_done("test", "example", 0, 1.0)
                print("Waiting for acknowledgement", file=sys.stderr)
                assert sys.stdin.read(1) == "x"

            dev.cmd_taskmd = command
            sys.argv = [sys.argv[1], "taskmd"]
            dev.main()
        """)
        env = os.environ.copy()
        env.pop("PYTHONUNBUFFERED", None)
        for destination in ("pipe", "file"):
            with self.subTest(destination=destination), tempfile.TemporaryFile() as log:
                with subprocess.Popen(
                    [sys.executable, "-c", source, str(ROOT / "dev.py")],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE if destination == "pipe" else log,
                    stderr=subprocess.PIPE,
                    env=env,
                ) as child:
                    try:
                        ready, _, _ = select.select([child.stderr], [], [], 15)
                        self.assertTrue(ready, "command did not reach the acknowledgement barrier")
                        self.assertEqual(b"Waiting for acknowledgement\n", child.stderr.readline())
                        self.assertIsNone(child.poll())
                        if destination == "pipe":
                            ready, _, _ = select.select([child.stdout], [], [], 0)
                            self.assertTrue(ready, "progress is buffered while the command waits")
                            output = os.read(child.stdout.fileno(), 4096)
                        else:
                            log.seek(0)
                            output = log.read()
                        self.assertEqual("Starting command\n  ✓ example            (1.0s)\n",
                                         output.decode())
                    finally:
                        child.communicate(input=b"x", timeout=15)
                    self.assertEqual(0, child.returncode)


if __name__ == "__main__":
    unittest.main()
