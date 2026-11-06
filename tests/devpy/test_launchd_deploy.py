"""Ordinary installer coverage. Retired protected/recovery contracts are documented in the deployment ADR."""
import contextlib
import dataclasses
import datetime
import hashlib
import importlib.util
import io
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def refuse_subprocess(command, *_args, **_kwargs):
    raise AssertionError(f"installer test attempted an unmocked subprocess: {command!r}")


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


helper = load(ROOT / "scripts/launchd_deploy_helper.py", "launchd_installer_helper_test")


class PreparationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load(ROOT / "dev.py", "devpy_launchd_deploy_test")

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="phoenix-installer-unit-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name).resolve()
        home_patch = mock.patch.dict(os.environ, {"HOME": str(root)})
        home_patch.start()
        self.addCleanup(home_patch.stop)
        tracing_patch = mock.patch.object(self.dev, "_bootstrap_dev_tracing")
        tracing_patch.start()
        self.addCleanup(tracing_patch.stop)
        for name, value in {
            "LAUNCHD_INSTALL_DIR": root / "installation",
            "LAUNCHD_PLIST_PATH": root / "LaunchAgents/service.plist",
            "LAUNCHD_DEPLOY_DIR": root / "deploy",
            "LAUNCHD_DEPLOY_ACTIVE_PATH": root / "deploy/active",
            "LAUNCHD_DEPLOY_STATUS_PATH": root / "deploy/status.json",
            "LAUNCHD_DEPLOY_LOCK_PATH": root / "deploy/activation.lock",
            "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH": root / "deploy/claim.lock",
            "LAUNCHD_RESTART_DIR": root / "restart",
            "LAUNCHD_RESTART_ACTIVE_PATH": root / "restart/active",
            "LAUNCHD_RESTART_TRANSACTIONS_DIR": root / "restart/transactions",
            "PROD_DB_PATH": root / "application.db",
            "PROD_SHA_PATH": root / "deployed.sha",
        }.items():
            patch = mock.patch.object(self.dev, name, value)
            patch.start()
            self.addCleanup(patch.stop)
        for name in ("run", "Popen"):
            patch = mock.patch.object(self.dev.subprocess, name, side_effect=refuse_subprocess)
            patch.start()
            self.addCleanup(patch.stop)


    def test_broken_pipe_after_handoff_does_not_release_claim(self):
        with mock.patch("builtins.print", side_effect=BrokenPipeError), \
             mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release:
            self.dev._report_launchd_handoff("tx", self.dev.RuntimeIdentity("1.0.0", "abc123"))
        release.assert_not_called()


    def test_positional_version_is_rejected_with_release_guidance(self):
        stderr = io.StringIO()
        stderr.reconfigure = mock.Mock()
        with mock.patch.object(sys, "argv", ["dev.py", "prod", "deploy", "v1.2.3"]), mock.patch.object(sys, "stderr", stderr):
            with self.assertRaisesRegex(SystemExit, "2"):
                self.dev.main()
        self.assertIn("--release", stderr.getvalue())


    def test_release_path_skips_checks_and_build(self):
        with mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev, "prod_build") as build:
            self.dev.cmd_prod_deploy("v1.2.3")
        deploy.assert_called_once_with("v1.2.3")
        check.assert_not_called()
        build.assert_not_called()


    def test_controller_release_revalidates_exact_tag_to_expected_commit(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "abc123def456" + "0" * 28}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            candidate = self.dev._prepare_release_candidate(
                "v1.2.3", staging, expected_full_commit=release_commit
            )
        self.assertEqual(release_commit, candidate.release_commit)
        self.assertEqual(3, len(run.call_args_list))


    def test_controller_rejects_latest_release_alias(self):
        with tempfile.TemporaryDirectory() as td:
            with self.assertRaisesRegex(SystemExit, "exact release tag"):
                self.dev._prepare_release_candidate("latest", Path(td), expected_full_commit="a" * 40)


    def test_controller_uses_installed_launchd_env_and_transaction_id(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v1.2.3",
            expected_full_commit="a" * 40,
            transaction_id="tx-123",
        )
        installed = {"PHOENIX_PASSWORD": "installed", "PHOENIX_PORT": "9443"}
        with mock.patch.object(
            self.dev, "_launchd_env_from_plist", return_value=installed
        ) as read_installed:
            env, path = self.dev._launchd_candidate_env(controller)
        self.assertEqual(installed, env)
        self.assertEqual("tx-123", controller.transaction_id)
        self.assertIsNone(path)
        read_installed.assert_called_once_with(self.dev.LAUNCHD_PLIST_PATH)


    def test_controller_release_path_skips_checks_and_build(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v1.2.3",
            expected_full_commit="a" * 40,
            transaction_id="tx-123",
            backend="launchd",
        )
        with mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev, "prod_build") as build:
            self.dev.cmd_prod_deploy("v1.2.3", controller=controller)
        deploy.assert_called_once_with("v1.2.3", controller=controller)
        check.assert_not_called()
        build.assert_not_called()


    def test_release_asset_selection_supports_all_native_targets(self):
        import platform

        cases = [
            ("darwin", "arm64", "phoenix_ide-aarch64-apple-darwin"),
            ("darwin", "x86_64", "phoenix_ide-x86_64-apple-darwin"),
            ("linux", "aarch64", "phoenix_ide-aarch64-unknown-linux-musl"),
            ("linux", "amd64", "phoenix_ide-x86_64-unknown-linux-musl"),
        ]
        for host_platform, machine, expected in cases:
            with self.subTest(host_platform=host_platform, machine=machine), \
                 mock.patch.object(self.dev.sys, "platform", host_platform), \
                 mock.patch.object(platform, "machine", return_value=machine):
                self.assertEqual(expected, self.dev._release_asset_name())


    def test_exact_rc_opt_in_validates_metadata_checksum_and_full_build_identity(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"rc release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v0.13.0-rc.2", "isPrerelease": True, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with mock.patch.object(
                self.dev,
                "_binary_identity",
                return_value={"version": "0.13.0-rc.2", "git_sha": release_commit},
            ) as identity:
                candidate = self.dev._prepare_release_candidate("v0.13.0-rc.2", staging)

        self.assertEqual("v0.13.0-rc.2", candidate.release_tag)
        self.assertEqual("0.13.0-rc.2", candidate.identity.version)
        self.assertEqual(release_commit, candidate.identity.git_sha)
        self.assertEqual(release_commit, candidate.release_commit)
        identity.assert_called_once_with(asset)
        self.assertIn("v0.13.0-rc.2", run.call_args_list[2].args[0])


    def test_exact_rc_rejects_build_identity_without_rc_suffix(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity") as identity, \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"rc release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            identity.return_value = {"version": "0.13.0", "git_sha": release_commit}
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v0.13.0-rc.2", "isPrerelease": True, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "expected 0.13.0-rc.2"):
                self.dev._prepare_release_candidate("v0.13.0-rc.2", staging)


    def test_explicit_release_rejects_prerelease_metadata_mismatch(self):
        cases = [
            ("v1.2.3", True, "isPrerelease=false"),
            ("v0.13.0-rc.2", False, "isPrerelease=true"),
        ]
        for tag, is_prerelease, message in cases:
            with self.subTest(tag=tag), tempfile.TemporaryDirectory() as td, \
                 mock.patch.object(
                     self.dev.subprocess,
                     "run",
                     return_value=subprocess.CompletedProcess(
                         [],
                         0,
                         json.dumps({"tagName": tag, "isPrerelease": is_prerelease, "isDraft": False}),
                         "",
                     ),
                 ) as run:
                with self.assertRaisesRegex(SystemExit, message):
                    self.dev._prepare_release_candidate(tag, Path(td))
            run.assert_called_once()


    def test_release_rejects_unsupported_explicit_tag_before_lookup(self):
        invalid_tags = [
            "1.2.3",
            "v01.2.3",
            "v1.2.3-beta.1",
            "v0.13.0-rc.0",
            "v0.12.9-rc.1",
            "v1.100.0",
        ]
        with mock.patch.object(self.dev.subprocess, "run") as run:
            for tag in invalid_tags:
                with self.subTest(tag=tag), self.assertRaisesRegex(
                    SystemExit, "unsupported release tag"
                ):
                    self.dev._prepare_release_candidate(tag, Path("unused"))
        run.assert_not_called()


    def test_latest_rejects_rc_release(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(
            self.dev.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                [],
                0,
                json.dumps({"tagName": "v0.13.0-rc.2", "isPrerelease": True, "isDraft": False}),
                "",
            ),
        ) as run:
            with self.assertRaisesRegex(SystemExit, "latest must resolve to a stable"):
                self.dev._prepare_release_candidate("latest", Path(td))
        run.assert_called_once()


    def test_latest_resolves_stable_once_then_downloads_immutable_tag_and_checks_checksum(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "abc123def456" + "0" * 28}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123def456" + "0" * 28
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            candidate = self.dev._prepare_release_candidate("latest", staging)
            self.assertEqual(asset, candidate.binary)
            self.assertTrue(candidate.binary.stat().st_mode & 0o100)
        self.assertEqual(self.dev.ProdSourceKind.PUBLISHED_RELEASE, candidate.source_kind)
        self.assertEqual(
            ("v1.2.3", release_commit, release_commit),
            (candidate.release_tag, candidate.identity.git_sha, candidate.release_commit),
        )
        self.assertIn("v1.2.3", run.call_args_list[2].args[0])


    def test_release_rejects_private_draft_before_download(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess(
                     [], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": True}), ""
                 ),
             ) as run:
            with self.assertRaisesRegex(SystemExit, "private drafts are not deployable"):
                self.dev._prepare_release_candidate("v1.2.3", Path(td))
        self.assertEqual("view", run.call_args.args[0][2])


    def test_release_rejects_asset_from_different_commit(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "bad123bad123"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            (staging / "SHA256SUMS").write_text(f"{self.dev._file_sha256(asset)}  {asset.name}\n")
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, "abc123" + "0" * 34 + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "asset embeds"):
                self.dev._prepare_release_candidate("latest", staging)


    def test_release_rejects_truncated_embedded_identity(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "a"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            (staging / "SHA256SUMS").write_text(f"{self.dev._file_sha256(asset)}  {asset.name}\n")
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, "abc123def456" + "0" * 28 + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "malformed git identity"):
                self.dev._prepare_release_candidate("latest", staging)


    def test_explicit_candidate_source_rejects_wrong_head_and_dirty_tree(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td).resolve()
            expected = "a" * 40
            source = self.dev.LocalCandidateSource(root=root, commit=expected)

            wrong_head = [
                subprocess.CompletedProcess([], 0, f"{root}\n", ""),
                subprocess.CompletedProcess([], 0, f"{'b' * 40}\n", ""),
            ]
            with mock.patch.object(self.dev.subprocess, "run", side_effect=wrong_head):
                with self.assertRaisesRegex(SystemExit, "does not match --candidate-source-commit"):
                    self.dev._validated_local_candidate_source(source)

            dirty = [
                subprocess.CompletedProcess([], 0, f"{root}\n", ""),
                subprocess.CompletedProcess([], 0, f"{expected}\n", ""),
                subprocess.CompletedProcess([], 0, " M dev.py\n", ""),
            ]
            with mock.patch.object(self.dev.subprocess, "run", side_effect=dirty):
                with self.assertRaisesRegex(SystemExit, "must be clean"):
                    self.dev._validated_local_candidate_source(source)


    def test_explicit_candidate_source_route_conflicts_fail_before_validation(self):
        options = self.dev.ProdDeployControllerOptions(
            enabled=True,
            backend="launchd",
            candidate_source_root=Path("/tmp/candidate"),
            candidate_source_commit="a" * 40,
        )
        with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(
            self.dev, "_validated_local_candidate_source"
        ) as validate:
            with self.assertRaisesRegex(SystemExit, "cannot be combined with --controller-mode"):
                options.require_local_candidate_source(None)
        validate.assert_not_called()


    def test_prod_deploy_backend_flags_are_mutually_exclusive(self):
        argv = [
            "dev.py",
            "prod",
            "deploy",
            "--backend",
            "launchd",
            "--controller-backend",
            "launchd",
        ]
        stderr = io.StringIO()
        stderr.reconfigure = mock.Mock()
        with mock.patch.object(sys, "argv", argv), mock.patch.object(sys, "stderr", stderr):
            with self.assertRaisesRegex(SystemExit, "2"):
                self.dev.main()
        self.assertIn("not allowed with argument --backend", stderr.getvalue())


    def test_candidate_prod_build_uses_isolated_output_without_touching_default_worktree(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            candidate_root = root / "candidate"
            (candidate_root / "ui").mkdir(parents=True)
            migrations = candidate_root / "crates" / "phoenix-db" / "src" / "migrations.rs"
            migrations.parent.mkdir(parents=True)
            migrations.write_text("candidate migrations\n")
            output_root = root / "output"
            output_root.mkdir()
            default_worktree = root / "default-worktree"
            default_worktree.mkdir()
            sentinel = default_worktree / "sentinel"
            sentinel.write_text("unchanged")
            source = self.dev.LocalCandidateSource(candidate_root, "a" * 40)
            cargo_calls = []

            def run(command, **kwargs):
                if command[:3] == ["git", "rev-parse", "--absolute-git-dir"]:
                    return subprocess.CompletedProcess(command, 0, f"{root / 'git-dir'}\n", "")
                if command[:2] == ["pnpm", "install"]:
                    return subprocess.CompletedProcess(command, 0, "", "")
                if command[:3] == ["git", "status", "--porcelain"]:
                    return subprocess.CompletedProcess(command, 0, "", "")
                if command[:2] == ["cargo", "build"]:
                    cargo_calls.append((command, Path(kwargs["cwd"])))
                    target_dir = Path(command[command.index("--target-dir") + 1])
                    binary = target_dir / "release" / "phoenix_ide"
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(b"candidate binary")
                return subprocess.CompletedProcess(command, 0, "", "")

            with mock.patch.object(
                self.dev, "_validated_local_candidate_source", return_value=source
            ), mock.patch.object(
                self.dev, "PROD_BUILD_WORKTREE", default_worktree
            ), mock.patch.object(
                self.dev, "ensure_corepack_pnpm"
            ), mock.patch.object(
                self.dev, "node_env", return_value={}
            ), mock.patch.object(
                self.dev, "_compiler_cache_subprocess_env", return_value=("cargo", {})
            ), mock.patch.object(
                self.dev, "_production_cargo_feature_args", return_value=[]
            ), mock.patch.object(
                self.dev.subprocess, "run", side_effect=run
            ):
                binary = self.dev.prod_build(
                    strip=False,
                    target=None,
                    candidate_source=source,
                    candidate_output_root=output_root,
                )

            self.assertEqual(output_root / "target" / "release" / "phoenix_ide", binary)
            self.assertEqual(b"candidate binary", binary.read_bytes())
            self.assertFalse((output_root / "migrations.rs").exists())
            self.assertFalse((output_root / "source").exists())
            self.assertEqual("unchanged", sentinel.read_text())
            self.assertEqual(1, len(cargo_calls))
            self.assertEqual(output_root / "source", cargo_calls[0][1])
            self.assertIn(str(output_root / "target"), cargo_calls[0][0])


    def test_candidate_prod_build_preserves_tracked_symlink_and_original_build_error(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            candidate_root = root / "candidate"
            (candidate_root / "ui").mkdir(parents=True)
            tracked_target = candidate_root / "skills" / "tracked-skill"
            tracked_target.mkdir(parents=True)
            tracked_link = candidate_root / ".agents" / "skills" / "tracked-skill"
            tracked_link.parent.mkdir(parents=True)
            tracked_link.symlink_to("../../skills/tracked-skill")
            output_root = root / "output"
            output_root.mkdir()
            source = self.dev.LocalCandidateSource(candidate_root, "a" * 40)
            build_error = RuntimeError("original cargo failure")
            cargo_reached = False

            def run(command, **kwargs):
                nonlocal cargo_reached
                if command[:3] == ["git", "rev-parse", "--absolute-git-dir"]:
                    return subprocess.CompletedProcess(command, 0, f"{root / 'git-dir'}\n", "")
                if command[:2] == ["pnpm", "install"]:
                    return subprocess.CompletedProcess(command, 0, "", "")
                if command[:3] == ["git", "status", "--porcelain"]:
                    copied_link = Path(kwargs["cwd"]) / ".agents" / "skills" / "tracked-skill"
                    link_matches_index = (
                        copied_link.is_symlink()
                        and copied_link.readlink() == Path("../../skills/tracked-skill")
                    )
                    status = "" if link_matches_index else " D .agents/skills/tracked-skill\n"
                    return subprocess.CompletedProcess(command, 0, status, "")
                if command[:2] == ["cargo", "build"]:
                    cargo_reached = True
                    raise build_error
                return subprocess.CompletedProcess(command, 0, "", "")

            with mock.patch.object(
                self.dev, "_validated_local_candidate_source", return_value=source
            ), mock.patch.object(
                self.dev, "ensure_corepack_pnpm"
            ), mock.patch.object(
                self.dev, "node_env", return_value={}
            ), mock.patch.object(
                self.dev, "_compiler_cache_subprocess_env", return_value=("cargo", {})
            ), mock.patch.object(
                self.dev, "_production_cargo_feature_args", return_value=[]
            ), mock.patch.object(
                self.dev.subprocess, "run", side_effect=run
            ):
                with self.assertRaises(RuntimeError) as raised:
                    self.dev.prod_build(
                        strip=False,
                        target=None,
                        candidate_source=source,
                        candidate_output_root=output_root,
                    )

            self.assertIs(build_error, raised.exception)
            self.assertTrue(cargo_reached)
            self.assertTrue(tracked_link.is_symlink())
            self.assertFalse((output_root / "source").exists())


    def test_candidate_identity_mismatch_fails_before_deploy_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = self.dev.LocalCandidateSource(root / "candidate", "a" * 40)
            source.root.mkdir()
            output_root = root / "output"
            output_root.mkdir()

            def build(**_kwargs):
                binary = output_root / "target" / "release" / "phoenix_ide"
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"candidate")
                (output_root / "migrations.rs").write_text("migrations\n")
                return binary

            controller = self.dev.ProdDeployControllerOptions(
                backend="launchd",
                candidate_source_root=source.root,
                candidate_source_commit=source.commit,
            )
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(
                self.dev, "_validated_local_candidate_source", return_value=source
            ), mock.patch.object(
                self.dev, "_trusted_controller_head", return_value="c" * 40
            ), mock.patch.object(
                self.dev.tempfile, "mkdtemp", return_value=str(output_root)
            ), mock.patch.object(
                self.dev, "prod_build", side_effect=build
            ), mock.patch.object(
                self.dev,
                "_binary_identity",
                return_value=self.dev.RuntimeIdentity("1.40.0", "b" * 40),
            ), mock.patch.object(
                self.dev, "_claim_launchd_deploy"
            ) as claim:
                with self.assertRaisesRegex(SystemExit, "does not exactly match selected source"):
                    self.dev.launchd_prod_deploy(controller=controller)
            claim.assert_not_called()


    def test_candidate_source_drift_fails_before_deploy_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = self.dev.LocalCandidateSource(root / "candidate", "a" * 40)
            source.root.mkdir()
            binary = root / "phoenix_ide"
            binary.write_bytes(b"candidate")
            registry = root / "migrations.rs"
            registry.write_text("migrations\n")
            prepared = self.dev.PreparedCandidate(
                binary=binary,
                source_kind=self.dev.ProdSourceKind.LOCAL_HEAD,
                source_commit=source.commit,
                identity=self.dev.RuntimeIdentity("1.40.0", source.commit),
            )
            drifted = dataclasses.replace(source, commit="b" * 40)
            controller = self.dev.ProdDeployControllerOptions(
                backend="launchd",
                candidate_source_root=source.root,
                candidate_source_commit=source.commit,
            )
            with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(
                self.dev,
                "_validated_local_candidate_source",
                side_effect=[source, drifted],
            ), mock.patch.object(
                self.dev, "_trusted_controller_head", return_value="c" * 40
            ), mock.patch.object(
                self.dev, "_prepare_local_candidate", return_value=prepared
            ), mock.patch.object(
                self.dev, "_claim_launchd_deploy"
            ) as claim:
                with self.assertRaisesRegex(SystemExit, "changed while the isolated build was running"):
                    self.dev.launchd_prod_deploy(controller=controller)
            claim.assert_not_called()


    def test_prebuilt_local_candidate_rejects_invalid_inputs_before_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = self.dev.LocalCandidateSource(root / "source", "a" * 40)
            source.root.mkdir()
            binary = root / "phoenix-ide"
            binary.write_bytes(b"candidate")
            registry = root / "migrations.rs"
            registry.write_text("registry\n")
            binary_sha256 = hashlib.sha256(binary.read_bytes()).hexdigest()
            registry_sha256 = hashlib.sha256(registry.read_bytes()).hexdigest()
            inputs = self.dev.PrebuiltLocalCandidateInputs(
                binary=binary,
                binary_sha256=binary_sha256,
                source=source,
            )
            controller = self.dev.ProdDeployControllerOptions(backend="launchd")
            cases = (
                (
                    "binary SHA",
                    dataclasses.replace(inputs, binary_sha256="b" * 64),
                    self.dev.RuntimeIdentity("1.40.0", source.commit),
                    [source],
                ),
                (
                    "embedded identity",
                    inputs,
                    self.dev.RuntimeIdentity("1.40.0", "b" * 40),
                    [source],
                ),
                (
                    "changed while the isolated build was running",
                    inputs,
                    self.dev.RuntimeIdentity("1.40.0", source.commit),
                    [source, dataclasses.replace(source, commit="b" * 40)],
                ),
            )
            for message, candidate, identity, validated_sources in cases:
                with self.subTest(message=message), mock.patch.object(
                    self.dev.sys, "platform", "darwin"
                ), mock.patch.object(
                    self.dev,
                    "_validated_local_candidate_source",
                    side_effect=validated_sources,
                ), mock.patch.object(
                    self.dev, "_trusted_controller_head", return_value="c" * 40
                ), mock.patch.object(
                    self.dev, "_binary_identity", return_value=identity
                ), mock.patch.object(
                    self.dev, "prod_build"
                ) as build, mock.patch.object(
                    self.dev, "_claim_launchd_deploy"
                ) as claim:
                    with self.assertRaisesRegex(SystemExit, message):
                        self.dev.launchd_prod_deploy(
                            controller=controller,
                            prebuilt_local_candidate=candidate,
                        )
                build.assert_not_called()
                claim.assert_not_called()


    def test_local_candidate_binds_exact_head_to_typed_identity(self):
        commit = "abc123def456" + "0" * 28
        identity = self.dev.RuntimeIdentity("2.0.0", commit)
        with mock.patch.object(self.dev, "prod_build", return_value=Path("candidate")) as build, \
             mock.patch.object(self.dev, "_binary_identity", return_value=identity), \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess([], 0, commit + "\n", ""),
             ):
            candidate = self.dev._prepare_local_candidate(target=None)
        self.assertEqual(self.dev.ProdSourceKind.LOCAL_HEAD, candidate.source_kind)
        self.assertEqual(commit, candidate.source_commit)
        self.assertEqual(identity, candidate.identity)
        self.assertIsNone(candidate.release_tag)
        self.assertIsNone(candidate.release_commit)
        build.assert_called_once_with(target=None)


    def test_local_candidate_rejects_identity_from_other_commit(self):
        with mock.patch.object(self.dev, "prod_build", return_value=Path("candidate")), \
             mock.patch.object(
                 self.dev,
                 "_binary_identity",
                 return_value=self.dev.RuntimeIdentity("2.0.0", "b" * 40),
             ), \
             mock.patch.object(
                 self.dev.subprocess,
                 "run",
                 return_value=subprocess.CompletedProcess([], 0, "a" * 40 + "\n", ""),
             ):
            with self.assertRaisesRegex(SystemExit, "does not exactly match selected HEAD"):
                self.dev._prepare_local_candidate(target=None)


    def test_status_always_reports_active_owner_when_last_status_mismatches(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            active.write_text("actual-owner\n")
            status.write_text(
                json.dumps(
                    {
                        "transaction_id": "older-terminal",
                        "state": "committed",
                        "source_kind": "local_head",
                    }
                )
            )
            output = io.StringIO()
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), contextlib.redirect_stdout(output):
                self.dev._print_launchd_deploy_status()

            rendered = output.getvalue()
            self.assertIn("Active deploy: actual-owner", rendered)
            self.assertIn("Last deploy: committed (older-terminal)", rendered)

            status.unlink()
            output = io.StringIO()
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), contextlib.redirect_stdout(output):
                self.dev._print_launchd_deploy_status()
            self.assertIn("Active deploy: actual-owner", output.getvalue())


    def test_active_nonregular_status_is_rejected_before_read(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            active = root / "active"
            status = root / "status.json"
            active.write_text("actual-owner\n")
            os.mkfifo(status)
            real_read_text = Path.read_text

            def reject_status_read(path, *args, **kwargs):
                if path == status:
                    raise AssertionError("non-regular status must not be read")
                return real_read_text(path, *args, **kwargs)

            output = io.StringIO()
            with mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_DIR", root
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status
            ), mock.patch.object(
                self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "claim.lock"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_DIR", root / "restart"
            ), mock.patch.object(
                self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart-active"
            ), mock.patch.object(
                Path, "read_text", reject_status_read
            ), contextlib.redirect_stdout(output):
                self.dev._print_launchd_deploy_status()
                with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                    self.dev._claim_launchd_restart("next-restart")

            rendered = output.getvalue()
            self.assertIn("Active deploy: actual-owner", rendered)
            self.assertIn("not a regular file", rendered)
            self.assertEqual(active.read_text(), "actual-owner\n")
            self.assertTrue(status.exists())




    def test_unreadable_active_markers_fence_deploy_and_restart(self):
        active = self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH
        active.parent.mkdir(parents=True)
        active.symlink_to(active.parent / "missing")
        original = {"transaction_id": "other", "state": "committed"}
        self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps(original))
        for action in (self.dev._claim_launchd_deploy, self.dev._claim_launchd_restart):
            with self.assertRaises(self.dev.ConcurrentLaunchdOperation):
                action("new")
        self.assertEqual(original, json.loads(self.dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text()))
        with mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), mock.patch.object(self.dev, "launchd_prod_stop") as stop:
            self.dev.cmd_prod_stop()
        stop.assert_called_once_with()




    def test_claim_release_is_transaction_owned(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", Path(td)), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", Path(td) / "active"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", Path(td) / "claim.lock"), \
             mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", Path(td) / "restart-active"):
            self.dev._claim_launchd_deploy("first")
            self.assertFalse(self.dev._release_launchd_deploy_claim("second"))
            self.assertEqual("first", self.dev._deploy_claim_owner())
            with self.assertRaisesRegex(SystemExit, "first"):
                self.dev._claim_launchd_deploy("second")


    def test_release_rejects_dirty_embedded_identity(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "abc123-dirty"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            (staging / "SHA256SUMS").write_text(f"{self.dev._file_sha256(asset)}  {asset.name}\n")
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False, "isDraft": False}), ""),
                subprocess.CompletedProcess([], 0, "abc123" + "0" * 34 + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "dirty git identity"):
                self.dev._prepare_release_candidate("latest", staging)


    def test_candidate_health_url_uses_candidate_tls_and_port(self):
        env = {"PHOENIX_TLS": "auto", "PHOENIX_PORT": "9443"}
        self.assertEqual("https://localhost:9443/version", self.dev._prod_local_health_url(env))


    def test_stopped_install_identity_falls_back_to_binary_probe(self):
        with mock.patch.object(self.dev, "_current_prod_identity", return_value=None), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.0.0", "git_sha": "oldsha"}) as probe:
            identity = self.dev._current_prod_identity({}) or self.dev._binary_identity(Path("installed"))
        self.assertEqual({"version": "1.0.0", "git_sha": "oldsha"}, identity)
        probe.assert_called_once_with(Path("installed"))


    def test_local_source_commit_stays_full_for_deployed_sha_comparison(self):
        full_sha = "abc123" + "0" * 34
        embedded = "abc123"
        self.assertTrue(full_sha.startswith(embedded.removesuffix("-dirty")))
        self.assertNotEqual(full_sha, embedded)


    def test_local_helper_is_materialized_from_selected_commit(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(self.dev.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, b"#!/usr/bin/python3\nprint('selected')\n", b"")
            destination = Path(td) / "helper.py"
            self.dev._materialize_helper("abc123", destination, "local_head")
            self.assertIn(b"selected", destination.read_bytes())
        self.assertEqual(["git", "show", "abc123:scripts/launchd_deploy_helper.py"], run.call_args.args[0])


    def test_release_helper_is_fetched_from_selected_commit(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.object(self.dev.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, b"#!/usr/bin/python3\nprint('release')\n", b"")
            destination = Path(td) / "helper.py"
            self.dev._materialize_helper("abc123", destination, "published_release")
            self.assertIn(b"release", destination.read_bytes())
        self.assertIn("ref=abc123", run.call_args.args[0][2])


    def test_display_url_uses_effective_launchd_port(self):
        self.assertEqual(
            "https://localhost:9555",
            self.dev._prod_display_url({"PHOENIX_TLS": "auto", "PHOENIX_PORT": "9555"}),
        )


    def test_launchd_manifest_defaults_to_bounded_120_second_health_budget(self):
        defaults = {
            field.name: field.default
            for field in helper.dataclasses.fields(helper.Manifest)
        }
        self.assertEqual(30.0, defaults["transition_timeout_secs"])
        self.assertEqual(120.0, defaults["health_timeout_secs"])
        self.assertEqual(
            defaults["transition_timeout_secs"],
            self.dev.LAUNCHD_TRANSITION_TIMEOUT_SECS,
        )
        self.assertEqual(
            defaults["health_timeout_secs"],
            self.dev.LAUNCHD_HEALTH_TIMEOUT_SECS,
        )
        self.assertEqual(
            390.0,
            4 * self.dev.LAUNCHD_TRANSITION_TIMEOUT_SECS
            + 2 * self.dev.LAUNCHD_HEALTH_TIMEOUT_SECS
            + self.dev.LAUNCHD_STALE_HANDOFF_ALLOWANCE_SECS,
        )


    def test_candidate_env_uses_only_repo_environment_file(self):
        with mock.patch.object(
            self.dev,
            "_load_env_file",
            side_effect=lambda env: env.update({"PHOENIX_PASSWORD": "repo-secret", "PHOENIX_PORT": "9443"}),
        ):
            env, _path = self.dev._launchd_candidate_env()
        self.assertEqual({"PHOENIX_PASSWORD": "repo-secret", "PHOENIX_PORT": "9443"}, env)


    def test_prod_override_commands_reject_without_backend_detection(self):
        with mock.patch.object(self.dev, "detect_prod_env") as detect:
            with self.assertRaisesRegex(SystemExit, r"edit \.phoenix-ide\.env directly"):
                self.dev.cmd_prod_override_set("PHOENIX_PORT", "9443")
            with self.assertRaisesRegex(SystemExit, r"edit \.phoenix-ide\.env directly"):
                self.dev.cmd_prod_override_unset("PHOENIX_PORT")
        detect.assert_not_called()


    def test_initial_status_failure_releases_claim(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", Path(td)), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", Path(td) / "active"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", Path(td) / "claim.lock"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", Path(td) / "restart-active"), \
             mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)), \
             mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
             mock.patch.object(self.dev, "_write_json_atomic", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                self.dev.launchd_prod_deploy()
            self.assertFalse(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())


    def test_prod_stop_uses_service_target_and_requires_absence(self):
        success = subprocess.CompletedProcess([], 0, "", "")
        absent = subprocess.CompletedProcess([], 113, 'Could not find service "test"', "")
        with mock.patch.object(self.dev.subprocess, "run", side_effect=[success, success, absent]) as run:
            self.dev._launchd_stop_if_loaded()
        self.assertEqual(run.call_args_list[1].args[0], ["launchctl", "bootout", f"gui/{os.getuid()}/{self.dev.LAUNCHD_LABEL}"])
        self.assertEqual("print", run.call_args_list[2].args[0][1])
        with mock.patch.object(self.dev.subprocess, "run", side_effect=[success, subprocess.CompletedProcess([], 5, "", "")]), self.assertRaisesRegex(SystemExit, "stop failed"):
            self.dev._launchd_stop_if_loaded()




    def test_staged_identity_mismatch_records_selected_candidate_identity(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            candidate = root / "built-phoenix"
            candidate.write_bytes(b"candidate")
            active_path = root / "deploy" / "active"
            status_path = root / "deploy" / "status.json"
            selected = self.dev.RuntimeIdentity("2.0.0", "abc123def456")
            observed = self.dev.RuntimeIdentity("9.9.9", "bad123def456")
            prepared = self.dev.PreparedCandidate(
                binary=candidate,
                source_kind=self.dev.ProdSourceKind.LOCAL_HEAD,
                source_commit="abc123def456" + "0" * 28,
                identity=selected,
            )

            with mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", root / "deploy"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", active_path), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_CLAIM_LOCK_PATH", root / "deploy" / "claim.lock"), \
                 mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", status_path), \
                 mock.patch.object(self.dev, "LAUNCHD_RESTART_ACTIVE_PATH", root / "restart" / "active"), \
                 mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({}, None)), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_prepare_local_candidate", return_value=prepared), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=observed), \
                 mock.patch.object(
                     self.dev.subprocess,
                     "run",
                     return_value=subprocess.CompletedProcess([], 0, "", ""),
                 ):
                with self.assertRaisesRegex(SystemExit, "staged candidate identity changed after signing"):
                    self.dev.launchd_prod_deploy()

            status = json.loads(status_path.read_text())
            self.assertEqual("precondition_failed", status["state"])
            self.assertEqual(selected.version, status["expected_version"])
            self.assertEqual(selected.git_sha, status["expected_git_sha"])
            self.assertNotEqual(observed.version, status["expected_version"])
            self.assertNotEqual(observed.git_sha, status["expected_git_sha"])
            self.assertFalse(active_path.exists())


    def test_candidate_env_snapshot_is_reused(self):
        snapshot = {"PHOENIX_PORT": "9443", "PHOENIX_PASSWORD": "one"}
        with mock.patch.object(self.dev, "_launchd_candidate_env", return_value=(snapshot, Path("env"))) as load:
            first, source = self.dev._launchd_candidate_env()
            staged = dict(first)
        self.assertEqual(snapshot, staged)
        self.assertEqual(Path("env"), source)
        load.assert_called_once()


    def test_prod_status_uses_effective_launchd_env(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_PLIST_PATH", Path(td) / "service.plist"), \
             mock.patch.object(self.dev, "_current_prod_identity", return_value=None) as identity, \
             mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "state = running\n", "")), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_PLIST_PATH.write_bytes(plistlib.dumps({
                "EnvironmentVariables": {},
                "Sockets": {"Listeners": {"SockServiceName": "9555"}},
            }))
            self.dev.launchd_prod_status()
        identity.assert_called_once_with({"PHOENIX_PORT": "9555"})
        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("Port: 9555", rendered)
        self.assertIn("URL: http://localhost:9555", rendered)


    def test_release_workflow_lists_all_native_assets_and_checksums(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text()
        for asset in (
            "phoenix_ide-aarch64-apple-darwin",
            "phoenix_ide-x86_64-apple-darwin",
            "phoenix_ide-aarch64-unknown-linux-musl",
            "phoenix_ide-x86_64-unknown-linux-musl",
        ):
            self.assertIn(asset, workflow)
        self.assertIn("SHA256SUMS", workflow)
        self.assertIn('missing required release asset: $asset', workflow)
        before, preparation_and_after = workflow.split("\n  build-incident-linux:", 1)
        preparation, after = preparation_and_after.split("\n  build-macos:", 1)
        release_jobs = before + "\n  build-macos:" + after
        self.assertEqual(2, release_jobs.count("git restore ui/dist/.gitkeep"))
        self.assertEqual(4, release_jobs.count('test -z "$(git status --porcelain)"'))
        self.assertEqual(1, preparation.count("git restore ui/dist/.gitkeep"))
        self.assertEqual(1, preparation.count('test -z "$(git status --porcelain)"'))
        self.assertIn("cargo build --locked --profile release-min", preparation)
        self.assertIn('"runner":"macos-15-intel"', workflow)
        self.assertIn('"runner":"macos-15"', workflow)
        self.assertIn("runner: ubuntu-24.04-arm", workflow)
        self.assertIn("runner: ubuntu-latest", workflow)
        self.assertNotIn("runner: macos-14", workflow)
        self.assertNotIn("runner: macos-13", workflow)


    def test_helper_plist_uses_active_interpreter(self):
        plist = plistlib.loads(self.dev._helper_plist(
            "test.helper", Path("helper.py"), Path("manifest.json"), Path("helper.log"), Path("/opt/python3")
        ))
        self.assertEqual("/opt/python3", plist["ProgramArguments"][0])


    def test_null_source_kind_status_remains_readable(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps({
                "transaction_id": "tx", "state": "preparing", "source_kind": None,
                "release_tag": "v1.2.3", "expected_version": None, "expected_git_sha": None,
                "updated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            }))
            self.dev._print_launchd_deploy_status()
        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("unknown v1.2.3", rendered)
        self.assertNotIn("unreadable status", rendered)


    def test_active_transaction_within_complete_activation_budget_is_not_stale(self):
        active = (
            datetime.datetime.now(datetime.timezone.utc)
            - datetime.timedelta(seconds=360)
        ).isoformat()
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"), \
             mock.patch("builtins.print") as output:
            self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps({
                "transaction_id": "tx", "state": "activating", "source_kind": "local_head",
                "expected_version": "1.0.0", "expected_git_sha": "abc", "updated_at": active,
            }))
            self.dev._print_launchd_deploy_status()
        self.assertFalse(any("STALE:" in str(call) for call in output.call_args_list))




    def test_socket_activation_shape_is_preserved(self):
        plist = self.dev.generate_launchd_plist("1.0.0", path_override="/usr/bin")
        self.assertIn("<key>Sockets</key>", plist)
        self.assertIn("<string>IPv4v6</string>", plist)
        self.assertIn("<string>8031</string>", plist)


    def test_launchd_has_no_second_file_writer_for_structured_log(self):
        plist = plistlib.loads(
            self.dev.generate_launchd_plist(
                "1.0.0",
                extra_env={
                    "PHOENIX_LOG_FILE": "",
                    "PHOENIX_FATAL_LOG_FILE": "",
                    "PHOENIX_LOG_STDOUT": "true",
                },
                path_override="/usr/bin",
            ).encode()
        )
        environment = plist["EnvironmentVariables"]
        self.assertEqual(str(self.dev.LAUNCHD_LOG_PATH), environment["PHOENIX_LOG_FILE"])
        self.assertEqual(
            str(self.dev.LAUNCHD_FATAL_LOG_PATH), environment["PHOENIX_FATAL_LOG_FILE"]
        )
        self.assertEqual("false", environment["PHOENIX_LOG_STDOUT"])
        self.assertEqual("/dev/null", plist["StandardOutPath"])
        self.assertEqual(
            str(self.dev.LAUNCHD_STDERR_LOG_PATH), plist["StandardErrorPath"]
        )


    def test_newsyslog_bounds_launchd_pre_main_stderr(self):
        config = self.dev._newsyslog_config("phoenix", "staff")

        self.assertNotIn(str(self.dev.LAUNCHD_LOG_PATH), config)
        self.assertIn(
            f"{self.dev.LAUNCHD_STDERR_LOG_PATH}    phoenix:staff    600  2    64",
            config,
        )
        self.assertIn("64    *  BJN", config)



    def test_retired_macos_options_and_commands_reject_before_subprocess(self):
        selectors = [
            ["deploy", "--paired-database-upgrade"], ["deploy", "--current-schema-upgrade"],
            ["deploy", "--same-schema-refresh"], ["resume-forward", "old"],
            ["recover-current", "old"], ["repair-current", "old"],
            ["finalize-paired", "old"], ["cleanup-current", "old"],
        ]
        for selection in selectors:
            stderr = io.StringIO()
            stderr.reconfigure = mock.Mock()
            with self.subTest(selection=selection), mock.patch.object(sys, "argv", ["dev.py", "prod", *selection]), mock.patch.object(sys, "stderr", stderr):
                with self.assertRaisesRegex(SystemExit, "2"):
                    self.dev.main()

    def test_even_committed_status_does_not_reclaim_an_existing_owner(self):
        active = self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH
        active.parent.mkdir(parents=True)
        active.write_text("owner\n")
        self.dev.LAUNCHD_DEPLOY_STATUS_PATH.write_text(json.dumps({"transaction_id": "owner", "state": "committed"}))
        for action in (self.dev._claim_launchd_deploy, self.dev._claim_launchd_restart):
            with self.assertRaisesRegex(self.dev.ConcurrentLaunchdOperation, "owner"):
                action("new")
        self.assertEqual("owner\n", active.read_text())

    def test_unmocked_service_control_is_refused(self):
        for command in (["launchctl", "bootout", "anything"], ["sudo", "systemctl", "stop", "anything"]):
            for operation in (self.dev.subprocess.run, self.dev.subprocess.Popen):
                with self.assertRaisesRegex(AssertionError, "unmocked subprocess"):
                    operation(command)

    def staged_deploy(self, *, protocol="2", bootstrap_error=False):
        root = self.dev.LAUNCHD_DEPLOY_DIR.parent
        binary = root / "built-candidate"
        binary.write_bytes(b"candidate bytes")
        identity = self.dev.RuntimeIdentity("2.0.0", "a" * 40)
        candidate = self.dev.PreparedCandidate(binary, self.dev.ProdSourceKind.LOCAL_HEAD, identity.git_sha, identity)
        database = self.dev.PROD_DB_PATH
        database.write_bytes(b"This database must not be opened during preparation")
        historical = self.dev.LAUNCHD_DEPLOY_DIR / "transactions/historical"
        historical.mkdir(parents=True)
        (historical / "manifest.json").write_text("historical evidence")
        events = []
        def materialize(commit, destination, kind):
            events.append(("helper", commit, kind))
            shutil.copy2(ROOT / "scripts/launchd_deploy_helper.py", destination)
        def run(command, **_kwargs):
            command = [str(part) for part in command]
            events.append(command)
            if "--protocol-version" in command:
                return subprocess.CompletedProcess(command, 0, protocol + "\n", "")
            if "--build-identity" in command:
                return subprocess.CompletedProcess(command, 0, json.dumps(dataclasses.asdict(identity)), "")
            if command[:2] == ["launchctl", "bootstrap"]:
                status = json.loads(self.dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())
                self.assertEqual("prepared", status["state"])
                if bootstrap_error:
                    raise subprocess.CalledProcessError(5, command)
            return subprocess.CompletedProcess(command, 0, "", "")
        with mock.patch.object(self.dev, "_launchd_candidate_env", return_value=({"PHOENIX_PASSWORD": "fixture"}, None)), mock.patch.object(self.dev, "_prepare_local_candidate", return_value=candidate), mock.patch.object(self.dev, "_binary_identity", return_value=identity), mock.patch.object(self.dev, "capture_login_shell_path", return_value=("/usr/bin:/bin", "test")), mock.patch.object(self.dev, "print_launchd_path_report"), mock.patch.object(self.dev, "_materialize_helper", side_effect=materialize), mock.patch.object(self.dev, "_ensure_newsyslog_config"), mock.patch.object(self.dev.subprocess, "run", side_effect=run):
            if protocol != "2":
                with self.assertRaisesRegex(SystemExit, "unsupported deployment helper protocol"):
                    self.dev.launchd_prod_deploy()
            elif bootstrap_error:
                with self.assertRaises(subprocess.CalledProcessError):
                    self.dev.launchd_prod_deploy()
            else:
                self.dev.launchd_prod_deploy()
        self.assertEqual(b"This database must not be opened during preparation", database.read_bytes())
        self.assertEqual("historical evidence", (historical / "manifest.json").read_text())
        transactions = [path for path in historical.parent.iterdir() if path != historical]
        self.assertEqual(1, len(transactions))
        return transactions[0], events

    def test_installer_stages_only_protocol_two_ordinary_handoff(self):
        staging, events = self.staged_deploy()
        manifest = json.loads((staging / "manifest.json").read_text())
        self.assertEqual(2, manifest["manifest_version"])
        loaded = helper.Manifest.load(staging / "manifest.json")
        self.assertEqual("a" * 40, loaded.expected.git_sha)
        self.assertEqual(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.read_text().strip(), manifest["transaction_id"])
        self.assertEqual(set(dataclasses.asdict(loaded)), set(manifest))
        self.assertFalse(any("registry" in key or "rollback" in key or "upgrade" in key for key in manifest))
        protocol = next(i for i, event in enumerate(events) if isinstance(event, list) and "--protocol-version" in event)
        bootstrap = next(i for i, event in enumerate(events) if isinstance(event, list) and event[:2] == ["launchctl", "bootstrap"])
        self.assertLess(protocol, bootstrap)

    def test_old_helper_protocol_fails_before_handoff_and_releases_claim(self):
        _staging, events = self.staged_deploy(protocol="1")
        self.assertFalse(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
        self.assertEqual("precondition_failed", json.loads(self.dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())["state"])
        self.assertFalse(any(isinstance(event, list) and "launchctl" in event for event in events))

    def test_uncertain_handoff_keeps_claim_and_prepared_status_for_manual_inspection(self):
        self.staged_deploy(bootstrap_error=True)
        self.assertTrue(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
        self.assertEqual("prepared", json.loads(self.dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text())["state"])

    def test_failed_status_reports_full_diagnostic_and_manual_recovery(self):
        status = self.dev.LAUNCHD_DEPLOY_STATUS_PATH
        status.parent.mkdir(parents=True)
        status.write_text(json.dumps({"transaction_id": "failed", "state": "failed", "failure": "migration_started: exit 7; stopping failed: denied"}))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.dev._print_launchd_deploy_status()
        self.assertIn("exit 7; stopping failed: denied", output.getvalue())
        self.assertIn("Manual recovery required", output.getvalue())
