"""Finite capture regressions extracted from 03c656942.

Capture fixtures use the diagnostic-directory route, not Goals orchestration.
Workflow job/metadata/retention assertions from test_diagnostic_routing are omitted:
workflow wiring belongs to its owner and is not part of this helper extraction.
All helper, pipe-ownership, readiness, wait-order and output assertions are retained.
"""

import ast
import importlib.util
import io
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import tempfile
import threading
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_devpy():
    spec = importlib.util.spec_from_file_location("qualification_devpy", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class QualificationOutputTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        env = mock.patch.dict(os.environ, {"PHOENIX_CHECK_DIAGNOSTIC_DIR": str(self.directory)}, clear=True)
        env.start()
        self.addCleanup(env.stop)
        self.results = []
        self.reporter = mock.Mock()
        source = ast.parse((ROOT / "dev.py").read_text())
        check = next(node for node in source.body if isinstance(node, ast.FunctionDef)
                     and node.name == "cmd_check")
        step = next(node for node in check.body if isinstance(node, ast.FunctionDef)
                    and node.name == "run_step")
        namespace = dict(vars(self.dev), CHECK_TIMEOUT=600, compiler_cache_env=None,
                         worker_budget=1, profile_work=False,
                         reporter=self.reporter, results=self.results, results_lock=threading.Lock(),
                         _begin_dev_span=mock.Mock(return_value=None),
                         _finish_check_step_span=mock.Mock(),
                         _check_step_artifact_dir=mock.Mock(return_value=None))
        exec(compile(ast.Module(body=[step], type_ignores=[]), str(ROOT / "dev.py"), "exec"), namespace)
        self.namespace = namespace
        self.run_step = namespace["run_step"]

    def execute(self, diagnostic, returncode=0, *, timeout=False, name="cargo clippy"):
        stream = (io.BytesIO(diagnostic.encode()) if bool(os.environ.get("PHOENIX_CHECK_DIAGNOSTIC_DIR"))
                  else io.StringIO(diagnostic))
        process = mock.Mock(pid=12345, stdout=stream, returncode=returncode)
        process.wait.side_effect = ([subprocess.TimeoutExpired(["fixture"], 7), returncode]
                                    if timeout else [returncode])
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process) as launch, \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            result = self.run_step(name, ["fixture"], timeout=7)
        launch.assert_called_once()
        if not timeout:
            process.wait.assert_called_once_with(timeout=7)
        self.assertEqual(launch.call_args.kwargs["stderr"], subprocess.STDOUT)
        self.assertTrue(launch.call_args.kwargs["start_new_session"])
        qualification = bool(os.environ.get("PHOENIX_CHECK_DIAGNOSTIC_DIR"))
        self.assertEqual(launch.call_args.kwargs["text"], not qualification)
        self.assertEqual(launch.call_args.kwargs["bufsize"], 0 if qualification else 1)
        if timeout:
            kill.assert_called_once_with(process.pid, self.dev.signal.SIGTERM)
        else:
            kill.assert_not_called()
        return result, process

    def test_clippy_diagnostics_capture_all_output_without_goal_reuse(self):
        os.environ.pop("PHOENIX_CHECK_DIAGNOSTIC_DIR", None)
        diagnostic_dir = self.directory / "diagnostics"
        payload = b"FIRST_CLIPPY_DIAGNOSTIC\n" + b"later diagnostic\n" * 400
        process = mock.Mock(pid=12345, stdout=io.BytesIO(payload), returncode=101)
        process.wait.return_value = 101
        command = ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]
        with mock.patch.dict(os.environ, {"PHOENIX_CHECK_DIAGNOSTIC_DIR": str(diagnostic_dir)}, clear=True), \
             mock.patch.object(self.dev.subprocess, "Popen", return_value=process) as launch, \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            result = self.run_step("cargo clippy", command)
        self.assertEqual(result, 101)
        self.assertEqual(launch.call_args.args[0], command)
        self.assertEqual(launch.call_args.kwargs["stderr"], subprocess.STDOUT)
        self.assertFalse(launch.call_args.kwargs["text"])
        self.assertEqual(launch.call_args.kwargs["bufsize"], 0)
        self.assertEqual(launch.call_args.kwargs["env"]["CARGO_BUILD_JOBS"], "1")
        self.assertNotIn("PHOENIX_GOAL_REUSE_DIR", launch.call_args.kwargs["env"])
        kill.assert_not_called()
        log = (diagnostic_dir / "cargo-clippy.full.log").read_bytes()
        self.assertTrue(log.startswith(payload))
        self.assertIn(b"process_returncode=101 timed_out=False reader_complete=True", log)
        self.assertIn(b"payload COMPLETE", log)
        self.assertEqual(list(diagnostic_dir.iterdir()), [diagnostic_dir / "cargo-clippy.full.log"])
        self.assertFalse((self.directory / "cargo-clippy.full.log").exists())

    def test_unittest_command_is_once_and_verbose_only_with_diagnostics(self):
        source = ast.parse((ROOT / "dev.py").read_text())
        check = next(node for node in source.body
                     if isinstance(node, ast.FunctionDef) and node.name == "cmd_check")
        block = next(node for node in ast.walk(check) if isinstance(node, ast.If)
                     and any(isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call)
                             and isinstance(stmt.value.func, ast.Name)
                             and stmt.value.func.id == "run_step" and stmt.value.args
                             and isinstance(stmt.value.args[0], ast.Constant)
                             and stmt.value.args[0].value == "dev.py unit tests"
                             for stmt in node.orelse))
        start = next(index for index, stmt in enumerate(block.orelse)
                     if isinstance(stmt, ast.Assign)
                     and any(isinstance(target, ast.Name) and target.id == "unittest_cmd"
                             for target in stmt.targets))
        code = compile(ast.Module(body=block.orelse[start:], type_ignores=[]),
                       str(ROOT / "dev.py"), "exec")
        for diagnostic in (None, "", str(self.directory)):
            for profile in (None, self.directory / "profile"):
                with self.subTest(diagnostic=diagnostic, profile=profile):
                    env = {} if diagnostic is None else {"PHOENIX_CHECK_DIAGNOSTIC_DIR": diagnostic}
                    run_step = mock.Mock()
                    namespace = dict(os=os, sys=sys, ROOT=ROOT, profile_work=profile is not None,
                                     _CHECK_PROFILE=profile, run_step=run_step)
                    with mock.patch.dict(os.environ, env, clear=True):
                        exec(code, namespace)
                    prefix = ([sys.executable, "-m", "unittest"] if profile is None else
                              [sys.executable, str(ROOT / "scripts" / "python_unittest_profile.py")])
                    expected = prefix + ["discover", "tests/devpy"] + (["-v"] if diagnostic else [])
                    run_step.assert_called_once_with("dev.py unit tests", expected)

    def test_unittest_capture_retains_verbose_summary_skips_and_completion(self):
        for returncode, ending in ((0, "OK (skipped=1)"),
                                  (1, "FAILED (failures=1, skipped=1)")):
            with self.subTest(returncode=returncode):
                log_path = self.directory / "devpy-unit-tests.full.log"
                payload = ("test_one (suite.Case.test_one) ... ok\n" * 240
                           + "test_skip (suite.Case.test_skip) ... skipped 'platform reason'\n"
                           + "Ran 241 tests in 0.123s\n\n" + ending + "\n")
                result, _ = self.execute(payload, returncode, name="dev.py unit tests")
                raw = log_path.read_bytes()
                self.assertEqual(result, returncode)
                self.assertTrue(raw.startswith(payload.encode()))
                self.assertIn(b"platform reason", raw)
                self.assertIn(b"Ran 241 tests", raw)
                self.assertIn(b"payload COMPLETE", raw)
                self.assertIn(f"process_returncode={returncode}".encode(), raw)
                self.assertFalse((self.directory / "cargo-clippy.full.log").exists())
                log_path.unlink()

    def test_clippy_capture_cap_keeps_failure_and_marks_truncation(self):
        os.environ.pop("PHOENIX_CHECK_DIAGNOSTIC_DIR", None)
        diagnostic_dir = self.directory / "capped-diagnostics"
        process = mock.Mock(pid=12345, stdout=io.BytesIO(b"x" * 10000), returncode=101)
        process.wait.return_value = 101
        with mock.patch.dict(os.environ, {"PHOENIX_CHECK_DIAGNOSTIC_DIR": str(diagnostic_dir)}, clear=True), \
             mock.patch.object(self.dev._QualificationOutputLog, "MAX_BYTES", 2048), \
             mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            result = self.run_step("cargo clippy", ["fixture"])
        self.assertEqual(result, 101)
        kill.assert_not_called()
        log = (diagnostic_dir / "cargo-clippy.full.log").read_bytes()
        self.assertLessEqual(len(log), 2048)
        self.assertIn(b"process_returncode=101 timed_out=False reader_complete=True", log)
        self.assertIn(b"raw diagnostic limit exceeded; payload INCOMPLETE", log)
        self.assertFalse((self.directory / "cargo-clippy.full.log").exists())

    def test_diagnostic_routing_is_separate_from_goal_reuse_and_tests(self):
        diagnostic = {"PHOENIX_CHECK_DIAGNOSTIC_DIR": str(self.directory)}
        self.assertEqual(self.dev._check_diagnostic_log_path("cargo clippy", diagnostic),
                         self.directory / "cargo-clippy.full.log")
        self.assertEqual(self.dev._check_diagnostic_log_path("dev.py unit tests", diagnostic),
                         self.directory / "devpy-unit-tests.full.log")
        self.assertIsNone(self.dev._check_diagnostic_log_path("dev.py unit tests", {}))
        self.assertIsNone(self.dev._check_diagnostic_log_path("cargo test", diagnostic))
        self.assertIsNone(self.dev._check_diagnostic_log_path("cargo clippy", {"PHOENIX_GOAL_REUSE_DIR": str(self.directory)}))

    def test_newline_free_stream_uses_bounded_reads_spool_and_byte_tail(self):
        sizes = []
        observed_sizes = []

        class GeneratedStream:
            remaining = 2 * 1024 * 1024

            def read(self, size):
                sizes.append(size)
                count = min(size, self.remaining)
                self.remaining -= count
                return b"x" * count

        with mock.patch.object(self.dev._QualificationOutputLog, "MAX_BYTES", 2048):
            log = self.dev._QualificationOutputLog(self.directory / "cargo-clippy.full.log")
            with mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
                log.read(GeneratedStream(), threading.Event(), lambda text: observed_sizes.append(len(text)))
            self.assertTrue(log.finish(0, False, True))
        self.assertEqual(set(sizes), {64 * 1024})
        self.assertLessEqual(max(observed_sizes), 64 * 1024)
        self.assertLessEqual(len(log.tail), 128 * 1024)
        text, truncated = log.console_tail()
        self.assertTrue(truncated)
        self.assertLessEqual(len(text.encode()), 128 * 1024)
        self.assertLessEqual((self.directory / "cargo-clippy.full.log").stat().st_size, 2048)

    def test_short_reads_split_utf8_and_eof_flush_preserve_raw_bytes(self):
        chunks = [b"first\xe2", b"\x82", b"\xac\nlast", b"\xe2", b""]
        stream = mock.Mock()
        stream.read.side_effect = chunks
        observed = []
        log = self.dev._QualificationOutputLog(self.directory / "cargo-clippy.full.log")
        with mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            log.read(stream, threading.Event(), observed.append)
        self.assertFalse(log.finish(0, False, True))
        self.assertEqual(observed, ["first€\n", "last�"])
        self.assertTrue((self.directory / "cargo-clippy.full.log").read_bytes().startswith(b"".join(chunks)))
        self.assertEqual(stream.read.call_args_list, [mock.call(64 * 1024)] * len(chunks))
        self.assertEqual(log.console_tail(), ("first€\nlast�", False))

    def pipe_result(self, *, returncode=0, timeout=False):
        read_fd, write_fd = os.pipe()
        stream = os.fdopen(read_fd, "rb", buffering=0)
        os.write(write_fd, b"early assertion from a test-owned pipe\n")
        process = mock.Mock(pid=12345, stdout=stream, returncode=returncode)
        process.wait.side_effect = ([subprocess.TimeoutExpired(["fixture"], 7), returncode]
                                    if timeout else [returncode])
        try:
            with mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
                 mock.patch.object(self.dev.os, "killpg") as kill, \
                 mock.patch.object(self.dev._QualificationOutputLog, "DRAIN_SECONDS", 0.01):
                result = self.run_step("cargo clippy", ["fixture"], timeout=7)
            self.assertTrue(stream.closed, "select/stop reader must close its own stream")
            raw = (self.directory / "cargo-clippy.full.log").read_bytes()
            self.assertIn(b"reader_complete=True", raw)
            self.assertIn(b"payload INCOMPLETE; writer ownership unresolved", raw)
            self.assertIn(f"process_returncode={returncode}".encode(), raw)
            self.assertTrue(raw.startswith(b"early assertion from a test-owned pipe\n"))
            return result, kill.call_args_list
        finally:
            os.close(write_fd)
            stream.close()

    def test_root_exit_held_pipe_stops_real_reader_without_signaling(self):
        result, calls = self.pipe_result()
        self.assertEqual(result, 1)
        self.assertEqual(calls, [])

    def test_nonzero_root_held_pipe_preserves_status_without_signaling(self):
        result, calls = self.pipe_result(returncode=42)
        self.assertEqual(result, 42)
        self.assertEqual(calls, [])

    def test_timeout_held_pipe_signals_only_before_successful_root_wait(self):
        result, calls = self.pipe_result(returncode=-15, timeout=True)
        self.assertEqual(result, -15)
        self.assertEqual(calls, [mock.call(12345, self.dev.signal.SIGTERM)])

    def test_actual_reaped_root_held_writer_is_not_signaled(self):
        read_fd, write_fd = os.pipe()
        stream = os.fdopen(read_fd, "rb", buffering=0)
        original_popen = subprocess.Popen
        ready_read, ready_write = os.pipe()
        writer = original_popen(
            [sys.executable, "-c",
             "import os,sys; print('writer ready',flush=True); os.write(int(sys.argv[1]),b'R'); "
             "os.close(int(sys.argv[1])); sys.stdin.read()", str(ready_write)],
            stdin=subprocess.PIPE, stdout=write_fd, stderr=subprocess.DEVNULL,
            start_new_session=True, pass_fds=(ready_write,),
        )
        os.close(ready_write)
        roots = []
        held_write_fds = [write_fd]

        def launch(*args, **kwargs):
            kwargs["stdout"] = write_fd
            root = original_popen(*args, **kwargs)
            root.stdout = stream
            roots.append(root)
            os.close(held_write_fds.pop())
            return root

        try:
            self.assertTrue(self.dev.select.select([ready_read], [], [], 5)[0],
                            "writer must publish readiness before root launch")
            self.assertEqual(os.read(ready_read, 1), b"R")
            started = time.monotonic()
            with mock.patch.object(self.dev.subprocess, "Popen", side_effect=launch), \
                 mock.patch.object(self.dev.os, "killpg") as kill, \
                 mock.patch.object(self.dev._QualificationOutputLog, "DRAIN_SECONDS", 0.05):
                result = self.run_step("cargo clippy", [sys.executable, "-c",
                                                     "print('root finished',flush=True)"], timeout=3)
            self.assertEqual(len(roots), 1)
            self.assertEqual(roots[0].returncode, 0)
            self.assertEqual(result, 1)
            self.assertTrue(stream.closed, "real reader terminated and closed its own FD")
            self.assertLess(time.monotonic() - started, 3)
            kill.assert_not_called()
            with self.assertRaises(subprocess.TimeoutExpired):
                writer.wait(timeout=0.01)
            raw = (self.directory / "cargo-clippy.full.log").read_text()
            self.assertIn("root finished", raw)
            self.assertIn("process_returncode=0 timed_out=False reader_complete=True", raw)
            self.assertIn("payload INCOMPLETE; writer ownership unresolved", raw)
            print(f"REAL_CAPTURE_HELD_PIPE root={roots[0].pid} root_status=0 "
                  f"writer={writer.pid} writer_still_live=True reader_closed=True "
                  "signals=0 payload=INCOMPLETE")
        finally:
            os.close(ready_read)
            for descriptor in held_write_fds:
                os.close(descriptor)
            writer.stdin.close()
            for owned in [*roots, writer]:
                try:
                    owned.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    # This test owns the unreaped child; no cached reaped PID is signaled.
                    owned.kill()
                    owned.wait(timeout=2)
            stream.close()
        self.assertEqual(writer.returncode, 0)

    def test_reader_exception_is_recorded_and_stream_closed_without_signaling(self):
        process = mock.Mock(pid=12345, stdout=io.BytesIO(b"partial"), returncode=0)
        process.wait.return_value = 0
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev._QualificationOutputLog, "read", side_effect=OSError("broken reader")), \
             mock.patch.object(threading, "excepthook") as thread_error:
            result = self.run_step("cargo clippy", ["fixture"])
        self.assertEqual(result, 1)
        self.assertTrue(process.stdout.closed)
        kill.assert_not_called()
        thread_error.assert_called_once()
        raw = (self.directory / "cargo-clippy.full.log").read_text()
        self.assertIn("output reader failed: broken reader; payload INCOMPLETE", raw)
        self.assertIn("reader_complete=True", raw)

    def test_interrupted_wait_retains_incomplete_status_without_signaling(self):
        process = mock.Mock(pid=12345, stdout=io.BytesIO(b"before interrupt\n"), returncode=None)
        process.wait.side_effect = KeyboardInterrupt
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            with self.assertRaises(KeyboardInterrupt):
                self.run_step("cargo clippy", ["fixture"])
        kill.assert_not_called()
        self.assertTrue(process.stdout.closed)
        raw = (self.directory / "cargo-clippy.full.log").read_text()
        self.assertIn("process_returncode=None", raw)
        self.assertIn("payload INCOMPLETE; process ownership unresolved", raw)
        self.assertNotIn("payload COMPLETE", raw)
        self.assertEqual(self.results, [])

    def test_actual_timeout_signals_while_root_is_unreaped_only(self):
        original_popen = subprocess.Popen
        original_killpg = os.killpg
        roots = []
        signal_states = []

        def launch(*args, **kwargs):
            root = original_popen(*args, **kwargs)
            roots.append(root)
            return root

        def signal_owned(group, sig):
            self.assertEqual(group, roots[0].pid)
            self.assertIsNone(roots[0].returncode, "timeout wait has not reaped the root")
            signal_states.append(sig)
            original_killpg(group, sig)

        try:
            with mock.patch.object(self.dev.subprocess, "Popen", side_effect=launch), \
                 mock.patch.object(self.dev.os, "killpg", side_effect=signal_owned):
                result = self.run_step("cargo clippy", [sys.executable, "-c",
                                                     "import time; print('started',flush=True); time.sleep(10)"],
                                       timeout=0.2)
            self.assertEqual(result, -signal.SIGTERM)
            self.assertEqual(signal_states, [signal.SIGTERM])
            self.assertTrue(roots[0].stdout.closed)
            raw = (self.directory / "cargo-clippy.full.log").read_text()
            self.assertIn("timed_out=True reader_complete=True", raw)
            self.assertIn("payload COMPLETE", raw)
            print(f"REAL_CAPTURE_TIMEOUT root={roots[0].pid} root_status={result} "
                  "signals=TERM_only_before_reap reader_closed=True payload=COMPLETE")
        finally:
            for root in roots:
                try:
                    root.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    root.kill()
                    root.wait(timeout=2)
                root.stdout.close()

    def test_normal_successful_eof_has_complete_capture_and_no_signal(self):
        result, _ = self.execute("complete normal output\n")
        self.assertEqual(result, 0)
        raw = (self.directory / "cargo-clippy.full.log").read_text()
        self.assertIn("process_returncode=0 timed_out=False reader_complete=True", raw)
        self.assertIn("payload COMPLETE", raw)

    def test_failure_payload_survives_console_tail_and_original_exit_is_preserved(self):
        diagnostic = "thread fixture panicked: assertion left=1 right=2\n" + "later output\n" * 400
        result, _ = self.execute(diagnostic, 42)
        self.assertEqual(result, 42)
        raw = (self.directory / "cargo-clippy.full.log").read_text()
        self.assertTrue(raw.startswith(diagnostic))
        self.assertIn("process_returncode=42", raw)
        self.assertIn("payload COMPLETE", raw)
        self.assertNotIn("assertion left", self.results[0][3])
        self.assertIn("Full qualification diagnostics:", self.results[0][3])

    def test_timeout_keeps_early_assertions_and_real_signal_status(self):
        diagnostic = "assertion failed before timeout\n" + "later output\n" * 400
        result, process = self.execute(diagnostic, -15, timeout=True)
        self.assertEqual(result, -15)
        self.assertEqual(process.wait.call_args_list, [mock.call(timeout=7), mock.call(timeout=3)])
        raw = (self.directory / "cargo-clippy.full.log").read_text()
        self.assertTrue(raw.startswith(diagnostic))
        self.assertIn("process_returncode=-15 timed_out=True reader_complete=True", raw)
        self.assertIn("TIMEOUT after 7s", self.results[0][3])
        self.assertIn("earlier lines dropped", self.results[0][3])

    def test_overflow_is_bounded_and_fails_a_successful_process(self):
        with mock.patch.object(self.dev._QualificationOutputLog, "MAX_BYTES", 2048):
            result, _ = self.execute("diagnostic\n" * 500)
        self.assertEqual(result, 1)
        raw = (self.directory / "cargo-clippy.full.log").read_bytes()
        self.assertLessEqual(len(raw), 2048)
        self.assertIn(b"process_returncode=0", raw)
        self.assertIn(b"payload INCOMPLETE", raw)
        self.assertIn("QUALIFICATION FAILED:", self.results[0][3])
        self.reporter.step_done.assert_called_once()
        self.assertEqual(self.reporter.step_done.call_args.args[2], 1)

    def test_overflow_does_not_replace_a_nonzero_process_status(self):
        with mock.patch.object(self.dev._QualificationOutputLog, "MAX_BYTES", 2048):
            result, _ = self.execute("diagnostic\n" * 500, 100)
        self.assertEqual(result, 100)
        self.assertIn("QUALIFICATION FAILED:", self.results[0][3])

    def test_non_qualification_step_has_no_spool_or_changed_exit(self):
        os.environ.pop("PHOENIX_CHECK_DIAGNOSTIC_DIR", None)
        result, _ = self.execute("not a qualification\n", 23)
        self.assertEqual(result, 23)
        self.assertFalse((self.directory / "cargo-clippy.full.log").exists())

    def test_reader_incomplete_marks_artifact_and_existing_artifact_is_not_overwritten(self):
        path = self.directory / "cargo-clippy.full.log"
        log = self.dev._QualificationOutputLog(path)
        log.append(b"partial output\n")
        error = log.finish(0, False, False)
        self.assertIn("INCOMPLETE", error)
        self.assertIn("reader_complete=False", path.read_text())
        before = path.read_bytes()
        with self.assertRaises(FileExistsError):
            self.dev._QualificationOutputLog(path)
        self.assertEqual(path.read_bytes(), before)

    def test_launch_error_retains_incomplete_marker_and_is_not_swallowed(self):
        with mock.patch.object(self.dev.subprocess, "Popen", side_effect=OSError("cannot launch")):
            with self.assertRaisesRegex(OSError, "cannot launch"):
                self.run_step("cargo clippy", ["fixture"])
        raw = (self.directory / "cargo-clippy.full.log").read_text()
        self.assertIn("process_returncode=None", raw)
        self.assertIn("payload INCOMPLETE", raw)
        self.assertEqual(self.results, [])

    def test_pipe_termination_escalation_retains_original_cleanup_and_status(self):
        process = mock.Mock(pid=12345, stdout=io.BytesIO(b"early assertion\n"), returncode=None)
        custody_order = []

        def wait(*, timeout):
            custody_order.append(("wait", timeout))
            if timeout in (7, 3):
                raise subprocess.TimeoutExpired(["fixture"], timeout)
            self.assertEqual(timeout, 5)
            process.returncode = -9
            return -9

        def signal_owned(group, sig):
            self.assertIsNone(process.returncode, "no signal after root was reaped")
            custody_order.append(("signal", sig))

        process.wait.side_effect = wait
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
             mock.patch.object(self.dev.os, "killpg", side_effect=signal_owned) as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            result = self.run_step("cargo clippy", ["fixture"], timeout=7)
        self.assertEqual(result, -9)
        self.assertEqual(kill.call_args_list, [mock.call(12345, self.dev.signal.SIGTERM),
                                               mock.call(12345, self.dev.signal.SIGKILL)])
        self.assertEqual(process.wait.call_args_list,
                         [mock.call(timeout=7), mock.call(timeout=3), mock.call(timeout=5)])
        self.assertEqual(custody_order, [("wait", 7), ("signal", signal.SIGTERM),
                                         ("wait", 3), ("signal", signal.SIGKILL), ("wait", 5)])
        self.assertIn("process_returncode=-9 timed_out=True", (self.directory / "cargo-clippy.full.log").read_text())
