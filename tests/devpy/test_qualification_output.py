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
    with mock.patch.dict(sys.modules, {spec.name: module}):
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
                         worker_budget=1, profile_work=False, focused_only=False, goal_reuse_commands=None,
                         reporter=self.reporter, results=self.results, results_lock=threading.Lock(),
                         _begin_dev_span=mock.Mock(return_value=None),
                         _finish_check_step_span=mock.Mock(),
                         _check_step_artifact_dir=mock.Mock(return_value=None))
        exec(compile(ast.Module(body=[step], type_ignores=[]), str(ROOT / "dev.py"), "exec"), namespace)
        self.namespace = namespace
        self.run_step = namespace["run_step"]

    def execute(self, diagnostic, returncode=0, *, timeout=False):
        stream = (io.BytesIO(diagnostic.encode()) if bool(os.environ.get("PHOENIX_CHECK_DIAGNOSTIC_DIR"))
                  else io.StringIO(diagnostic))
        process = mock.Mock(pid=12345, stdout=stream, returncode=returncode)
        process.wait.side_effect = ([subprocess.TimeoutExpired(["fixture"], 7), returncode]
                                    if timeout else [returncode])
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process) as launch, \
             mock.patch.object(self.dev.os, "killpg") as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            result = self.run_step("cargo clippy", ["fixture"], timeout=7)
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

    def test_goal_broad_log_route_does_not_replace_clippy_route(self):
        self.namespace["goal_reuse_commands"] = ([], [], [])
        os.environ["PHOENIX_GOAL_REUSE_DIR"] = str(self.directory)
        process = mock.Mock(pid=12345, stdout=io.BytesIO(b"goal-broad-output\n"), returncode=0)
        process.wait.return_value = 0
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=process), \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            self.assertEqual(self.run_step("cargo test", ["fixture"]), 0)
        self.assertIn(b"goal-broad-output", (self.directory / "cargo-tests.full.log").read_bytes())
        self.assertFalse((self.directory / "cargo-clippy.full.log").exists())

    def test_focused_timeout_uses_one_cleanup_cap_and_reaps_before_return(self):
        import types
        clock = [100.0]
        root = mock.Mock(pid=12345, returncode=None, stdout=None)
        def wait(*, timeout):
            self.assertIsNone(root.returncode)
            self.assertLessEqual(timeout, 2)
            clock[0] += 1.5
            root.returncode = -signal.SIGTERM
            return root.returncode
        root.wait.side_effect = wait
        profile = types.SimpleNamespace(metadata={"goal_step_deadline_ns": 600_000_000_000},
            goal_owned_process=root, goal_owned_reader=None, goal_reader_stop=None,
            goal_owned_output=None, goal_owned_finisher=None, goal_capture_error="")
        calls = []
        def kill(pid, signum):
            calls.append((pid, signum, root.returncode))
            if signum == 0:
                raise ProcessLookupError()
            self.assertIsNone(root.returncode, "TERM/KILL requires actual unreaped custody")
        with mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: clock[0]), \
             mock.patch.object(self.dev.os, "killpg", side_effect=kill):
            self.assertEqual(self.dev._goal_begin_cleanup(profile), 110)
            self.dev._goal_reap_before_finalize(profile, terminate=True, timed_out=True)
            clock[0] = 108
            self.assertEqual(self.dev._goal_begin_cleanup(profile), 110)
        self.assertEqual(calls, [(root.pid, signal.SIGTERM, None), (root.pid, 0, -signal.SIGTERM)])
        self.assertIsNone(profile.goal_owned_process)


    def test_reaped_root_is_not_signalled_again_from_a_remembered_pgid(self):
        import types
        proc = mock.Mock(pid=12345, returncode=0, stdout=None)
        profile = types.SimpleNamespace(metadata={"goal_step_deadline_ns": time.monotonic_ns() + 600_000_000_000},
            goal_owned_process=proc, goal_owned_reader=None, goal_reader_stop=None,
            goal_owned_output=None, goal_owned_finisher=None, goal_capture_error="")
        with mock.patch.object(self.dev.os, "killpg", side_effect=ProcessLookupError()) as kill:
            self.dev._goal_reap_before_finalize(profile, terminate=True)
        kill.assert_called_once_with(proc.pid, 0)  # Observation only, never TERM/KILL.
        proc.wait.assert_not_called()
        self.assertIsNone(profile.goal_owned_process)


    def test_wait_sets_reaped_status_then_interrupts_preserves_error_without_signal(self):
        import types
        profile = types.SimpleNamespace(metadata={"goal_step_deadline_ns": time.monotonic_ns() + 600_000_000_000},
            goal_owned_process=None, goal_owned_reader=None, goal_reader_stop=None,
            goal_owned_output=None, goal_owned_finisher=None, goal_capture_error="")
        self.namespace.update(focused_only=True, _CHECK_PROFILE=profile,
                              goal_step_deadline_ns=profile.metadata["goal_step_deadline_ns"])
        proc = mock.Mock(pid=12345, returncode=None, stdout=io.BytesIO(b"before interrupt\n"))
        original = InterruptedError("deterministic post-reap handler gap")
        def wait(*, timeout):
            proc.returncode = 0
            raise original
        proc.wait.side_effect = wait
        with mock.patch.object(self.dev.subprocess, "Popen", return_value=proc), \
             mock.patch.object(self.dev.os, "killpg", side_effect=ProcessLookupError()) as kill, \
             mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
            with self.assertRaises(InterruptedError) as caught:
                self.run_step("cargo clippy", ["fixture"], timeout=7)
        self.assertIs(caught.exception, original)
        self.assertEqual(kill.call_args_list, [mock.call(proc.pid, 0)])
        self.assertEqual(proc.wait.call_count, 1)
        self.assertIsNone(profile.goal_owned_process)
        self.assertEqual(self.results, [])

    def test_one_fake_clock_budget_covers_timeout_reap_drain_and_repeat_finalizer(self):
        import types
        clock = [200.0]
        proc = mock.Mock(pid=12345, returncode=None, stdout=None)
        def wait(*, timeout):
            self.assertLessEqual(timeout, 2)
            clock[0] += timeout
            proc.returncode = -15
        proc.wait.side_effect = wait
        joined = []
        reader = mock.Mock(ident=1)
        reader.is_alive.return_value = True
        def join(*, timeout):
            joined.append(timeout)
            self.assertLessEqual(timeout, max(0, 208 - clock[0]))
            clock[0] += timeout
        reader.join.side_effect = join
        log = mock.Mock(DRAIN_SECONDS=5, error="")
        profile = types.SimpleNamespace(artifacts_finalized=False,
            metadata={"goal_step_deadline_ns": 208_000_000_000}, goal_owned_process=proc,
            goal_owned_reader=reader, goal_reader_stop=threading.Event(), goal_owned_output=log,
            goal_owned_finisher=None, goal_capture_error="")
        def kill(pid, signum):
            if signum == 0:
                raise ProcessLookupError()
            self.assertIsNone(proc.returncode)
        with mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: clock[0]), \
             mock.patch.object(self.dev.os, "killpg", side_effect=kill), \
             mock.patch.object(self.dev, "_CHECK_PROFILE", profile), \
             mock.patch.object(self.dev.subprocess, "run") as report:
            self.dev._goal_begin_cleanup(profile)
            with self.assertRaisesRegex(RuntimeError, "reader remains alive"):
                self.dev._goal_reap_before_finalize(profile, terminate=True, timed_out=True)
            self.assertEqual(profile.metadata["goal_cleanup_deadline"], 208)
            self.assertEqual(clock[0], 208)
            calls = list(joined)
            for _ in range(2):
                with self.assertRaisesRegex(RuntimeError, "incomplete focused closure"):
                    self.dev._finalize_check_profile(None)
            self.assertEqual(joined, calls)
        self.assertEqual(joined, [5, 1])
        log.settle_reader.assert_not_called()
        log.finish.assert_not_called()
        report.assert_not_called()
        self.assertFalse(profile.artifacts_finalized)

    def test_real_owned_reader_blocked_append_never_enters_finish_lock(self):
        import types
        for mode in ("normal", "timeout", "interrupt"):
            with self.subTest(mode=mode):
                log = self.dev._QualificationOutputLog(self.directory / (mode + ".log"))
                entered, release = threading.Event(), threading.Event()
                actual_output = log.output
                class HeldWrite:
                    closed = False
                    def write(inner, data):
                        entered.set()
                        if not release.wait(timeout=2):
                            raise RuntimeError("test failed to release held writer")
                        return actual_output.write(data)
                    def close(inner):
                        inner.closed = True
                        actual_output.close()
                log.output = HeldWrite()
                reader = threading.Thread(target=lambda: log.append(b"owned append\n"), daemon=True)
                reader.start()
                self.assertTrue(entered.wait(timeout=2))
                clock = [300.0]
                real_join = reader.join
                joins = []
                def virtual_join(*, timeout):
                    self.assertLessEqual(timeout, max(0, 310 - clock[0]))
                    joins.append(timeout)
                    clock[0] += timeout
                    real_join(timeout=0)
                root = mock.Mock(pid=12345, returncode=0, stdout=None)
                profile = types.SimpleNamespace(artifacts_finalized=False,
                    metadata={"goal_step_deadline_ns": 900_000_000_000}, goal_owned_process=root,
                    goal_owned_reader=reader, goal_reader_stop=threading.Event(), goal_owned_output=log,
                    goal_owned_finisher=None, goal_capture_error="")
                try:
                    with mock.patch.object(reader, "join", side_effect=virtual_join), \
                         mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: clock[0]), \
                         mock.patch.object(self.dev.os, "killpg", side_effect=ProcessLookupError()) as signal_group, \
                         mock.patch.object(log, "finish", wraps=log.finish) as finish, \
                         mock.patch.object(self.dev, "_CHECK_PROFILE", profile), \
                         mock.patch.object(self.dev.subprocess, "run") as report:
                        self.assertEqual(self.dev._goal_begin_cleanup(profile), 310)
                        with self.assertRaisesRegex(RuntimeError, "reader remains alive"):
                            self.dev._goal_reap_before_finalize(profile,
                                terminate=mode != "normal", timed_out=mode == "timeout")
                        self.assertEqual(clock[0], 310)
                        self.assertEqual(joins, [5, 5])
                        finish.assert_not_called()
                        for _ in range(2):
                            with self.assertRaisesRegex(RuntimeError, "incomplete focused closure"):
                                self.dev._finalize_check_profile(None)
                        self.assertFalse(profile.artifacts_finalized)
                        report.assert_not_called()
                        signal_group.assert_called_once_with(root.pid, 0)
                finally:
                    release.set()
                    real_join(timeout=2)
                    self.assertFalse(reader.is_alive())
                    if not actual_output.closed:
                        actual_output.close()

    def test_incomplete_finalizer_preserves_existing_interrupt_and_never_reports(self):
        import types
        original = InterruptedError("original interrupted wait")
        profile = types.SimpleNamespace(artifacts_finalized=False,
                                        metadata={"goal_cleanup_failed": "reader held"})
        with mock.patch.object(self.dev, "_CHECK_PROFILE", profile), \
             mock.patch.object(self.dev.subprocess, "run") as report:
            self.assertIsNone(self.dev._finalize_check_profile(original))
            self.assertIn("incomplete focused closure", original.__notes__[0])
            with self.assertRaisesRegex(RuntimeError, "incomplete focused closure"):
                self.dev._finalize_check_profile(None)
        self.assertFalse(profile.artifacts_finalized)
        report.assert_not_called()

    def test_finish_blocked_write_is_owned_bounded_and_never_finalized(self):
        import types
        log = self.dev._QualificationOutputLog(self.directory / "blocked-footer.log")
        entered, release = threading.Event(), threading.Event()
        actual_output = log.output
        class HeldFooter:
            closed = False
            def write(inner, data):
                entered.set()
                if not release.wait(timeout=2):
                    raise RuntimeError("test did not release footer")
                return actual_output.write(data)
            def close(inner):
                inner.closed = True
                actual_output.close()
        log.output = HeldFooter()
        root = mock.Mock(pid=12345, returncode=0, stdout=None)
        reader = mock.Mock(ident=1)
        reader.is_alive.return_value = False
        clock = [350.0]
        profile = types.SimpleNamespace(artifacts_finalized=False,
            metadata={"goal_step_deadline_ns": 900_000_000_000}, goal_owned_process=root,
            goal_owned_reader=reader, goal_reader_stop=threading.Event(), goal_owned_output=log,
            goal_owned_finisher=None, goal_capture_error="")
        actual_join = threading.Thread.join
        def join_finisher(thread, *, timeout):
            self.assertTrue(entered.wait(timeout=2))
            self.assertLessEqual(timeout, 360 - clock[0])
            clock[0] += timeout
            actual_join(thread, timeout=0)
        try:
            with mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: clock[0]), \
                 mock.patch.object(self.dev.os, "killpg", side_effect=ProcessLookupError()), \
                 mock.patch.object(threading.Thread, "join", autospec=True, side_effect=join_finisher), \
                 mock.patch.object(self.dev, "_CHECK_PROFILE", profile), \
                 mock.patch.object(self.dev.subprocess, "run") as report:
                with self.assertRaisesRegex(RuntimeError, "capture finish remains alive"):
                    self.dev._goal_reap_before_finalize(profile)
                self.assertEqual(clock[0], 360)
                for _ in range(2):
                    with self.assertRaisesRegex(RuntimeError, "incomplete focused closure"):
                        self.dev._finalize_check_profile(None)
                self.assertFalse(profile.artifacts_finalized)
                report.assert_not_called()
        finally:
            release.set()
            if profile.goal_owned_finisher is not None:
                actual_join(profile.goal_owned_finisher, timeout=2)
                self.assertFalse(profile.goal_owned_finisher.is_alive())
            if not actual_output.closed:
                actual_output.close()

    def test_real_owned_pipe_normal_root_deadline_precedes_reader_drain(self):
        import types
        read_fd, write_fd = os.pipe()
        stream = os.fdopen(read_fd, "rb", buffering=0)
        log = self.dev._QualificationOutputLog(self.directory / "focused-held-pipe.log")
        observed, stopped = threading.Event(), threading.Event()
        os.write(write_fd, b"ready from owned pipe\n")
        def read():
            try:
                log.read(stream, stopped, lambda _: observed.set())
            finally:
                stream.close()
        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        self.assertTrue(observed.wait(timeout=2))
        clock = [400.0]
        joins = []
        real_join = reader.join
        def join(*, timeout):
            self.assertLessEqual(timeout, max(0, 410 - clock[0]))
            joins.append(timeout)
            if len(joins) == 1:
                clock[0] += timeout
            else:
                # Release the test-owned writer at the stop barrier; no sleep/poll surrogate.
                os.close(write_fd)
                real_join(timeout=min(2, timeout))
        root = mock.Mock(pid=12345, returncode=0, stdout=stream)
        profile = types.SimpleNamespace(metadata={"goal_step_deadline_ns": 900_000_000_000},
            goal_owned_process=root, goal_owned_reader=reader, goal_reader_stop=stopped,
            goal_owned_output=log, goal_owned_finisher=None, goal_capture_error="")
        try:
            with mock.patch.object(reader, "join", side_effect=join), \
                 mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: clock[0]), \
                 mock.patch.object(self.dev.os, "killpg", side_effect=ProcessLookupError()):
                self.dev._goal_begin_cleanup(profile)
                error = self.dev._goal_reap_before_finalize(profile)
            self.assertEqual(profile.metadata["goal_cleanup_deadline"], 410)
            self.assertEqual(joins, [5, 5])
            self.assertIn("writer ownership unresolved", error)
            self.assertTrue(stream.closed)
            self.assertIsNone(profile.goal_owned_process)
            self.assertIn(b"payload INCOMPLETE", log.path.read_bytes())
        finally:
            stopped.set()
            try:
                os.close(write_fd)
            except OSError:
                pass
            real_join(timeout=2)
            if not log.output.closed:
                log.output.close()

    def test_unreaped_focused_command_cannot_finalize_profile(self):
        import types
        profile = types.SimpleNamespace(artifacts_finalized=False, metadata={"goal_unreaped": True})
        with mock.patch.object(self.dev, "_CHECK_PROFILE", profile), \
             mock.patch.object(self.dev.subprocess, "run") as report:
            with self.assertRaisesRegex(RuntimeError, "before focused command reap"):
                self.dev._finalize_check_profile(None)
        report.assert_not_called()
        self.assertFalse(profile.artifacts_finalized)

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

    def test_python_diagnostics_capture_full_headers_and_preserve_failure_and_cap(self):
        diagnostic_dir = self.directory / "python-diagnostics"
        payload = b"ERROR: first_python_identity\n" + b"retained output\n" * 400
        for cap, footer in ((None, b"payload COMPLETE"), (2048, b"payload INCOMPLETE")):
            with self.subTest(cap=cap):
                process = mock.Mock(pid=12345, stdout=io.BytesIO(payload), returncode=1)
                process.wait.return_value = 1
                limit = mock.patch.object(self.dev._QualificationOutputLog, "MAX_BYTES", cap or 64 * 1024 * 1024)
                with mock.patch.dict(os.environ, {"PHOENIX_CHECK_DIAGNOSTIC_DIR": str(diagnostic_dir)}, clear=True), \
                     limit, mock.patch.object(self.dev.subprocess, "Popen", return_value=process) as launch, \
                     mock.patch.object(self.dev.os, "killpg") as kill, \
                     mock.patch.object(self.dev.select, "select", side_effect=lambda streams, *_: (streams, [], [])):
                    command = [sys.executable, "-m", "unittest", "discover", "tests/devpy"]
                    self.assertEqual(self.run_step("dev.py unit tests", command), 1)
                self.assertEqual(launch.call_args.args[0], command)
                self.assertFalse(launch.call_args.kwargs["text"])
                kill.assert_not_called()
                raw = (diagnostic_dir / "python-unittest.full.log").read_bytes()
                self.assertTrue(raw.startswith(b"ERROR: first_python_identity\n"))
                self.assertIn(b"process_returncode=1 timed_out=False reader_complete=True", raw)
                self.assertIn(footer, raw)
                if cap:
                    self.assertLessEqual(len(raw), cap)
                else:
                    self.assertTrue(raw.startswith(payload))

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
                         self.directory / "python-unittest.full.log")
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
