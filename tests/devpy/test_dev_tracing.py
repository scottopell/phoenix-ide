import importlib.util
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_devpy():
    spec = importlib.util.spec_from_file_location("devpy_tracing_under_test", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class FakeSpan:
    pass


class FakeTracing:
    def __init__(self):
        self.started = []
        self.finished = []

    def start_span(self, name, attributes=None, parent=None, start_time=None):
        span = FakeSpan()
        self.started.append((name, attributes, span, parent, start_time))
        return span

    def finish_span(self, span, attributes, failed=False, end_time=None):
        self.finished.append((span, attributes, failed, end_time))


class FakeProcess:
    def __init__(self, lines, returncode=0):
        self.stderr = io.StringIO("".join(lines))
        self.returncode = returncode
        self.terminated = False
        self.killed = False

    def poll(self):
        return self.returncode

    def terminate(self):
        self.terminated = True
        self.returncode = -15

    def kill(self):
        self.killed = True
        self.returncode = -9

    def wait(self, timeout=None):
        return self.returncode


class DevTracingTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()
        self.dev._DEV_TRACE_AVAILABLE = None

    def test_trace_endpoint_defaults_locally_and_is_off_in_ci(self):
        self.assertEqual(
            self.dev._DEFAULT_DEV_TRACE_ENDPOINT,
            self.dev._dev_trace_endpoint({}),
        )
        self.assertIsNone(self.dev._dev_trace_endpoint({"CI": "1"}))
        self.assertEqual(
            "http://collector.test/v1/traces",
            self.dev._dev_trace_endpoint({
                "CI": "1",
                "PHOENIX_DEV_TRACE_ENDPOINT": "http://collector.test/v1/traces",
            }),
        )
        for value in ("", "off", "none", "0"):
            with self.subTest(value=value):
                self.assertIsNone(self.dev._dev_trace_endpoint({
                    "PHOENIX_DEV_TRACE_ENDPOINT": value,
                }))

    def test_disabled_tracing_never_attempts_dependency_bootstrap(self):
        with (
            mock.patch.object(self.dev, "_dev_tracing_importable") as importable,
            mock.patch.object(self.dev.subprocess, "run") as run,
        ):
            self.dev._bootstrap_dev_tracing({"PHOENIX_DEV_TRACE_ENDPOINT": "off"})

        importable.assert_not_called()
        run.assert_not_called()
        self.assertFalse(self.dev._DEV_TRACE_AVAILABLE)

    def test_missing_offline_tracing_deps_continue_without_exec(self):
        with (
            mock.patch.object(self.dev, "_dev_tracing_importable", return_value=False),
            mock.patch.object(
                self.dev.subprocess,
                "run",
                return_value=mock.Mock(returncode=1),
            ) as run,
            mock.patch.object(self.dev.os, "execvpe") as execvpe,
        ):
            self.dev._bootstrap_dev_tracing({})

        self.assertIn("--offline", run.call_args.args[0])
        execvpe.assert_not_called()
        self.assertFalse(self.dev._DEV_TRACE_AVAILABLE)

    def test_cached_tracing_deps_reexec_offline(self):
        with (
            mock.patch.object(self.dev, "_dev_tracing_importable", return_value=False),
            mock.patch.object(
                self.dev.subprocess,
                "run",
                return_value=mock.Mock(returncode=0),
            ),
            mock.patch.object(self.dev.os, "execvpe") as execvpe,
        ):
            self.dev._bootstrap_dev_tracing({})

        command = execvpe.call_args.args[1]
        environment = execvpe.call_args.args[2]
        self.assertIn("--offline", command)
        self.assertEqual("1", environment["_PHOENIX_DEV_TRACE_BOOTSTRAP"])

    def test_nested_span_scope_supplies_parent(self):
        tracing = FakeTracing()
        self.dev._DEV_TRACING = tracing
        parent = FakeSpan()

        with self.dev._DevSpanScope(parent):
            child = self.dev._begin_dev_span("child")

        self.assertIsInstance(child, FakeSpan)
        self.assertIs(parent, tracing.started[0][3])

    def test_profile_command_cpu_finalization_is_idempotent(self):
        profile = self.dev.CheckWorkProfile.start()
        first = profile.finalize_cpu()
        for _ in range(10000):
            pass
        second = profile.finalize_cpu()

        self.assertIs(first, second)
        self.assertEqual(first["cpu.total_ms"], second["cpu.total_ms"])
        self.assertIsNotNone(profile.finalized_wall_ns)
        self.assertIsNotNone(profile.finalized_monotonic_ns)

    def test_exceptional_profile_cleanup_writes_failed_command_and_report(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = self.dev.CheckWorkProfile.start(Path(directory))
            profile.metadata["active_lanes"] = ["tsc"]
            self.dev._CHECK_PROFILE = profile
            report = mock.Mock(returncode=0, stderr="")
            with (
                mock.patch.object(self.dev.subprocess, "run", return_value=report) as run,
                mock.patch("builtins.print"),
            ):
                error = subprocess.CalledProcessError(7, ["pnpm", "install"])
                self.dev._finalize_check_profile(error)
                self.dev._finalize_check_profile(error)

            command = json.loads((Path(directory) / "command.json").read_text())

        self.assertEqual("failed", command["status"])
        self.assertEqual(7, command["returncode"])
        self.assertEqual(["tsc"], command["metadata"]["active_lanes"])
        self.assertEqual(1, run.call_count)

    def test_profile_returncode_matches_process_exit_semantics(self):
        cases = (
            (None, 0),
            (SystemExit(), 0),
            (SystemExit(4), 4),
            (SystemExit("invalid arguments"), 1),
            (subprocess.CalledProcessError(9, ["tool"]), 9),
            (RuntimeError("broken"), 1),
        )
        for error, expected in cases:
            with self.subTest(error=error):
                self.assertEqual(expected, self.dev._profile_returncode(error))

    def test_profile_cleanup_uses_shared_trace_and_artifact_boundary(self):
        class CommandTracing(FakeTracing):
            def __init__(self):
                super().__init__()
                self.command_span = FakeSpan()

        with tempfile.TemporaryDirectory() as directory:
            profile = self.dev.CheckWorkProfile.start(Path(directory))
            tracing = CommandTracing()
            self.dev._CHECK_PROFILE = profile
            self.dev._DEV_TRACING = tracing
            with (
                mock.patch.object(
                    self.dev.subprocess, "run", return_value=mock.Mock(returncode=0, stderr="")
                ),
                mock.patch("builtins.print"),
            ):
                self.dev._finalize_check_profile(None)
            command = json.loads((Path(directory) / "command.json").read_text())

        self.assertEqual(profile.finalized_wall_ns, tracing.finished[0][3])
        self.assertEqual(command["wall_ms"] / 1000.0, tracing.finished[0][1]["dev.elapsed_seconds"])
        self.assertFalse(tracing.finished[0][2])

    def test_profile_start_captures_wall_and_monotonic_boundaries(self):
        profile = self.dev.CheckWorkProfile.start()
        self.assertGreater(profile.started_wall_ns, 0)
        self.assertGreater(profile.started_monotonic_ns, 0)

    def test_profile_rejects_nonempty_explicit_artifact_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "stale.jsonl").write_text("stale\n")
            with self.assertRaisesRegex(ValueError, "must be empty"):
                self.dev.CheckWorkProfile.start(root)

    def test_profile_accepts_empty_explicit_artifact_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "new-profile"
            profile = self.dev.CheckWorkProfile.start(root)

        self.assertEqual(root.resolve(), profile.artifact_dir)

    def test_profile_atomically_claims_explicit_artifact_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "claimed-profile"
            first = self.dev.CheckWorkProfile.start(root)

            with self.assertRaisesRegex(ValueError, "already claimed"):
                self.dev.CheckWorkProfile.start(root)

        self.assertEqual(root.resolve(), first.artifact_dir)

    def test_external_profile_paths_remain_absolute_labels(self):
        external = Path(tempfile.gettempdir()) / "outside-phoenix" / "record.json"
        self.assertEqual(str(external), self.dev._display_path(external))

    def test_external_cpu_measurement_is_read_without_repo_relative_assumption(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "measurement.json"
            path.write_text(json.dumps({
                "user_cpu_ms": 1, "system_cpu_ms": 2,
                "total_cpu_ms": 7,
                "provenance": "exact_waited_descendants",
                "tree_closure": "waited_descendants_only_survivors_unverified",
                "reader_thread_cpu_ms": 4,
            }))
            attributes = self.dev._read_cpu_measurement(path)

        self.assertEqual(str(path), attributes["cpu.measurement_path"])
        self.assertEqual(7, attributes["cpu.total_ms"])
        self.assertEqual(4, attributes["cpu.reader_thread_ms"])

    def test_profile_record_attributes_normalize_runner_units(self):
        source = self.dev.ROOT / "target" / "record.jsonl"
        attributes = self.dev._profile_record_attributes({
            "provenance": "windowed_process",
            "test_name": "renders",
            "started_unix_ns": 1_000_000_000,
            "cpu_user_us": 1250,
            "cpu_system_us": 750,
            "wall_time_ms": 4.5,
            "status": "pass",
        }, source)

        self.assertEqual(2.0, attributes["cpu.total_ms"])
        self.assertEqual("renders", attributes["check.test.identity"])
        self.assertEqual("pass", attributes["check.test.status"])

    def test_imported_profile_span_preserves_recorded_interval(self):
        tracing = FakeTracing()
        self.dev._DEV_TRACING = tracing
        parent = FakeSpan()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tests.jsonl").write_text(json.dumps({
                "identity": "test:one", "provenance": "windowed_process",
                "started_unix_ns": 1_000_000_000, "wall_ms": 2.5,
                "user_cpu_ms": 1, "system_cpu_ms": 0, "status": "passed",
            }) + "\n")
            emitted = self.dev._emit_profile_record_spans(
                root, parent, ("tests.jsonl",)
            )

        self.assertEqual(1, emitted)
        self.assertEqual(1_000_000_000, tracing.started[0][4])
        self.assertEqual(1_002_500_000, tracing.finished[0][3])

    def test_vitest_trace_identity_is_file_qualified(self):
        source = self.dev.ROOT / "target" / "record.jsonl"
        attributes = self.dev._profile_record_attributes({
            "full_name": "src/a.test.ts > suite > works",
            "full_test_name": "suite > works", "file": "src/a.test.ts",
            "provenance": "windowed_process", "started_unix_ns": 1_000_000_000,
            "wall_ms": 2.0, "cpu_user_us": 1, "cpu_system_us": 0,
        }, source)

        self.assertEqual(
            "src/a.test.ts > suite > works", attributes["check.test.identity"]
        )

    def test_runner_metadata_is_threaded_into_profile_attributes(self):
        source = self.dev.ROOT / "target" / "record.jsonl"
        attributes = self.dev._profile_record_attributes({
            "identity": "rust:test", "provenance": "exact_waited_descendants",
            "started_unix_ns": 1_000_000_000, "wall_ms": 2.0,
            "user_cpu_ms": 1, "system_cpu_ms": 0,
            "attempt": "2", "pid": 42, "worker_id": "3",
            "test_id": "stable-id", "binary_id": "crate::bin", "concurrent": True,
        }, source)

        self.assertEqual("2", attributes["check.test.attempt"])
        self.assertEqual(42, attributes["process.pid"])
        self.assertEqual("3", attributes["check.test.worker_id"])
        self.assertEqual("stable-id", attributes["check.test.runner_id"])
        self.assertTrue(attributes["check.test.concurrent"])

    def test_truncated_jsonl_keeps_valid_records_and_emits_error_span(self):
        tracing = FakeTracing()
        self.dev._DEV_TRACING = tracing
        parent = FakeSpan()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            valid = {
                "identity": "test:one", "provenance": "windowed_process",
                "started_unix_ns": 1_000_000_000, "wall_ms": 2.5,
                "user_cpu_ms": 1, "system_cpu_ms": 0, "status": "passed",
            }
            (root / "tests.jsonl").write_text(json.dumps(valid) + "\n{truncated\n")
            emitted = self.dev._emit_profile_record_spans(
                root, parent, ("tests.jsonl",)
            )

        self.assertEqual(1, emitted)
        self.assertEqual(
            ["dev.check.profile_error", "dev.check.test"],
            [item[0] for item in tracing.started],
        )
        self.assertTrue(tracing.finished[0][2])

    def test_unavailable_profile_record_is_preserved_without_cpu_totals(self):
        source = self.dev.ROOT / "target" / "record.jsonl"
        attributes = self.dev._profile_record_attributes({
            "identity": "e2e:server", "provenance": "unavailable",
            "started_unix_ns": 1_000_000_000, "wall_ms": 2.0,
            "user_cpu_ms": None, "system_cpu_ms": None,
        }, source)

        self.assertEqual("unavailable", attributes["cpu.provenance"])
        self.assertNotIn("cpu.total_ms", attributes)

    def test_total_only_profile_record_does_not_fabricate_components(self):
        source = self.dev.ROOT / "target" / "record.jsonl"
        attributes = self.dev._profile_record_attributes({
            "identity": "e2e:server", "provenance": "windowed_process_total_only",
            "started_unix_ns": 1_000_000_000, "wall_ms": 2.0,
            "user_cpu_ms": None, "system_cpu_ms": None, "total_cpu_ms": 9.0,
        }, source)

        self.assertEqual(9.0, attributes["cpu.total_ms"])
        self.assertNotIn("cpu.user_ms", attributes)
        self.assertNotIn("cpu.system_ms", attributes)

    def test_nextest_profile_config_wraps_only_run_phase(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = self.dev.CheckWorkProfile(
                run_id="run", artifact_dir=Path(directory),
                started_self=mock.Mock(), started_children=mock.Mock(),
                started_thread_ns=0, started_wall_ns=0, started_monotonic_ns=0,
                initial_git_sha="abc", initial_git_dirty=False,
            )
            config = self.dev._write_nextest_profile_config(profile).read_text()

        self.assertIn('experimental = ["wrapper-scripts"]', config)
        self.assertIn('run-wrapper = "phoenix-cpu"', config)
        self.assertNotIn("list-wrapper", config)
        self.assertIn("check_profile_command.py", config)

    def test_cargo_lock_timer_accumulates_multiple_intervals(self):
        timer = self.dev.CargoLockWaitTimer(started_at=10.0)

        timer.observe_line("Blocking waiting for file lock on build directory", 11.0)
        timer.observe_line("Compiling one", 13.5)
        timer.observe_line("Blocking waiting for file lock on package cache", 14.0)
        timer.observe_line("Compiling two", 15.25)

        self.assertEqual(3.75, timer.finish(18.0))

    def test_cargo_lock_timer_closes_open_interval_and_bounds_wait(self):
        timer = self.dev.CargoLockWaitTimer(started_at=10.0)
        timer.observe_line("Blocking waiting for file lock on build directory", 11.0)

        self.assertEqual(2.0, timer.finish(13.0))
        self.assertEqual(2.0, timer.finish(14.0))

        skewed = self.dev.CargoLockWaitTimer(started_at=20.0)
        skewed.observe_line("Blocking waiting for file lock", 19.0)
        self.assertEqual(0.0, skewed.finish(19.5))

    def test_cargo_build_emits_timing_and_lock_wait_span(self):
        tracing = FakeTracing()
        process = FakeProcess([
            "Blocking waiting for file lock on build directory\n",
            "Compiling phoenix\n",
        ])
        clock = iter([10.0, 11.0, 13.0, 16.0])
        self.dev._DEV_TRACING = tracing

        with (
            mock.patch.object(self.dev.subprocess, "Popen", return_value=process),
            mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: next(clock)),
            mock.patch("builtins.print"),
        ):
            self.dev._run_cargo_build(["cargo", "build", "--release"], ROOT, "release")

        self.assertEqual("dev.build", tracing.started[0][0])
        self.assertEqual({"build.profile": "release"}, tracing.started[0][1])
        _, attributes, failed, _ = tracing.finished[0]
        self.assertFalse(failed)
        self.assertEqual(6.0, attributes["build.elapsed_seconds"])
        self.assertEqual(2.0, attributes["cargo.lock_wait_seconds"])
        self.assertEqual(0, attributes["process.exit_code"])

    def test_cargo_build_interrupt_terminates_and_waits_for_child(self):
        class InterruptingStream:
            def __iter__(self):
                raise KeyboardInterrupt

            def close(self):
                pass

        tracing = FakeTracing()
        process = FakeProcess([])
        process.stderr = InterruptingStream()
        process.returncode = None
        self.dev._DEV_TRACING = tracing

        with (
            mock.patch.object(self.dev.subprocess, "Popen", return_value=process),
            mock.patch.object(self.dev.time, "monotonic", side_effect=[10.0, 11.0]),
        ):
            with self.assertRaises(KeyboardInterrupt):
                self.dev._run_cargo_build(["cargo", "build"], ROOT, "debug")

        self.assertTrue(process.terminated)
        self.assertEqual(-15, process.returncode)
        self.assertEqual(-15, tracing.finished[0][1]["process.exit_code"])

    def test_cargo_build_failure_preserves_exit_and_marks_span_failed(self):
        tracing = FakeTracing()
        self.dev._DEV_TRACING = tracing
        process = FakeProcess(["compile error\n"], returncode=7)

        with (
            mock.patch.object(self.dev.subprocess, "Popen", return_value=process),
            mock.patch.object(self.dev.time, "monotonic", side_effect=[10.0, 11.0, 12.0]),
            mock.patch("builtins.print"),
        ):
            with self.assertRaises(subprocess.CalledProcessError) as raised:
                self.dev._run_cargo_build(["cargo", "build"], ROOT, "debug")

        self.assertEqual(7, raised.exception.returncode)
        self.assertTrue(tracing.finished[0][2])
        self.assertEqual(7, tracing.finished[0][1]["process.exit_code"])

    def test_check_step_span_records_timeout_and_lock_wait(self):
        tracing = FakeTracing()
        self.dev._DEV_TRACING = tracing
        span = FakeSpan()

        self.dev._finish_check_step_span(
            span,
            elapsed=8.5,
            timed_out=True,
            lock_wait=2.25,
            returncode=1,
            end_time=123_000_000,
        )

        self.assertEqual(span, tracing.finished[0][0])
        self.assertEqual({
            "check.elapsed_seconds": 8.5,
            "check.timed_out": True,
            "cargo.lock_wait_seconds": 2.25,
            "process.exit_code": 1,
        }, tracing.finished[0][1])
        self.assertTrue(tracing.finished[0][2])
        self.assertEqual(123_000_000, tracing.finished[0][3])

    def test_span_recording_failure_does_not_mask_command_result(self):
        class BrokenTracing:
            def finish_span(self, *_args, **_kwargs):
                raise RuntimeError("span processor failed")

        self.dev._DEV_TRACING = BrokenTracing()
        with mock.patch("builtins.print") as printed:
            self.dev._finish_dev_span(FakeSpan(), {"value": 1})

        self.assertIn("dev span recording failed", printed.call_args.args[0])

    def test_shutdown_marks_root_failure_without_masking_it(self):
        class CommandTracing(FakeTracing):
            def __init__(self):
                super().__init__()
                self.command_span = FakeSpan()
                self.command_started_at = 10.0
                self.shutdown_called = False

            def shutdown(self):
                self.shutdown_called = True

        tracing = CommandTracing()
        self.dev._DEV_TRACING = tracing
        error = SystemExit(3)
        with mock.patch.object(self.dev.time, "monotonic", return_value=12.5):
            self.dev._shutdown_dev_tracing(error)

        self.assertTrue(tracing.shutdown_called)
        self.assertEqual(2.5, tracing.finished[0][1]["dev.elapsed_seconds"])
        self.assertFalse(tracing.finished[0][1]["dev.success"])
        self.assertTrue(tracing.finished[0][2])

    def test_shutdown_export_failure_does_not_mask_command_result(self):
        class BrokenTracing:
            command_span = None

            def shutdown(self):
                raise RuntimeError("collector unavailable")

        self.dev._DEV_TRACING = BrokenTracing()
        with mock.patch("builtins.print") as printed:
            self.dev._shutdown_dev_tracing(None)

        self.assertIsNone(self.dev._DEV_TRACING)
        self.assertIn("dev trace export failed", printed.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
