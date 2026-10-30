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
PINNED_CANDIDATE = "237100c79d9ce672867b284167a099fa0b7a2bcf"
PINNED_REGISTRY_SHA256 = "a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6"
WORKFLOW = (ROOT / ".github/workflows/release.yml").read_text()
REAL_CHECK_OUTPUT = subprocess.check_output


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
        self.registry.write_bytes(
            b"const MIGRATIONS: &[Migration] = &[\n    Migration { version: 128, },\n];\n"
            b"#[cfg(test)]\nconst TEST_FIXTURE: Migration = Migration { version: 120, };\n"
        )
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

    def test_pinned_git_candidate_has_bounded_registry_target_128(self):
        try:
            registry = controller.git(ROOT, "show", f"{PINNED_CANDIDATE}:{controller.REGISTRY_PATH}")
        except subprocess.CalledProcessError:
            self.skipTest("pinned candidate Git object is not available in this checkout")
        self.assertEqual(hashlib.sha256(registry).hexdigest(), PINNED_REGISTRY_SHA256)
        controller.verify_registry_bytes(registry)

    def test_registry_version_scan_is_bounded_before_trailing_fixture(self):
        controller.verify_registry_bytes(self.registry.read_bytes())
        with self.assertRaisesRegex(ValueError, "migration target"):
            controller.verify_registry_bytes(
                b"const MIGRATIONS: &[Migration] = &[\n    Migration { version: 127, },\n];\n"
                b"Migration { version: 128, }\n"
            )
        with self.assertRaisesRegex(ValueError, "migration registry"):
            controller.verify_registry_bytes(b"Migration { version: 128, }\n")

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

class LinuxArtifactGateTests(CandidateGateTests):
    def test_linux_gate_fixes_source_registry_version_and_target(self):
        self.assertEqual(controller.linux_gate_outputs(self.repo), {
            "preparation": "true", "release": "false", "create_tag": "false", "tag": "",
            "version": controller.VERSION, "channel": "rc", "commit": controller.CANDIDATE,
            "operation": controller.LINUX_OPERATION, "target": controller.LINUX_TARGET,
        })
        for name, value in [("VERSION", "0.13.0-rc.5"), ("REGISTRY_SHA256", "0" * 64),
                            ("REGISTRY_TARGET_VERSION", 129), ("CANDIDATE", "f" * 40)]:
            with self.subTest(name=name), mock.patch.object(controller, name, value):
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    controller.linux_gate_outputs(self.repo)

    def _linux_fixture(self, *, identity=None, dirty=False, dynamic=False, machine="Advanced Micro Devices X86-64"):
        directory = Path(self.temporary.name) / "out"
        directory.mkdir(exist_ok=True)
        artifact = directory / controller.LINUX_PREPARED_ASSET
        identity = identity or {"version": controller.VERSION, "git_sha": controller.CANDIDATE}
        identity_output = identity if isinstance(identity, str) else json.dumps(identity)
        artifact.write_text(f"#!{sys.executable}\nprint({identity_output!r})\n")
        artifact.chmod(0o755)
        def readelf(args, text=True, timeout=None):
            if args[0] == "git" and args[1:3] == ["-C", str(self.repo)]:
                return " M tracked-file\\n" if dirty else ""
            self.assertEqual(timeout, 10)
            if args[0] == "readelf" and args[1] == "-h":
                return f"Class: ELF64\\nMachine: {machine}\\n"
            if args[0] == "readelf":
                return "INTERP /lib/ld-musl.so" if dynamic else "LOAD"
            return REAL_CHECK_OUTPUT(args, text=text, timeout=timeout)
        self.patches = mock.patch.object(controller.subprocess, "check_output", side_effect=readelf)
        self.patches.start()
        self.addCleanup(self.patches.stop)
        return artifact, directory

    def test_receipt_is_closed_exact_contract_and_hashes_prepared_binary(self):
        artifact, directory = self._linux_fixture()
        receipt_path = directory / controller.LINUX_RECEIPT
        evidence = controller.write_linux_receipt(
            receipt_path, "b" * 40, 123, 2, artifact,
            event="workflow_dispatch", ref="refs/heads/main", repo=self.repo,
        )
        self.assertEqual(set(evidence), {
            "schema", "operation", "target", "version", "commit", "controller_commit", "registry_sha256",
            "run_id", "run_attempt", "event", "ref", "sha256", "checks",
        })
        self.assertEqual(evidence, {
            "schema": 1, "operation": controller.LINUX_OPERATION, "target": controller.LINUX_TARGET,
            "version": controller.VERSION, "commit": controller.CANDIDATE, "controller_commit": "b" * 40,
            "registry_sha256": controller.REGISTRY_SHA256, "run_id": 123, "run_attempt": 2,
            "event": "workflow_dispatch", "ref": "refs/heads/main",
            "sha256": {controller.LINUX_PREPARED_ASSET: hashlib.sha256(artifact.read_bytes()).hexdigest()},
            "checks": controller.LINUX_CHECKS,
        })
        self.assertEqual(json.loads(receipt_path.read_text()), evidence)
        self.assertEqual({path.name for path in directory.iterdir()}, {artifact.name, receipt_path.name})

    def test_failed_artifact_source_or_workflow_check_never_writes_receipt(self):
        fixtures = [
            {"dirty": True}, {"dynamic": True}, {"machine": "AArch64"},
            {"identity": {"version": "wrong", "git_sha": controller.CANDIDATE}},
            {"identity": {"version": controller.VERSION, "git_sha": "f" * 40}},
            {"identity": '{"version":"x","version":"y","git_sha":"' + controller.CANDIDATE + '"}'},
        ]
        for fixture in fixtures:
            with self.subTest(fixture=fixture):
                artifact, directory = self._linux_fixture(**fixture)
                receipt_path = directory / controller.LINUX_RECEIPT
                with self.assertRaises(ValueError):
                    controller.write_linux_receipt(receipt_path, "b" * 40, 1, 1, artifact,
                        event="workflow_dispatch", ref="refs/heads/main", repo=self.repo)
                self.assertFalse(receipt_path.exists())
                self.temporary.cleanup()
                self.temporary = tempfile.TemporaryDirectory()
                self.addCleanup(self.temporary.cleanup)

    def test_rejects_invalid_workflow_ids_source_controller_symlinks_and_extra_files(self):
        artifact, directory = self._linux_fixture()
        receipt_path = directory / controller.LINUX_RECEIPT
        for kwargs in [
            {"controller": controller.CANDIDATE}, {"controller": "B" * 40},
            {"run_id": 0}, {"run_attempt": 0}, {"run_id": True},
            {"event": "push"}, {"ref": "refs/heads/other"},
        ]:
            args = dict(controller="b" * 40, run_id=1, run_attempt=1, event="workflow_dispatch", ref="refs/heads/main")
            args.update(kwargs)
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                controller.write_linux_receipt(receipt_path, args.pop("controller"), args.pop("run_id"),
                    args.pop("run_attempt"), artifact, event=args.pop("event"), ref=args.pop("ref"), repo=self.repo)
            self.assertFalse(receipt_path.exists())
        (directory / "extra").write_text("no")
        with self.assertRaises(ValueError):
            controller.write_linux_receipt(receipt_path, "b" * 40, 1, 1, artifact,
                event="workflow_dispatch", ref="refs/heads/main", repo=self.repo)
        self.assertFalse(receipt_path.exists())

    def test_rejects_wrong_name_missing_or_empty_artifact_without_receipt(self):
        artifact, directory = self._linux_fixture()
        receipt_path = directory / controller.LINUX_RECEIPT
        wrong = directory / "phoenix_ide-x86_64-unknown-linux-musl"
        wrong.write_text("payload")
        with self.assertRaises(ValueError):
            controller.write_linux_receipt(receipt_path, "b" * 40, 1, 1, wrong,
                event="workflow_dispatch", ref="refs/heads/main", repo=self.repo)
        wrong.unlink()
        artifact.write_text("")
        with self.assertRaises(ValueError):
            controller.write_linux_receipt(receipt_path, "b" * 40, 1, 1, artifact,
                event="workflow_dispatch", ref="refs/heads/main", repo=self.repo)
        artifact.unlink()
        with self.assertRaises(ValueError):
            controller.write_linux_receipt(receipt_path, "b" * 40, 1, 1, artifact,
                event="workflow_dispatch", ref="refs/heads/main", repo=self.repo)
        self.assertFalse(receipt_path.exists())

    def test_rejects_symlink_artifact_without_receipt(self):
        artifact, directory = self._linux_fixture()
        real = directory / "real"
        artifact.rename(real)
        artifact.symlink_to(real)
        with self.assertRaises(ValueError):
            controller.write_linux_receipt(directory / controller.LINUX_RECEIPT, "b" * 40, 1, 1, artifact,
                event="workflow_dispatch", ref="refs/heads/main", repo=self.repo)

    def test_workflow_dispatch_is_source_only_and_artifact_only(self):
        self.assertIn(f"- {controller.LINUX_OPERATION}", WORKFLOW)
        self.assertIn("python3 scripts/incident_stapler_controller.py linux-gate", WORKFLOW)
        self.assertIn("needs.gate.outputs.operation == 'prepare-incident-237100-linux-x86_64'", WORKFLOW)
        self.assertIn("build-incident-linux:", WORKFLOW)
        self.assertIn("cargo build --locked --profile release-min", WORKFLOW)
        self.assertIn("target: ${{ steps.ver.outputs.target }}", WORKFLOW)
        self.assertIn("linux-receipt", WORKFLOW)
        self.assertIn(controller.LINUX_RECEIPT, WORKFLOW)
        self.assertIn(controller.LINUX_PREPARED_ASSET, WORKFLOW)
        self.assertNotIn("SHA256SUMS", WORKFLOW.split("  build-incident-linux:", 1)[1].split("  build-macos:", 1)[0])
        self.assertIn("needs.gate.outputs.operation != 'prepare-incident-237100-linux-x86_64'", WORKFLOW)
        self.assertIn("matrix={\"include\":[]}", WORKFLOW)
        inputs = WORKFLOW.split("    inputs:\n", 1)[1].split("\njobs:", 1)[0]
        self.assertNotRegex(inputs, r"(?m)^      (candidate|target|commit):")
        self.assertNotIn("contents: write", WORKFLOW.split("  build-incident-linux:", 1)[1].split("  build-macos:", 1)[0])
        self.assertIn("retention-days: 14", WORKFLOW.split("  build-incident-linux:", 1)[1].split("  build-macos:", 1)[0])
        consumer = WORKFLOW.split("  build-incident-linux:", 1)[1].split("  build-macos:", 1)[0]
        self.assertIn("needs.gate.outputs.target == 'x86_64-unknown-linux-musl'", consumer)
        outputs = WORKFLOW.split("    outputs:\n", 1)[1].split("    steps:\n", 1)[0]
        mapping = dict(re.findall(r"(?m)^      (\w+): \$\{\{ (.*?) \}\}$", outputs))
        condition_names = set(re.findall(r"needs\.gate\.outputs\.(\w+)", consumer))
        self.assertTrue(condition_names <= set(mapping), f"unexported Linux condition outputs: {condition_names - set(mapping)}")
        self.assertEqual(mapping["target"], "steps.ver.outputs.target")
        self.assertEqual(mapping["operation"], "steps.macos.outputs.operation")
        self.assertEqual(mapping["controller_commit"], "github.sha")
        without_target = {key: value for key, value in mapping.items() if key != "target"}
        self.assertFalse(condition_names <= set(without_target), "removing target export must fail the consumer contract")
        self.assertIn("git status --porcelain", consumer)
        self.assertIn('subprocess.check_output(["readelf", "-h", str(artifact)]', SCRIPT.read_text())
        self.assertNotIn("cargo run", consumer)


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
            ("workflow_dispatch", controller.LINUX_OPERATION, []),
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
                if matrix:
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
            registry.write_text("const MIGRATIONS: &[Migration] = &[\n    Migration { version: 128, },\n];\nMigration { version: 120, }\n")
            git("add", ".")
            git("commit", "-qm", "candidate")
            candidate = git("rev-parse", "HEAD")
            git("update-ref", "refs/remotes/origin/main", candidate)
            # Test-only pin substitution; production exposes no candidate/target CLI.
            script = repo / "scripts/incident_stapler_controller.py"
            script.parent.mkdir()
            script.write_text(SCRIPT.read_text().replace(controller.CANDIDATE, candidate).replace(
                controller.REGISTRY_SHA256, hashlib.sha256(registry.read_bytes()).hexdigest()))
            for operation in [controller.OPERATION, controller.LINUX_OPERATION]:
                for tag, expected in [("", 0), ("v0.13.0-rc.4", 1)]:
                    output = repo / "output"
                    output.unlink(missing_ok=True)
                    result = subprocess.run(["bash", "-euo", "pipefail", "-c", shell], cwd=repo,
                                            env=dict(env, GITHUB_EVENT_NAME="workflow_dispatch", OPERATION=operation,
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
                        self.assertEqual(values.get("target"), controller.LINUX_TARGET if operation == controller.LINUX_OPERATION else None)
                    else:
                        self.assertFalse(output.exists())

    def test_linux_artifact_validation_shell_rejects_bad_binary_before_receipt(self):
        shell = step_shell("Validate Linux artifact and write receipt")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            artifact_dir = root / "incident-linux-artifact"
            artifact_dir.mkdir()
            artifact = artifact_dir / controller.LINUX_PREPARED_ASSET
            artifact.write_text(f"#!{sys.executable}\nprint('{json.dumps({'version': controller.VERSION, 'git_sha': controller.CANDIDATE})}')\n")
            artifact.chmod(0o755)
            controller_copy = root / "incident_stapler_controller.py"
            controller_copy.write_text(SCRIPT.read_text())
            bin_dir = root / "bin"
            bin_dir.mkdir()
            readelf = bin_dir / "readelf"
            readelf.write_text(f"#!{sys.executable}\nimport os, sys\nif sys.argv[1] == '-h': print(os.environ.get('FAKE_ELF_HEADER', 'Class: ELF64\\nMachine: Advanced Micro Devices X86-64'))\nelse: print(os.environ.get('FAKE_PROGRAM_HEADERS', ''))\n")
            readelf.chmod(0o755)
            git = bin_dir / "git"
            git.write_text(f"#!{sys.executable}\nimport sys\nsys.stdout.write('')\n")
            git.chmod(0o755)
            base_env = dict(os.environ, PATH=f"{bin_dir}:{os.environ['PATH']}", RUNNER_TEMP=temp,
                            GITHUB_SHA="b" * 40, GITHUB_RUN_ID="123", GITHUB_RUN_ATTEMPT="1",
                            GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main",
                            GITHUB_WORKSPACE=temp, GITHUB_OUTPUT=str(root / "outputs"))
            receipt = artifact_dir / controller.LINUX_RECEIPT
            for overrides in [{}, {"FAKE_PROGRAM_HEADERS": " INTERP 0x0000"},
                              {"FAKE_ELF_HEADER": "Class: ELF32\nMachine: Advanced Micro Devices X86-64"},
                              {"FAKE_ELF_HEADER": "Class: ELF64\nMachine: AArch64"}]:
                receipt.unlink(missing_ok=True)
                output = root / "outputs"
                output.write_text("")
                result = subprocess.run(["bash", "-euo", "pipefail", "-c", shell],
                                        env={**base_env, **overrides}, capture_output=True, text=True)
                # First case succeeds; remaining cases fail before the receipt and artifact upload.
                if not overrides:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    digest = hashlib.sha256(receipt.read_bytes()).hexdigest()
                    self.assertEqual(output.read_text(), f"receipt_sha256={digest}\n")
                    self.assertEqual({p.name for p in artifact_dir.iterdir()},
                                     {controller.LINUX_RECEIPT, controller.LINUX_PREPARED_ASSET})
                    receipt.unlink()
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(receipt.exists())
                    self.assertEqual(output.read_text(), "")
            upload = WORKFLOW.index("      - id: upload", WORKFLOW.index("Validate Linux artifact"))
            self.assertGreater(upload, WORKFLOW.index("Validate Linux artifact"))
            self.assertNotIn("publish:", WORKFLOW.split("  build-incident-linux:", 1)[1].split("  build-macos:", 1)[0])

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




class LinuxReceiptEvidenceTests(unittest.TestCase):
    def test_post_upload_summary_binds_exact_evidence_and_rejects_missing_upload(self):
        job = WORKFLOW.split("  build-incident-linux:", 1)[1].split("  build-macos:", 1)[0]
        evidence = job.split("      - name: Publish authenticated Linux receipt evidence", 1)[1]
        self.assertIn("if: ${{ success() && steps.upload.outcome == 'success' }}", evidence)
        self.assertLess(job.index("id: upload"), job.index("id: evidence"))
        self.assertIn("receipt_sha256: ${{ steps.evidence.outputs.receipt_sha256 }}", job)
        self.assertIn("artifact_id: ${{ steps.evidence.outputs.artifact_id }}", job)
        self.assertNotIn("continue-on-error", job)
        self.assertNotIn("always()", evidence)
        shell = textwrap.dedent(evidence.split("        run: |\n", 1)[1])
        with tempfile.TemporaryDirectory() as temp:
            summary, output = Path(temp) / "summary", Path(temp) / "output"
            env = dict(os.environ, RECEIPT_SHA256="a" * 64, ARTIFACT_ID="42",
                       APP_COMMIT=controller.CANDIDATE, CONTROLLER_COMMIT="b" * 40,
                       GITHUB_RUN_ID="123", GITHUB_RUN_ATTEMPT="2",
                       GITHUB_REPOSITORY="scottopell/phoenix-ide",
                       GITHUB_STEP_SUMMARY=str(summary), GITHUB_OUTPUT=str(output))
            for overrides in [
                {"ARTIFACT_ID": ""}, {"ARTIFACT_ID": "0"}, {"ARTIFACT_ID": "not-an-id"},
                {"RECEIPT_SHA256": ""}, {"RECEIPT_SHA256": "a" * 63},
                {"RECEIPT_SHA256": "g" * 64}, {},
            ]:
                summary.unlink(missing_ok=True)
                output.unlink(missing_ok=True)
                result = subprocess.run(["bash", "-euo", "pipefail", "-c", shell],
                                        env={**env, **overrides}, capture_output=True, text=True)
                if overrides:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(summary.exists())
                    self.assertFalse(output.exists())
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    for value in [f"receipt_sha256: `{'a' * 64}`", "artifact_id: `42`",
                                  "run_id: `123`", "run_attempt: `2`",
                                  f"app_commit: `{controller.CANDIDATE}`",
                                  f"controller_commit: `{'b' * 40}`",
                                  f"receipt_basename: `{controller.LINUX_RECEIPT}`",
                                  "--expected-preparation-receipt-sha256", "NOT the artifact archive digest"]:
                        self.assertIn(value, summary.read_text())
                    self.assertEqual(output.read_text(), f"receipt_sha256={'a' * 64}\nartifact_id=42\n")


if __name__ == "__main__":
    unittest.main()
