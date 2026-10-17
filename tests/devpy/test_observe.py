import importlib.util
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("devpy_observe_under_test", ROOT / "dev.py")
dev = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dev)


class ObserveTests(unittest.TestCase):
    def invoke(self, *arguments):
        with mock.patch.object(sys, "argv", ["dev.py", *arguments]), \
                mock.patch.object(sys, "stdout"), mock.patch.object(sys, "stderr"):
            with self.assertRaises(SystemExit) as result:
                dev.main()
        return result.exception.code

    def test_parser_defaults_and_no_bootstrap_even_with_pretty(self):
        with mock.patch.object(dev, "cmd_observe", return_value=7) as launch, \
                mock.patch.object(dev, "_bootstrap_dev_tracing") as tracing, \
                mock.patch.object(dev, "_bootstrap_rich") as rich, \
                mock.patch.object(dev, "_start_dev_command_tracing") as start, \
                mock.patch.object(dev, "build_rust") as build:
            self.assertEqual(7, self.invoke("--pretty", "observe", "--fixture", "/private/fixture.db",
                                           "--target", "pc:example", "--binary", "/candidate/phoenix_ide"))
            launch.assert_called_once_with(fixture=Path("/private/fixture.db"), target="pc:example",
                                           binary=Path("/candidate/phoenix_ide"), port=0)
            for forbidden in (tracing, rich, start, build):
                forbidden.assert_not_called()

    def test_invalid_arguments_never_launch(self):
        valid = ["observe", "--fixture", "/fixture.db", "--target", "example", "--binary", "/binary"]
        cases = [valid[:-2], [*valid, "--port", "-1"], [*valid, "--port", "65536"],
                 [*valid, "--port", "not-a-port"],
                 ["observe", "--fixture", "relative.db", *valid[3:]],
                 ["observe", "--fixture", "/fixture.db", "--target", " ", *valid[5:]]]
        with mock.patch.object(dev, "cmd_observe") as launch:
            for case in cases:
                with self.subTest(case=case):
                    self.assertEqual(2, self.invoke(*case))
            launch.assert_not_called()

    def test_explicit_port(self):
        with mock.patch.object(dev, "cmd_observe", return_value=0) as launch:
            self.assertEqual(0, self.invoke("observe", "--fixture", "/fixture.db", "--target", "example",
                                           "--binary", "/binary", "--port", "65535"))
            self.assertEqual(65535, launch.call_args.kwargs["port"])

    def test_launch_is_allowlisted_isolated_and_does_not_open_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "candidate"
            binary.touch()
            binary.chmod(0o700)
            fixture = Path(temporary) / "deliberately-missing.db"
            child = mock.Mock()
            child.wait.return_value = 23
            child.poll.return_value = 23
            captured = {}

            def popen(command, *, cwd, env):
                captured.update(cwd=cwd, env=env)
                self.assertEqual([str(binary.resolve()), "--observe-fixture", str(fixture),
                                  "--target", "exact target", "--port", "0"], command)
                self.assertEqual([], list(cwd.glob(".env*")))
                self.assertEqual({"HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
                                  "XDG_STATE_HOME", "TMPDIR", "TMP", "TEMP", "LANG", "LC_ALL"}, set(env))
                for key in ("HOME", "XDG_CONFIG_HOME", "TMPDIR"):
                    self.assertTrue(Path(env[key]).is_dir())
                    self.assertTrue(Path(env[key]).is_relative_to(cwd))
                return child

            with mock.patch.dict(os.environ, {"ANTHROPIC_API_KEY": "secret", "PHOENIX_DB_PATH": "/live.db",
                                               "OTEL_EXPORTER_OTLP_ENDPOINT": "http://collector",
                                               "DYLD_INSERT_LIBRARIES": "unsafe", "HOME": "/personal"}), \
                    mock.patch.object(dev.subprocess, "Popen", side_effect=popen) as launch:
                self.assertEqual(23, dev.cmd_observe(fixture=fixture, target="exact target", binary=binary))
                launch.assert_called_once()
            self.assertFalse(captured["cwd"].exists())
            self.assertFalse(fixture.exists())

    def test_missing_or_nonexecutable_binary_never_spawns(self):
        with tempfile.TemporaryDirectory() as temporary, mock.patch.object(dev.subprocess, "Popen") as launch:
            binary = Path(temporary) / "binary"
            with self.assertRaises(FileNotFoundError):
                dev.cmd_observe(fixture=Path("/fixture.db"), target="target", binary=binary)
            binary.touch()
            with self.assertRaises(ValueError):
                dev.cmd_observe(fixture=Path("/fixture.db"), target="target", binary=binary)
            launch.assert_not_called()

    def test_signals_are_forwarded_and_handlers_restored(self):
        child = mock.Mock()
        child.poll.return_value = -signal.SIGTERM
        handlers = {}

        def install(signum, handler):
            handlers[signum] = handler
            return signal.SIG_DFL

        def wait():
            handlers[signal.SIGTERM](signal.SIGTERM, None)
            return -signal.SIGTERM

        child.wait.side_effect = wait
        with mock.patch.object(dev.subprocess, "Popen", return_value=child), \
                mock.patch.object(dev.signal, "signal", side_effect=install):
            self.assertEqual(143, dev.cmd_observe(fixture=Path("/fixture.db"), target="target",
                                                 binary=Path(sys.executable)))
        child.send_signal.assert_called_once_with(signal.SIGTERM)
        self.assertEqual({signal.SIGINT: signal.SIG_DFL, signal.SIGTERM: signal.SIG_DFL}, handlers)

    def test_exception_reaps_child_before_removing_scratch(self):
        child = mock.Mock()
        child.poll.return_value = None
        child.wait.side_effect = [RuntimeError("wait failed"), subprocess.TimeoutExpired("candidate", 10), 0]
        with mock.patch.object(dev.subprocess, "Popen", return_value=child):
            with self.assertRaisesRegex(RuntimeError, "wait failed"):
                dev.cmd_observe(fixture=Path("/fixture.db"), target="target", binary=Path(sys.executable))
        child.terminate.assert_called_once()
        child.kill.assert_called_once()
        self.assertEqual(3, child.wait.call_count)


class ObservationExtractorSourceGuard(unittest.TestCase):
    def test_custom_live_state_is_never_named_in_axum_namespace(self):
        root = ROOT
        violations = [
            str(source.relative_to(root))
            for source in (root / "crates/phoenix-ide/src").rglob("*.rs")
            if "axum::extract::" + "LiveState" in source.read_text()
        ]
        self.assertEqual(violations, [])


if __name__ == "__main__":
    unittest.main()
