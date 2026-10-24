import importlib.util
import io
import json
import os
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

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
        receipt, child_stderr = result.stderr.split("\n", 1)
        self.assertIn("check profile started:", receipt)
        self.assertIn(measurement["start_file"], receipt)
        self.assertEqual("err\n", child_stderr)
        self.assertEqual("out\n", Path(measurement["stdout_file"]).read_text())
        self.assertEqual("err\n", Path(measurement["stderr_file"]).read_text())
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

    def start_wrapper(self, directory, code, *, env=None):
        process = subprocess.Popen(
            [sys.executable, str(WRAPPER), "--output-dir", str(directory), "--",
             sys.executable, "-u", "-c", code],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            start_new_session=True, env=env,
        )

        def cleanup():
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)

        self.addCleanup(cleanup)
        return process

    def wait_for(self, predicate):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if value := predicate():
                return value
            time.sleep(0.01)
        self.fail("timed out waiting for wrapper evidence")

    def test_start_and_incremental_output_survive_kill_without_cpu_sample(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            process = self.start_wrapper(root,
                "import pathlib,sys,time; "
                f"assert len(list(pathlib.Path({directory!r}).glob('*.start'))) == 1; "
                "print('alive', flush=True); print('error alive', file=sys.stderr, flush=True); "
                "time.sleep(30)"
            )
            start_file = self.wait_for(lambda: next(iter(root.glob("*.start")), None))
            start = json.loads(start_file.read_text())
            self.wait_for(lambda: Path(start["stdout_file"]).read_bytes() == b"alive\n")
            self.wait_for(lambda: Path(start["stderr_file"]).read_bytes() == b"error alive\n")
            self.assertIsNone(process.poll())
            self.assertEqual(process.pid, start["parent_pid"])
            self.assertGreater(start["pid"], 0)
            self.assertEqual("started", start["event"])
            self.assertEqual(sys.executable, start["command"][0])
            self.assertNotIn("total_cpu_ms", start)
            self.assertFalse(list(root.glob("process-*.json")))
            # Forwarding is live, not deferred until the command completes.
            self.assertTrue(select.select([process.stdout], [], [], 5)[0])
            self.assertEqual(b"alive\n", process.stdout.readline())
            self.assertTrue(select.select([process.stderr], [], [], 5)[0])
            receipt = process.stderr.readline()
            self.assertIn(str(start_file).encode(), receipt)
            self.assertIn(start["identity"].encode(), receipt)
            self.assertEqual(b"error alive\n", process.stderr.readline())
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)
            self.assertEqual(-signal.SIGKILL, process.returncode)
            self.assertTrue(start_file.exists())
            self.assertEqual(b"alive\n", Path(start["stdout_file"]).read_bytes())
            self.assertFalse(list(root.glob("process-*.json")))
            spec = importlib.util.spec_from_file_location(
                "profile_report_under_test", ROOT / "scripts" / "check_profile_report.py",
            )
            report = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(report)
            errors = []
            self.assertEqual([], list(report._records(root, errors)))
            self.assertEqual([], errors)

    def test_parallel_and_retry_invocations_have_unique_joinable_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            processes = [self.start_wrapper(root, "pass", env={
                **os.environ, "NEXTEST_TEST_NAME": "demo::same_test",
                "NEXTEST_BINARY_ID": "demo-binary", "NEXTEST_ATTEMPT": attempt,
                "DO_NOT_CAPTURE_ENV": "synthetic-secret-sentinel",
            }) for attempt in ("1", "1", "2")]
            for process in processes:
                process.communicate(timeout=5)
                self.assertEqual(0, process.returncode)
            starts = [json.loads(path.read_text()) for path in root.glob("*.start")]
            samples = [json.loads(path.read_text()) for path in root.glob("process-*.json")]
            self.assertEqual(3, len(starts))
            self.assertEqual(3, len(samples))
            self.assertEqual(3, len({start["invocation_id"] for start in starts}))
            self.assertEqual({"1", "2"}, {start["attempt"] for start in starts})
            for sample in samples:
                start = json.loads(Path(sample["start_file"]).read_text())
                for key in ("invocation_id", "identity", "pid", "parent_pid", "command",
                            "attempt", "binary_id", "test_name", "stdout_file", "stderr_file"):
                    self.assertEqual(start[key], sample[key])
                self.assertEqual("rust:demo-binary:demo::same_test", start["identity"])
                self.assertEqual("rust_test", sample["kind"])
                self.assertNotIn("synthetic-secret-sentinel", json.dumps(start))
                self.assertNotIn("synthetic-secret-sentinel", json.dumps(sample))

    def test_forwards_signal_and_preserves_signal_exit_status(self):
        for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            with self.subTest(signum=signum), tempfile.TemporaryDirectory() as directory:
                process = self.start_wrapper(directory, "import time; print('ready'); time.sleep(30)")
                self.assertTrue(select.select([process.stdout], [], [], 5)[0])
                self.assertEqual(b"ready\n", process.stdout.readline())
                process.send_signal(signum)
                process.communicate(timeout=5)
                self.assertEqual(128 + signum, process.returncode)
                sample = json.loads(next(Path(directory).glob("process-*.json")).read_text())
                self.assertEqual(-signum, sample["returncode"])

    def test_closed_consumers_still_reap_and_record_completed_children(self):
        for stream in ("stdout", "stderr"):
            for child_status in (0, 7):
                with self.subTest(stream=stream, child_status=child_status), \
                     tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    other_stream = "stderr" if stream == "stdout" else "stdout"
                    # Close the consumer before launch, including the receipt sink.
                    reader, writer = os.pipe()
                    os.close(reader)
                    try:
                        process = subprocess.Popen([
                            sys.executable, str(WRAPPER), "--output-dir", directory, "--",
                            sys.executable, "-u", "-c",
                            "import sys,time; "
                            f"sys.{stream}.buffer.write(b'x' * 200000); "
                            "time.sleep(0.2); "
                            f"sys.{stream}.write('finished\\n'); "
                            f"sys.{other_stream}.write('other finished\\n'); sys.exit({child_status})",
                        ], **{stream: writer, other_stream: subprocess.PIPE})
                    finally:
                        os.close(writer)
                    out, err = process.communicate(timeout=5)
                    self.assertIn(b'other finished\n', out if other_stream == "stdout" else err)
                    self.assertEqual(child_status or 1, process.returncode)
                    sample = json.loads(next(root.glob("process-*.json")).read_text())
                    self.assertEqual(child_status, sample["returncode"])
                    self.assertEqual(process.returncode, sample["wrapper_returncode"])
                    self.assertEqual([stream], [error["stream"] for error in sample["forwarding_errors"]])
                    self.assertEqual(b'x' * 200000 + b'finished\n',
                                     Path(sample[f"{stream}_file"]).read_bytes())
                    self.assertEqual(sample["pid"], json.loads(
                        Path(sample["start_file"]).read_text())["pid"])
                    with self.assertRaises(ProcessLookupError):
                        os.kill(sample["pid"], 0)

    def test_reaped_child_is_invalidated_with_signals_blocked_before_output_drain(self):
        spec = importlib.util.spec_from_file_location("wrapper_under_test", WRAPPER)
        wrapper = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(wrapper)
        forwarded_signals = {signal.SIGTERM, signal.SIGINT, signal.SIGHUP}
        previous_handlers = {s: signal.getsignal(s) for s in forwarded_signals}
        previous_mask = signal.pthread_sigmask(signal.SIG_BLOCK, [])
        real_fstat = os.fstat
        reaped = False
        drained = False

        def reap(pid, options):
            nonlocal reaped
            self.assertEqual((4242, os.WNOHANG), (pid, options))
            self.assertTrue(forwarded_signals <= signal.pthread_sigmask(signal.SIG_BLOCK, []))
            # Delivery is deferred until wait4 returns and owned state is cleared.
            signal.raise_signal(signal.SIGTERM)
            reaped = True
            return pid, 0, SimpleNamespace(ru_utime=0, ru_stime=0)

        def drain(fd):
            nonlocal drained
            self.assertTrue(reaped)
            self.assertEqual(previous_mask, signal.pthread_sigmask(signal.SIG_BLOCK, []))
            for signum in forwarded_signals:
                signal.getsignal(signum)(signum, None)
            drained = True
            return real_fstat(fd)

        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(wrapper.os, "fork", return_value=4242), \
             mock.patch.object(wrapper.os, "wait4", side_effect=reap) as wait, \
             mock.patch.object(wrapper.os, "fstat", side_effect=drain), \
             mock.patch.object(wrapper.os, "kill") as kill, \
             mock.patch.object(wrapper.sys, "stdout", SimpleNamespace(buffer=io.BytesIO())), \
             mock.patch.object(wrapper.sys, "stderr", SimpleNamespace(buffer=io.BytesIO())):
            self.assertEqual(0, wrapper.measure(["unused"], output_dir=Path(directory)))
            wait.assert_called_once()
            kill.assert_not_called()
            self.assertTrue(drained)
        self.assertEqual(previous_mask, signal.pthread_sigmask(signal.SIG_BLOCK, []))
        self.assertEqual(previous_handlers, {s: signal.getsignal(s) for s in forwarded_signals})

    def test_binary_output_is_forwarded_and_retained_without_truncation(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "sample.json"
            result = subprocess.run([
                sys.executable, str(WRAPPER), "--output", str(output), "--",
                sys.executable, "-c",
                "import sys; sys.stdout.buffer.write(b'\\x00\\xff' * 100000); "
                "sys.stderr.buffer.write(b'err' * 100000)",
            ], capture_output=True, timeout=5)
            self.assertEqual(0, result.returncode)
            sample = json.loads(output.read_text())
            self.assertEqual(b"\x00\xff" * 100000, result.stdout)
            self.assertEqual(b"err" * 100000, result.stderr.split(b"\n", 1)[1])
            self.assertEqual(result.stdout, Path(sample["stdout_file"]).read_bytes())
            self.assertEqual(b"err" * 100000, Path(sample["stderr_file"]).read_bytes())

    def test_exec_failure_still_records_exit_127(self):
        result, sample = self.run_wrapper(["/nonexistent/phoenix-check-command"])
        self.assertEqual(127, result.returncode)
        self.assertEqual(127, sample["returncode"])
        self.assertIn("cannot execute", Path(sample["stderr_file"]).read_text())

    def test_codegen_profile_shaping_preserves_filters_scope_and_threads(self):
        spec = importlib.util.spec_from_file_location("devpy_profile_under_test", ROOT / "dev.py")
        dev = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(dev)
        profile_args = ["--config-file", "profile.toml", "--profile", "phoenix-cpu"]
        for nextest in (True, False):
            with self.subTest(nextest=nextest):
                plain = dev._rust_check_commands(
                    nextest, ["-p", "compile"], ["-p", "tests"], ["-p", "exports"], 3, [],
                )
                profiled = dev._rust_check_commands(
                    nextest, ["-p", "compile"], ["-p", "tests"], ["-p", "exports"], 3,
                    profile_args,
                )
                self.assertEqual(plain[0], profiled[0])
                for original, measured in zip(plain[1:], profiled[1:]):
                    if nextest:
                        self.assertEqual(profile_args, measured[2:6])
                        self.assertEqual(original, measured[:2] + measured[6:])
                    else:
                        self.assertEqual(original, measured)
                if nextest:
                    self.assertIn("not test(/export_bindings/)", profiled[1])
                    self.assertIn("test(/export_bindings/)", profiled[2])
                else:
                    self.assertEqual("export_bindings", profiled[1][-1])
                    self.assertIn("export_bindings", profiled[2])

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
