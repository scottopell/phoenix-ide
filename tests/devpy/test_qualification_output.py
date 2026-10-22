"""Qualification capture checks, including one bounded real owned-process-group witness."""

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
        self.real_child_harness = os.environ.get("PHOENIX_CAPTURE_REAL_CHILD_TEST") == "1"
        self.dev = load_devpy()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        env = mock.patch.dict(os.environ, {"PHOENIX_GOAL_REUSE_DIR": str(self.directory)}, clear=True)
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
                         worker_budget=1, profile_work=False, goal_reuse_commands=("fixture",),
                         reporter=self.reporter, results=self.results, results_lock=threading.Lock(),
                         _begin_dev_span=mock.Mock(return_value=None),
                         _finish_check_step_span=mock.Mock(),
                         _check_step_artifact_dir=mock.Mock(return_value=None))
        exec(compile(ast.Module(body=[step], type_ignores=[]), str(ROOT / "dev.py"), "exec"), namespace)
        self.namespace = namespace
        self.run_step = namespace["run_step"]

    def execute(self, diagnostic, returncode=0, *, timeout=False):
        stream = (io.BytesIO(diagnostic.encode()) if self.namespace["goal_reuse_commands"] is not None
                  else io.StringIO(diagnostic))
        process = mock.Mock(pid=12345, stdout=stream, returncode=returncode)
        process.wait.side_effect = ([subprocess.TimeoutExpired(["fixture"], 7), returncode]
                                    if timeout else [returncode])
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process) as launch, \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            result = self.run_step("cargo test", ["fixture"], timeout=7)
        self.assertEqual(launch.call_args.kwargs["stderr"], subprocess.STDOUT)
        self.assertTrue(launch.call_args.kwargs["start_new_session"])
        qualification = self.namespace["goal_reuse_commands"] is not None
        self.assertEqual(launch.call_args.kwargs["text"], not qualification)
        self.assertEqual(launch.call_args.kwargs["bufsize"], 0 if qualification else 1)
        if timeout:
            kill.assert_called_once_with(process.pid, self.dev.signal.SIGTERM)
        else:
            kill.assert_not_called()
        return result, process

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
            log = self.dev._QualificationOutputLog(self.directory / "cargo-tests.full.log")
            with mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
                log.read(GeneratedStream(), threading.Event(), lambda text: observed_sizes.append(len(text)))
            self.assertTrue(log.finish(0, False, True))
        self.assertEqual(set(sizes), {64 * 1024})
        self.assertLessEqual(max(observed_sizes), 64 * 1024)
        self.assertLessEqual(len(log.tail), 128 * 1024)
        text, truncated = log.console_tail()
        self.assertTrue(truncated)
        self.assertLessEqual(len(text.encode()), 128 * 1024)
        self.assertLessEqual((self.directory / "cargo-tests.full.log").stat().st_size, 2048)

    def test_short_reads_split_utf8_and_eof_flush_preserve_raw_bytes(self):
        chunks = [b"first\xe2", b"\x82", b"\xac\nlast", b"\xe2", b""]
        stream = mock.Mock()
        stream.read.side_effect = chunks
        observed = []
        log = self.dev._QualificationOutputLog(self.directory / "cargo-tests.full.log")
        with mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            log.read(stream, threading.Event(), observed.append)
        self.assertFalse(log.finish(0, False, True))
        self.assertEqual(observed, ["first€\n", "last�"])
        self.assertTrue((self.directory / "cargo-tests.full.log").read_bytes().startswith(b"".join(chunks)))
        self.assertEqual(stream.read.call_args_list, [mock.call(64 * 1024)] * len(chunks))
        self.assertEqual(log.console_tail(), ("first€\nlast�", False))

    def pipe_result(self, close_on, *, timeout=False):
        read_fd, write_fd = os.pipe()
        stream = os.fdopen(read_fd, "rb", buffering=0)
        held = [write_fd]
        os.write(write_fd, b"early assertion from a mock-owned pipe\n")
        status = -15 if timeout else 0
        process = mock.Mock(pid=12345, stdout=stream, returncode=status)
        process.wait.side_effect = ([subprocess.TimeoutExpired(["fixture"], 7), status]
                                    if timeout else [status])

        def release_pipe(group, sig):
            self.assertEqual(group, process.pid)
            if sig == close_on and held:
                os.close(held.pop())

        try:
            with mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
                 mock.patch.object(self.dev.os, "killpg", side_effect=release_pipe) as kill, \
                 mock.patch.object(self.dev._QualificationOutputLog, "DRAIN_SECONDS", 0.01), \
                 mock.patch.object(self.dev._QualificationOutputLog, "TERM_GRACE_SECONDS", 0.01), \
                 mock.patch.object(self.dev._QualificationOutputLog, "KILL_GRACE_SECONDS", 0.01):
                result = self.run_step("cargo test", ["fixture"], timeout=7)
            self.assertTrue(stream.closed, "bounded reader must finish and close its owned pipe")
            raw = (self.directory / "cargo-tests.full.log").read_bytes()
            self.assertIn(b"reader_complete=True", raw)
            self.assertIn(b"payload INCOMPLETE", raw)
            self.assertIn(f"process_returncode={status}".encode(), raw)
            return result, kill.call_args_list, raw
        finally:
            for descriptor in held:
                os.close(descriptor)
            stream.close()

    def test_mock_group_term_closes_real_pipe_after_root_exit(self):
        result, calls, raw = self.pipe_result(self.dev.signal.SIGTERM)
        self.assertEqual(result, 1)
        self.assertEqual(calls, [mock.call(12345, self.dev.signal.SIGTERM)])
        self.assertTrue(raw.startswith(b"early assertion from a mock-owned pipe\n"))

    def test_mock_group_term_ignored_until_kill_closes_real_pipe(self):
        result, calls, _ = self.pipe_result(self.dev.signal.SIGKILL)
        self.assertEqual(result, 1)
        self.assertEqual(calls, [mock.call(12345, self.dev.signal.SIGTERM),
                                 mock.call(12345, self.dev.signal.SIGKILL)])

    def test_mock_timeout_status_survives_real_pipe_cleanup(self):
        result, calls, _ = self.pipe_result(self.dev.signal.SIGKILL, timeout=True)
        self.assertEqual(result, -15)
        self.assertEqual(calls, [mock.call(12345, self.dev.signal.SIGTERM),
                                 mock.call(12345, self.dev.signal.SIGKILL)])

    def test_pipe_still_open_after_group_cleanup_cancels_reader_boundedly(self):
        result, calls, _ = self.pipe_result(None)
        self.assertEqual(result, 1)
        self.assertEqual(calls, [mock.call(12345, self.dev.signal.SIGTERM),
                                 mock.call(12345, self.dev.signal.SIGKILL)])

    def test_actual_session_descendant_stdout_cleanup(self):
        if not self.real_child_harness:
            completed = subprocess.run(
                [sys.executable, "-m", "unittest", f"{__name__}.QualificationOutputTests.test_actual_session_descendant_stdout_cleanup", "-v"],
                cwd=ROOT, env={**os.environ, "PYTHONPATH": str(ROOT / "tests/devpy"),
                               "PHOENIX_CAPTURE_REAL_CHILD_TEST": "1"},
                capture_output=True, text=True, timeout=12, check=False,
            )
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            print(completed.stdout + completed.stderr, end="")
            return
        self.assert_actual_session_cleanup()

    def assert_actual_session_cleanup(self):
        pidfile = self.directory / "descendant.pid"
        stopped = self.directory / "descendant.terminated"
        descendant = (
            "import os,signal,sys; from pathlib import Path; "
            "signal.alarm(4); "
            "signal.signal(signal.SIGTERM, lambda *_: (Path(sys.argv[2]).write_text('TERM'), os._exit(0))); "
            "os.write(int(sys.argv[1]), b'R'); os.close(int(sys.argv[1])); signal.pause()"
        )
        parent = (
            "import os,signal,subprocess,sys; from pathlib import Path; signal.alarm(3); "
            "r,w=os.pipe(); child=subprocess.Popen([sys.executable,'-c',sys.argv[1],str(w),sys.argv[3]],pass_fds=(w,)); "
            "assert os.getpgid(child.pid)==os.getpid(); "
            "Path(sys.argv[2]).write_text(str(child.pid)); os.close(w); "
            "assert os.read(r,1)==b'R'; os.close(r); print('descendant ready',flush=True)"
        )
        original_popen = subprocess.Popen
        created = []
        child_pid = None
        child_status = None
        libc = None
        previous_subreaper = None
        previous_handler = signal.getsignal(signal.SIGALRM)
        previous_timer = signal.getitimer(signal.ITIMER_REAL)
        started = time.monotonic()

        def deadline(*_):
            raise TimeoutError("real qualification descendant test exceeded 8 seconds")

        def launch(*args, **kwargs):
            process = original_popen(*args, **kwargs)
            created.append(process)
            return process

        def reap_descendant():
            nonlocal child_status
            if child_pid is None or child_status is not None:
                return
            until = time.monotonic() + 1
            while time.monotonic() < until:
                if sys.platform == "linux":
                    waited, status = os.waitpid(child_pid, os.WNOHANG)
                    if waited == child_pid:
                        child_status = status
                        return
                else:
                    try:
                        os.kill(child_pid, 0)
                    except ProcessLookupError:
                        child_status = "absent after init reaping"
                        return
                threading.Event().wait(0.01)
            self.fail("known owned descendant did not terminate/reap within one second")

        try:
            if sys.platform == "linux":
                import ctypes
                libc = ctypes.CDLL(None, use_errno=True)
                prior = ctypes.c_int()
                self.assertEqual(libc.prctl(37, ctypes.byref(prior), 0, 0, 0), 0)
                previous_subreaper = prior.value
                self.assertEqual(libc.prctl(36, 1, 0, 0, 0), 0)
            signal.signal(signal.SIGALRM, deadline)
            signal.setitimer(signal.ITIMER_REAL, 8)
            with mock.patch.object(self.dev.subprocess, "Popen", side_effect=launch), \
                 mock.patch.object(self.dev._QualificationOutputLog, "DRAIN_SECONDS", 0.05), \
                 mock.patch.object(self.dev._QualificationOutputLog, "TERM_GRACE_SECONDS", 1), \
                 mock.patch.object(self.dev._QualificationOutputLog, "KILL_GRACE_SECONDS", 1):
                result = self.run_step("cargo test", [sys.executable, "-c", parent, descendant,
                                                     str(pidfile), str(stopped)], timeout=2)
            self.assertEqual(len(created), 1)
            root = created[0]
            child_pid = int(pidfile.read_text())
            self.assertEqual(root.returncode, 0)
            self.assertTrue(root.stdout.closed)
            self.assertEqual(result, 1)
            self.assertEqual(stopped.read_text(), "TERM")
            raw = (self.directory / "cargo-tests.full.log").read_text()
            self.assertIn("descendant ready", raw)
            self.assertIn("process_returncode=0 timed_out=False reader_complete=True", raw)
            self.assertIn("payload INCOMPLETE", raw)
            reap_descendant()
            if sys.platform == "linux":
                self.assertTrue(os.WIFEXITED(child_status))
                self.assertEqual(os.WEXITSTATUS(child_status), 0)
            print(f"ACTUAL_CHILD_PROOF platform={sys.platform} root={root.pid} root_status=0 "
                  f"descendant={child_pid} terminal={child_status} reader_closed=True qualification=1")
        finally:
            try:
                for root in created:
                    for sig in (signal.SIGTERM, signal.SIGKILL):
                        try:
                            os.killpg(root.pid, sig)
                        except ProcessLookupError:
                            pass
                    root.wait(timeout=1)
                    if root.stdout is not None:
                        root.stdout.close()
                if child_pid is None and pidfile.exists():
                    child_pid = int(pidfile.read_text())
                reap_descendant()
            finally:
                try:
                    if libc is not None and previous_subreaper is not None:
                        self.assertEqual(libc.prctl(36, previous_subreaper, 0, 0, 0), 0)
                finally:
                    signal.setitimer(signal.ITIMER_REAL, 0)
                    signal.signal(signal.SIGALRM, previous_handler)
                    remaining = max(0.001, previous_timer[0] - (time.monotonic() - started)) if previous_timer[0] else 0
                    signal.setitimer(signal.ITIMER_REAL, remaining, previous_timer[1])

    def test_failure_payload_survives_console_tail_and_original_exit_is_preserved(self):
        diagnostic = "thread fixture panicked: assertion left=1 right=2\n" + "later output\n" * 400
        result, _ = self.execute(diagnostic, 42)
        self.assertEqual(result, 42)
        raw = (self.directory / "cargo-tests.full.log").read_text()
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
        raw = (self.directory / "cargo-tests.full.log").read_text()
        self.assertTrue(raw.startswith(diagnostic))
        self.assertIn("process_returncode=-15 timed_out=True reader_complete=True", raw)
        self.assertIn("TIMEOUT after 7s", self.results[0][3])
        self.assertIn("earlier lines dropped", self.results[0][3])

    def test_overflow_is_bounded_and_fails_a_successful_process(self):
        with mock.patch.object(self.dev._QualificationOutputLog, "MAX_BYTES", 2048):
            result, _ = self.execute("diagnostic\n" * 500)
        self.assertEqual(result, 1)
        raw = (self.directory / "cargo-tests.full.log").read_bytes()
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
        self.namespace["goal_reuse_commands"] = None
        result, _ = self.execute("not a qualification\n", 23)
        self.assertEqual(result, 23)
        self.assertFalse((self.directory / "cargo-tests.full.log").exists())

    def test_reader_incomplete_marks_artifact_and_existing_artifact_is_not_overwritten(self):
        path = self.directory / "cargo-tests.full.log"
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
                self.run_step("cargo test", ["fixture"])
        raw = (self.directory / "cargo-tests.full.log").read_text()
        self.assertIn("process_returncode=None", raw)
        self.assertIn("payload INCOMPLETE", raw)
        self.assertEqual(self.results, [])

    def test_pipe_termination_escalation_retains_original_cleanup_and_status(self):
        process = mock.Mock(pid=12345, stdout=io.BytesIO(b"early assertion\n"), returncode=-9)
        process.wait.side_effect = [subprocess.TimeoutExpired(["fixture"], 7),
                                    subprocess.TimeoutExpired(["fixture"], 3), -9]
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            result = self.run_step("cargo test", ["fixture"], timeout=7)
        self.assertEqual(result, -9)
        self.assertEqual(kill.call_args_list, [mock.call(12345, self.dev.signal.SIGTERM),
                                               mock.call(12345, self.dev.signal.SIGKILL)])
        self.assertEqual(process.wait.call_args_list,
                         [mock.call(timeout=7), mock.call(timeout=3), mock.call(timeout=5)])
        self.assertIn("process_returncode=-9 timed_out=True", (self.directory / "cargo-tests.full.log").read_text())
