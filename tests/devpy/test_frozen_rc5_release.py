import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("frozen_rc5_release_test", ROOT / "scripts/frozen_rc5_release.py")
release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = release
SPEC.loader.exec_module(release)


RC9_SPEC = importlib.util.spec_from_file_location("frozen_rc9_prepare_test", ROOT / "scripts/frozen_rc8_prepare.py")
rc9_prepare = importlib.util.module_from_spec(RC9_SPEC)
sys.modules[RC9_SPEC.name] = rc9_prepare
RC9_SPEC.loader.exec_module(rc9_prepare)


def git(repo, *args, input=None):
    return subprocess.run(["git", "-C", str(repo), *args], input=input, check=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout.decode().strip()


class FrozenRc7GateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="frozen-rc5-test-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        git(self.repo, "init", "-q")
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "config", "user.email", "test@example.invalid")
        (self.repo / "crates/phoenix-ide").mkdir(parents=True)
        (self.repo / "crates/phoenix-ide/Cargo.toml").write_bytes(
            b'[package]\nname = "phoenix_ide"\nversion = "0.13.2-rc.4"\n')
        (self.repo / "Cargo.lock").write_bytes(
            b'[[package]]\nname = "phoenix_ide"\nversion = "0.13.2-rc.4"\n')
        (self.repo / "stable.txt").write_text("unchanged\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "base")
        self.base = git(self.repo, "rev-parse", "HEAD")
        self.patch_base = mock.patch.object(release, "BASE", self.base)
        git(self.repo, "update-ref", "refs/remotes/origin/main", self.base)
        self.patch_base.start()
        self.addCleanup(self.patch_base.stop)

    def candidate(self, mutate=None, mode=None, parent=None):
        git(self.repo, "checkout", "-q", "--detach", parent or self.base)
        manifest = self.repo / "crates/phoenix-ide/Cargo.toml"
        lock = self.repo / "Cargo.lock"
        manifest.write_bytes(manifest.read_bytes().replace(b"0.13.2-rc.4", b"0.13.2-rc.7"))
        lock.write_bytes(lock.read_bytes().replace(b'phoenix_ide"\nversion = "0.13.2-rc.4',
                                                    b'phoenix_ide"\nversion = "0.13.2-rc.7'))
        if mutate:
            mutate(self.repo)
        if mode:
            os.chmod(manifest, mode)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "candidate")
        candidate = git(self.repo, "rev-parse", "HEAD")
        with mock.patch.object(release, "CANDIDATE", candidate):
            return release.verify_candidate(self.repo)

    def test_accepts_exact_version_only_candidate(self):
        self.assertTrue(self.candidate())

    def test_frozen_application_base_must_be_on_main(self):
        self.assertTrue(release.verify_frozen_base_on_main(self.repo))
        tree = git(self.repo, "rev-parse", self.base + "^{tree}")
        other = git(self.repo, "commit-tree", tree, input=b"unrelated main\n")
        git(self.repo, "update-ref", "refs/remotes/origin/main", other)
        with self.assertRaisesRegex(ValueError, "not an ancestor of main"):
            release.verify_frozen_base_on_main(self.repo)

    def test_rejects_extra_path(self):
        def add_extra(repo):
            (repo / "extra.txt").write_text("unexpected")
        with self.assertRaisesRegex(ValueError, "tracked paths"):
            self.candidate(mutate=add_extra)

    def test_rejects_deleted_path(self):
        def delete_stable(repo):
            (repo / "stable.txt").unlink()
        with self.assertRaisesRegex(ValueError, "tracked paths"):
            self.candidate(mutate=delete_stable)

    def test_rejects_version_mismatch(self):
        def wrong_version(repo):
            path = repo / "crates/phoenix-ide/Cargo.toml"
            path.write_bytes(path.read_bytes().replace(b"0.13.2-rc.7", b"0.13.2-rc.5"))
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mutate=wrong_version)

    def test_rejects_blob_change_outside_two_version_files(self):
        def edit(repo):
            (repo / "stable.txt").write_text("changed\n")
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mutate=edit)

    def test_rejects_mode_change(self):
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mode=0o755)

    def test_rejects_symlink_replacement(self):
        def replace(repo):
            (repo / "stable.txt").unlink()
            (repo / "stable.txt").symlink_to("Cargo.lock")
        with self.assertRaisesRegex(ValueError, "exact two-file/version-only"):
            self.candidate(mutate=replace)

    def test_rejects_empty_subtree(self):
        empty = subprocess.check_output(["git", "-C", str(self.repo), "mktree"], input=b"").decode().strip()
        listing = subprocess.check_output(["git", "-C", str(self.repo), "ls-tree", self.base])
        root = subprocess.check_output(["git", "-C", str(self.repo), "mktree"],
                                      input=listing + f"040000 tree {empty}\tempty\n".encode()).decode().strip()
        with self.assertRaisesRegex(ValueError, "empty subtree"):
            release.tree(self.repo, root)

    def test_rejects_wrong_parent(self):
        git(self.repo, "checkout", "-q", "--detach", self.base)
        (self.repo / "stable.txt").write_text("different parent\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "other parent")
        wrong_parent = git(self.repo, "rev-parse", "HEAD")
        with self.assertRaisesRegex(ValueError, "sole parent"):
            self.candidate(parent=wrong_parent)

    def test_ci_gate_requires_exact_completed_success(self):
        good = {"path": ".github/workflows/ci.yml", "head_sha": release.CANDIDATE,
                "event": "workflow_dispatch", "status": "completed", "conclusion": "success",
                "run_number": 12, "id": 77}
        with mock.patch.object(release.urllib.request, "urlopen") as open_url:
            for runs in ([], [dict(good, event="push")], [dict(good, head_sha="f" * 40)],
                         [dict(good, status="in_progress")], [dict(good, conclusion="failure")]):
                payload = __import__("io").BytesIO(__import__("json").dumps({"workflow_runs": runs}).encode())
                open_url.return_value = __import__("contextlib").nullcontext(payload)
                with self.assertRaises(ValueError):
                    release.workflow_dispatch_ci_run()
            payload = __import__("io").BytesIO(__import__("json").dumps({"workflow_runs": [good]}).encode())
            open_url.return_value = __import__("contextlib").nullcontext(payload)
            self.assertEqual(77, release.workflow_dispatch_ci_run()["id"])

    def test_newest_matching_ci_run_must_succeed(self):
        successful = {"path": ".github/workflows/ci.yml", "head_sha": release.CANDIDATE,
                      "event": "workflow_dispatch", "status": "completed", "conclusion": "success",
                      "run_number": 12, "id": 77}
        newer_failure = dict(successful, run_number=13, id=78, conclusion="failure")
        payload = __import__("io").BytesIO(__import__("json").dumps(
            {"workflow_runs": [successful, newer_failure]}).encode())
        with mock.patch.object(release.urllib.request, "urlopen",
                               return_value=__import__("contextlib").nullcontext(payload)):
            with self.assertRaisesRegex(ValueError, "latest exact-candidate"):
                release.workflow_dispatch_ci_run()

    def test_workflow_wires_only_the_frozen_rc7_controller(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("release-frozen-rc7", workflow)
        self.assertIn("refs/heads/release/frozen-rc7", workflow)
        self.assertIn(release.CANDIDATE, workflow)
        self.assertIn("refs/heads/main", workflow)
        self.assertIn('test \"$(git rev-parse FETCH_HEAD)\" = \"$GITHUB_SHA\"', workflow)
        self.assertIn("(needs.gate.outputs.operation == 'release-frozen-rc7' || needs.gate.outputs.operation == 'release-frozen-rc8') && needs.gate.outputs.controller_commit", workflow)
        self.assertIn('"${{ needs.gate.outputs.operation }}" == release-frozen-rc7 || "${{ needs.gate.outputs.operation }}" == release-frozen-rc8', workflow)
        self.assertIn("86071b4cc2ab0f9efdb8a3debf03b8569baf26f9", workflow)
        frozen_case = workflow.split("              release-frozen-rc7)", 1)[1].split("                ;;", 1)[0]
        self.assertLess(frozen_case.index('[[ -z "$RETRY_TAG" ]]'), frozen_case.index("git fetch --no-tags origin refs/heads/release/frozen-rc7"))
        self.assertIn('exit 1', frozen_case.split('git fetch', 1)[0])


class FrozenRc9PreparationTests(unittest.TestCase):
    def test_profile_and_candidate_verification_boundaries(self):
        profile = rc9_prepare.RC9
        self.assertEqual(profile.commit, "35ee3cacf590e755d62b88ed4f140b7ae4d7acc8")
        self.assertEqual(profile.base, release.CANDIDATE)
        self.assertEqual(profile.version, "0.13.2-rc.9")
        self.assertEqual(profile.tree, "57fa3c14e2830d06a629026086268c2e96279cd0")
        self.assertEqual(rc9_prepare.RC9_PATHS, frozenset({
            ".github/workflows/ci.yml", "Cargo.lock", "crates/phoenix-ide/Cargo.toml",
            "scripts/launchd_deploy_helper.py", "tests/devpy/test_backup_comparator.py",
            "tests/devpy/test_backup_verification_scale.py",
            "tests/devpy/test_simple_launchd_helper.py",
            "tests/integration/backup_verification_scale.py",
            "tests/integration/qualify_release_helper.py",
        }))
        old = b'name = "phoenix_ide"\nversion = "0.13.2-rc.7"'
        new = old.replace(b"rc.7", b"rc.9")
        manifest = b'[package]\n' + old + b'\n'
        lock = b'[[package]]\n' + old + b'\n'
        paths = sorted(rc9_prepare.RC9_PATHS)
        def fake_git(repo, *args):
            if args == ("rev-parse", f"{profile.commit}^{{commit}}"):
                return profile.commit.encode()
            if args == ("show", "-s", "--format=%T", profile.commit):
                return profile.tree.encode()
            if args == ("rev-parse", f"{profile.base}^"):
                return profile.application_base.encode()
            if args[0] == "merge-base":
                return b""
            if args[0] == "rev-list":
                return f"{profile.commit} {profile.base}".encode()
            if args[0] == "diff-tree":
                return "\n".join(paths).encode()
            if args[0] == "show":
                source, file = args[1].split(":", 1)
                if file == rc9_prepare.HELPER:
                    return b"helper"
                contents = manifest if file.endswith("Cargo.toml") else lock
                return contents.replace(old, new) if source == profile.commit else contents
            raise AssertionError(args)
        from dataclasses import replace
        import hashlib
        with mock.patch.object(rc9_prepare, "git", side_effect=fake_git), mock.patch.object(
            rc9_prepare, "RC9", replace(profile, helper_sha256=hashlib.sha256(b"helper").hexdigest())
        ):
            self.assertEqual(rc9_prepare.verify_rc9_candidate()["commit"], profile.commit)
            with mock.patch.object(rc9_prepare, "git", side_effect=lambda repo, *args: (
                f"{profile.commit} {profile.base} {'f'*40}".encode() if args[0] == "rev-list" else fake_git(repo, *args)
            )):
                with self.assertRaisesRegex(ValueError, "sole direct parent"):
                    rc9_prepare.verify_rc9_candidate()
            with mock.patch.object(rc9_prepare, "git", side_effect=lambda repo, *args: (
                f"{profile.commit} {'f'*40}".encode() if args[0] == "rev-list" else fake_git(repo, *args)
            )):
                with self.assertRaisesRegex(ValueError, "sole direct parent"):
                    rc9_prepare.verify_rc9_candidate()
            paths.append("extra")
            with self.assertRaisesRegex(ValueError, "changed paths"):
                rc9_prepare.verify_rc9_candidate()
            paths.pop()
            with mock.patch.object(rc9_prepare, "git", side_effect=lambda repo, *args: (
                manifest.replace(old, b'name = "phoenix_ide"\nversion = "0.13.2-rc.8"')
                if args == ("show", f"{profile.commit}:crates/phoenix-ide/Cargo.toml") else fake_git(repo, *args)
            )):
                with self.assertRaisesRegex(ValueError, "manifest"):
                    rc9_prepare.verify_rc9_candidate()
            with mock.patch.object(rc9_prepare, "git", side_effect=lambda repo, *args: (
                lock.replace(old, b'name = "phoenix_ide"\nversion = "0.13.2-rc.8"')
                if args == ("show", f"{profile.commit}:Cargo.lock") else fake_git(repo, *args)
            )):
                with self.assertRaisesRegex(ValueError, "lockfile"):
                    rc9_prepare.verify_rc9_candidate()

            for file, contents in (
                ("crates/phoenix-ide/Cargo.toml", manifest),
                ("Cargo.lock", lock),
            ):
                with self.subTest(file=file), mock.patch.object(
                    rc9_prepare, "git", side_effect=lambda repo, *args: (
                        contents.replace(old, new) + b"# unexpected non-version edit\n"
                        if args == ("show", f"{profile.commit}:{file}") else fake_git(repo, *args)
                    )
                ):
                    with self.assertRaisesRegex(ValueError, "beyond its version replacement"):
                        rc9_prepare.verify_rc9_candidate()

    def test_rc8_profile_pins_remain_unchanged(self):
        profile = rc9_prepare.rc8_profile()
        self.assertEqual(profile.commit, "d5d2a45651035f58f3231f0dc82fd9ef59b7cb3c")
        self.assertEqual(profile.version, "0.13.2-rc.8")
        self.assertEqual(profile.tree, "5ebc83729fc5630f8ffcd03477b0aaac1187982b")

    def test_latest_exact_ci_failure_refuses_and_no_selectors(self):
        profile = rc9_prepare.RC9
        good = {"path": rc9_prepare.CI_WORKFLOW, "head_sha": profile.commit,
                "event": "workflow_dispatch", "status": "completed", "conclusion": "success",
                "run_number": 12, "id": 77, "run_attempt": 1}
        import contextlib
        import io
        import json
        failure = dict(good, run_number=13, id=78, conclusion="failure")
        payload = io.BytesIO(json.dumps({"workflow_runs": [good, failure]}).encode())
        with mock.patch.object(rc9_prepare.urllib.request, "urlopen", return_value=contextlib.nullcontext(payload)) as request:
            with self.assertRaisesRegex(ValueError, "latest exact-candidate"):
                rc9_prepare.latest_rc9_ci_run()
            url = request.call_args.args[0].full_url
            self.assertIn("head_sha=" + profile.commit, url)
            self.assertIn("event=workflow_dispatch", url)
        with self.assertRaisesRegex(SystemExit, "no source, version, tag, or run selectors"):
            rc9_prepare.main(["gate-rc9", "--source", "other"])

    def test_tag_rejected_before_source_or_api_and_provenance_is_distinct(self):
        with mock.patch.object(rc9_prepare, "verify_rc9_candidate") as source, mock.patch.object(
            rc9_prepare, "latest_rc9_ci_run"
        ) as ci:
            with self.assertRaisesRegex(ValueError, "release-tag input"):
                rc9_prepare.gate_rc9(tag_input="v0.13.2-rc.9")
            source.assert_not_called()
            ci.assert_not_called()
        with self.assertRaisesRegex(ValueError, "distinct"):
            rc9_prepare.rc9_provenance_json(rc9_prepare.RC9.commit, 10, 1, 11, 1)
        provenance = rc9_prepare.rc9_provenance_json("a" * 40, 10, 1, 11, 1)
        self.assertEqual(provenance["operation"], "prepare-frozen-rc9")
        self.assertEqual(provenance["source_commit"], rc9_prepare.RC9.commit)
        self.assertEqual(provenance["controller_commit"], "a" * 40)

    def test_workflow_is_preparation_only_and_uses_protected_macos_path(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        case = workflow.split("              prepare-frozen-rc9)", 1)[1].split("                ;;", 1)[0]
        self.assertLess(case.index('[[ -z "$RETRY_TAG" ]]'), case.index("git fetch"))
        self.assertIn("refs/heads/main:refs/remotes/origin/main", case)
        self.assertIn('test "$(git rev-parse origin/main)" = "$GITHUB_SHA"', case)
        self.assertIn("refs/heads/candidate/rc9-helper-qualification:refs/remotes/origin/candidate/rc9-helper-qualification", case)
        self.assertIn(rc9_prepare.RC9.commit, case)
        self.assertIn('git show "$GITHUB_SHA:scripts/frozen_rc8_prepare.py"', case)
        self.assertIn('gate-rc9 >> "$GITHUB_OUTPUT"', case)
        self.assertNotIn("release=true", case)
        self.assertNotIn("release-frozen-rc9", workflow)
        self.assertNotIn("publish-frozen-rc9", workflow)
        self.assertIn("inputs.operation == 'prepare-frozen-rc9') && 'preparation'", workflow)
        material = workflow.split("      - name: Materialize trusted frozen RC9 controller outside candidate source", 1)[1].split("      - uses: pnpm/action-setup", 1)[0]
        self.assertIn("CONTROLLER_COMMIT: ${{ needs.gate.outputs.controller_commit }}", material)
        self.assertIn('test "$CONTROLLER_COMMIT" != "$(git rev-parse HEAD)"', material)
        self.assertIn("provenance-rc9", material)
        self.assertIn("rc9-provenance.json", material)
        self.assertIn("RC9-SOURCE-AND-HELPER-PROVENANCE-${{ matrix.target }}.json", workflow)
        self.assertIn("name: desktop-preparation-${{ matrix.target }}-${{ needs.gate.outputs.commit }}", workflow)
        self.assertIn("path: ${{ runner.temp }}/release-assets/*", workflow)
        self.assertIn("if: needs.gate.outputs.preparation == 'true'", workflow)
        self.assertIn("environment: macos-release-signing", workflow)
        self.assertIn("needs.gate.outputs.preparation == 'true'", workflow)
        self.assertIn("if: needs.gate.outputs.release == 'true'", workflow)


if __name__ == "__main__":
    unittest.main()
