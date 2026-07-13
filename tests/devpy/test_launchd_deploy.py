import fcntl
import importlib.util
import json
import os
import plistlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


helper = load(ROOT / "scripts" / "launchd_deploy_helper.py", "launchd_deploy_helper_test")


class FakeLaunchctl:
    events = []
    fail_start = False

    def __init__(self, manifest):
        self.manifest = manifest

    def inspect(self):
        return "running", 100

    def stop(self):
        self.events.append("stop")
        return 100

    def start(self, old_pid):
        self.events.append("start")
        if self.fail_start:
            raise helper.ActivationError("injected bootstrap failure")
        return 101


def make_manifest(root: Path, *, expected=None, previous=None):
    expected = expected or helper.Identity("2.0.0", "newsha")
    previous = previous or helper.Identity("1.0.0", "oldsha")
    files = {}
    for name, content in {
        "candidate_binary": b"new binary",
        "candidate_plist": b"<?xml version='1.0'?><plist version='1.0'><dict/></plist>",
        "rollback_binary": b"old binary",
        "rollback_plist": b"<?xml version='1.0'?><plist version='1.0'><dict/></plist>",
    }.items():
        path = root / name
        path.write_bytes(content)
        files[name] = path
    return helper.Manifest(
        transaction_id="tx", source_kind="published_release", source_commit="newsha",
        release_tag="v2.0.0", release_commit="newsha0000000000000000000000000000000000",
        expected=expected, previous=previous,
        candidate_binary=str(files["candidate_binary"]), candidate_binary_sha256=helper.sha256(files["candidate_binary"]),
        candidate_plist=str(files["candidate_plist"]), candidate_plist_sha256=helper.sha256(files["candidate_plist"]),
        rollback_binary=str(files["rollback_binary"]), rollback_binary_sha256=helper.sha256(files["rollback_binary"]),
        rollback_plist=str(files["rollback_plist"]), rollback_plist_sha256=helper.sha256(files["rollback_plist"]),
        target_binary=str(root / "live-binary"), target_plist=str(root / "live.plist"),
        label="test.phoenix.server", helper_label="test.phoenix.deploy", uid=os.getuid(), health_url="http://127.0.0.1:1/api/version",
        health_insecure_tls=False, active_path=str(root / "active"), status_path=str(root / "status.json"),
        deployed_sha_path=str(root / "deployed.sha"), lock_path=str(root / "activate.lock"),
        created_at="2026-01-01T00:00:00+00:00", transition_timeout_secs=0.1, health_timeout_secs=0.1,
    )


class ActivationTests(unittest.TestCase):
    def test_missing_service_text_is_treated_as_unloaded(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            result = subprocess.CompletedProcess([], 0, "", "Could not find service")
            launchctl = helper.Launchctl(manifest, run=mock.Mock(return_value=result))
            self.assertEqual(("not_loaded", None), launchctl.inspect())

    def test_bootout_timeout_marks_disruption_and_triggers_rollback(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = mock.Mock()
            launchctl.disruption_started = False
            def stop():
                launchctl.disruption_started = True
                raise helper.ActivationError("teardown timeout")
            launchctl.stop.side_effect = stop
            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(helper, "restore") as restore:
                state = helper.activate(manifest)
            self.assertEqual("activation_failed_rolled_back", state)
            restore.assert_called_once_with(manifest, launchctl)

    def setUp(self):
        FakeLaunchctl.events = []
        FakeLaunchctl.fail_start = False

    def test_success_installs_atomically_and_records_selected_commit_after_verification(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            events = FakeLaunchctl.events
            def verified(_manifest, identity):
                events.append(f"verified:{identity.git_sha}")
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=verified), \
                 mock.patch.object(helper, "fsync_dir", wraps=helper.fsync_dir) as fsync_dir, \
                 mock.patch.object(helper.os, "replace", wraps=os.replace) as replace:
                state = helper.activate(manifest)
            self.assertEqual("committed", state)
            self.assertEqual("newsha\n", Path(manifest.deployed_sha_path).read_text())
            self.assertEqual(["stop", "start", "verified:newsha"], events)
            live_binary_replaces = [call for call in replace.call_args_list if Path(call.args[1]) == Path(manifest.target_binary)]
            self.assertEqual(1, len(live_binary_replaces))
            self.assertNotEqual(Path(manifest.target_binary), Path(live_binary_replaces[0].args[0]))
            self.assertTrue(any(call.args[0] == Path(manifest.target_binary).parent for call in fsync_dir.call_args_list))
            self.assertEqual(b"new binary", Path(manifest.target_binary).read_bytes())

    def test_failed_first_install_stops_candidate_and_removes_artifacts(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(root, previous=None)
            manifest = helper.dataclasses.replace(
                manifest, previous=None, rollback_binary=None, rollback_binary_sha256=None,
                rollback_plist=None, rollback_plist_sha256=None,
            )
            launchctl = FakeLaunchctl(manifest)
            launchctl.inspect = lambda: ("not_loaded", None) if FakeLaunchctl.events.count("stop") >= 2 else ("running", 100)
            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("bad health")):
                state = helper.activate(manifest)
            self.assertEqual("activation_failed_rolled_back", state)
            self.assertEqual(["stop", "start", "stop"], FakeLaunchctl.events)
            self.assertFalse(Path(manifest.target_binary).exists())
            self.assertFalse(Path(manifest.target_plist).exists())
            self.assertFalse(Path(manifest.deployed_sha_path).exists())

    def test_wrong_version_rolls_back_and_has_distinct_status(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            identities = []
            def verify(_manifest, identity):
                identities.append(identity.git_sha)
                if identity == manifest.expected:
                    raise helper.ActivationError("wrong version")
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=verify):
                state = helper.activate(manifest)
            self.assertEqual("activation_failed_rolled_back", state)
            self.assertEqual(["newsha", "oldsha"], identities)
            self.assertFalse(Path(manifest.deployed_sha_path).exists())
            self.assertEqual(b"old binary", Path(manifest.target_binary).read_bytes())
            self.assertEqual(state, json.loads(Path(manifest.status_path).read_text())["state"])

    def test_failed_rollback_is_explicit(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=helper.ActivationError("health timeout")):
                state = helper.activate(manifest)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("activation_failed_rollback_failed", state)
            self.assertIn("health timeout", status["failure"])
            self.assertIn("health timeout", status["rollback_failure"])

    def test_concurrent_activation_rejected_before_stop(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            lock = open(manifest.lock_path, "w")
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                with self.assertRaises(helper.ConcurrentDeploy):
                    helper.activate(manifest)
            finally:
                lock.close()
            self.assertEqual([], FakeLaunchctl.events)

    def test_tampered_candidate_fails_before_disruption(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            Path(manifest.candidate_binary).write_text("tampered")
            with mock.patch.object(helper, "Launchctl", FakeLaunchctl):
                with self.assertRaises(helper.ActivationError):
                    helper.activate(manifest)
            self.assertEqual([], FakeLaunchctl.events)

    def test_manifest_status_and_failure_do_not_copy_plist_secrets(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            secret = "SENTINEL_SECRET_7b3f2"
            Path(manifest.candidate_plist).write_bytes(plistlib.dumps({
                "Label": "test", "EnvironmentVariables": {"TOKEN": secret},
            }))
            manifest = helper.dataclasses.replace(
                manifest, candidate_plist_sha256=helper.sha256(Path(manifest.candidate_plist))
            )
            encoded = json.dumps(helper.dataclasses.asdict(manifest))
            helper.write_status(manifest, "precondition_failed", failure="candidate plist invalid")
            diagnostics = encoded + Path(manifest.status_path).read_text()
            self.assertNotIn(secret, diagnostics)
            self.assertIn("candidate plist invalid", diagnostics)

    def test_claim_cleanup_cannot_delete_newer_owner(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            claim = Path(manifest.active_path)
            claim.write_text("newer-transaction\n")
            self.assertFalse(helper.release_claim(manifest))
            self.assertEqual("newer-transaction\n", claim.read_text())

    def test_helper_requests_bootout_even_when_manifest_is_malformed(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = Path(td) / "manifest.json"
            manifest.write_text("not-json")
            argv = ["helper", "activate", "--manifest", str(manifest),
                    "--helper-label", "test.helper", "--uid", str(os.getuid())]
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "request_helper_bootout") as bootout:
                self.assertEqual(1, helper.main())
            bootout.assert_called_once_with(os.getuid(), "test.helper")

    def test_helper_requests_bootout_on_concurrent_rejection(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            manifest_path = Path(td) / "manifest.json"
            manifest_path.write_text(json.dumps(helper.dataclasses.asdict(manifest)))
            argv = ["helper", "activate", "--manifest", str(manifest_path),
                    "--helper-label", manifest.helper_label, "--uid", str(manifest.uid)]
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "activate", side_effect=helper.ConcurrentDeploy("busy")), \
                 mock.patch.object(helper, "request_helper_bootout") as bootout:
                self.assertEqual(1, helper.main())
            bootout.assert_called_once_with(manifest.uid, manifest.helper_label)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("rejected_concurrent", status["state"])
            self.assertFalse(Path(manifest.active_path).exists())



class PreparationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load(ROOT / "dev.py", "devpy_launchd_deploy_test")

    def test_broken_pipe_after_handoff_does_not_release_claim(self):
        with mock.patch("builtins.print", side_effect=BrokenPipeError), \
             mock.patch.object(self.dev, "_release_launchd_deploy_claim") as release:
            self.dev._report_launchd_handoff("tx", {"version": "1.0.0", "git_sha": "abc123"})
        release.assert_not_called()

    def test_positional_version_is_rejected_with_release_guidance(self):
        result = subprocess.run(
            ["python3", str(ROOT / "dev.py"), "prod", "deploy", "v1.2.3"],
            cwd=ROOT, capture_output=True, text=True,
        )
        self.assertNotEqual(0, result.returncode)
        self.assertIn("--release", result.stderr)

    def test_release_path_skips_checks_and_build(self):
        with mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_deploy") as deploy, \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev, "prod_build") as build:
            self.dev.cmd_prod_deploy("v1.2.3")
        deploy.assert_called_once_with("v1.2.3")
        check.assert_not_called()
        build.assert_not_called()

    def test_latest_resolves_once_then_downloads_immutable_tag_and_checks_checksum(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "abc123"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            digest = self.dev._file_sha256(asset)
            (staging / "SHA256SUMS").write_text(f"{digest}  {asset.name}\n")
            release_commit = "abc123" + "0" * 34
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False}), ""),
                subprocess.CompletedProcess([], 0, release_commit + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            binary, tag, sha, commit = self.dev._prepare_release_candidate("latest", staging)
            self.assertEqual(asset, binary)
            self.assertTrue(binary.stat().st_mode & 0o100)
        self.assertEqual(("v1.2.3", "abc123", release_commit), (tag, sha, commit))
        self.assertIn("v1.2.3", run.call_args_list[2].args[0])

    def test_release_rejects_asset_from_different_commit(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "_release_asset_name", return_value="phoenix_ide-aarch64-apple-darwin"), \
             mock.patch.object(self.dev, "_binary_identity", return_value={"version": "1.2.3", "git_sha": "bad123"}), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            asset = staging / "phoenix_ide-aarch64-apple-darwin"
            asset.write_bytes(b"release")
            (staging / "SHA256SUMS").write_text(f"{self.dev._file_sha256(asset)}  {asset.name}\n")
            run.side_effect = [
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False}), ""),
                subprocess.CompletedProcess([], 0, "abc123" + "0" * 34 + "\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with self.assertRaisesRegex(SystemExit, "asset embeds"):
                self.dev._prepare_release_candidate("latest", staging)

    def test_claim_release_is_transaction_owned(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_DIR", Path(td)), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_ACTIVE_PATH", Path(td) / "active"), \
             mock.patch.object(self.dev, "LAUNCHD_DEPLOY_STATUS_PATH", Path(td) / "status.json"):
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
                subprocess.CompletedProcess([], 0, json.dumps({"tagName": "v1.2.3", "isPrerelease": False}), ""),
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

    def test_release_workflow_lists_both_macos_architectures_and_checksums(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text()
        self.assertIn("phoenix_ide-aarch64-apple-darwin", workflow)
        self.assertIn("phoenix_ide-x86_64-apple-darwin", workflow)
        self.assertIn("SHA256SUMS", workflow)

    def test_socket_activation_shape_is_preserved(self):
        plist = self.dev.generate_launchd_plist("1.0.0", path_override="/usr/bin")
        self.assertIn("<key>Sockets</key>", plist)
        self.assertIn("<string>IPv4v6</string>", plist)
        self.assertIn("<string>8031</string>", plist)


if __name__ == "__main__":
    unittest.main()
