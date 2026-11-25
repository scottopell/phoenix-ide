"""Dispatcher fixtures never invoke sudo, the real detector, or a Rust binary."""
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
DISPATCHER = ROOT / "scripts/ci/close-nextest-wrapper.sh"
QUALIFICATION = ROOT / "scripts/ci/close-unprivileged-qualification.sh"
PREFIX = (
    "runtime::executor::creation_completion_lifecycle_tests::"
    "native_close_admission_tests::worktree_physical::"
)
TEST_NAMES = [PREFIX + suffix for suffix in (
    "registered_native_close_with_production_detector_retires_worktree_and_replays",
    "registered_native_close_captured_loss_public_retry_with_production_detector_reclaims_and_replays",
    "registered_native_close_captured_loss_retry_stop_during_assessment_preserves_worktree_and_history",
    "registered_native_close_captured_loss_retry_stop_while_prepared_preserves_worktree",
    "registered_native_close_captured_loss_retry_stop_before_writer_refuses_dispatch",
    "registered_native_close_captured_loss_retry_stop_before_physical_observation_retains_stopped_result",
)]


class CloseNextestWrapperTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="close nextest fixture ")
        self.addCleanup(temporary.cleanup)
        self.workspace = Path(temporary.name)
        self.harness = self.workspace / "scripts/ci/close-unprivileged-qualification.sh"
        self.harness.parent.mkdir(parents=True)
        # The production dispatcher resolves its fixed harness path in this fake
        # workspace. No production override or identity-check bypass is added.
        self.harness.write_text(
            "#!/bin/bash\nprintf '%s\\0' \"$@\"\n"
            "printf 'qualification fixture stderr\\n' >&2\nexit 7\n"
        )
        self.binary = self.workspace / "target/debug/deps/phoenix_ide-0123456789abcdef"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_text(
            "#!/bin/bash\nprintf '%s\\0' \"$@\"\n"
            "printf 'original fixture stderr\\n' >&2\nexit 23\n"
        )
        self.binary.chmod(0o700)

    def environment(self, name):
        env = os.environ.copy()
        env.update(NEXTEST_TEST_NAME=name, NEXTEST_BINARY_ID="phoenix_ide")
        return env

    def dispatch(self, name, args, *, binary=None, binary_id="phoenix_ide"):
        env = self.environment(name)
        env["NEXTEST_BINARY_ID"] = binary_id
        return subprocess.run(
            ["bash", str(DISPATCHER), str(self.workspace),
             str(binary or self.binary), *args],
            env=env, capture_output=True, timeout=5,
        )

    def test_nonmatching_tests_exec_original_argv_streams_and_status(self):
        for name in ("unrelated::test", TEST_NAMES[0] + "_suffix", ""):
            with self.subTest(name=name):
                args = ["--exact", "unrelated::test", "argument with spaces", ""]
                result = self.dispatch(name, args, binary_id="another_package")
                self.assertEqual(23, result.returncode)
                self.assertEqual(args, result.stdout.decode().split("\0")[:-1])
                self.assertEqual(b"original fixture stderr\n", result.stderr)

    def test_allowlist_is_exactly_the_existing_harness_six(self):
        pattern = re.escape(PREFIX) + r"[a-z0-9_]+"
        expected = set(TEST_NAMES)
        self.assertEqual(6, len(expected))
        for path in (DISPATCHER, QUALIFICATION):
            self.assertEqual(expected, set(re.findall(pattern, path.read_text())))

    def test_each_allowed_test_delegates_original_binary_and_fixed_exact_options(self):
        for name in TEST_NAMES:
            for args in (["--exact", name, "--nocapture"],
                         [name, "--nocapture", "--test-threads=1", "--exact"]):
                with self.subTest(name=name, args=args):
                    result = self.dispatch(name, args)
                    self.assertEqual(7, result.returncode)
                    self.assertEqual([
                        str(self.workspace), str(self.binary),
                        str(self.workspace / "target/debug/phoenix_ide"),
                        str(os.getuid()), name, "--exact", "--nocapture", "--test-threads=1",
                    ], result.stdout.decode().split("\0")[:-1])
                    self.assertEqual(b"qualification fixture stderr\n", result.stderr)

    def test_launcher_matches_ci_build_and_archive_contract(self):
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("cargo build -p phoenix_ide --bin phoenix_ide", ci)
        self.assertIn("test -x target/debug/phoenix_ide", ci)
        self.assertIn("            target/debug/phoenix_ide\n", ci)
        self.assertIn('"$workspace/target/debug/phoenix_ide"', DISPATCHER.read_text())

    def test_matching_identity_refuses_broadened_or_mismatched_invocations(self):
        name = TEST_NAMES[0]
        for args in (
            [name, "--nocapture"], [name, "--exact"],
            [TEST_NAMES[1], "--exact", "--nocapture"],
            [PREFIX, "--exact", "--nocapture"],
            [name, "--exact", "--nocapture", "--ignored"],
            [name, "--exact", "--nocapture", "--list"],
            [name, "--exact", "--nocapture", "--skip", name],
            [name, "--exact", "--nocapture", "--test-threads=2"],
            [name, "--exact", "--nocapture", name],
            [name, "--exact", "--nocapture", "--exact"],
        ):
            with self.subTest(args=args):
                result = self.dispatch(name, args)
                self.assertEqual(2, result.returncode)
                self.assertEqual(b"", result.stdout)
                self.assertIn(b"close nextest wrapper:", result.stderr)
        for binary_id in ("", "other", "phoenix_ide::bin/phoenix_ide"):
            with self.subTest(binary_id=binary_id):
                result = self.dispatch(name, ["--exact", name, "--nocapture"],
                                       binary_id=binary_id)
                self.assertEqual(2, result.returncode)
                self.assertEqual(b"", result.stdout)
        result = self.dispatch(name, ["--exact", name, "--nocapture"],
                               binary="/tmp/not-phoenix-tests")
        self.assertEqual(2, result.returncode)
        self.assertEqual(b"", result.stdout)

    def test_profiler_remains_parent_and_records_same_test_output_and_exit(self):
        name = TEST_NAMES[0]
        for status, outcome, counts in (
            (0, "ok", "1 passed; 0 failed; 0 ignored"),
            (7, "FAILED", "0 passed; 1 failed; 0 ignored"),
        ):
            with self.subTest(status=status):
                output = self.workspace / f"profile-{status}.json"
                self.harness.write_text(
                    "#!/bin/bash\nprintf 'running 1 test\\n'\n"
                    f"printf 'test %s ... {outcome}\\n' \"$5\"\n"
                    f"printf 'test result: {outcome}. {counts}\\n'\nexit {status}\n"
                )
                result = subprocess.run([
                    sys.executable, str(ROOT / "scripts/check_profile_command.py"),
                    "--output", str(output), "--", "bash", str(DISPATCHER),
                    str(self.workspace), str(self.binary), "--exact", name, "--nocapture",
                ], env=self.environment(name), capture_output=True, timeout=5)
                record = json.loads(output.read_text())
                self.assertEqual(status, result.returncode)
                self.assertEqual(status, record["returncode"])
                self.assertIn(name, record["identity"])
                self.assertEqual(result.stdout, Path(record["stdout_file"]).read_bytes())
                self.assertIn(counts.encode(), result.stdout)


class CloseNextestProfileConfigTests(unittest.TestCase):
    def test_profile_composition_and_default_are_unchanged_without_explicit_opt_in(self):
        spec = importlib.util.spec_from_file_location("devpy_close_profile", ROOT / "dev.py")
        dev = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(dev)
        with tempfile.TemporaryDirectory() as directory:
            profile = SimpleNamespace(artifact_dir=Path(directory))
            base = [
                sys.executable, str(ROOT / "scripts/check_profile_command.py"),
                "--output-dir", str(profile.artifact_dir / "rust-tests"), "--",
            ]
            configs = {}
            for value in (None, "", "0", "true", "1"):
                env = {} if value is None else {"PHOENIX_CHECK_CLOSE_UNPRIVILEGED": value}
                with self.subTest(value=value), mock.patch.dict(os.environ, env, clear=True):
                    config = dev._write_nextest_profile_config(profile).read_text()
                    configs[value] = config
                    command_line = next(line for line in config.splitlines()
                                        if line.startswith("command = "))
                    command = json.loads(command_line.removeprefix("command = "))
                    expected = base + (["bash", str(DISPATCHER), str(ROOT)] if value == "1" else [])
                    self.assertEqual(expected, command)
                    self.assertEqual(1, config.count('run-wrapper = "phoenix-cpu"'))
            for value in ("", "0", "true"):
                self.assertEqual(configs[None], configs[value])
            default = tomllib.loads(configs[None])
            isolated = tomllib.loads(configs["1"])
            self.assertEqual(default["profile"]["phoenix-cpu"]["scripts"],
                             isolated["profile"]["phoenix-cpu"]["scripts"])
            self.assertNotIn("test-groups", default)
            self.assertNotIn("overrides", default["profile"]["phoenix-cpu"])
            self.assertEqual({"close-physical-identity": {"max-threads": 1}},
                             isolated["test-groups"])
            self.assertEqual([{
                "filter": "binary(=phoenix_ide) & test(worktree_physical::)",
                "test-group": "close-physical-identity",
            }], isolated["profile"]["phoenix-cpu"]["overrides"])


if __name__ == "__main__":
    unittest.main()
