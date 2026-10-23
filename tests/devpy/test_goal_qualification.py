"""Authored, subprocess-free fixtures for same-job qualification custody."""

import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_helper():
    spec = importlib.util.spec_from_file_location("goal_qualification", ROOT / "scripts/goal_qualification.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class GoalQualificationTests(unittest.TestCase):
    def setUp(self):
        self.helper = load_helper()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.directory = self.root / "goal-qualification-123-1-check-rust"
        env = {"GITHUB_ACTIONS": "true", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1",
               "GITHUB_JOB": "check-rust", "GITHUB_SHA": "head", "RUNNER_TEMP": str(self.root),
               "PHOENIX_GOAL_REUSE_DIR": str(self.directory)}
        patch = mock.patch.dict(os.environ, env, clear=True)
        patch.start()
        self.addCleanup(patch.stop)
        patch = mock.patch.object(self.helper, "ROOT", self.root)
        patch.start()
        self.addCleanup(patch.stop)
        self.real_identity = self.helper.identity
        patch = mock.patch.object(self.helper, "identity", return_value={"head": "head"})
        self.identity = patch.start()
        self.addCleanup(patch.stop)
        target = self.root / "target"
        target.mkdir()
        packages, binaries = [], {}
        for name in self.helper.PACKAGES:
            package = f"path+file:///{name}#0.1.0"
            packages.append({"id": package, "name": name})
            binary = target / name
            binary.write_bytes(b"test fixture, never executed")
            binary.chmod(0o700)
            binaries[name] = {"binary-id": name, "binary-name": name, "package-id": package,
                              "kind": "lib", "binary-path": str(binary), "build-platform": "target"}
        self.cargo = {"workspace_root": str(self.root), "target_directory": str(target),
                      "packages": packages, "workspace_members": [p["id"] for p in packages],
                      "resolve": {"nodes": [{"id": p["id"], "dependencies": []} for p in packages]}}
        self.binaries = {"rust-build-meta": {"target-directory": str(target),
                                            "base-output-directories": ["debug"]},
                         "rust-binaries": binaries}

    def write_capture(self, command, directory, name):
        expected = self.helper.METADATA_COMMAND if name == "cargo" else self.helper.CAPTURE_COMMAND
        self.assertEqual(command, expected)
        payload = self.cargo if name == "cargo" else self.binaries
        (directory / f"{name}.json").write_text(json.dumps(payload))
        (directory / f"{name}.stderr.log").write_text("full stderr fixture\n" * 250)

    def capture(self):
        with mock.patch.object(self.helper, "capture_json", side_effect=self.write_capture) as capture:
            self.helper.capture()
        return capture

    def test_same_size_restored_mtime_binary_mutation_is_rejected(self):
        self.capture()
        path = Path(next(iter(self.binaries["rust-binaries"].values()))["binary-path"])
        before = path.stat()
        data = path.read_bytes()
        path.write_bytes(bytes([data[0] ^ 1]) + data[1:])
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        self.assertEqual(path.stat().st_size, before.st_size)
        self.assertEqual(path.stat().st_mtime_ns, before.st_mtime_ns)
        with self.assertRaisesRegex(ValueError, "stale, changed"):
            self.helper.reuse_command("broad", 2)

    def test_default_receipt_cannot_be_relabelled_as_phoenix_cpu(self):
        self.capture()
        with mock.patch.object(self.helper, "validate_execution", return_value=[]):
            with self.assertRaisesRegex(ValueError, "masquerade"):
                self.helper.reuse_command("broad", 2, execution={"kind": "phoenix-cpu"})

    def test_profile_config_byte_mutation_and_same_size_executable_mutation(self):
        import types
        interpreter = self.root / "python"
        wrapper = self.root / "scripts/check_profile_command.py"
        wrapper.parent.mkdir()
        interpreter.write_bytes(b"python-fixture")
        interpreter.chmod(0o700)
        wrapper.write_bytes(b"wrapper-fixture")
        artifact = self.root / "profile-check"
        artifact.mkdir()
        (artifact / ".phoenix-profile-claim").write_text(f"pid={os.getpid()}\n")
        def render(output, *, interpreter, wrapper):
            return json.dumps([interpreter, str(wrapper), str(output)])
        config = artifact / "nextest-profile.toml"
        config.write_text(render(artifact / "rust-tests", interpreter=str(interpreter), wrapper=wrapper))
        profile = types.SimpleNamespace(artifact_dir=artifact, run_id="check", artifacts_finalized=False, finalized_cpu=None)
        with mock.patch.object(self.helper.sys, "executable", str(interpreter)), \
             mock.patch.object(self.helper, "profile_renderer", return_value=(render, "renderer")):
            binding = self.helper.execution_binding(profile, "check", render)
            self.assertEqual(self.helper.validate_execution(binding)[-1], "phoenix-cpu")
            original = config.read_bytes()
            config.write_bytes(original + b" ")
            with self.assertRaisesRegex(ValueError, "config or claim"):
                self.helper.validate_execution(binding)
            config.write_bytes(original)
            stat = interpreter.stat()
            interpreter.write_bytes(b"Python-fixture")
            os.utime(interpreter, ns=(stat.st_atime_ns, stat.st_mtime_ns))
            with self.assertRaisesRegex(ValueError, "executable bytes"):
                self.helper.validate_execution(binding)
            profile.finalized_cpu = {}
            with self.assertRaisesRegex(ValueError, "closed"):
                self.helper.execution_binding(profile, "focused", render)

    def test_phase_b_uses_fresh_binding_and_does_not_rewrite_ready(self):
        self.capture()
        ready = (self.directory / "ready.json").read_bytes()
        producer = {"kind": "phoenix-cpu", "phase": "check", "stable": {"template": "same"}, "artifact_dir": "/check"}
        receipt = json.loads(ready)
        receipt["execution"] = producer
        receipt["commands"] = {"metadata": self.helper.METADATA_COMMAND,
                               "capture": ["cargo", "nextest", "--config-file", "/check/config", "--profile", "phoenix-cpu", *self.helper.CAPTURE_COMMAND[2:]]}
        (self.directory / "ready.json").write_text(json.dumps(receipt))
        immutable = (self.directory / "ready.json").read_bytes()
        focused = {**producer, "phase": "focused", "artifact_dir": "/focused", "producer_receipt_sha256": self.helper.digest(immutable)}
        def args(binding):
            return ["--config-file", binding["artifact_dir"] + "/config", "--profile", "phoenix-cpu"]
        with mock.patch.object(self.helper, "validate_execution", side_effect=args):
            command = self.helper.reuse_command("focused", 2, "test(/goal/)", focused)
            self.assertEqual(command[2:6], ["--config-file", "/focused/config", "--profile", "phoenix-cpu"])
            self.assertEqual((self.directory / "ready.json").read_bytes(), immutable)
            for bad in ({**focused, "artifact_dir": "/check"}, {**focused, "stable": {"template": "other"}}):
                with self.assertRaises(ValueError):
                    self.helper.reuse_command("focused", 2, "test(/goal/)", bad)

    def test_capture_publishes_ready_last_after_one_metadata_and_one_build(self):
        calls = self.capture().call_args_list
        self.assertEqual([c.args[0] for c in calls], [
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            ["cargo", "nextest", "list", "--locked", "--workspace", "--list-type", "binaries-only",
             "--message-format", "json"],
        ])
        self.assertEqual(self.identity.call_count, 2)
        receipt = json.loads((self.directory / "ready.json").read_text())
        self.assertEqual(receipt["identity"], {"head": "head"})
        self.assertEqual(len(receipt["artifacts"]["binaries"]), 3)
        self.assertFalse((self.directory / "ready.tmp").exists())
        self.assertEqual(len((self.directory / "binaries.stderr.log").read_text().splitlines()), 250)

    def test_capture_tees_full_stderr_for_lock_timer_without_mixing_json_or_new_session(self):
        self.directory.mkdir()
        diagnostic = b"Blocking waiting for file lock on build directory\n" + b"warning\n" * 300
        inherited_stderr = io.BytesIO()
        process = mock.MagicMock()
        process.stderr = io.BytesIO(diagnostic)
        process.wait.return_value = 0
        process.__enter__.return_value = process

        def popen(command, **kwargs):
            self.assertEqual(command, self.helper.METADATA_COMMAND)
            self.assertEqual(set(kwargs), {"cwd", "stdout", "stderr"})
            self.assertEqual(kwargs["stderr"], subprocess.PIPE)
            kwargs["stdout"].write(b'{"fixture":true}')
            return process

        with mock.patch.object(self.helper.subprocess, "Popen", side_effect=popen), \
             mock.patch.object(self.helper.sys, "stderr", buffer=inherited_stderr):
            self.helper.capture_json(self.helper.METADATA_COMMAND, self.directory, "cargo")
        self.assertEqual(json.loads((self.directory / "cargo.json").read_text()), {"fixture": True})
        self.assertEqual((self.directory / "cargo.stderr.log").read_bytes(), diagnostic)
        self.assertEqual(inherited_stderr.getvalue(), diagnostic)
        process.wait.assert_called_once_with()

    def test_nonzero_capture_retains_teed_diagnostics_and_does_not_publish_ready(self):
        self.directory.mkdir()
        process = mock.MagicMock()
        process.__enter__.return_value = process
        process.stderr = io.BytesIO(b"capture failed fixture\n")
        process.wait.return_value = 1
        inherited_stderr = io.BytesIO()
        with mock.patch.object(self.helper.subprocess, "Popen", return_value=process), \
             mock.patch.object(self.helper.sys, "stderr", buffer=inherited_stderr):
            with self.assertRaisesRegex(ValueError, "capture failed \\(1\\)"):
                self.helper.capture_json(self.helper.CAPTURE_COMMAND, self.directory, "binaries")
        self.assertEqual((self.directory / "binaries.stderr.log").read_bytes(), inherited_stderr.getvalue())
        self.assertEqual(inherited_stderr.getvalue(), b"capture failed fixture\n")
        self.assertFalse((self.directory / "ready.json").exists())

    def test_capture_failure_or_identity_drift_never_publishes_readiness(self):
        for failure in ("command", "version", "identity", "partial"):
            with self.subTest(failure=failure):
                if self.directory.exists():
                    shutil.rmtree(self.directory)
                self.identity.side_effect = [{"head": "head"}, {"head": "changed"}] if failure == "identity" else None
                if failure == "version":
                    self.identity.side_effect = subprocess.CalledProcessError(1, ["cargo", "-Vv"])
                side_effect = OSError("fixture failure") if failure == "command" else self.write_capture
                if failure == "partial":
                    self.cargo["resolve"] = None
                with mock.patch.object(self.helper, "capture_json", side_effect=side_effect):
                    with self.assertRaises((ValueError, OSError, TypeError, subprocess.CalledProcessError)):
                        self.helper.capture()
                self.assertFalse((self.directory / "ready.json").exists())

    def test_existing_or_wrong_job_directory_is_not_reused(self):
        self.capture()
        with self.assertRaises(FileExistsError), mock.patch.object(self.helper, "capture_json") as capture:
            self.helper.capture()
        capture.assert_not_called()
        for key, value in (("GITHUB_RUN_ATTEMPT", "2"), ("GITHUB_JOB", "another-job"),
                           ("GITHUB_ACTIONS", "false"), ("RUNNER_TEMP", str(self.root / "other")),
                           ("NEXTEST_PROFILE", "cpu-attribution")):
            with self.subTest(key=key), mock.patch.dict(os.environ, {key: value}):
                with self.assertRaises(ValueError):
                    self.helper.job_directory()

    def test_all_consumers_use_only_metadata_and_fixed_phase_filters(self):
        self.capture()
        for mode, expression in (("codegen", "test(/export_bindings/)"),
                                 ("broad", "not test(/export_bindings/)")):
            command = self.helper.reuse_command(mode, 3)
            self.assertEqual(command[:3], ["cargo", "nextest", "run"])
            self.assertEqual(command[command.index("-E") + 1], expression)
            self.assertEqual(command[command.index("--test-threads") + 1], "3")
            self.assertEqual(command[command.index("--no-tests") + 1], "fail")
            if mode == "broad":
                self.assertEqual(command[-2:], ["--failure-output", "immediate"])
            for forbidden in ("--workspace", "-p", "--no-run", "--ignored", "--ignore-default-filter", "--profile"):
                self.assertNotIn(forbidden, command)
            for option, name in (("--cargo-metadata", "cargo.json"), ("--binaries-metadata", "binaries.json")):
                self.assertEqual(command[command.index(option) + 1], str(self.directory / name))
        expression = "test(/goal/) or test(/::historical$/)"
        command = self.helper.reuse_command("focused", 2, expression)
        self.assertEqual(command[command.index("-E") + 1],
                         "(package(=phoenix-core) | package(=phoenix-db) | package(=phoenix_ide)) & (" + expression + ")")
        self.assertEqual(command[-7:], ["--no-fail-fast", "--failure-output", "immediate",
                                       "--status-level", "all", "--final-status-level", "all"])
        with self.assertRaises(ValueError):
            self.helper.reuse_command("focused", 3, expression)

    def test_missing_stale_partial_changed_or_missing_binary_evidence_cannot_execute(self):
        with self.assertRaises(FileNotFoundError):
            self.helper.reuse_command("broad", 2)
        self.capture()
        with mock.patch.object(self.helper, "identity", return_value={"head": "different"}):
            with self.assertRaises(ValueError):
                self.helper.reuse_command("broad", 2)
        path = self.directory / "cargo.json"
        payload = path.read_bytes()
        path.write_bytes(b"{")
        with self.assertRaises(ValueError):
            self.helper.reuse_command("broad", 2)
        path.write_bytes(payload + b" ")
        with self.assertRaises(ValueError):
            self.helper.reuse_command("broad", 2)
        path.write_bytes(payload)
        binary = Path(next(iter(self.binaries["rust-binaries"].values()))["binary-path"])
        binary.write_bytes(b"changed fixture binary")
        with self.assertRaises(ValueError):
            self.helper.reuse_command("broad", 2)
        binary.unlink()
        with self.assertRaises(ValueError):
            self.helper.reuse_command("broad", 2)

    def test_complete_graph_all_workspace_and_three_package_binary_coverage_required(self):
        self.directory.mkdir()
        for mutation in ("resolve", "dependency", "workspace", "binary", "target", "profile"):
            with self.subTest(mutation=mutation):
                cargo = json.loads(json.dumps(self.cargo))
                binaries = json.loads(json.dumps(self.binaries))
                if mutation == "resolve":
                    cargo["resolve"] = None
                elif mutation == "dependency":
                    cargo["resolve"]["nodes"][0]["dependencies"] = ["missing"]
                elif mutation == "workspace":
                    cargo["workspace_members"].append("missing")
                elif mutation == "binary":
                    binaries["rust-binaries"].pop("phoenix-db")
                elif mutation == "target":
                    cargo["target_directory"] += "/other"
                else:
                    binaries["rust-build-meta"]["base-output-directories"] = ["release"]
                (self.directory / "cargo.json").write_text(json.dumps(cargo))
                (self.directory / "binaries.json").write_text(json.dumps(binaries))
                with self.assertRaises((ValueError, TypeError)):
                    self.helper.validate_metadata(self.directory)

    def test_main_missing_ready_fails_without_exec_or_cargo(self):
        with mock.patch.object(self.helper.sys, "argv", ["helper", "broad", "--test-threads", "2"]), \
             mock.patch.object(self.helper.os, "execvp") as execute, \
             mock.patch.object(self.helper.subprocess, "run") as run:
            self.assertEqual(self.helper.main(), 1)
        execute.assert_not_called()
        run.assert_not_called()

    def test_codegen_exec_keeps_scratch_and_existing_process_environment(self):
        self.capture()
        env = {"TS_RS_EXPORT_DIR": str(self.root / "scratch"), "RUSTC_WRAPPER": "/tools/sccache"}
        with mock.patch.dict(os.environ, env), \
             mock.patch.object(self.helper.sys, "argv", ["helper", "codegen", "--test-threads", "3"]), \
             mock.patch.object(self.helper.os, "execvp") as execute:
            self.assertEqual(self.helper.main(), 0)
            self.assertEqual(os.environ["TS_RS_EXPORT_DIR"], env["TS_RS_EXPORT_DIR"])
            self.assertEqual(os.environ["RUSTC_WRAPPER"], env["RUSTC_WRAPPER"])
        self.assertEqual(execute.call_args.args[0], "cargo")
        self.assertIn("--binaries-metadata", execute.call_args.args[1])

    def test_workflow_keeps_focused_expression_budgets_and_full_evidence(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        expression = next(line for line in workflow.splitlines() if "goal_filter='test(/conversation_goal" in line).split("'")[1]
        self.assertEqual(self.helper.digest(expression.encode()),
                         "736e43fadd8dd8feb198934d99a089b718c609ca4ad755b93d01f9e2c8e901d0")
        expression = expression.replace(
            "migrations::goal_execution::|direct_turn::tests::("
            "message_free_start_materializes_once_without_canonical_message|"
            "start_cannot_be_steering_or_rehydrate_with_a_message_materialization)|",
            "",
        )
        self.assertEqual(self.helper.digest(expression.encode()),
                         "c48b9059938ada212c42e489a65914b3600d1bdcdb31734a152ed587bb6f5d5a")
        expression = expression.replace(
            "coordinator_watches::goal_facts::|migrations::local_goal_watch::|"
            "runtime::executor::watch_goal_context_tests::|captured_notification_shared_|"
            "actual_local_and_remote_stable_id_collision_never_reinterprets_remote_fact_as_local_watch|",
            "",
        )
        self.assertEqual(self.helper.digest(expression.encode()),
                         "2783437b7a762c80522985f3795913782b8ca0502584998274caca513d13204a")
        for text in ("--goal-focused-filter", "--goal-step-deadline-ns", 'timeout-minutes: 10',
                     'PHOENIX_CHECK_WORKERS: "2"', "PHOENIX_BASH_LAUNCH_TEST_REQUIRED: '1'",
                     'PHOENIX_CHECK_CARGO_TEST_TIMEOUT_SECS: "1500"'):
            self.assertIn(text, workflow)
        rust = workflow.split("  check-rust:", 1)[1].split("\n  check-clippy:", 1)[0]
        for bound in ("    timeout-minutes: 113", "        timeout-minutes: 72", "        timeout-minutes: 20"):
            self.assertIn(bound, rust)
        self.assertEqual(workflow.count("PHOENIX_GOAL_REUSE_DIR:"), 1)
        self.assertIn("github.event_name == 'push' || github.event_name == 'workflow_dispatch'", rust)
        self.assertIn("if: ${{ !cancelled() }}", rust)
        self.assertIn("--identity step:ci:focused-default --", rust)
        self.assertIn('timeout --signal=TERM --kill-after=10s 9m cargo nextest run --locked', rust)
        self.assertNotIn("123", rust)

    def test_workflow_admits_reuse_only_before_push_dispatch_and_keeps_original_pr_command(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        admissions = [line for line in workflow.splitlines() if "PHOENIX_GOAL_REUSE_DIR:" in line]
        self.assertEqual(admissions, ["      PHOENIX_GOAL_REUSE_DIR: ''"])
        rust_job = workflow.split("  check-rust:", 1)[1].split("\n  check-clippy:", 1)[0]
        job_env = rust_job.split("    env:\n", 1)[1].split("    steps:\n", 1)[0]
        self.assertNotIn("runner.", job_env)
        setup = rust_job.split("      - name: Initialize Rust evidence\n", 1)[1].split("      - name:", 1)[0]
        self.assertIn('case "$GITHUB_EVENT_NAME" in\n            push|workflow_dispatch)', setup)
        self.assertIn('reuse_dir="$RUNNER_TEMP/goal-qualification-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT-check-rust"', setup)
        self.assertIn('printf \'PHOENIX_GOAL_REUSE_DIR=%s\\n\' "$reuse_dir" >> "$GITHUB_ENV"', setup)
        self.assertIn('*) printf \'PHOENIX_GOAL_REUSE_DIR=\\n\' >> "$GITHUB_ENV" ;;', setup)
        self.assertLess(setup.index('>> "$GITHUB_ENV"'), setup.index('mkdir -p "$RUST_EVIDENCE_DIR"'))
        self.assertLess(rust_job.index("- name: Initialize Rust evidence"), rust_job.index("- name: ./dev.py check (rust,cargo-fmt)"))
        self.assertLess(rust_job.index("- name: ./dev.py check (rust,cargo-fmt)"), rust_job.index("- name: Focused goal qualification"))
        self.assertEqual(rust_job.count('reuse_dir="$RUNNER_TEMP/goal-qualification-'), 1)
        self.assertEqual(rust_job.count("goal-qualification-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT-check-rust"), 1)
        self.assertIn("    runs-on: ubuntu-latest", rust_job)
        script = workflow.split("goal_filter=", 1)[1].split("- name: Check production feature on musl", 1)[0]
        producer, pr = script.split("          else\n", 1)
        self.assertEqual(producer.count("--goal-focused-filter"), 1)
        self.assertNotIn("cargo nextest run", producer)
        self.assertNotIn("scripts/goal_qualification.py", pr)
        for text in ('timeout --signal=TERM --kill-after=10s 9m cargo nextest run --locked',
                     '-p phoenix-core -p phoenix-db -p phoenix_ide', '-E "$goal_filter"',
                     '--test-threads 2 --no-fail-fast --failure-output immediate',
                     '--status-level all --final-status-level all'):
            self.assertIn(text, pr)
        self.assertIn('fi 2>&1 | tee "$RUST_EVIDENCE_DIR/goals.log"', script)
        self.assertIn('--profile-work-dir "$RUST_EVIDENCE_DIR/focused"', producer)
        self.assertIn('--goal-step-deadline-ns "$deadline"', producer)
        self.assertIn("            ${{ env.PHOENIX_GOAL_REUSE_DIR }}", workflow)
        rust_job = workflow.split("  check-rust:", 1)[1].split("\n  check-clippy:", 1)[0]
        self.assertIn("    timeout-minutes: 113\n", rust_job)
        self.assertIn("        timeout-minutes: 10\n", rust_job)
        self.assertIn("        timeout-minutes: 72\n", rust_job)
        self.assertEqual(600 - 540 - 10, 50)  # Arithmetic only; no claim the allowance was measured.

    def test_workflow_reuse_setup_rejects_missing_inputs_and_multiline_without_claiming(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        setup = workflow.split("      - name: Initialize Rust evidence\n", 1)[1].split("      - name:", 1)[0]
        full_event = setup.split("            push|workflow_dispatch)\n", 1)[1].split("            *)", 1)[0]
        for field, diagnostic in (("RUNNER_TEMP", "missing runner temp"),
                                  ("GITHUB_RUN_ID", "missing run id"),
                                  ("GITHUB_RUN_ATTEMPT", "missing run attempt"),
                                  ("GITHUB_ENV", "missing environment file")):
            guard = "${" + field + ":?" + diagnostic + "}"
            self.assertIn(guard, full_event)
            self.assertLess(full_event.index(guard), full_event.index("reuse_dir="))
        self.assertIn("*$'\\n'*|*$'\\r'*) echo 'invalid multiline reuse path' >&2; exit 1 ;;", full_event)
        self.assertLess(full_event.index("invalid multiline reuse path"), full_event.index('>> "$GITHUB_ENV"'))
        for forbidden in ("mkdir", "goal_qualification.py", "ready.json", ".phoenix-profile-claim"):
            self.assertNotIn(forbidden, full_event)
        self.assertNotIn("${{", setup)
        self.assertNotIn("export PHOENIX_GOAL_REUSE_DIR", setup)

    def test_clean_identity_binds_inputs_tools_flags_but_never_exports_credentials(self):
        (self.root / "Cargo.lock").write_text("lock fixture")
        (self.root / "Cargo.toml").write_text("manifest fixture")

        def read(command):
            if command[0] != "git":
                return b"tool version fixture"
            if command[1] == "status":
                return b""
            if command[1] == "ls-files":
                return b"Cargo.lock\0Cargo.toml\0"
            return b"head\n"

        with mock.patch.object(self.helper, "read_command", side_effect=read), \
             mock.patch.object(self.helper.shutil, "which", side_effect=lambda p: f"/tools/{Path(p).name}"), \
             mock.patch.object(self.helper, "digest_file", return_value="tool-bytes-fixture"), \
             mock.patch.dict(os.environ, {"CARGO_HOME": str(self.root / "cargo-home"),
                                          "CARGO_REGISTRIES_PRIVATE_TOKEN": "never-print-this", "RUSTC_WRAPPER": "sccache"}):
            before = self.real_identity()
            self.assertNotIn("never-print-this", json.dumps(before))
            with mock.patch.dict(os.environ, {"CARGO_BUILD_JOBS": "2", "RUST_TEST_THREADS": "2"}):
                self.assertEqual(self.real_identity(), before)
            with mock.patch.dict(os.environ, {"RUSTFLAGS": "--cfg different"}):
                self.assertNotEqual(self.real_identity(), before)
            (self.root / "Cargo.lock").write_text("changed lock")
            self.assertNotEqual(self.real_identity(), before)
        with mock.patch.object(self.helper, "read_command", return_value=b" M Cargo.lock\n"):
            with self.assertRaisesRegex(ValueError, "clean source"):
                self.real_identity()


if __name__ == "__main__":
    unittest.main()
