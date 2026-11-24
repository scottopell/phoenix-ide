"""Focused source-gate and release-workflow regressions; no builds or network."""

import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/frozen_rc8_prepare.py"
spec = importlib.util.spec_from_file_location("frozen_rc8_prepare", SCRIPT)
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)
WORKFLOW = (ROOT / ".github/workflows/release.yml").read_text()
PINNED = {name: getattr(controller, name) for name in ("APPLICATION_BASE", "BASE", "CANDIDATE", "TREE", "HELPER_SHA256")}


class GitFixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.env = dict(os.environ, GIT_AUTHOR_NAME="fixture", GIT_AUTHOR_EMAIL="f@example.test",
                        GIT_COMMITTER_NAME="fixture", GIT_COMMITTER_EMAIL="f@example.test")
        self.git("init", "-q")
        self.write_candidate_files()
        self.application_base = self.commit("application base")
        self.base = self.commit("frozen RC7 version")
        self.write_candidate_files(version=controller.VERSION)
        self.candidate = self.commit("candidate")
        self.git("update-ref", "refs/remotes/origin/main", self.commit("main"))
        self.pins = contextlib.ExitStack()
        self.addCleanup(self.pins.close)
        self.pins.enter_context(mock.patch.object(controller, "APPLICATION_BASE", self.application_base))
        self.pins.enter_context(mock.patch.object(controller, "BASE", self.base))
        self.pins.enter_context(mock.patch.object(controller, "CANDIDATE", self.candidate))
        self.pins.enter_context(mock.patch.object(controller, "TREE", self.git("show", "-s", "--format=%T", self.candidate)))
        self.pins.enter_context(mock.patch.object(controller, "HELPER_SHA256", hashlib.sha256(b"candidate helper\n").hexdigest()))

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], env=self.env,
                                       stderr=subprocess.DEVNULL).decode().strip()

    def write_candidate_files(self, version="0.13.2-rc.7"):
        manifest = self.repo / "crates/phoenix-ide/Cargo.toml"
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(f'[package]\nversion = "{version}"\n')
        (self.repo / "Cargo.lock").write_text(
            f'[[package]]\nname = "phoenix_ide"\nversion = "{version}"\n')
        helper = self.repo / controller.HELPER
        helper.parent.mkdir(parents=True, exist_ok=True)
        helper.write_bytes(b"candidate helper\n")

    def commit(self, message):
        self.git("add", ".")
        self.git("commit", "--allow-empty", "-qm", message)
        return self.git("rev-parse", "HEAD")


class CandidateVerificationTests(GitFixture):
    def test_accepts_real_git_candidate_objects_without_checkout_mutation(self):
        before = self.git("status", "--porcelain")
        result = controller.verify_candidate(self.repo)
        self.assertEqual(result["commit"], self.candidate)
        self.assertEqual(result["tree"], controller.TREE)
        self.assertEqual(self.git("status", "--porcelain"), before)

    def test_accepts_main_divergent_from_frozen_rc7_at_application_base(self):
        self.git("checkout", "--detach", self.application_base)
        main = self.commit("main application work")
        self.git("update-ref", "refs/remotes/origin/main", main)
        with self.assertRaises(subprocess.CalledProcessError):
            self.git("merge-base", "--is-ancestor", self.base, main)
        self.assertEqual(controller.verify_candidate(self.repo)["commit"], self.candidate)

    def test_rejects_main_without_application_base_ancestry(self):
        tree = self.git("mktree")
        unrelated = subprocess.check_output(
            ["git", "-C", str(self.repo), "commit-tree", tree, "-m", "unrelated main"], env=self.env).decode().strip()
        self.git("update-ref", "refs/remotes/origin/main", unrelated)
        with self.assertRaisesRegex(ValueError, "ancestry"):
            controller.verify_candidate(self.repo)

    def test_rejects_wrong_application_base_pin(self):
        with mock.patch.object(controller, "APPLICATION_BASE", self.candidate), self.assertRaisesRegex(ValueError, "application base"):
            controller.verify_candidate(self.repo)

    def test_rejects_wrong_tree_pin(self):
        with mock.patch.object(controller, "TREE", "0" * 40), self.assertRaisesRegex(ValueError, "tree"):
            controller.verify_candidate(self.repo)

    def test_rejects_wrong_helper_hash(self):
        with mock.patch.object(controller, "HELPER_SHA256", "0" * 64), self.assertRaisesRegex(ValueError, "SHA-256"):
            controller.verify_candidate(self.repo)

    def test_rejects_wrong_candidate_commit_pin(self):
        with mock.patch.object(controller, "CANDIDATE", "f" * 40), self.assertRaises(ValueError):
            controller.verify_candidate(self.repo)

    def test_rejects_non_ancestor_base(self):
        unrelated = self.git("mktree")
        unrelated_commit = subprocess.check_output(
            ["git", "-C", str(self.repo), "commit-tree", unrelated, "-m", "unrelated"], env=self.env).decode().strip()
        with mock.patch.object(controller, "BASE", unrelated_commit), self.assertRaisesRegex(ValueError, "ancestry"):
            controller.verify_candidate(self.repo)

    def test_rejects_bad_manifest_version(self):
        self.write_candidate_files(version="0.13.2-rc.7")
        bad = self.commit("bad manifest")
        with mock.patch.object(controller, "CANDIDATE", bad), \
             mock.patch.object(controller, "TREE", self.git("show", "-s", "--format=%T", bad)), \
             self.assertRaisesRegex(ValueError, "manifest"):
            controller.verify_candidate(self.repo)

    def test_rejects_bad_lockfile_version(self):
        (self.repo / "Cargo.lock").write_text('[[package]]\nname = "phoenix_ide"\nversion = "0.13.2-rc.7"\n')
        bad = self.commit("bad lock")
        with mock.patch.object(controller, "CANDIDATE", bad), \
             mock.patch.object(controller, "TREE", self.git("show", "-s", "--format=%T", bad)), \
             self.assertRaisesRegex(ValueError, "lockfile"):
            controller.verify_candidate(self.repo)


class PinnedCandidateTests(unittest.TestCase):
    def test_real_pinned_candidate_git_objects_pass_exact_source_gate(self):
        try:
            tree = controller.git(ROOT, "show", "-s", "--format=%T", PINNED["CANDIDATE"]).decode().strip()
        except subprocess.CalledProcessError:
            self.skipTest("frozen candidate object is unavailable in this checkout")
        self.assertEqual(tree, PINNED["TREE"])
        with mock.patch.multiple(controller, **PINNED):
            result = controller.verify_candidate(ROOT)
        self.assertEqual(result["commit"], PINNED["CANDIDATE"])


class EvidenceAndCliTests(unittest.TestCase):
    def test_latest_ci_requires_exact_candidate_and_success(self):
        runs = [{"path": controller.CI_WORKFLOW, "head_sha": controller.CANDIDATE,
                 "event": "workflow_dispatch", "run_number": 8, "id": 44,
                 "status": "completed", "conclusion": "success"}]
        with mock.patch.object(controller.urllib.request, "urlopen") as open_url:
            response = mock.MagicMock()
            response.__enter__.return_value.read.return_value = json.dumps({"workflow_runs": runs}).encode()
            open_url.return_value = response
            self.assertEqual(controller.latest_ci_run()["id"], 44)


    def test_latest_ci_rejects_failure_or_absence(self):
        for payload in ({"workflow_runs": []}, {"workflow_runs": [{
                "path": controller.CI_WORKFLOW, "head_sha": controller.CANDIDATE,
                "event": "workflow_dispatch", "run_number": 1,
                "status": "completed", "conclusion": "failure"}]}):
            with self.subTest(payload=payload), mock.patch.object(controller.urllib.request, "urlopen") as open_url:
                response = mock.MagicMock()
                response.__enter__.return_value.read.return_value = json.dumps(payload).encode()
                open_url.return_value = response
                with self.assertRaises(ValueError):
                    controller.latest_ci_run()

    def test_gate_rejects_tag_input_and_emits_nonrelease_outputs(self):
        with self.assertRaisesRegex(ValueError, "release-tag"):
            controller.gate(tag_input="v0.13.2-rc.8")
        with mock.patch.object(controller, "verify_candidate", return_value={
                "commit": controller.CANDIDATE, "tree": controller.TREE,
                "helper_sha256": controller.HELPER_SHA256}), \
             mock.patch.object(controller, "latest_ci_run", return_value={"id": 123, "run_attempt": 2}), \
             mock.patch("sys.stdout", new_callable=__import__("io").StringIO) as output:
            controller.gate()
            self.assertIn("release=false", output.getvalue())
            self.assertIn("create_tag=false", output.getvalue())
            self.assertIn("tag=\n", output.getvalue())
            values = dict(line.split("=", 1) for line in output.getvalue().splitlines())
            self.assertEqual(values["preparation"], "true")
            self.assertEqual(values["commit"], controller.CANDIDATE)
            self.assertEqual(values["version"], controller.VERSION)
            self.assertEqual(values["qualification_ci_run"], "123")
            self.assertEqual(values["qualification_ci_attempt"], "2")

    def test_provenance_binds_candidate_controller_and_workflow_run(self):
        result = controller.provenance_json("a" * 40, "42", "2", "123", "1")
        self.assertEqual(result["source_commit"], controller.CANDIDATE)
        self.assertEqual(result["controller_commit"], "a" * 40)
        self.assertEqual((result["run_id"], result["run_attempt"]), (42, 2))
        self.assertEqual(result["qualification_ci_run"], 123)
        self.assertTrue(result["qualification_ci_url"].endswith("/123/attempts/1"))
        for args in [(controller.CANDIDATE, "42", "2"), ("A" * 40, "42", "2"),
                     ("a" * 40, "0", "2"), ("a" * 40, "42", "0")]:
            with self.subTest(args=args), self.assertRaises(ValueError):
                controller.provenance_json(*args, "123", "1")

    def test_cli_rejects_source_version_tag_and_run_selectors(self):
        for args in (["release", "v1"], ["tag", "v1"], ["source", "HEAD"], ["run", "1"]):
            with self.subTest(args=args), self.assertRaises(SystemExit):
                controller.main(list(args))


class WorkflowBoundaryTests(unittest.TestCase):
    def test_rc8_controller_is_materialized_from_main_commit_outside_candidate_checkout(self):
        self.assertIn('git show "$GITHUB_SHA:scripts/frozen_rc8_prepare.py" > "$RUNNER_TEMP/frozen_rc8_prepare.py"', WORKFLOW)
        self.assertIn('git fetch --no-tags origin "$CONTROLLER_COMMIT"', WORKFLOW)
        self.assertIn('git show "$CONTROLLER_COMMIT:scripts/frozen_rc8_prepare.py" > "$RUNNER_TEMP/frozen_rc8_prepare.py"', WORKFLOW)
        self.assertIn('python3 "$RUNNER_TEMP/frozen_rc8_prepare.py" provenance', WORKFLOW)
        self.assertNotIn('python3 scripts/frozen_rc8_prepare.py', WORKFLOW)

    def test_matrix_uses_existing_both_architecture_default(self):
        self.assertNotIn('OPERATION" == prepare-frozen-rc8 ]]; then\n            echo \'matrix=', WORKFLOW)
        self.assertIn('macos-15-intel', WORKFLOW)

    def test_gate_is_pinned_nonrelease_and_checks_latest_ci(self):
        gate = WORKFLOW.split('prepare-frozen-rc8)', 1)[1].split('release-frozen-rc7)', 1)[0]
        self.assertIn('Frozen RC8 preparation does not accept a release tag', gate)
        self.assertIn('git show "$GITHUB_SHA:scripts/frozen_rc8_prepare.py"', gate)
        self.assertIn('controller_commit: ${{ github.sha }}', WORKFLOW)
        self.assertIn('release=false', SCRIPT.read_text())
        self.assertIn('latest_ci_run()', SCRIPT.read_text())


class ExecutionBoundaryTests(unittest.TestCase):
    def test_provenance_cli_executes_from_controller_file_with_candidate_cwd(self):
        candidate_checkout = ROOT.parent.parent
        checkout = subprocess.run(["git", "-C", str(candidate_checkout), "rev-parse", "HEAD"],
                                  capture_output=True, text=True)
        if checkout.returncode or checkout.stdout.strip() != controller.CANDIDATE:
            self.skipTest("immutable candidate checkout not available")
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "proof.json"
            subprocess.run([sys.executable, str(SCRIPT), "provenance", str(output),
                            "a" * 40, "42", "2", "123", "1"],
                           cwd=candidate_checkout, check=True, capture_output=True)
            self.assertEqual(json.loads(output.read_text())["source_commit"], controller.CANDIDATE)

    def test_provenance_cli_rejects_wrong_checkout(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "proof.json"
            result = subprocess.run([sys.executable, str(SCRIPT), "provenance", str(output),
                                     "a" * 40, "42", "2", "123", "1"],
                                    cwd=ROOT, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("checked-out RC8", result.stderr)
            self.assertFalse(output.exists())

    def test_provenance_cli_rejects_modified_helper(self):
        with mock.patch.object(controller, "git", side_effect=[controller.TREE.encode(), controller.CANDIDATE.encode()]), \
             mock.patch("builtins.open", mock.mock_open(read_data=b"modified helper")):
            with self.assertRaisesRegex(ValueError, "checked-out RC8"):
                controller.main(["provenance", "out.json", "a" * 40, "42", "2", "123", "1"])

    def test_gate_and_publisher_guards_remain_structural(self):
        gate = WORKFLOW.split("  gate:\n", 1)[1].split("  tag-release:\n", 1)[0]
        self.assertIn("if: github.ref == 'refs/heads/main'", gate)
        tag = WORKFLOW.split("  tag-release:\n", 1)[1].split("  build-linux:\n", 1)[0]
        self.assertIn("if: needs.gate.outputs.create_tag == 'true'", tag)
        linux = WORKFLOW.split("  build-linux:\n", 1)[1].split("    steps:", 1)[0]
        publish = WORKFLOW.split("  publish:\n", 1)[1].split("    steps:", 1)[0]
        macos = WORKFLOW.split("  build-macos:\n", 1)[1].split("    steps:", 1)[0]
        self.assertIn("needs.gate.outputs.release == 'true'", linux)
        self.assertIn("needs.gate.outputs.release == 'true'", publish)
        self.assertIn("environment: macos-release-signing", macos)

    def test_actual_rc8_shell_case_exits_without_release_fallthrough(self):
        import os
        case = WORKFLOW.split("              prepare-frozen-rc8)\n", 1)[1].split("                ;;", 1)[0]
        script = "set -euo pipefail\n" + case + "\necho FALLTHROUGH\n"
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            git = temp / "git"
            git.write_text('''#!/bin/bash
if [[ "$1" == rev-parse ]]; then
  if [[ "$2" == origin/candidate/* ]]; then echo "$TEST_CANDIDATE"; else echo "$GITHUB_SHA"; fi
elif [[ "$1" == show ]]; then
  echo 'print("preparation=true\\nrelease=false\\ncreate_tag=false\\ntag=")'
fi
''')
            git.chmod(0o755)
            env = dict(os.environ, PATH=str(temp) + os.pathsep + os.environ["PATH"],
                       GITHUB_SHA="a" * 40, TEST_CANDIDATE=controller.CANDIDATE,
                       RUNNER_TEMP=str(temp), RETRY_TAG="", GITHUB_OUTPUT=str(temp / "outputs"))
            good = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)
            self.assertEqual(good.returncode, 0, good.stderr)
            self.assertNotIn("FALLTHROUGH", good.stdout)
            self.assertIn("release=false", (temp / "outputs").read_text())
            for changes in ({"RETRY_TAG": "v0.13.2-rc.8"}, {"TEST_CANDIDATE": "b" * 40}):
                bad = subprocess.run(["bash", "-c", script], env=dict(env, **changes), capture_output=True, text=True)
                self.assertNotEqual(bad.returncode, 0)
                self.assertNotIn("FALLTHROUGH", bad.stdout)


if __name__ == "__main__":
    unittest.main()
