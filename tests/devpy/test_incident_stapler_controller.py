"""Controller-only fixtures: no signing, real xcrun, builds, or hosted dispatch."""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
from pathlib import Path
import signal
import select
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/incident_stapler_controller.py"
spec = importlib.util.spec_from_file_location("incident_stapler_controller", SCRIPT)
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)
WORKFLOW = (ROOT / ".github/workflows/release.yml").read_text()


def step_shell(name):
    step = WORKFLOW.split(f"      - name: {name}\n", 1)[1].split("\n      - ", 1)[0]
    return textwrap.dedent(step.split("        run: |\n", 1)[1])


class CandidateGateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        self.env = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.com",
                        GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.com")
        self.git("init", "-q")
        self.registry = self.repo / controller.REGISTRY_PATH
        self.registry.parent.mkdir(parents=True)
        self.registry.write_bytes(b"complete immutable fixture registry\n")
        self.cargo = self.repo / "crates/phoenix-ide/Cargo.toml"
        self.cargo.parent.mkdir(parents=True)
        self.cargo.write_text('[package]\nversion = "0.13.0-rc.4"\n')
        self.candidate = self.commit("candidate")
        self.git("update-ref", "refs/remotes/origin/main", self.candidate)
        self.pins = contextlib.ExitStack()
        self.addCleanup(self.pins.close)
        self.pins.enter_context(mock.patch.object(controller, "CANDIDATE", self.candidate))
        self.pins.enter_context(mock.patch.object(controller, "REGISTRY_SHA256", hashlib.sha256(self.registry.read_bytes()).hexdigest()))

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], env=self.env, stderr=subprocess.DEVNULL).decode().strip()

    def commit(self, message):
        self.git("add", ".")
        self.git("commit", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def test_gate_uses_git_objects_not_controller_worktree_and_returns_candidate(self):
        self.cargo.write_text('[package]\nversion = "99.99.99"\n')
        self.registry.write_text("different controller registry")
        other = self.commit("controller")
        self.git("update-ref", "refs/remotes/origin/main", other)
        before = self.git("status", "--porcelain")
        outputs = controller.gate_outputs(self.repo)
        self.assertEqual(outputs, dict(preparation="true", release="false", create_tag="false", tag="",
                                       version="0.13.0-rc.4", channel="rc", commit=self.candidate))
        self.assertNotEqual(outputs["commit"], other)
        self.assertEqual(self.git("status", "--porcelain"), before)
        self.assertEqual(self.git("rev-parse", "HEAD"), other)
        self.assertEqual(self.git("tag", "--list"), "")

    def test_gate_rejects_missing_candidate_non_ancestor_wrong_version_or_digest(self):
        for attribute, value in [("CANDIDATE", "f" * 40), ("VERSION", "0.13.0-rc.5"),
                                 ("REGISTRY_SHA256", "0" * 64)]:
            with self.subTest(attribute=attribute), mock.patch.object(controller, attribute, value):
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    controller.gate_outputs(self.repo)
        self.git("checkout", "--orphan", "other")
        (self.repo / "other").write_text("unrelated root")
        other = self.commit("unrelated")
        self.git("update-ref", "refs/remotes/origin/main", other)
        with self.assertRaises(subprocess.CalledProcessError):
            controller.gate_outputs(self.repo)
        self.git("update-ref", "-d", "refs/remotes/origin/main")
        with self.assertRaises(subprocess.CalledProcessError):
            controller.gate_outputs(self.repo)


class WrapperTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.app = self.root / "phoenix-desktop-aarch64-apple-darwin.A1b2C3/Build/Products/Release/Phoenix.app"
        self.app.mkdir(parents=True)
        self.args = ["stapler", "staple", str(self.app)]
        self.wrapper = self.root / controller.WRAPPER_NAME
        controller.write_wrapper(self.wrapper)
        self.count = self.root / "calls.json"
        self.fake = self.root / "fake-xcrun"
        self.fake.write_text(f"#!{sys.executable}\n" + textwrap.dedent('''\
            import json, os, pathlib, signal, subprocess, sys, time
            calls_path = pathlib.Path(os.environ["FAKE_CALLS"])
            calls = json.loads(calls_path.read_text()) if calls_path.exists() else []
            calls.append(sys.argv[1:])
            calls_path.write_text(json.dumps(calls))
            print("SECRET-CHILD-STDOUT", flush=True)
            print("SECRET-CHILD-STDERR", file=sys.stderr, flush=True)
            if os.environ.get("FAKE_HANG"):
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
                pathlib.Path(os.environ["FAKE_PIDS"]).write_text(json.dumps([os.getpid(), child.pid]))
                time.sleep(60)
            if os.environ.get("FAKE_SIGNAL"):
                os.kill(os.getpid(), int(os.environ["FAKE_SIGNAL"]))
            codes = json.loads(os.environ.get("FAKE_CODES", "[0]"))
            sys.exit(codes[min(len(calls) - 1, len(codes) - 1)])
            '''))
        self.fake.chmod(0o700)
        self.env = dict(os.environ, RUNNER_TEMP=str(self.root), FAKE_CALLS=str(self.count),
                        FAKE_PIDS=str(self.root / "pids.json"))

    def launcher(self, args=None, **settings):
        settings = {"REAL_XCRUN": str(self.fake), "DELAYS": [.01, .02, .03],
                    "COMMAND_TIMEOUT": 1, "TOTAL_TIMEOUT": 3, **settings}
        code = '''import runpy, sys, json
module = runpy.run_path(sys.argv[1], run_name="fixture")
entry = module["wrapper_main"]
entry.__globals__.update(json.loads(sys.argv[2]))
sys.exit(entry(json.loads(sys.argv[3])))
'''
        return [sys.executable, "-c", code, str(self.wrapper), json.dumps(settings), json.dumps(args or self.args)]

    def run_fake(self, codes, args=None, **settings):
        return subprocess.run(self.launcher(args, **settings), env=dict(self.env, FAKE_CODES=json.dumps(codes)),
                              text=True, capture_output=True, timeout=10)

    def calls(self):
        return json.loads(self.count.read_text())

    def test_generated_wrapper_is_exact_executable_controller_copy(self):
        self.assertEqual(self.wrapper.read_bytes(), SCRIPT.read_bytes())
        self.assertEqual(self.wrapper.stat().st_mode & 0o777, 0o700)
        with self.assertRaises(FileExistsError):
            controller.write_wrapper(self.wrapper)
        with self.assertRaises(ValueError):
            controller.write_wrapper(self.root / "generic-xcrun")

    def test_exact_path_and_argv_only(self):
        for action in ["staple", "validate"]:
            self.assertTrue(controller.retry_eligible(["stapler", action, str(self.app)], str(self.root)))
        outside = self.root.parent / "outside.app"
        bad = [[], ["stapler", "staple"], ["stapler", "staple", "relative.app"],
               ["stapler", "staple", str(outside)], ["stapler", "staple", str(self.app), "--verbose"],
               ["stapler", "--verbose", str(self.app)], ["notarytool", "submit", str(self.app)]]
        for args in bad:
            with self.subTest(args=args):
                self.assertFalse(controller.retry_eligible(args, str(self.root)))
        intel = self.root / "phoenix-desktop-x86_64-apple-darwin.A1b2C3/Build/Products/Release/Phoenix.app"
        intel.mkdir(parents=True)
        self.assertFalse(controller.retry_eligible(["stapler", "staple", str(intel)], str(self.root)))
        # A syntactically matching path cannot escape through a symlink.
        with tempfile.TemporaryDirectory() as outside_temp:
            link = self.root / "phoenix-desktop-aarch64-apple-darwin.abcdef"
            link.symlink_to(outside_temp, target_is_directory=True)
            escaped = link / "Build/Products/Release/Phoenix.app"
            escaped.mkdir(parents=True)
            self.assertFalse(controller.retry_eligible(["stapler", "staple", str(escaped)], str(self.root)))

    def test_transient_retry_then_success_has_no_child_or_argument_logs(self):
        for action in ["staple", "validate"]:
            self.count.unlink(missing_ok=True)
            args = ["stapler", action, str(self.app)]
            result = self.run_fake([68, 68, 0], args)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.calls(), [args] * 3)
            self.assertEqual(result.stdout, "")
            self.assertEqual(result.stderr.count("service retry"), 2)
            self.assertNotIn("SECRET", result.stderr)
            self.assertNotIn(str(self.app), result.stderr)

    def test_retry_exhaustion_is_four_attempts_and_nontransient_codes_fail_fast(self):
        for code, attempts in [(68, 4), (75, 1), (65, 1), (64, 1), (66, 1), (73, 1), (77, 1), (1, 1)]:
            self.count.unlink(missing_ok=True)
            result = self.run_fake([code])
            self.assertEqual(result.returncode, code, result.stderr)
            self.assertEqual(len(self.calls()), attempts)
            self.assertNotIn("SECRET", result.stdout + result.stderr)

    def test_exact_policy_delays_and_initial_success_without_trailing_sleep(self):
        for codes, expected_calls, expected_delays in [([0], 1, []), ([68], 4, [10, 30, 60])]:
            self.count.unlink(missing_ok=True)
            with mock.patch.object(controller, "REAL_XCRUN", str(self.fake)), \
                    mock.patch.dict(os.environ, dict(self.env, FAKE_CODES=json.dumps(codes))), \
                    mock.patch.object(controller, "time", mock.Mock(monotonic=time.monotonic, sleep=mock.Mock())) as clock, \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(controller.run_stapler(self.args), codes[0])
            self.assertEqual(len(self.calls()), expected_calls)
            self.assertEqual(clock.sleep.call_args_list, [mock.call(delay) for delay in expected_delays])

    def test_spawn_failure_reports_only_static_error(self):
        missing = self.root / "SECRET-NONEXISTENT-DELEGATE"
        self.wrapper.write_text(self.wrapper.read_text().replace('REAL_XCRUN = "/usr/bin/xcrun"',
                                                               f'REAL_XCRUN = {str(missing)!r}'))
        result = subprocess.run([sys.executable, str(self.wrapper), *self.args], env=self.env,
                                capture_output=True, text=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr.strip(), "Incident controller refused operation; private details withheld.")
        self.assertFalse(self.count.exists())

    def test_notary_credentials_are_delegated_unchanged_once_via_exec(self):
        args = ["notarytool", "submit", "/private/archive.zip", "--key", "SECRET-PATH",
                "--key-id", "SECRET-ID", "--issuer", "SECRET-ISSUER", "--wait"]
        with mock.patch.object(controller.os, "execv", side_effect=SystemExit(68)) as execute:
            with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()) as stderr:
                controller.wrapper_main(args)
        execute.assert_called_once_with("/usr/bin/xcrun", ["/usr/bin/xcrun", *args])
        self.assertEqual(stderr.getvalue(), "")
        result = self.run_fake([68], args)
        self.assertEqual(result.returncode, 68)
        self.assertEqual(self.calls(), [args])
        self.assertEqual(result.stdout, "SECRET-CHILD-STDOUT\n")
        self.assertEqual(result.stderr, "SECRET-CHILD-STDERR\n")

    def assert_children_dead(self):
        for pid in json.loads((self.root / "pids.json").read_text()):
            state = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], text=True, capture_output=True).stdout.strip()
            self.assertTrue(not state or state.startswith("Z"), (pid, state))

    def test_command_timeout_kills_process_group_without_retry(self):
        result = subprocess.run(self.launcher(COMMAND_TIMEOUT=.3), env=dict(self.env, FAKE_HANG="1"),
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 124, result.stderr)
        self.assertEqual(len(self.calls()), 1)
        self.assertNotIn("SECRET", result.stdout + result.stderr)
        self.assert_children_dead()

    def test_signaled_delegate_fails_once_without_child_output(self):
        for signum in (signal.SIGKILL, signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            self.count.unlink(missing_ok=True)
            result = subprocess.run(self.launcher(), env=dict(self.env, FAKE_SIGNAL=str(signum)),
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, -signum, result.stderr)
            self.assertEqual(len(self.calls()), 1)
            self.assertEqual(result.stdout, "")
            self.assertEqual(result.stderr, "")

    def test_total_deadline_refuses_delay_instead_of_extra_attempt(self):
        result = self.run_fake([68], DELAYS=[2, 2, 2], TOTAL_TIMEOUT=.5)
        self.assertEqual(result.returncode, 124, result.stderr)
        self.assertEqual(len(self.calls()), 1)

    def test_cancellation_propagates_and_kills_and_reaps_child_session(self):
        for signum in [signal.SIGTERM, signal.SIGINT, signal.SIGHUP]:
            self.count.unlink(missing_ok=True)
            pid_file = self.root / "pids.json"
            pid_file.unlink(missing_ok=True)
            child = subprocess.Popen(self.launcher(), env=dict(self.env, FAKE_HANG="1"),
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 5
                while not pid_file.exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue(pid_file.exists())
                child.send_signal(signum)
                stdout, stderr = child.communicate(timeout=5)
                self.assertEqual(child.returncode, -signum, stderr)
                self.assertNotIn("SECRET", stdout + stderr)
                self.assert_children_dead()
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()


    def test_cancellation_during_retry_sleep_does_not_start_another_child(self):
        child = subprocess.Popen(self.launcher(DELAYS=[5, 5, 5], TOTAL_TIMEOUT=20),
                                 env=dict(self.env, FAKE_CODES="[68]"),
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            ready, _, _ = select.select([child.stderr], [], [], 5)
            self.assertTrue(ready, "retry message did not arrive")
            self.assertEqual(child.stderr.readline(), "Incident stapler service retry within bounded policy.\n")
            child.send_signal(signal.SIGTERM)
            stdout, stderr = child.communicate(timeout=5)
            self.assertEqual(child.returncode, -signal.SIGTERM)
            self.assertEqual(len(self.calls()), 1)
            self.assertNotIn("SECRET", stdout + stderr)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()


class WorkflowTests(unittest.TestCase):
    def test_incident_and_normal_matrix_execute_actual_workflow_shell(self):
        shell = step_shell("Select protected macOS architecture matrix")
        gate_outputs = WORKFLOW.split("    outputs:\n", 1)[1].split("    steps:\n", 1)[0]
        self.assertIn("      operation: ${{ steps.macos.outputs.operation }}\n", gate_outputs)
        producer = WORKFLOW.split("      - name: Select protected macOS architecture matrix", 1)[1].split("      - name:", 1)[0]
        self.assertIn("        id: macos\n", producer)
        package = WORKFLOW.split("      - name: Sign, verify, notarize", 1)[1].split("      - name:", 1)[0]
        timeout = re.search(
            r"timeout-minutes: \$\{\{ needs\.gate\.outputs\.operation == '([^']+)' && (\d+) \|\| (\d+) \}\}", package,
        )
        self.assertIsNotNone(timeout)
        incident_operation, incident_minutes, ordinary_minutes = timeout.groups()
        self.assertEqual(incident_operation, controller.OPERATION)
        for event, operation, targets in [
            ("workflow_dispatch", controller.OPERATION, [controller.TARGET]),
            ("workflow_dispatch", "prepare-main", [controller.TARGET, "x86_64-apple-darwin"]),
            ("workflow_dispatch", "retry-release", [controller.TARGET, "x86_64-apple-darwin"]),
            ("push", "", [controller.TARGET, "x86_64-apple-darwin"]),
            ("push", controller.OPERATION, [controller.TARGET, "x86_64-apple-darwin"]),
        ]:
            with self.subTest(event=event, operation=operation), tempfile.TemporaryDirectory() as temp:
                output = Path(temp) / "output"
                subprocess.run(["bash", "-euo", "pipefail", "-c", shell], check=True,
                               env=dict(os.environ, GITHUB_EVENT_NAME=event, OPERATION=operation, GITHUB_OUTPUT=str(output)))
                produced = dict(line.split("=", 1) for line in output.read_text().splitlines())
                expected_operation = operation if event == "workflow_dispatch" else ""
                self.assertEqual(produced["operation"], expected_operation)
                minutes = int(incident_minutes if produced["operation"] == incident_operation else ordinary_minutes)
                self.assertEqual(minutes, 30 if len(targets) == 1 else 360)
                matrix = json.loads(produced["matrix"])["include"]
                self.assertEqual([row["target"] for row in matrix], targets)
                self.assertEqual(matrix[0], dict(runner="macos-15", target=controller.TARGET, asset=f"phoenix_ide-{controller.TARGET}"))
                if len(matrix) == 2:
                    self.assertEqual(matrix[1]["runner"], "macos-15-intel")
        self.assertIn("macos_matrix: ${{ steps.macos.outputs.matrix }}", WORKFLOW)
        self.assertIn("matrix: ${{ fromJSON(needs.gate.outputs.macos_matrix) }}", WORKFLOW)

    def test_workflow_incident_gate_reads_real_git_objects_and_rejects_tag(self):
        shell = step_shell("Resolve version, tag if new")
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp)
            env = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.com",
                       GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.com")
            def git(*args):
                return subprocess.check_output(["git", "-C", str(repo), *args], env=env).decode().strip()
            git("init", "-q")
            cargo = repo / "crates/phoenix-ide/Cargo.toml"
            cargo.parent.mkdir(parents=True)
            cargo.write_text('[package]\nversion = "0.13.0-rc.4"\n')
            registry = repo / controller.REGISTRY_PATH
            registry.parent.mkdir(parents=True)
            registry.write_text("immutable fixture registry")
            git("add", ".")
            git("commit", "-qm", "candidate")
            candidate = git("rev-parse", "HEAD")
            git("update-ref", "refs/remotes/origin/main", candidate)
            # Test-only pin substitution; production exposes no candidate/target CLI.
            script = repo / "scripts/incident_stapler_controller.py"
            script.parent.mkdir()
            script.write_text(SCRIPT.read_text().replace(controller.CANDIDATE, candidate).replace(
                controller.REGISTRY_SHA256, hashlib.sha256(registry.read_bytes()).hexdigest()))
            for tag, expected in [("", 0), ("v0.13.0-rc.4", 1)]:
                output = repo / "output"
                output.unlink(missing_ok=True)
                result = subprocess.run(["bash", "-euo", "pipefail", "-c", shell], cwd=repo,
                                        env=dict(env, GITHUB_EVENT_NAME="workflow_dispatch", OPERATION=controller.OPERATION,
                                                 RETRY_TAG=tag, GITHUB_OUTPUT=str(output), GITHUB_SHA="a" * 40),
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stderr)
                if expected == 0:
                    values = dict(line.split("=", 1) for line in output.read_text().splitlines())
                    self.assertEqual(values["commit"], candidate)
                    self.assertEqual(values["release"], "false")
                    self.assertEqual(values["create_tag"], "false")
                    self.assertEqual(values["preparation"], "true")
                    self.assertEqual(values["version"], controller.VERSION)
                else:
                    self.assertFalse(output.exists())

    def test_controller_scope_provenance_and_gate_ownership(self):
        materialize = WORKFLOW.index("      - name: Materialize exact incident packaging controller")
        native_build = WORKFLOW.index("      - name: Build native macOS binary")
        package = WORKFLOW.index("      - name: Sign, verify, notarize")
        cleanup = WORKFLOW.index("      - name: Remove protected material")
        acceptance = WORKFLOW.index("      - name: Check isolated signed-helper startup")
        upload = WORKFLOW.index("          name: desktop-preparation-")
        self.assertTrue(native_build < materialize < package < cleanup < acceptance < upload)
        timeout = re.search(
            r"timeout-minutes: \$\{\{ needs\.gate\.outputs\.operation == '([^']+)' && (\d+) \|\| (\d+) \}\}",
            WORKFLOW[package:cleanup],
        )
        self.assertIsNotNone(timeout)
        incident_operation, incident_minutes, ordinary_minutes = timeout.groups()
        self.assertEqual(incident_operation, controller.OPERATION)
        for operation, expected in [(controller.OPERATION, 30), ("prepare-main", 360),
                                    ("retry-release", 360), ("", 360), ("push", 360)]:
            actual = int(incident_minutes if operation == incident_operation else ordinary_minutes)
            self.assertEqual(actual, expected, operation)
        self.assertIn('git show "$CONTROLLER_COMMIT:scripts/incident_stapler_controller.py" > "$RUNNER_TEMP/incident_stapler_controller.py"', WORKFLOW)
        self.assertIn('CONTROLLER_COMMIT: ${{ github.sha }}', WORKFLOW)
        self.assertNotIn(".packaging-controller", WORKFLOW)
        self.assertNotIn("GITHUB_ENV", WORKFLOW)
        self.assertEqual(WORKFLOW.count('package_env=(env "XCRUN=$RUNNER_TEMP/incident-xcrun")'), 1)
        self.assertIn('"${package_env[@]}" macos/Phoenix/scripts/package-desktop-release.sh', WORKFLOW)
        self.assertIn('test -z "$(git status --porcelain)"', WORKFLOW[materialize:package])
        self.assertIn('rm -f "$RUNNER_TEMP/incident-xcrun"', WORKFLOW[cleanup:acceptance])
        inputs = WORKFLOW.split("    inputs:\n", 1)[1].split("\njobs:", 1)[0]
        self.assertNotRegex(inputs, r"(?m)^      (candidate|target|commit):")
        self.assertIn("if: github.ref == 'refs/heads/main'", WORKFLOW)
        self.assertIn('INCIDENT-CONTROLLER-PROVENANCE-aarch64-apple-darwin.json', WORKFLOW)
        evidence = controller.provenance("a" * 40, "123", "2")
        self.assertEqual(evidence["operation"], controller.OPERATION)
        self.assertEqual(evidence["candidate_commit"], controller.CANDIDATE)
        self.assertEqual(evidence["controller_commit"], "a" * 40)
        self.assertEqual(evidence["retry_policy"], dict(id="stapler-service-68-v1", exit_codes=[68],
                                                       max_attempts=4, delays_seconds=[10, 30, 60], command_timeout_seconds=45,
                                                       total_timeout_seconds=300))
        self.assertEqual(set(evidence), {"schema", "operation", "candidate_commit", "controller_commit", "target", "run_id", "run_attempt", "retry_policy"})
        for sha, run, attempt in [(controller.CANDIDATE, "1", "1"), ("a" * 7, "1", "1"),
                                  ("a" * 40, "secret", "1"), ("a" * 40, "1", "0")]:
            with self.assertRaises(ValueError):
                controller.provenance(sha, run, attempt)

    def test_no_generic_cli_candidate_or_target_selection(self):
        for args in [["gate", "--candidate", controller.CANDIDATE], ["gate", "--target", controller.TARGET]]:
            result = subprocess.run([sys.executable, str(SCRIPT), *args], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
