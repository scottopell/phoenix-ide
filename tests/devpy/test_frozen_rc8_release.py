import importlib.util
import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("frozen_rc8_release_test", ROOT / "scripts/frozen_rc8_release.py")
release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = release
SPEC.loader.exec_module(release)


class FrozenRc8ReleaseTests(unittest.TestCase):
    def test_fixed_release_emits_release_and_tag_admission(self):
        ci = {"id": 901, "run_attempt": 2}
        prep = {"id": release.PREPARATION_RUN}
        with mock.patch.object(release._PREP, "verify_candidate", return_value={}) as candidate, \
             mock.patch.object(release._PREP, "latest_ci_run", return_value=ci), \
             mock.patch.object(release, "verify_preparation_qualification", return_value=prep), \
             mock.patch.object(release, "verify_tag_unused") as tag:
            output = io.StringIO()
            with redirect_stdout(output):
                release.gate()
        candidate.assert_called_once()
        tag.assert_called_once()
        self.assertIn("release=true", output.getvalue())
        self.assertIn("create_tag=true", output.getvalue())
        self.assertIn("tag=v0.13.2-rc.8", output.getvalue())
        self.assertIn(f"commit={release.CANDIDATE}", output.getvalue())
        self.assertNotIn("preparation=true", output.getvalue())

    def test_nonempty_tag_input_refuses_before_qualification_or_candidate(self):
        with mock.patch.object(release._PREP, "verify_candidate") as candidate, \
             mock.patch.object(release, "verify_preparation_qualification") as qualification:
            with self.assertRaisesRegex(ValueError, "rejects release-tag input"):
                release.gate(tag_input="v0.13.2-rc.8")
        candidate.assert_not_called()
        qualification.assert_not_called()

    def test_protected_preparation_must_be_completed_success_exact_controller(self):
        wrong = {"id": release.PREPARATION_RUN, "path": ".github/workflows/release.yml",
                 "event": "workflow_dispatch", "head_branch": "main", "head_sha": release.PREPARATION_CONTROLLER,
                 "run_attempt": 1, "status": "in_progress", "conclusion": None}
        with mock.patch.object(release, "api_json", return_value=wrong):
            with self.assertRaisesRegex(ValueError, "status"):
                release.verify_preparation_qualification()

    def test_protected_preparation_requires_both_native_payload_and_acceptance_artifacts(self):
        run = {"id": release.PREPARATION_RUN, "path": ".github/workflows/release.yml",
               "event": "workflow_dispatch", "head_branch": "main", "head_sha": release.PREPARATION_CONTROLLER,
               "run_attempt": 1, "status": "completed", "conclusion": "success"}
        artifacts = {"artifacts": [{"name": name, "expired": False} for name in (
            f"desktop-preparation-aarch64-apple-darwin-{release.CANDIDATE}",
            f"desktop-preparation-x86_64-apple-darwin-{release.CANDIDATE}",
            f"headless-acceptance-aarch64-apple-darwin-{release.CANDIDATE}-{release.PREPARATION_RUN}-1-success",
            f"headless-acceptance-x86_64-apple-darwin-{release.CANDIDATE}-{release.PREPARATION_RUN}-1-success")]}
        with mock.patch.object(release, "api_json", side_effect=[run, artifacts]):
            self.assertEqual(run, release.verify_preparation_qualification())
        artifacts["artifacts"].pop()
        with mock.patch.object(release, "api_json", side_effect=[run, artifacts]):
            with self.assertRaisesRegex(ValueError, "missing expected native artifacts"):
                release.verify_preparation_qualification()

    def test_existing_tag_or_newer_version_refuses(self):
        for tags in (release.TAG, "v0.13.2-rc.9", "v0.13.2", "v0.14.0"):
            with self.subTest(tags=tags), mock.patch.object(release.subprocess, "check_output", return_value=("a" * 40 + "\trefs/tags/" + tags + "\n").encode()):
                with self.assertRaises((ValueError, release.subprocess.CalledProcessError)):
                    release.verify_tag_unused(ROOT)

    def test_older_tag_and_fetch_succeed(self):
        with mock.patch.object(release.subprocess, "check_output", return_value=b"86071b4cc2ab0f9efdb8a3debf03b8569baf26f9\trefs/tags/v0.13.2-rc.7\n") as remote:
            release.verify_tag_unused(ROOT)
        remote.assert_called_once()
        self.assertIn("ls-remote", remote.call_args.args[0])

    def test_wrong_preparation_identity_refuses_each_field(self):
        run = dict(id=release.PREPARATION_RUN, path=".github/workflows/release.yml",
                   event="workflow_dispatch", head_sha=release.PREPARATION_CONTROLLER,
                   head_branch="main", run_attempt=1, status="completed", conclusion="success")
        for key in run:
            with self.subTest(key=key), mock.patch.object(release, "api_json", return_value={**run, key: "wrong"}):
                with self.assertRaises(ValueError):
                    release.verify_preparation_qualification()

    def test_workflow_has_exact_rc8_release_separation_and_candidate_tag_target(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("release-frozen-rc8", workflow)
        case = workflow.split("              release-frozen-rc8)\n", 1)[1].split("                ;;", 1)[0]
        self.assertLess(case.index('[[ -z "$RETRY_TAG" ]]'), case.index("git fetch --no-tags origin refs/heads/candidate/rc8-helper-correction"))
        self.assertIn('test "$(git rev-parse FETCH_HEAD)" = "$GITHUB_SHA"', case)
        self.assertIn(release.CANDIDATE, case)
        self.assertIn("scripts/frozen_rc8_release.py gate", case)
        self.assertIn("(needs.gate.outputs.operation == 'release-frozen-rc7' || needs.gate.outputs.operation == 'release-frozen-rc8')", workflow)
        tag_case = workflow.split('if [[ "${{ needs.gate.outputs.operation }}" == release-frozen-rc7', 1)[1].split("          else", 1)[0]
        self.assertIn('git tag -a "$TAG" "$COMMIT"', tag_case)
        self.assertIn("cancel-in-progress: false", workflow)
        preparation = workflow.split("              prepare-frozen-rc8)\n", 1)[1].split("                ;;", 1)[0]
        self.assertIn('echo "release=false"', (ROOT / ".github/workflows/release.yml").read_text())
        self.assertIn('python3 "$RUNNER_TEMP/frozen_rc8_prepare.py" gate', preparation)
        self.assertIn('exit 0', preparation)
        self.assertNotIn("release-frozen-rc8", preparation)


if __name__ == "__main__":
    unittest.main()
