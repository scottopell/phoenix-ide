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
            mock.patch.object(self.dev, "PHOENIX_PORT_FILE", self.dev.ROOT / ".missing-test-port"),
            mock.patch.object(self.dev, "build_rust") as build,
            mock.patch.object(self.dev, "stop_process"),
            mock.patch.object(self.dev.time, "sleep"),
            mock.patch.object(self.dev, "start_phoenix", return_value=False) as start,
            mock.patch("builtins.print"),
        ):
            self.dev.cmd_restart()

        build.assert_called_once_with()
        start.assert_called_once_with(port=8041, release=False, tls=False)

    def test_doctor_probe_runs_in_requested_directory_and_handles_launch_errors(self):
        with mock.patch.object(self.dev.subprocess, "run", side_effect=PermissionError("denied")) as run:
            ok, detail = self.dev._doctor_version(["pnpm", "--version"], cwd=self.dev.UI_DIR)

        self.assertFalse(ok)
        self.assertIn("cannot execute", detail)
        self.assertEqual(self.dev.UI_DIR, run.call_args.kwargs["cwd"])

    def test_doctor_probe_matches_required_line_across_multiline_output(self):
        completed = self.dev.subprocess.CompletedProcess(
            args=[], returncode=0, stdout="stable\nx86_64-unknown-linux-musl\n", stderr="",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed):
            ok, _detail = self.dev._doctor_version(
                ["rustup", "target", "list"],
                required_line="x86_64-unknown-linux-musl",
            )
        self.assertTrue(ok)

    def test_doctor_honors_explicit_chrome_executable(self):
        chrome = Path("/custom/Google Chrome")
        versions = {
            "node": (True, "v26.0.0"),
            "corepack": (True, "0.35.0"),
            "pnpm": (True, "11.0.8"),
            "rustup": (True, "rustc 1.95.0 (test)"),
            "rustc": (True, "rustc 1.95.0 (test)"),
            "cargo": (True, "cargo 1.95.0"),
            "allium": (False, "not found"),
            "rg": (False, "not found"),
            "ast-grep": (False, "not found"),
        }

        def probe(command, **_kwargs):
            if command[0] == "rustup":
                return (True, "rustc 1.95.0 (test)") if "rustc" in command else (True, "cargo 1.95.0")
            return versions.get(command[0], (True, "ok"))

        with (
            mock.patch.dict(self.dev.os.environ, {"PHOENIX_CHROME_EXECUTABLE": str(chrome)}),
            mock.patch.object(self.dev, "_doctor_version", side_effect=probe),
            mock.patch.object(Path, "is_file", autospec=True, side_effect=lambda path: path == chrome),
            mock.patch.object(self.dev.os, "access", return_value=True),
            mock.patch.object(self.dev.shutil, "which", return_value="/usr/bin/strip"),
        ):
            results = self.dev.collect_doctor_results()

        browser = next(result for result in results if result.name == "chrome")
        self.assertTrue(browser.ok)
        self.assertEqual(str(chrome), browser.detail)

    def test_doctor_does_not_bootstrap_taskmd(self):
        with mock.patch.object(self.dev.os, "execvpe") as execvpe:
            self.dev._ensure_taskmd_for_command("doctor")
        execvpe.assert_not_called()

    def test_doctor_is_recognized_after_global_options(self):
        self.assertEqual("doctor", self.dev._requested_command(["--pretty", "doctor"]))

    def test_doctor_pnpm_probe_disables_corepack_network(self):
        seen_environment = None

        def probe(command, **kwargs):
            nonlocal seen_environment
            if command[0] == "pnpm":
                seen_environment = kwargs["env"]
            return True, {
                "node": "v26.0.0",
                "corepack": "0.35.0",
                "pnpm": "11.0.8",
                "rustup": "rustc 1.95.0 (test)" if "rustc" in command else "cargo 1.95.0",
                "cargo": "cargo 1.95.0",
            }.get(command[0], "ok")

        with (
            mock.patch.object(self.dev, "_doctor_version", side_effect=probe),
            mock.patch.object(self.dev, "_find_chromium_binary", return_value=Path("/chrome")),
            mock.patch.object(Path, "is_file", return_value=True),
            mock.patch.object(self.dev.os, "access", return_value=True),
            mock.patch.object(self.dev.shutil, "which", return_value="/usr/bin/strip"),
        ):
            self.dev.collect_doctor_results()

        self.assertEqual("0", seen_environment["COREPACK_ENABLE_NETWORK"])

    def test_doctor_fails_only_for_missing_required_prerequisites(self):
        results = [
            self.dev.DoctorResult("cargo", False, "not found"),
            self.dev.DoctorResult("allium", False, "not found", required=False),
        ]

        with mock.patch.object(self.dev, "collect_doctor_results", return_value=results):
            with mock.patch("builtins.print") as output:
                self.assertFalse(self.dev.cmd_doctor())

        rendered = "\n".join(" ".join(map(str, call.args)) for call in output.call_args_list)
        self.assertIn("✗ cargo: not found", rendered)
        self.assertIn("- allium (optional): not found", rendered)
        self.assertIn("Missing required prerequisites: cargo", rendered)

    def test_doctor_succeeds_when_only_optional_tools_are_missing(self):
        results = [
            self.dev.DoctorResult("cargo", True, "cargo 1.95.0"),
            self.dev.DoctorResult("ast-grep", False, "not found", required=False),
        ]

        with mock.patch.object(self.dev, "collect_doctor_results", return_value=results):
            with mock.patch("builtins.print") as output:
                self.assertTrue(self.dev.cmd_doctor())

        rendered = "\n".join(" ".join(map(str, call.args)) for call in output.call_args_list)
        self.assertIn("Ready for full local development and deployment checks.", rendered)

    def test_chromium_discovery_prefers_native_signed_chrome_over_path_chromium(self):
        signed_chrome = Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        path_chromium = Path("/opt/homebrew/bin/chromium")

        with (
            mock.patch.object(self.dev, "_native_chromium_candidates", return_value=(signed_chrome,)),
            mock.patch.object(Path, "is_file", autospec=True, side_effect=lambda path: path == signed_chrome),
            mock.patch.object(self.dev.os, "access", return_value=True),
            mock.patch("shutil.which", return_value=str(path_chromium)) as which,
        ):
            selected = self.dev._find_chromium_binary()

        self.assertEqual(signed_chrome, selected)
        which.assert_not_called()

    def test_chromium_discovery_falls_back_to_path(self):
        path_chromium = Path("/opt/homebrew/bin/chromium")

        with (
            mock.patch.object(self.dev, "_native_chromium_candidates", return_value=()),
            mock.patch("shutil.which", side_effect=lambda name: str(path_chromium) if name == "chromium" else None),
        ):
            selected = self.dev._find_chromium_binary()

        self.assertEqual(path_chromium, selected)

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
            (crate / "rust-toolchain.toml").write_text((ROOT / "rust-toolchain.toml").read_text())
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

    def test_required_ci_compiles_production_feature_for_musl(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        self.assertIn("sudo apt-get install -y musl-tools", workflow)
        self.assertIn(
            "cargo check --target x86_64-unknown-linux-musl "
            "--features phoenix_ide/datadog-tracing",
            workflow,
        )

    def test_production_cargo_features_include_datadog(self):
        self.assertEqual(
            ["--features", "phoenix_ide/datadog-tracing"],
            self.dev._production_cargo_feature_args(),
        )
        metadata = subprocess.run(
            [
                "cargo",
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                *self.dev._production_cargo_feature_args(),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(0, metadata.returncode, metadata.stderr)


if __name__ == "__main__":
    unittest.main()
