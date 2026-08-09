import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_devpy():
    spec = importlib.util.spec_from_file_location("devpy_workflow_under_test", ROOT / "dev.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class DevWorkflowArtifactTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()

    def test_build_rust_defaults_to_debug_profile(self):
        with mock.patch.object(self.dev, "_run_cargo_build") as run:
            self.dev.build_rust()

        run.assert_called_once_with(["cargo", "build"], self.dev.ROOT, "debug")

    def test_up_builds_and_starts_debug_binary(self):
        with (
            mock.patch.object(self.dev, "reap_orphans", return_value=0),
            mock.patch.object(self.dev, "get_default_ports", return_value=(8041, 8042)),
            mock.patch.object(self.dev, "get_pid", return_value=None),
            mock.patch.object(self.dev, "select_dev_ports", return_value=(8041, 8042)),
            mock.patch.object(self.dev, "get_port_offsets", return_value=(0, 0)),
            mock.patch.object(self.dev, "get_worktree_hash", return_value="deadbeef"),
            mock.patch.object(self.dev, "build_rust") as build,
            mock.patch.object(self.dev, "start_phoenix", return_value=False) as start,
            mock.patch.object(self.dev, "start_vite", return_value=False),
            mock.patch("builtins.print"),
        ):
            self.dev.cmd_up(no_seed=True)

        build.assert_called_once_with()
        start.assert_called_once_with(port=8041, release=False, tls=False)

    def test_restart_builds_and_starts_debug_binary(self):
        def pid_for(path):
            return 100 if path == self.dev.PHOENIX_PID_FILE else None

        with (
            mock.patch.object(self.dev, "get_default_ports", return_value=(8041, 8042)),
            mock.patch.object(self.dev, "get_pid", side_effect=pid_for),
            mock.patch.object(self.dev, "build_rust") as build,
            mock.patch.object(self.dev, "stop_process"),
            mock.patch.object(self.dev.time, "sleep"),
            mock.patch.object(self.dev, "start_phoenix", return_value=False) as start,
            mock.patch("builtins.print"),
        ):
            self.dev.cmd_restart()

        build.assert_called_once_with()
        start.assert_called_once_with(port=8041, release=False, tls=False)

    def test_verification_cargo_environment_disables_incremental(self):
        self.assertEqual({"CARGO_INCREMENTAL": "0"}, self.dev._verification_cargo_env())

    def test_clippy_lane_invocation_isolated_and_non_incremental(self):
        command, env = self.dev._clippy_invocation(["-p", "phoenix-core"])
        self.assertEqual(
            [
                "cargo",
                "clippy",
                "-p",
                "phoenix-core",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            command,
        )
        self.assertEqual(str(self.dev.ROOT / "target" / "clippy"), env["CARGO_TARGET_DIR"])
        self.assertEqual("0", env["CARGO_INCREMENTAL"])

    def test_clippy_second_run_detects_new_denied_lint(self):
        with tempfile.TemporaryDirectory(prefix="phoenix-clippy-cache-") as directory:
            crate = Path(directory)
            (crate / "src").mkdir()
            (crate / "Cargo.toml").write_text(
                '[package]\nname = "clippy-cache-regression"\nversion = "0.1.0"\n'
                'edition = "2021"\n\n[lints.clippy]\npedantic = "deny"\n'
            )
            source = crate / "src" / "lib.rs"
            source.write_text("#[must_use]\npub fn value() -> &'static str { \"ok\" }\n")
            env = os.environ | self.dev._verification_cargo_env()

            first = subprocess.run(
                ["cargo", "clippy", "--quiet", "--", "-D", "warnings"],
                cwd=crate,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(0, first.returncode, first.stderr)

            source.write_text('#[must_use]\npub fn value() -> &\'static str { r#"ok"# }\n')
            second = subprocess.run(
                ["cargo", "clippy", "--quiet", "--", "-D", "warnings"],
                cwd=crate,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(0, second.returncode)
            self.assertIn("needless_raw_string_hashes", second.stderr)

    def test_standard_rust_lane_no_longer_describes_local_musl(self):
        self.assertNotIn("musl", self.dev._LANE_DESCS["rust"].lower())

    def test_production_cargo_features_include_datadog(self):
        self.assertEqual(
            ["--features", "phoenix-ide/datadog-tracing"],
            self.dev._production_cargo_feature_args(),
        )


if __name__ == "__main__":
    unittest.main()
