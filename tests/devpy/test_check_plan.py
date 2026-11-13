import io
import importlib.util
import json
import os
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


GATED_CI_ENV = {"CI": "1", "PHOENIX_CHECK_BASE": "origin/main", "PHOENIX_CHECK_ALL": ""}


def load_devpy():
    spec = importlib.util.spec_from_file_location("devpy_under_test", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def launcher_artifact(root, executable, *, fresh=False):
    return {
        "reason": "compiler-artifact",
        "manifest_path": str(root / "crates" / "phoenix-ide" / "Cargo.toml"),
        "target": {"name": "phoenix_ide", "kind": ["bin"]},
        "executable": str(executable) if executable is not None else None,
        "fresh": fresh,
    }


class CheckPlanTests(unittest.TestCase):
    def setUp(self):
        env_patch = mock.patch.dict(os.environ)
        env_patch.start()
        self.addCleanup(env_patch.stop)
        os.environ.pop("PHOENIX_CHECK_WORKERS", None)
        os.environ.pop("CARGO_TARGET_DIR", None)
        self.dev = load_devpy()

    def test_git_config_key_validation_preserves_url_subsections(self):
        for key in (
            "url.https://github.com/.insteadOf",
            "http.https://example.com.proxy",
        ):
            with self.subTest(key=key):
                self.assertTrue(self.dev._is_valid_git_config_key(key))

        for key in ("bad key", "nosection", "section.9name", "section.name\nother.value"):
            with self.subTest(key=key):
                self.assertFalse(self.dev._is_valid_git_config_key(key))

    def test_git_config_override_preserves_url_subsection_entry(self):
        env = {
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "url.https://github.com/.insteadOf",
            "GIT_CONFIG_VALUE_0": "ssh://git@github.com/",
        }

        self.dev._append_git_config_override("commit.gpgsign", "false", env)

        self.assertEqual("1", env["GIT_CONFIG_COUNT"])
        self.assertEqual("url.https://github.com/.insteadOf", env["GIT_CONFIG_KEY_0"])
        self.assertEqual("ssh://git@github.com/", env["GIT_CONFIG_VALUE_0"])
        self.assertEqual("'commit.gpgsign=false'", env["GIT_CONFIG_PARAMETERS"])

    def test_git_config_override_composes_with_complete_inherited_entries(self):
        env = {
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "fetch.prune",
            "GIT_CONFIG_VALUE_0": "true",
        }

        self.dev._append_git_config_override("commit.gpgsign", "false", env)

        self.assertEqual("1", env["GIT_CONFIG_COUNT"])
        self.assertEqual("fetch.prune", env["GIT_CONFIG_KEY_0"])
        self.assertEqual("true", env["GIT_CONFIG_VALUE_0"])
        self.assertEqual("'commit.gpgsign=false'", env["GIT_CONFIG_PARAMETERS"])

    def test_git_config_override_preserves_parameters_and_disables_signing_last(self):
        env = {"GIT_CONFIG_PARAMETERS": "'safe.directory=/repo' 'commit.gpgsign=true'"}

        self.dev._append_git_config_override("commit.gpgsign", "false", env)

        self.assertEqual(
            "'safe.directory=/repo' 'commit.gpgsign=true' 'commit.gpgsign=false'",
            env["GIT_CONFIG_PARAMETERS"],
        )
        self.assertEqual("0", env["GIT_CONFIG_COUNT"])

    def test_git_config_override_resets_malformed_inherited_entries(self):
        for malformed in (
            {"GIT_CONFIG_COUNT": "wat", "GIT_CONFIG_KEY_9": "stale"},
            {"GIT_CONFIG_COUNT": "-1", "GIT_CONFIG_VALUE_9": "stale"},
            {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "incomplete"},
            {
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": "bad key",
                "GIT_CONFIG_VALUE_0": "true",
            },
        ):
            with self.subTest(malformed=malformed):
                env = malformed.copy()
                self.dev._append_git_config_override("commit.gpgsign", "false", env)
                self.assertEqual(
                    {
                        "GIT_CONFIG_COUNT": "0",
                        "GIT_CONFIG_PARAMETERS": "'commit.gpgsign=false'",
                    },
                    env,
                )

    def test_check_cpu_budget_saturates_at_four(self):
        with mock.patch.object(self.dev.os, "cpu_count", return_value=10):
            self.assertEqual(4, self.dev._check_cpu_budget())
        with mock.patch.object(self.dev.os, "cpu_count", return_value=2):
            self.assertEqual(2, self.dev._check_cpu_budget())
        with mock.patch.object(self.dev.os, "cpu_count", return_value=None):
            self.assertEqual(1, self.dev._check_cpu_budget())

    def test_check_worker_override_one(self):
        with mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": "1"}), \
             mock.patch.object(self.dev.os, "cpu_count", return_value=10):
            self.assertEqual(1, self.dev._check_cpu_budget())
            self.assertEqual("1", self.dev._vitest_check_command()[-1])

    def test_invalid_check_worker_override_fails_before_lane_resolution(self):
        for raw in ("", " ", "0", "-1", "1.5", "abc", "+1", "1_0", "١"):
            with self.subTest(raw=raw), \
                 mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": raw}), \
                 mock.patch.object(self.dev, "_resolve_check_lanes") as resolve:
                with self.assertRaisesRegex(SystemExit, "PHOENIX_CHECK_WORKERS"):
                    self.dev.cmd_check(gate=False, lanes="task")
                resolve.assert_not_called()

    def test_check_worker_budget_clamps_override_to_available_cpus(self):
        for raw, cpus, expected in (("2", 10, 2), ("8", 10, 8), ("8", 2, 2), ("1", None, 1)):
            with self.subTest(raw=raw, cpus=cpus), \
                 mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": raw}), \
                 mock.patch.object(self.dev.os, "cpu_count", return_value=cpus):
                self.assertEqual(expected, self.dev._check_cpu_budget())

    def test_all_check_worker_consumers_use_requested_one(self):
        with mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": "1"}), \
             mock.patch.object(self.dev.os, "cpu_count", return_value=10):
            budget = self.dev._check_cpu_budget()
            self.assertEqual({
                "CARGO_BUILD_JOBS": "1", "RAYON_NUM_THREADS": "1", "RUST_TEST_THREADS": "1",
            }, self.dev._check_worker_env(budget))
            self.assertEqual("1", self.dev._vitest_check_command(budget)[-1])
            for nextest in (True, False):
                with self.subTest(nextest=nextest):
                    compile_cmd, test_cmd, codegen_cmd = self.dev._rust_check_commands(
                        nextest, ["-p", "compile"], ["-p", "tests"], ["-p", "exports"],
                        budget, ["--config-file", "profile.toml", "--profile", "phoenix-cpu"],
                    )
                    self.assertIn("--no-run", compile_cmd)
                    self.assertIn("compile", compile_cmd)
                    self.assertIn("tests", test_cmd)
                    self.assertIn("exports", codegen_cmd)
                    for cmd in (test_cmd, codegen_cmd):
                        self.assertEqual("1", cmd[cmd.index("--test-threads") + 1])
                        if not nextest:
                            self.assertLess(cmd.index("--"), cmd.index("--test-threads"))
                    if nextest:
                        self.assertIn("profile.toml", test_cmd)
                        self.assertIn("not test(/export_bindings/)", test_cmd)
                        self.assertIn("test(/export_bindings/)", codegen_cmd)
                    else:
                        self.assertEqual("export_bindings", test_cmd[-1])
                        self.assertIn("export_bindings", codegen_cmd)

    def test_rust_lane_builds_launcher_before_codegen_and_tests(self):
        cases = (
            ("success", 0, 0, False, 0, 0),
            ("default target", 0, 0, False, 0, 0),
            ("absolute target", 0, 0, False, 0, 0),
            ("environment target", 0, 0, False, 0, 0),
            ("cargo config target", 0, 0, False, 0, 0),
            ("fresh configured target", 0, 0, False, 0, 0),
            ("launcher failure", 17, 0, False, 0, 0),
            ("codegen failure", 0, 1, False, 2, 3),
            ("stale export", 0, 0, True, 2, 3),
            ("compile failure", 0, 0, False, 1, 0),
        )
        for nextest in (True, False):
            for label, launcher_rc, codegen_rc, stale, compile_rc, test_rc in cases:
                with self.subTest(nextest=nextest, case=label), \
                     tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    generated = root / "ui" / "src" / "generated"
                    generated.mkdir(parents=True)
                    content = "// This file was generated by ts-rs.\nexport type Binding = string;\n"
                    (generated / "Binding.ts").write_text(content)
                    compile_cmd, test_cmd, codegen_cmd = self.dev._rust_check_commands(
                        nextest, [], [], [], 1, [],
                    )
                    commands = [["cargo", "build", "-p", "phoenix_ide", "--bin", "phoenix_ide",
                                 "--message-format=json-render-diagnostics"]]
                    returncodes = [launcher_rc]
                    if launcher_rc == 0:
                        commands += [codegen_cmd, compile_cmd, test_cmd]
                        returncodes += [codegen_rc, compile_rc, test_rc]
                    launched = []
                    target_setting = (None if label == "default target" else
                                      str(root / "absolute-target") if label == "absolute target" else
                                      "custom-target")
                    target_dir = root / (target_setting or "target")
                    stale_launcher = target_dir / "debug" / "phoenix_ide"
                    stale_launcher.parent.mkdir(parents=True)
                    stale_launcher.write_text("stale fixture must not skip cargo build")
                    stale_launcher.chmod(0o755)
                    configured_target = label in ("environment target", "cargo config target", "fresh configured target")
                    launcher = (target_dir / "host-triple" / "debug" / "phoenix_ide"
                                if configured_target else stale_launcher)
                    launcher.parent.mkdir(parents=True, exist_ok=True)
                    launcher.write_text("current native launcher")
                    launcher.chmod(0o755)
                    launcher_stat = launcher.stat()
                    if label == "cargo config target":
                        (root / ".cargo").mkdir()
                        (root / ".cargo" / "config.toml").write_text('[build]\ntarget = "host-triple"\n')
                    inherited_fixture = {
                        "PHOENIX_BASH_LAUNCH_TEST_EXE": "/stale/elsewhere/phoenix_ide",
                        "PHOENIX_BASH_LAUNCH_TEST_REQUIRED": "0",
                    }

                    def launch(cmd, **kwargs):
                        index = len(launched)
                        self.assertEqual(commands[index], cmd)
                        launched.append(cmd)
                        env = kwargs["env"]
                        self.assertEqual(str(root), str(kwargs["cwd"]))
                        self.assertEqual(target_setting, env.get("CARGO_TARGET_DIR"))
                        self.assertEqual("/selected/kache", env["RUSTC_WRAPPER"])
                        for key in ("CARGO_BUILD_JOBS", "RAYON_NUM_THREADS", "RUST_TEST_THREADS"):
                            self.assertEqual("1", env[key])
                        if index > 0:
                            self.assertEqual(str(launcher), env["PHOENIX_BASH_LAUNCH_TEST_EXE"])
                            self.assertEqual("1", env["PHOENIX_BASH_LAUNCH_TEST_REQUIRED"])
                        if index == 1:
                            base = Path(env["TS_RS_EXPORT_DIR"])
                            (base.parents[2] / "Binding.ts").write_text(
                                content + ("// stale\n" if stale else "")
                            )
                        artifact = launcher_artifact(root, launcher, fresh=label == "fresh configured target")
                        # Metadata must survive the bounded console tail and rendered diagnostics.
                        build_output = json.dumps(artifact) + "\n" + "warning: rendered diagnostic\n" * 205
                        proc = mock.Mock(stdout=io.StringIO(build_output if index == 0 else ""))
                        proc.wait.return_value = returncodes[index]
                        return proc

                    reporter = mock.Mock()
                    output = io.StringIO()
                    failures = bool(launcher_rc) + bool(codegen_rc) + bool(stale) + bool(compile_rc) + bool(test_rc)
                    cache_setup = ("kache", {"RUSTC_WRAPPER": "/selected/kache"})
                    with mock.patch.dict(os.environ, {
                             **inherited_fixture,
                             **({"CARGO_TARGET_DIR": target_setting} if target_setting else {}),
                             "PHOENIX_CHECK_WORKERS": "1",
                             **({"CARGO_BUILD_TARGET": "host-triple"}
                                if label in ("environment target", "fresh configured target") else {}),
                         }), \
                         mock.patch.object(self.dev, "ROOT", root), \
                         mock.patch.object(self.dev, "_resolve_check_lanes", return_value=({"rust"}, {})), \
                         mock.patch.object(self.dev, "_make_reporter", return_value=reporter), \
                         mock.patch.object(self.dev, "_classify_browser_env"), \
                         mock.patch.object(self.dev, "_cargo_test_timeout_secs", return_value=(3000, "environment")), \
                         mock.patch.object(self.dev.subprocess, "Popen", side_effect=launch), \
                         mock.patch.object(self.dev.subprocess, "run", return_value=mock.Mock(returncode=0 if nextest else 1)) as probe, \
                         mock.patch.object(self.dev.sys, "stdout", output):
                        if failures:
                            with self.assertRaises(SystemExit) as raised:
                                self.dev.cmd_check(gate=False, lanes="rust", compiler_cache_setup=cache_setup)
                            self.assertEqual(1, raised.exception.code)
                        else:
                            self.dev.cmd_check(gate=False, lanes="rust", compiler_cache_setup=cache_setup)
                        for key, value in inherited_fixture.items():
                            self.assertEqual(value, os.environ[key])

                    self.assertEqual(launcher_stat, launcher.stat())
                    probe.assert_called_once_with(
                        ["cargo", "nextest", "--version"], capture_output=True, timeout=30,
                    )
                    self.assertEqual(commands, launched)
                    steps = [("native bash launcher", launcher_rc)]
                    if launcher_rc == 0:
                        steps.append(("codegen", codegen_rc))
                        if codegen_rc == 0:
                            steps.append(("codegen-stale", int(stale)))
                        steps += [("cargo test compile", compile_rc), ("cargo test", test_rc)]
                    self.assertEqual(
                        [name for name, _rc in steps],
                        [call.args[1] for call in reporter.step_start.call_args_list],
                    )
                    self.assertEqual(
                        steps,
                        [(call.args[1], call.args[2]) for call in reporter.step_done.call_args_list],
                    )
                    summary = (f"{failures} of {len(steps)} checks failed" if failures
                               else f"All {len(steps)} checks passed")
                    self.assertIn(summary, output.getvalue())

    def test_launcher_metadata_fails_closed_before_normal_or_split_consumers(self):
        cases = (
            "missing", "malformed", "unrelated", "duplicate", "no executable", "missing executable field",
            "missing executable", "relative executable", "non-executable", "directory", "build failure",
        )
        for split_build in (False, True):
            for case in cases:
                with self.subTest(split_build=split_build, case=case), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    stale = root / "target" / "debug" / "phoenix_ide"
                    stale.parent.mkdir(parents=True)
                    stale.write_text("stale default executable must not be used")
                    stale.chmod(0o755)
                    artifact = launcher_artifact(root, stale)
                    if case in ("missing", "build failure"):
                        records = []
                    elif case == "missing executable field":
                        artifact.pop("executable")
                        records = [json.dumps(artifact)]
                    elif case == "malformed":
                        records = ["{malformed JSON", "null", '[]']
                    elif case == "unrelated":
                        records = [json.dumps({**artifact, "manifest_path": "/dependency/Cargo.toml"}),
                                   json.dumps({**artifact, "target": {"name": "phoenix_ide", "kind": ["lib"]}}),
                                   json.dumps({**artifact, "target": {"name": "other", "kind": ["bin"]}}),
                                   json.dumps({**artifact, "reason": "compiler-message"})]
                    elif case == "duplicate":
                        records = [json.dumps(artifact)] * 2
                    else:
                        executable = {
                            "no executable": None,
                            "missing executable": root / "missing",
                            "relative executable": "target/debug/phoenix_ide",
                            "non-executable": root / "non-executable",
                            "directory": stale.parent,
                        }[case]
                        if case == "non-executable":
                            executable.write_text("not executable")
                            executable.chmod(0o644)
                        records = [json.dumps(launcher_artifact(root, executable))]
                    proc = mock.Mock(stdout=io.StringIO("\n".join(records) + "\nwarning: rendered diagnostic\n"))
                    build_rc = 17 if case == "build failure" else 0
                    proc.wait.return_value = build_rc
                    reporter = mock.Mock()
                    output = io.StringIO()
                    split = self.dev.RustSplit(self.dev.RustPhase.BUILD) if split_build else None
                    with mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": "1"}), \
                         mock.patch.object(self.dev, "ROOT", root), \
                         mock.patch.object(self.dev, "_resolve_check_lanes", return_value=({"rust"}, {})), \
                         mock.patch.object(self.dev, "_make_reporter", return_value=reporter), \
                         mock.patch.object(self.dev, "_classify_browser_env"), \
                         mock.patch.object(self.dev, "_write_rust_split_scope"), \
                         mock.patch.object(self.dev.subprocess, "Popen", return_value=proc) as launch, \
                         mock.patch.object(self.dev.subprocess, "run", return_value=mock.Mock(returncode=0)), \
                         mock.patch.object(self.dev.sys, "stdout", output):
                        with self.assertRaises(SystemExit) as raised:
                            self.dev.cmd_check(gate=False, lanes="rust", rust_split=split,
                                               compiler_cache_setup=("none", {}))
                    self.assertEqual(1, raised.exception.code)
                    launch.assert_called_once()
                    reporter.step_done.assert_called_once_with("rust", "native bash launcher", build_rc or 1, mock.ANY)
                    if build_rc:
                        self.assertNotIn("OUTPUT VALIDATION FAILED", output.getvalue())
                    else:
                        self.assertIn("OUTPUT VALIDATION FAILED", output.getvalue())
                    self.assertIn("warning: rendered diagnostic", output.getvalue())
                    self.assertIn("1 of 1 checks failed", output.getvalue())

    def test_split_rust_launcher_build_and_consumer_phases(self):
        for phase in self.dev.RustPhase:
            for launcher_rc in ((0, 17) if phase is self.dev.RustPhase.BUILD else (0,)):
                with self.subTest(phase=phase.value, launcher_rc=launcher_rc), \
                     tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    generated = root / "ui" / "src" / "generated"
                    generated.mkdir(parents=True)
                    content = "// This file was generated by ts-rs.\nexport type Binding = string;\n"
                    (generated / "Binding.ts").write_text(content)
                    launched = []
                    fixture = str(root / "downloaded-ci-fixture" / "phoenix_ide")
                    Path(fixture).parent.mkdir(parents=True)
                    Path(fixture).write_text("#!/bin/sh\nexit 0\n")
                    Path(fixture).chmod(0o755)

                    def launch(cmd, **kwargs):
                        launched.append(cmd)
                        env = kwargs["env"]
                        self.assertEqual(fixture, env["PHOENIX_BASH_LAUNCH_TEST_EXE"])
                        if phase is self.dev.RustPhase.TEST and cmd[:2] == ["cargo", "nextest"]:
                            self.assertEqual("1", env["PHOENIX_BASH_LAUNCH_TEST_REQUIRED"])
                        if "TS_RS_EXPORT_DIR" in env:
                            base = Path(env["TS_RS_EXPORT_DIR"])
                            (base.parents[2] / "Binding.ts").write_text(content)
                        artifact = launcher_artifact(root, fixture)
                        proc = mock.Mock(stdout=io.StringIO(json.dumps(artifact) + "\n"))
                        proc.wait.return_value = launcher_rc if cmd[:2] == ["cargo", "build"] else 0
                        return proc

                    split = self.dev.RustSplit(phase)
                    with mock.patch.object(self.dev, "ROOT", root):
                        split_cmds = self.dev._rust_split_commands(split, [], None, 1, [])
                    expected = list(split_cmds.values())
                    if phase is self.dev.RustPhase.BUILD:
                        expected.insert(0, ["cargo", "build", "-p", "phoenix_ide", "--bin", "phoenix_ide",
                                            "--message-format=json-render-diagnostics"])
                        if launcher_rc:
                            expected = expected[:1]
                    with mock.patch.dict(os.environ, {
                             "PHOENIX_CHECK_WORKERS": "1",
                             "PHOENIX_BASH_LAUNCH_TEST_EXE": fixture,
                             "PHOENIX_BASH_LAUNCH_TEST_REQUIRED": "0",
                         }), \
                         mock.patch.object(self.dev, "ROOT", root), \
                         mock.patch.object(self.dev, "_resolve_check_lanes", return_value=({"rust"}, {})), \
                         mock.patch.object(self.dev, "_make_reporter"), \
                         mock.patch.object(self.dev, "_classify_browser_env"), \
                         mock.patch.object(self.dev, "_write_rust_split_scope") as write_scope, \
                         mock.patch.object(self.dev, "_read_rust_split_scope", return_value=None), \
                         mock.patch.object(self.dev.subprocess, "Popen", side_effect=launch), \
                         mock.patch.object(self.dev.subprocess, "run", return_value=mock.Mock(returncode=0)), \
                         mock.patch.object(self.dev.sys, "stdout", io.StringIO()):
                        if launcher_rc:
                            with self.assertRaises(SystemExit) as raised:
                                self.dev.cmd_check(gate=False, lanes="rust", rust_split=split,
                                                   compiler_cache_setup=("none", {}))
                            self.assertEqual(1, raised.exception.code)
                        else:
                            self.dev.cmd_check(gate=False, lanes="rust", rust_split=split,
                                               compiler_cache_setup=("none", {}))
                        self.assertEqual("0", os.environ["PHOENIX_BASH_LAUNCH_TEST_REQUIRED"])
                    self.assertEqual(expected, launched)
                    self.assertEqual(phase is self.dev.RustPhase.BUILD, write_scope.called)

    def test_split_test_rejects_invalid_launcher_even_for_empty_partition(self):
        for case in ("unset", "empty", "missing", "non-executable", "relative", "directory"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                launcher = root / "phoenix_ide"
                if case == "non-executable":
                    launcher.write_text("#!/bin/sh\nexit 0\n")
                    launcher.chmod(0o644)
                elif case == "directory":
                    launcher.mkdir()
                value = "" if case == "empty" else "phoenix_ide" if case == "relative" else str(launcher)
                split = self.dev.RustSplit(self.dev.RustPhase.TEST, self.dev.RustPartition(2, 4))
                reporter = mock.Mock()
                output = io.StringIO()
                with mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": "1"}), \
                     mock.patch.object(self.dev, "ROOT", root), \
                     mock.patch.object(self.dev, "_resolve_check_lanes", return_value=({"rust"}, {})), \
                     mock.patch.object(self.dev, "_make_reporter", return_value=reporter), \
                     mock.patch.object(self.dev, "_classify_browser_env"), \
                     mock.patch.object(self.dev, "_read_rust_split_scope", return_value=["phoenix-core"]), \
                     mock.patch.object(self.dev, "_ts_export_crates", return_value={"phoenix_ide"}), \
                     mock.patch.object(self.dev.subprocess, "Popen") as launch, \
                     mock.patch.object(self.dev.subprocess, "run", return_value=mock.Mock(returncode=0)), \
                     mock.patch.object(self.dev.sys, "stdout", output):
                    if case == "unset":
                        os.environ.pop("PHOENIX_BASH_LAUNCH_TEST_EXE", None)
                    else:
                        os.environ["PHOENIX_BASH_LAUNCH_TEST_EXE"] = value
                    with self.assertRaises(SystemExit) as raised:
                        self.dev.cmd_check(gate=False, lanes="rust", rust_split=split,
                                           compiler_cache_setup=("none", {}))
                self.assertEqual(1, raised.exception.code)
                launch.assert_not_called()
                reporter.step_start.assert_called_once_with("rust", "native bash launcher validation")
                reporter.step_done.assert_called_once_with(
                    "rust", "native bash launcher validation", 1, mock.ANY,
                )
                self.assertIn("1 of 1 checks failed", output.getvalue())
                self.assertIn("PHOENIX_BASH_LAUNCH_TEST_EXE", output.getvalue())
                self.assertIn("existing absolute executable file", output.getvalue())

    def test_split_codegen_does_not_require_launcher(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            generated = root / "ui" / "src" / "generated"
            generated.mkdir(parents=True)
            content = "// This file was generated by ts-rs.\nexport type Binding = string;\n"
            (generated / "Binding.ts").write_text(content)
            launched = []

            def launch(cmd, **kwargs):
                launched.append(cmd)
                env = kwargs["env"]
                if "TS_RS_EXPORT_DIR" in env:
                    base = Path(env["TS_RS_EXPORT_DIR"])
                    (base.parents[2] / "Binding.ts").write_text(content)
                proc = mock.Mock(stdout=io.StringIO(""))
                proc.wait.return_value = 0
                return proc

            split = self.dev.RustSplit(self.dev.RustPhase.CODEGEN)
            with mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": "1"}), \
                 mock.patch.object(self.dev, "ROOT", root), \
                 mock.patch.object(self.dev, "_resolve_check_lanes", return_value=({"rust"}, {})), \
                 mock.patch.object(self.dev, "_make_reporter") as reporter, \
                 mock.patch.object(self.dev, "_classify_browser_env"), \
                 mock.patch.object(self.dev, "_read_rust_split_scope", return_value=None), \
                 mock.patch.object(self.dev.subprocess, "Popen", side_effect=launch), \
                 mock.patch.object(self.dev.subprocess, "run", return_value=mock.Mock(returncode=0)), \
                 mock.patch.object(self.dev.sys, "stdout", io.StringIO()) as output:
                os.environ.pop("PHOENIX_BASH_LAUNCH_TEST_EXE", None)
                self.dev.cmd_check(gate=False, lanes="rust", rust_split=split,
                                   compiler_cache_setup=("none", {}))
            self.assertEqual(["tar", "cargo"], [cmd[0] for cmd in launched])
            self.assertEqual(["extract test archive", "codegen", "codegen-stale"],
                             [call.args[1] for call in reporter.return_value.step_start.call_args_list])
            self.assertIn("All 3 checks passed", output.getvalue())

    def test_rust_graph_steps_match_lane_order(self):
        self.assertEqual(
            ["native bash launcher", "codegen export", "codegen-stale diff",
             "cargo test (compile)", "cargo test (run)"],
            self.dev._GRAPH_LANE_STEPS["rust"],
        )

    def test_vitest_lane_launch_enforces_budget_over_inherited_limits(self):
        inherited = {name: "99" for name in (
            "CARGO_BUILD_JOBS", "RAYON_NUM_THREADS", "RUST_TEST_THREADS",
        )}
        proc = mock.Mock(stdout=io.StringIO(""))
        proc.wait.return_value = 0
        with mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": "1"}), \
             mock.patch.object(self.dev, "_resolve_check_lanes", return_value=({"vitest"}, {})), \
             mock.patch.object(self.dev, "_make_reporter"), \
             mock.patch.object(self.dev, "ensure_ui_deps"), \
             mock.patch.object(self.dev, "node_env", return_value=inherited), \
             mock.patch.object(self.dev.subprocess, "Popen", return_value=proc) as launch, \
             mock.patch.object(self.dev.subprocess, "run", side_effect=AssertionError("unexpected subprocess")):
            self.dev.cmd_check(gate=False, lanes="vitest")
        launch.assert_called_once()
        self.assertEqual(["pnpm", "exec", "vitest", "run", "--maxWorkers", "1"],
                         launch.call_args.args[0])
        for name in inherited:
            self.assertEqual("1", launch.call_args.kwargs["env"][name])
        self.assertTrue(all(value == "99" for value in inherited.values()))

    def test_cli_rejects_invalid_workers_before_optional_bootstrap(self):
        with mock.patch.object(self.dev.sys, "argv", ["dev.py", "check", "--pretty"]), \
             mock.patch.dict(os.environ, {"PHOENIX_CHECK_WORKERS": "0"}), \
             mock.patch.object(self.dev, "_bootstrap_rich") as rich, \
             mock.patch.object(self.dev, "_bootstrap_dev_tracing") as tracing:
            with self.assertRaisesRegex(SystemExit, "PHOENIX_CHECK_WORKERS"):
                self.dev.main()
            rich.assert_not_called()
            tracing.assert_not_called()

    def test_clippy_check_disables_incremental_artifacts(self):
        root = Path("/tmp/phoenix-check")
        self.assertEqual(
            {
                "CARGO_INCREMENTAL": "0",
                "CARGO_TARGET_DIR": str(root / "target" / "clippy"),
            },
            self.dev._clippy_check_env(root),
        )

    def test_clippy_target_cleanup_removes_only_incremental_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            incremental = root / "target" / "clippy" / "debug" / "incremental"
            retained = root / "target" / "clippy" / "debug" / "deps" / "artifact"
            incremental.mkdir(parents=True)
            (incremental / "query-cache.bin").write_text("stale")
            retained.parent.mkdir(parents=True)
            retained.write_text("keep")

            self.dev._prepare_clippy_check_target(root)

            self.assertFalse(incremental.exists())
            self.assertEqual("keep", retained.read_text())

    def test_vitest_worker_count_uses_check_budget(self):
        with mock.patch.object(self.dev, "_check_cpu_budget", return_value=4):
            self.assertEqual(
                ["pnpm", "exec", "vitest", "run", "--maxWorkers", "4"],
                self.dev._vitest_check_command(),
            )

    def test_check_lane_runs_and_records_no_failure(self):
        lane = mock.Mock()
        results = []

        self.dev._run_check_lane("rust", lane, results, threading.Lock())

        lane.assert_called_once_with()
        self.assertEqual([], results)

    def test_check_lane_exception_records_failure(self):
        results = []

        self.dev._run_check_lane(
            "clippy",
            mock.Mock(side_effect=PermissionError("incremental target is protected")),
            results,
            threading.Lock(),
        )

        self.assertEqual(1, len(results))
        name, returncode, _elapsed, detail = results[0]
        self.assertEqual("clippy lane", name)
        self.assertEqual(1, returncode)
        self.assertIn("PermissionError: incremental target is protected", detail)

    def test_hung_lane_does_not_start_queued_lanes(self):
        first = mock.MagicMock()
        first.is_alive.return_value = True
        second = mock.MagicMock()

        stuck = self.dev._run_check_threads_sequentially([first, second], 0.01)

        first.start.assert_called_once_with()
        first.join.assert_called_once()
        self.assertTrue(all(call.kwargs["timeout"] <= 0.1
                            for call in first.join.call_args_list))
        second.start.assert_not_called()
        self.assertEqual([first], stuck)

    def test_check_threads_start_in_sequence(self):
        first = mock.MagicMock()
        first.is_alive.return_value = False
        second = mock.MagicMock()
        second.is_alive.return_value = False

        stuck = self.dev._run_check_threads_sequentially([first, second], 1)

        first.start.assert_called_once_with()
        first.join.assert_called_once_with(timeout=1)
        second.start.assert_called_once_with()
        second.join.assert_called_once_with(timeout=1)
        self.assertEqual([], stuck)

    def test_ci_lane_inventory_covers_every_devpy_lane_once(self):
        self.assertEqual([], self.dev._ci_lane_inventory_errors())

    def test_changed_path_planner_reuses_resolved_merge_base(self):
        with (
            mock.patch.object(self.dev, "_resolve_check_merge_base", return_value="abc123") as resolve,
            mock.patch.object(self.dev.subprocess, "run") as run,
        ):
            run.side_effect = [
                mock.Mock(returncode=0, stdout="crates/example.rs\n"),
                mock.Mock(returncode=0, stdout="tasks/new.md\n"),
            ]

            paths = self.dev._compute_changed_paths_vs_base()

        resolve.assert_called_once_with()
        self.assertEqual({"crates/example.rs", "tasks/new.md"}, paths)
        self.assertEqual(
            ["git", "diff", "--name-only", "--no-renames", "abc123"],
            run.call_args_list[0].args[0],
        )

    def test_explicit_unresolvable_base_does_not_fall_back(self):
        missing = mock.Mock(returncode=1, stdout="")
        with mock.patch.dict(
            os.environ,
            {"PHOENIX_CHECK_BASE": "missing-explicit-base"},
            clear=True,
        ), mock.patch.object(self.dev.subprocess, "run", return_value=missing) as run:
            self.assertIsNone(self.dev._resolve_check_merge_base())

        run.assert_called_once_with(
            ["git", "merge-base", "HEAD", "missing-explicit-base"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    def test_changed_paths_preserve_both_sides_of_renamed_dependency(self):
        old_path = "specs/adrs/113_coordinator-api-recipes-are-non-exhaustive.md"
        new_path = "specs/adrs/renamed.md"
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            source = repo / old_path
            source.parent.mkdir(parents=True)
            source.write_text("# ADR\n")
            subprocess.run(["git", "add", old_path], cwd=repo, check=True)
            subprocess.run(
                [
                    "git",
                    "-c",
                    "user.name=Phoenix Test",
                    "-c",
                    "user.email=phoenix@example.invalid",
                    "commit",
                    "-qm",
                    "fixture",
                ],
                cwd=repo,
                check=True,
            )
            subprocess.run(["git", "mv", old_path, new_path], cwd=repo, check=True)

            with mock.patch.dict(
                os.environ,
                {"PHOENIX_CHECK_BASE": "HEAD"},
                clear=True,
            ), mock.patch.object(self.dev, "ROOT", repo):
                paths = self.dev._compute_changed_paths_vs_base()

        self.assertEqual({old_path, new_path}, paths)
        self.assertIn("DEVTOOLS", self.dev._categorize_changed_paths(paths))

    def test_rust_checker_changes_run_structural_and_devpy_tests(self):
        for path in ("scripts/check_rust_test_timing.py", "scripts/check_manual_sql_transactions.py"):
            with self.subTest(path=path):
                cats = self.dev._categorize_changed_paths({path})
                self.assertIn("ASTGREP", cats)
                self.assertIn("DEVTOOLS", cats)
                with mock.patch.dict(
                    os.environ, GATED_CI_ENV, clear=False,
                ), mock.patch.object(
                    self.dev, "_changed_paths_vs_base", return_value={path},
                ):
                    active, _ = self.dev._gate_lanes()
                self.assertIn("ast-grep", active)
                self.assertIn("devpy", active)
                self.assertNotIn("spec-shape", active)

    def test_devpy_dependency_changes_run_devpy_tests(self):
        paths = (
            "tests/devpy/test_check_plan.py",
            "tests/integration/systemd_vm_harness.py",
            "scripts/systemd_deploy_helper.py",
            "skills/phoenix-adversarial-review/SKILL.md",
            ".agents/skills/phoenix-adversarial-review",
            "Cargo.toml",
            "rust-toolchain.toml",
            "crates/phoenix-core/src/unrelated.rs",
            "pyproject.toml",
            "uv.lock",
            "crates/phoenix-skills/src/builtin/phoenix-api/references/api-reference.md",
            "ui/src/components/QuestionPanel.tsx",
            "specs/api/requirements.md",
            "specs/builtin-skills/builtin-skills.allium",
            "specs/global-recall/requirements.md",
            "specs/compatibility/requirements.md",
        )
        for path in paths:
            with self.subTest(path=path), mock.patch.object(
                self.dev, "_changed_paths_vs_base", return_value={path},
            ):
                active, _ = self.dev._gate_lanes()
                self.assertIn("devpy", active)

    def test_irrelevant_spec_change_skips_devpy_tests(self):
        path = "specs/bash/executive.md"
        with mock.patch.dict(
            os.environ, GATED_CI_ENV, clear=False,
        ), mock.patch.object(self.dev, "_changed_paths_vs_base", return_value={path}):
            active, _ = self.dev._gate_lanes()

        self.assertIn("spec-shape", active)
        self.assertNotIn("devpy", active)

    def test_renamed_or_deleted_devpy_dependency_is_selected_by_path(self):
        for path in ("scripts/removed_helper.py", "tests/devpy/deleted_test.py"):
            with self.subTest(path=path), mock.patch.object(
                self.dev, "_changed_paths_vs_base", return_value={path},
            ):
                active, _ = self.dev._gate_lanes()
                self.assertIn("devpy", active)

    def test_spec_shape_excludes_subprocess_and_compiler_setup(self):
        shape = mock.Mock(errors=[], legacy_design_docs=[])
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), mock.patch.object(
            self.dev, "_changed_paths_vs_base", return_value={"specs/bash/executive.md"}
        ), mock.patch.object(self.dev, "_make_reporter"), mock.patch.object(
            self.dev, "_validate_spears_v2_shape", return_value=shape
        ) as validate_shape, mock.patch.object(
            self.dev, "_compiler_cache_subprocess_env",
            side_effect=AssertionError("compiler setup crossed spec-shape boundary"),
        ) as compiler_setup, mock.patch.object(
            self.dev.subprocess, "Popen",
            side_effect=AssertionError("subprocess crossed spec-shape boundary"),
        ) as popen, mock.patch.object(
            self.dev.subprocess, "run",
            side_effect=AssertionError("subprocess crossed spec-shape boundary"),
        ) as run, mock.patch("builtins.print"):
            active, skipped = self.dev._resolve_check_lanes(gate=True, lanes="spec-shape")
            self.assertEqual({"spec-shape"}, active)
            self.assertEqual({}, skipped)
            self.dev.cmd_check(gate=True, lanes="spec-shape")

        validate_shape.assert_called_once_with(ROOT / "specs")
        compiler_setup.assert_not_called()
        popen.assert_not_called()
        run.assert_not_called()

    def test_pr_433_task_only_rename_activates_only_task_group(self):
        paths = {"tasks/45002-p1-done--deterministic-message-scroll-state-machine.md"}
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value=paths):
            active, skipped = self.dev._resolve_check_lanes()

        self.assertEqual({"task"}, active)
        self.assertIn("rust", skipped)
        self.assertIn("clippy", skipped)
        self.assertIn("e2e", skipped)
        self.assertIn("ui-lint", skipped)
        self.assertEqual(
            {
                "clippy": False,
                "devpy": False,
                "e2e": False,
                "fast": True,
                "rust": False,
                "ui": False,
            },
            self.dev._ci_groups_for_active_lanes(active),
        )

    def test_fast_lane_alias_preserves_documented_commands(self):
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value={"tasks/example.md"}):
            active, skipped = self.dev._resolve_check_lanes(lanes="fast")

        self.assertEqual({"task"}, active)
        self.assertEqual({}, skipped)

    def test_explicit_filters_keep_spec_shape_and_devpy_independent(self):
        paths = {"specs/bash/executive.md", "tests/devpy/test_check_plan.py"}
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), mock.patch.object(
            self.dev, "_changed_paths_vs_base", return_value=paths,
        ):
            shape, _ = self.dev._resolve_check_lanes(lanes="spec-shape")
            devpy, _ = self.dev._resolve_check_lanes(lanes="devpy")

        self.assertEqual({"spec-shape"}, shape)
        self.assertEqual({"devpy"}, devpy)

    def test_full_plan_contains_both_split_lanes(self):
        active, skipped = self.dev._resolve_check_lanes(gate=False)

        self.assertEqual(set(), set(skipped))
        self.assertIn("spec-shape", active)
        self.assertIn("devpy", active)

    def test_unknown_local_base_runs_both_split_lanes(self):
        with mock.patch.dict(os.environ, {"CI": "", "PHOENIX_CHECK_ALL": ""}, clear=False), \
             mock.patch.object(self.dev, "_on_integration_base", return_value=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value=None):
            active, skipped = self.dev._resolve_check_lanes()

        self.assertEqual(set(), set(skipped))
        self.assertIn("spec-shape", active)
        self.assertIn("devpy", active)

    def test_unresolvable_ci_base_fails_closed(self):
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value=None), \
             self.assertRaises(SystemExit):
            self.dev._resolve_check_lanes()

    def test_ui_change_activates_ui_group_without_task_group(self):
        paths = {"ui/src/App.tsx"}
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value=paths):
            active, _skipped = self.dev._resolve_check_lanes()

        self.assertIn("tsc", active)
        self.assertIn("ui-lint", active)
        self.assertIn("vitest", active)
        self.assertIn("pkglock", active)
        self.assertIn("task", active)
        groups = self.dev._ci_groups_for_active_lanes(active)
        self.assertTrue(groups["ui"])
        self.assertTrue(groups["fast"])

    def test_devpy_change_runs_every_ci_group(self):
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value={"dev.py"}):
            active, skipped = self.dev._resolve_check_lanes()

        self.assertEqual(set(), set(skipped))
        self.assertEqual(self.dev._all_lanes(), active)
        self.assertTrue(all(self.dev._ci_groups_for_active_lanes(active).values()))

    def test_workflow_change_runs_every_ci_group(self):
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value={".github/workflows/ci.yml"}):
            active, skipped = self.dev._resolve_check_lanes()

        self.assertEqual(set(), set(skipped))
        self.assertEqual(self.dev._all_lanes(), active)
        self.assertTrue(all(self.dev._ci_groups_for_active_lanes(active).values()))

    def test_workflow_check_commands_match_ci_lane_groups(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        for group, lanes in self.dev._CI_LANE_GROUPS.items():
            if group == "fast":
                expected = "./dev.py check --lanes task"
            else:
                expected = "./dev.py check --lanes " + ",".join(
                    lane for lane in workflow_order(group) if lane in lanes
                )
            self.assertIn(expected, workflow)

    def test_split_lanes_have_independent_graph_steps(self):
        self.assertEqual(
            ["spEARS v2 artifact-shape validation"],
            self.dev._GRAPH_LANE_STEPS["spec-shape"],
        )
        self.assertEqual(
            ["python unittest discover tests/devpy"],
            self.dev._GRAPH_LANE_STEPS["devpy"],
        )

    def test_workflow_failure_jobs_depend_on_devpy_job(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        self.assertIn("check-devpy:", workflow)
        self.assertEqual(2, workflow.count("check-e2e, check-devpy, check-ui"))

    def test_devpy_ci_job_installs_ast_grep_before_running_tests(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        devpy_job = workflow.split("  check-devpy:\n", 1)[1].split("  check-ui:\n", 1)[0]

        self.assertIn("npm i -g @ast-grep/cli", devpy_job)
        self.assertLess(
            devpy_job.index("npm i -g @ast-grep/cli"),
            devpy_job.index("./dev.py check --lanes devpy"),
        )

    def test_linux_musl_smoke_runs_once_in_rust_ci_group(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        self.assertEqual(1, workflow.count("cargo check --target x86_64-unknown-linux-musl"))
        self.assertIn("sudo apt-get install -y musl-tools", workflow)
        self.assertIn("target: x86_64-unknown-linux-musl", workflow)
        graph_steps = self.dev._GRAPH_LANE_STEPS["rust"]
        self.assertFalse(any("musl" in step for step in graph_steps))


    def test_workflow_check_plan_fails_closed(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        self.assertIn(
            "./dev.py check-plan --format github-env > check-plan.env",
            workflow,
        )
        self.assertNotIn("check-plan --format github-env |", workflow)


def workflow_order(group):
    return {
        "rust": ["rust", "cargo-fmt"],
        "clippy": ["clippy"],
        "e2e": ["e2e"],
        "devpy": ["devpy"],
        "ui": ["tsc", "ui-lint", "vitest", "ast-grep", "allium", "spec-shape", "spec-anchors", "pkglock"],
    }[group]


class RustSplitTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()

    def test_partition_parse_accepts_only_in_range_k_of_n(self):
        self.assertEqual("count:2/4", self.dev.RustPartition.parse("2/4").nextest_arg())
        for raw in ("0/4", "5/4", "2", "a/b", "1/0", " 1/4", "-1/4"):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                self.dev.RustPartition.parse(raw)

    def test_partition_only_applies_to_test_phase(self):
        partition = self.dev.RustPartition(1, 4)
        for phase in (self.dev.RustPhase.BUILD, self.dev.RustPhase.CODEGEN):
            with self.subTest(phase=phase), self.assertRaises(ValueError):
                self.dev.RustSplit(phase, partition)
        self.dev.RustSplit(self.dev.RustPhase.TEST, partition)

    def test_build_phase_archives_the_unsplit_compile_scope(self):
        cmds = self.dev._rust_split_commands(
            self.dev.RustSplit(self.dev.RustPhase.BUILD), ["-p", "a", "-p", "exports"],
            ["a"], 3, [],
        )
        self.assertEqual(["cargo test compile"], list(cmds))
        self.assertEqual(
            ["cargo", "nextest", "archive", "-p", "a", "-p", "exports",
             "--archive-file", str(self.dev.RUST_SPLIT_ARCHIVE)],
            cmds["cargo test compile"],
        )

    def test_test_phase_runs_scoped_partition_from_archive(self):
        profile = ["--config-file", "profile.toml", "--profile", "phoenix-cpu"]
        cmds = self.dev._rust_split_commands(
            self.dev.RustSplit(self.dev.RustPhase.TEST, self.dev.RustPartition(2, 4)),
            ["-p", "a", "-p", "exports"], ["a", "b"], 3, profile,
        )
        self.assertEqual(["extract test archive", "cargo test"], list(cmds))
        self.assertEqual(["tar", "--zstd", "-xf", str(self.dev.RUST_SPLIT_ARCHIVE),
                          "-C", str(self.dev.ROOT)], cmds["extract test archive"])
        test = cmds["cargo test"]
        self.assertEqual(["cargo", "nextest", *profile, "run"], test[:7])
        self.assertNotIn("-p", test)
        self.assertIn("--binaries-metadata", test)
        self.assertIn("--cargo-metadata", test)
        self.assertEqual(
            "(not test(/export_bindings/)) and (package(=a) or package(=b))",
            test[test.index("-E") + 1],
        )
        self.assertEqual("3", test[test.index("--test-threads") + 1])
        self.assertEqual("count:2/4", test[test.index("--partition") + 1])
        self.assertIn("--no-tests=warn", test)

    def test_unpartitioned_full_workspace_test_phase_requires_tests(self):
        test = self.dev._rust_split_commands(
            self.dev.RustSplit(self.dev.RustPhase.TEST), [], None, 3, [],
        )["cargo test"]
        self.assertEqual("not test(/export_bindings/)", test[test.index("-E") + 1])
        self.assertNotIn("--partition", test)
        self.assertNotIn("--no-tests=warn", test)

    def test_codegen_phase_runs_only_export_tests_from_archive(self):
        cmds = self.dev._rust_split_commands(
            self.dev.RustSplit(self.dev.RustPhase.CODEGEN), [], ["a"], 3, [],
        )
        self.assertEqual(["extract test archive", "codegen"], list(cmds))
        codegen = cmds["codegen"]
        self.assertEqual("test(/export_bindings/)", codegen[codegen.index("-E") + 1])
        self.assertIn("--binaries-metadata", codegen)

    def test_scope_round_trips_and_missing_scope_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "split" / "scope.json"
            with self.assertRaises(SystemExit):
                self.dev._read_rust_split_scope(path)
            for scope in (None, ["phoenix-db", "phoenix_ide"]):
                with self.subTest(scope=scope):
                    self.dev._write_rust_split_scope(scope, path)
                    self.assertEqual(scope, self.dev._read_rust_split_scope(path))
            path.write_text('{"rust_scope": []}')
            with self.assertRaises(SystemExit):
                self.dev._read_rust_split_scope(path)

    def test_workflow_runs_every_split_phase_and_partition(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        self.assertIn("--rust-phase build", workflow)
        self.assertIn("phase: codegen", workflow)
        for index in range(1, 5):
            self.assertIn(f"partition: {index}/4", workflow)


class CargoTestTimeoutTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()

    def _with_file(self, values):
        def fake_load(env, filename=".phoenix-ide.env"):
            if values is None:
                return None
            env.update(values)
            return "/host/.phoenix-ide.env"
        return mock.patch.object(self.dev, "_load_env_file", side_effect=fake_load)

    def test_default_without_override(self):
        with self._with_file(None):
            self.assertEqual(self.dev._cargo_test_timeout_secs({}), (900, None))

    def test_host_env_file_sets_budget(self):
        with self._with_file({self.dev.CARGO_TEST_TIMEOUT_ENV: "2700"}):
            self.assertEqual(
                self.dev._cargo_test_timeout_secs({}), (2700, "/host/.phoenix-ide.env"))

    def test_process_environment_wins_over_host_file(self):
        with self._with_file({self.dev.CARGO_TEST_TIMEOUT_ENV: "2700"}):
            self.assertEqual(
                self.dev._cargo_test_timeout_secs({self.dev.CARGO_TEST_TIMEOUT_ENV: "1200"}),
                (1200, "environment"))

    def test_empty_value_means_default(self):
        with self._with_file(None):
            self.assertEqual(
                self.dev._cargo_test_timeout_secs({self.dev.CARGO_TEST_TIMEOUT_ENV: " "}),
                (900, None))

    def test_task_only_check_ignores_invalid_rust_budget(self):
        with mock.patch.object(self.dev, "_resolve_check_lanes", return_value=({"task"}, {})), \
             mock.patch.object(self.dev, "_cargo_test_timeout_secs", side_effect=AssertionError("rust-only setting read")), \
             mock.patch.object(self.dev, "_run_check_threads_sequentially", return_value=[]), \
             mock.patch.object(self.dev, "_make_reporter") as reporter:
            self.dev.cmd_check(gate=False, lanes="task")
            self.assertFalse(reporter.return_value.lane_failed.called)

    def test_platform_max_budget_is_accepted(self):
        maximum = self.dev.MAX_CARGO_TEST_TIMEOUT_SECS
        with self._with_file(None):
            self.assertEqual(
                self.dev._cargo_test_timeout_secs({self.dev.CARGO_TEST_TIMEOUT_ENV: str(maximum)}),
                (maximum, "environment"),
            )

    def test_invalid_values_fail_loud(self):
        for raw in ("abc", "0", "-5", "1.5", str(self.dev.MAX_CARGO_TEST_TIMEOUT_SECS + 1), "9999999999"):
            with self.subTest(raw=raw), self._with_file(None):
                with self.assertRaisesRegex(SystemExit, "positive integer"):
                    self.dev._cargo_test_timeout_secs({self.dev.CARGO_TEST_TIMEOUT_ENV: raw})


if __name__ == "__main__":
    unittest.main()
