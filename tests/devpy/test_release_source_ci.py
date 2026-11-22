import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_source_ci", ROOT / "scripts/release_source_ci.py")
ci = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci)
SHA = "a" * 40


def run(run_id=1, number=1, **changes):
    value = {"id": run_id, "run_number": number, "run_attempt": 1,
             "path": ci.WORKFLOW, "head_sha": SHA, "event": "push",
             "head_branch": "main", "status": "completed", "conclusion": "success"}
    return value | changes


class Clock:
    def __init__(self):
        self.now = 0
        self.sleeps = []

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds


class ReleaseSourceCiTests(unittest.TestCase):
    def qualify_with(self, responses, *, detail=None, deadline=60):
        clock = Clock()
        calls = []
        index = 0

        def api(endpoint, timeout):
            calls.append((endpoint, timeout))
            if "/actions/runs/" in endpoint:
                if isinstance(detail, Exception):
                    raise detail
                return detail if detail is not None else run()
            nonlocal index
            response = responses[min(index, len(responses) - 1)]
            index += 1
            if isinstance(response, Exception):
                raise response
            return response

        with mock.patch.object(ci, "DEADLINE_SECONDS", deadline):
            result = ci.qualify(SHA, api=api, monotonic=clock.monotonic, sleep=clock.sleep)
        return result, calls, clock

    def test_discovery_pending_then_success_and_output(self):
        pending = run(status="in_progress", conclusion=None)
        result, calls, clock = self.qualify_with([
            {"workflow_runs": []}, {"workflow_runs": [pending]},
            {"workflow_runs": [run(run_id=12, number=8, run_attempt=3)]},
        ], detail=run(run_id=12, number=8, run_attempt=3), deadline=90)
        self.assertEqual(result, (12, 3))
        self.assertEqual(clock.sleeps, [30, 30])
        self.assertEqual(calls[-1][0], "repos/scottopell/phoenix-ide/actions/runs/12")
        self.assertLessEqual(max(timeout for _, timeout in calls), 20)
        self.assertIn("head_sha=" + SHA, calls[0][0])
        self.assertIn("event=push", calls[0][0])
        self.assertIn("per_page=100", calls[0][0])

    def test_missing_run_times_out(self):
        clock = Clock()
        with mock.patch.object(ci, "DEADLINE_SECONDS", 45), self.assertRaisesRegex(ValueError, "timed out"):
            ci.qualify(SHA, api=lambda *_: {"workflow_runs": []}, monotonic=clock.monotonic, sleep=clock.sleep)
        self.assertEqual(clock.sleeps, [30, 15])

    def test_wrong_metadata_is_not_qualified(self):
        for field, value in (("head_sha", "b" * 40), ("path", "other.yml"),
                             ("event", "workflow_dispatch"), ("head_branch", "release")):
            with self.subTest(field=field):
                clock = Clock()
                bad = run(**{field: value})
                with mock.patch.object(ci, "DEADLINE_SECONDS", 1), self.assertRaisesRegex(ValueError, "timed out"):
                    ci.qualify(SHA, api=lambda *_: {"workflow_runs": [bad]},
                               monotonic=clock.monotonic, sleep=clock.sleep)

    def test_malformed_data_and_api_errors_reject(self):
        for payload in (None, {}, {"workflow_runs": {}}, {"workflow_runs": [None]},
                        {"workflow_runs": [run(run_number="one")]},
                        {"workflow_runs": [run(run_attempt=0)]}):
            with self.subTest(payload=payload), self.assertRaises(ValueError):
                self.qualify_with([payload])
        with self.assertRaisesRegex(ValueError, "network"):
            self.qualify_with([ValueError("network failure")])

    def test_terminal_failure_and_cancellation_reject(self):
        for conclusion in ("failure", "cancelled"):
            with self.subTest(conclusion=conclusion), self.assertRaisesRegex(ValueError, "did not succeed"):
                self.qualify_with([{"workflow_runs": [run(conclusion=conclusion)]}])

    def test_latest_run_wins_over_older_success(self):
        older = run(run_id=1, number=2)
        for latest in (run(run_id=2, number=3, conclusion="failure"),
                       run(run_id=2, number=3, status="in_progress", conclusion=None)):
            with self.subTest(latest=latest):
                clock = Clock()
                calls = []
                def api(endpoint, timeout):
                    calls.append(endpoint)
                    return {"workflow_runs": [older, latest]}
                with mock.patch.object(ci, "DEADLINE_SECONDS", 1), self.assertRaises(ValueError):
                    ci.qualify(SHA, api=api, monotonic=clock.monotonic, sleep=clock.sleep)
                self.assertNotIn("/actions/runs/1", calls)

    def test_run_by_id_identity_and_current_attempt_rechecked(self):
        listing = run(run_id=7, number=4, run_attempt=2)
        for changed in (run(run_id=8, number=4, run_attempt=2),
                        run(run_id=7, number=4, run_attempt=1),
                        run(run_id=7, number=4, conclusion="failure")):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                self.qualify_with([{"workflow_runs": [listing]}], detail=changed)

    def test_cli_prints_only_qualification_values(self):
        output = []
        with mock.patch.object(ci, "qualify", return_value=(42, 2)), mock.patch("builtins.print", side_effect=output.append):
            ci.main([SHA])
        self.assertEqual(output, ["qualification_ci_run=42", "qualification_ci_attempt=2"])

    def test_invalid_sha_and_argument_shape(self):
        for sha in ("A" * 40, "a" * 39, "a" * 41, "g" * 40, ""):
            with self.subTest(sha=sha), self.assertRaisesRegex(ValueError, "SHA"):
                ci.qualify(sha)
        for args in ([], [SHA, SHA]):
            with self.subTest(args=args), self.assertRaisesRegex(ValueError, "usage"):
                ci.main(args)

    def test_api_uses_authenticated_gh_and_json(self):
        completed = mock.Mock(stdout=json.dumps({"ok": True}))
        with mock.patch.object(ci.subprocess, "run", return_value=completed) as launch:
            self.assertEqual(ci._api("repos/example", 9), {"ok": True})
        args, kwargs = launch.call_args
        self.assertEqual(args[0], ["gh", "api", "repos/example"])
        self.assertEqual(kwargs["timeout"], 9)
        self.assertTrue(kwargs["check"])


class WorkflowBoundaryTests(unittest.TestCase):
    workflow = (ROOT / ".github/workflows/release.yml").read_text()
    gate = workflow.split("  gate:\n", 1)[1].split("  tag-release:\n", 1)[0]
    step = gate.split("      - name: Qualify exact ordinary release source CI\n", 1)[1].split("      - name:", 1)[0]

    def test_actual_condition_selects_only_admitted_ordinary_releases(self):
        condition = re.search(r"^        if: (.+)$", self.step, re.M).group(1)
        for release in ("true", "false"):
            for event in ("push", "workflow_dispatch"):
                for operation in ("", "retry-release", "release-frozen-rc7", "release-frozen-rc8",
                                  "prepare-main", "prepare-frozen-rc8",
                                  "prepare-incident-237100-arm64", "prepare-incident-237100-linux-x86_64"):
                    expression = condition.replace("steps.ver.outputs.release", repr(release)).replace(
                        "github.event_name", repr(event)).replace("inputs.operation", repr(operation))
                    expression = expression.replace("&&", " and ").replace("||", " or ")
                    actual = eval(expression, {"__builtins__": {}})
                    self.assertEqual(actual, release == "true" and (event == "push" or operation == "retry-release"))

    def test_actual_command_qualifies_exact_source_and_fails_without_outputs(self):
        command = re.search(r"^        run: (.+)$", self.step, re.M).group(1)
        for success in (True, False):
            with self.subTest(success=success), tempfile.TemporaryDirectory() as directory:
                tmp = Path(directory)
                gh = tmp / "gh"
                gh.write_text("#!" + sys.executable + "\n" +
                              "import json, os, sys\n" +
                              "assert os.environ['GH_TOKEN'] == 'fixture-token'\n" +
                              "assert sys.argv[1] == 'api'\n" +
                              "value = json.loads(os.environ['RUN_JSON'])\n" +
                              "print(json.dumps(value if '/actions/runs/' in sys.argv[2] else {'workflow_runs': [value]}))\n")
                gh.chmod(0o755)
                output = tmp / "output"
                value = run(conclusion="success" if success else "failure")
                result = subprocess.run(["bash", "-e", "-c", command], cwd=ROOT, text=True,
                    capture_output=True, env=dict(os.environ, PATH=str(tmp)+os.pathsep+os.environ["PATH"],
                    GH_TOKEN="fixture-token", SOURCE_COMMIT=SHA, GITHUB_OUTPUT=str(output), RUN_JSON=json.dumps(value)),
                    timeout=10)
                self.assertEqual(result.returncode == 0, success, result.stderr)
                self.assertEqual(output.read_text(), "qualification_ci_run=1\nqualification_ci_attempt=1\n" if success else "")

    def test_gate_owns_qualification_before_any_mutation_job(self):
        self.assertIn("actions: read", self.gate)
        self.assertNotIn("contents: write", self.gate)
        self.assertIn("timeout-minutes: 65", self.gate)
        self.assertIn("timeout-minutes: 61", self.step)
        self.assertIn("GH_TOKEN: ${{ github.token }}", self.step)
        self.assertIn("SOURCE_COMMIT: ${{ steps.ver.outputs.commit }}", self.step)
        tag_job = self.workflow.split("  tag-release:\n", 1)[1].split("  build-linux:\n", 1)[0]
        self.assertIn("needs: gate", tag_job)
        self.assertIn("if: needs.gate.outputs.create_tag == 'true'", tag_job)
        self.assertNotIn("always()", tag_job)
        self.assertNotIn("continue-on-error", self.gate)
        for job in ("build-linux", "build-macos", "publish"):
            section = self.workflow.split("  " + job + ":\n", 1)[1]
            self.assertIn("needs.gate.result == 'success'", section.split("steps:", 1)[0])


if __name__ == "__main__":
    unittest.main()
