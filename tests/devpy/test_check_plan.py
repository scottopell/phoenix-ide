import ast
import io
import importlib.util
import os
import sys
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
    with mock.patch.dict(sys.modules, {spec.name: module}):
        spec.loader.exec_module(module)
    return module


class DevpyLoaderLifecycleTests(unittest.TestCase):
    def test_load_devpy_registers_exact_module_during_real_exec_and_restores_registry(self):
        name = "devpy_under_test"
        for had_prior in (False, True):
            for inject_failure in (False, True):
                with self.subTest(had_prior=had_prior, inject_failure=inject_failure):
                    spec = importlib.util.spec_from_file_location(name, ROOT / "dev.py")
                    assert spec is not None and spec.loader is not None
                    real_exec = spec.loader.exec_module
                    prior = type(sys)("previous_devpy_fixture")
                    observed = []
                    injected = RuntimeError("injected loader failure after real module execution")
                    def wrapped_exec(module):
                        self.assertIs(sys.modules[name], module)
                        self.assertIsNot(module, prior)
                        observed.append(module)
                        real_exec(module)
                        self.assertTrue(hasattr(module, "CheckWorkProfile"))
                        self.assertIs(sys.modules[name], module)
                        if inject_failure:
                            raise injected
                    with mock.patch.dict(sys.modules):
                        sys.modules.pop(name, None)
                        if had_prior:
                            sys.modules[name] = prior
                        with mock.patch.object(importlib.util, "spec_from_file_location", return_value=spec), \
                             mock.patch.object(spec.loader, "exec_module", side_effect=wrapped_exec):
                            if inject_failure:
                                with self.assertRaises(RuntimeError) as caught:
                                    load_devpy()
                                self.assertIs(caught.exception, injected)
                            else:
                                loaded = load_devpy()
                                self.assertIs(loaded, observed[0])
                        self.assertEqual(len(observed), 1)
                        if had_prior:
                            self.assertIs(sys.modules[name], prior)
                        else:
                            self.assertNotIn(name, sys.modules)


class CheckPlanTests(unittest.TestCase):
    def setUp(self):
        env_patch = mock.patch.dict(os.environ)
        env_patch.start()
        self.addCleanup(env_patch.stop)
        os.environ.pop("PHOENIX_CHECK_WORKERS", None)
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
        first.join.assert_called_once_with(timeout=0.01)
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
            ["git", "diff", "--name-only", "abc123"],
            run.call_args_list[0].args[0],
        )

    def test_rust_checker_changes_run_structural_and_self_tests(self):
        for path in ("scripts/check_rust_test_timing.py", "scripts/check_manual_sql_transactions.py"):
            with self.subTest(path=path):
                cats = self.dev._categorize_changed_paths({path})
                self.assertIn("ASTGREP", cats)
                self.assertIn("SPECS", cats)
                with mock.patch.object(
                    self.dev, "_changed_paths_vs_base", return_value={path},
                ):
                    active, _ = self.dev._gate_lanes()
                self.assertIn("ast-grep", active)
                self.assertIn("spec-shape", active)

    def test_goal_qualification_helper_only_change_runs_spec_shape(self):
        changed = {"scripts/goal_qualification.py"}
        self.assertEqual(self.dev._categorize_changed_paths(changed), {"SPECS"})
        with mock.patch.object(self.dev, "_changed_paths_vs_base", return_value=changed):
            active, _ = self.dev._gate_lanes()
        self.assertIn("spec-shape", active)
        self.assertNotIn("rust", active)
        self.assertNotIn("vitest", active)
        self.assertNotIn("e2e", active)

    def test_check_profile_scripts_run_their_devpy_unit_tests(self):
        for path in (
            "scripts/check_profile_command.py",
            "scripts/check_profile_report.py",
            "scripts/python_unittest_profile.py",
        ):
            with self.subTest(path=path):
                cats = self.dev._categorize_changed_paths({path})
                self.assertIn("SPECS", cats)
                with mock.patch.object(
                    self.dev, "_changed_paths_vs_base", return_value={path},
                ):
                    active, _ = self.dev._gate_lanes()
                self.assertIn("spec-shape", active)

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
            {"clippy": False, "e2e": False, "fast": True, "rust": False, "ui": False},
            self.dev._ci_groups_for_active_lanes(active),
        )

    def test_fast_lane_alias_preserves_documented_commands(self):
        with mock.patch.dict(os.environ, GATED_CI_ENV, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value={"tasks/example.md"}):
            active, skipped = self.dev._resolve_check_lanes(lanes="fast")

        self.assertEqual({"task"}, active)
        self.assertEqual({}, skipped)

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
        "ui": ["tsc", "ui-lint", "vitest", "ast-grep", "allium", "spec-shape", "spec-anchors", "pkglock"],
    }[group]


class GoalQualificationCommandTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()

    def test_focused_only_parser_dispatch_and_no_compile_control_flow(self):
        source = (ROOT / "dev.py").read_text()
        self.assertIn('"--goal-focused-filter"', source)
        self.assertIn('"--goal-step-deadline-ns"', source)
        self.assertIn('goal_focused_filter=args.goal_focused_filter', source)
        check = next(n for n in ast.parse(source).body if isinstance(n, ast.FunctionDef) and n.name == "cmd_check")
        branch = next(n for n in check.body if isinstance(n, ast.If) and ast.unparse(n.test) == "focused_only")
        body = ast.unparse(branch)
        self.assertIn("run_step('goal focused'", body)
        self.assertNotIn("compile_cmd", body)
        self.assertNotIn("codegen_cmd", body)
        self.assertNotIn("_compiler_cache_subprocess_env", body)
        self.assertIsInstance(branch.body[-1], ast.Return)

    def test_foreign_profile_args_remain_rejected_without_blanket_profile_ban(self):
        with mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "PHOENIX_GOAL_REUSE_DIR": "/job/private"}, clear=True), \
             mock.patch.object(self.dev, "_goal_active_execution", return_value={"kind": "fixture"}), \
             mock.patch.object(self.dev, "_goal_helper") as helper:
            helper.return_value.validate_execution.return_value = ["--config-file", "/owner/config", "--profile", "phoenix-cpu"]
            with self.assertRaisesRegex(SystemExit, "foreign profile"):
                self.dev._goal_qualification_commands(True, [], [], [], 2, ["--profile", "foreign"])

    def test_closed_or_absent_in_process_profile_owner_cannot_authorize(self):
        import types
        for profile in (None, types.SimpleNamespace(artifacts_finalized=True, finalized_cpu=None),
                        types.SimpleNamespace(artifacts_finalized=False, finalized_cpu={})):
            with mock.patch.object(self.dev, "_CHECK_PROFILE", profile):
                with self.assertRaisesRegex(ValueError, "open in-process"):
                    self.dev._goal_active_execution("focused")

    def test_non_opt_in_keeps_existing_commands_and_cpu_profile(self):
        for opt_in in (None, "", "false"):
            env = {} if opt_in is None else {"PHOENIX_GOAL_REUSE_DIR": opt_in}
            with self.subTest(opt_in=opt_in), mock.patch.dict(os.environ, env, clear=True):
                self.assertIsNone(self.dev._goal_qualification_commands(
                    True, ["-p", "phoenix-db"], ["-p", "phoenix-db"], [], 3,
                    ["--profile", "cpu-attribution"]))
        _, broad, _ = self.dev._rust_check_commands(
            True, [], [], [], 3, ["--profile", "cpu-attribution"])
        self.assertEqual(broad, [
            "cargo", "nextest", "--profile", "cpu-attribution", "run",
            "-E", "not test(/export_bindings/)", "--test-threads", "3",
        ])

    def test_opt_in_replaces_compile_and_reuses_for_both_remaining_phases(self):
        with mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "PHOENIX_GOAL_REUSE_DIR": "/job/private"}, clear=True):
            compile_cmd, broad_cmd, codegen_cmd = self.dev._goal_qualification_commands(
                True, [], [], [], 3, [])
        helper = [sys.executable, str(ROOT / "scripts/goal_qualification.py")]
        self.assertEqual(compile_cmd, [*helper, "capture"])
        self.assertEqual(broad_cmd, [*helper, "broad", "--test-threads", "3"])
        self.assertEqual(codegen_cmd, [*helper, "codegen", "--test-threads", "3"])

    def test_invalid_opt_in_scope_or_profile_fails_before_subprocess(self):
        cases = [
            (False, [], [], [], []),
            (True, ["-p", "phoenix-db"], [], [], []),
            (True, [], ["-p", "phoenix-db"], [], []),
            (True, [], [], ["-p", "phoenix-db"], []),
        ]
        with mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "PHOENIX_GOAL_REUSE_DIR": "/job/private"}, clear=True), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            for has_nextest, compile_p, test_p, codegen_p, profile in cases:
                with self.subTest(case=(has_nextest, compile_p, test_p, codegen_p, profile)):
                    with self.assertRaisesRegex(SystemExit, "full-workspace nextest"):
                        self.dev._goal_qualification_commands(has_nextest, compile_p, test_p, codegen_p, 3, profile)
        run.assert_not_called()

    def test_opt_in_source_preserves_timeout_group_cache_and_codegen_scratch(self):
        source = (ROOT / "dev.py").read_text()
        self.assertIn('compile_rc = run_step("cargo test compile", compile_cmd, env_extra=reuse_env, goal_phase=goal_phase)', source)
        self.assertIn('if compile_rc != 0 and goal_reuse_commands is not None:\n            return', source)
        self.assertIn('reuse_env = compiler_cache_env if goal_reuse_commands is not None else None', source)
        self.assertIn('env_extra={**(reuse_env or {}), "TS_RS_EXPORT_DIR": str(codegen_export_base)}', source)
        self.assertIn('run_step("cargo test", test_cmd, env_extra=reuse_env, timeout=CARGO_TEST_TIMEOUT, goal_phase=goal_phase)', source)
        self.assertIn('text=output_log is None, env=env, start_new_session=True', source)
        self.assertIn('bufsize=1 if output_log is None else 0', source)
        self.assertIn('def run_step(name, cmd, cwd=ROOT, env_extra=None, *, timeout=CHECK_TIMEOUT, goal_phase=None):', source)
        check = next(node for node in ast.parse(source).body
                     if isinstance(node, ast.FunctionDef) and node.name == "cmd_check")
        timeout = next(node.value for node in check.body
                       if isinstance(node, ast.Assign)
                       and any(isinstance(target, ast.Name) and target.id == "CHECK_TIMEOUT"
                               for target in node.targets))
        self.assertEqual(ast.literal_eval(timeout), 600)
        run_step = next(node for node in check.body
                        if isinstance(node, ast.FunctionDef) and node.name == "run_step")
        self.assertEqual(run_step.args.kwonlyargs[0].arg, "timeout")
        self.assertEqual(ast.dump(run_step.args.kw_defaults[0]),
                         ast.dump(ast.Name(id="CHECK_TIMEOUT", ctx=ast.Load())))


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
