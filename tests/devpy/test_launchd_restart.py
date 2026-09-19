import dataclasses
import importlib.util
import io
import json
import os
import plistlib
import signal
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


helper = load(ROOT / "scripts" / "launchd_restart_helper.py", "launchd_restart_helper_test")


def make_manifest(root: Path) -> helper.Manifest:
    binary = root / "phoenix-ide"
    binary.write_bytes(b"installed binary")
    plist = root / "service.plist"
    plist.write_bytes(plistlib.dumps({"Label": "test.phoenix.server"}))
    deployed_sha = root / "deployed.sha"
    deployed_sha.write_text("a" * 40 + "\n")
    active = root / "restart-active"
    active.write_text("restart-tx\n")
    return helper.Manifest(
        manifest_version=helper.HANDOFF_PROTOCOL_VERSION,
        transaction_id="restart-tx",
        expected=helper.Identity("2.0.0", "a" * 40),
        previous_pid=100,
        binary_path=str(binary),
        binary_sha256=helper.sha256(binary),
        plist_path=str(plist),
        plist_sha256=helper.sha256(plist),
        socket_service=1,
        deployed_sha_path=str(deployed_sha),
        deployed_sha256=helper.sha256(deployed_sha),
        label="test.phoenix.server",
        helper_label="test.phoenix.restart",
        uid=os.getuid(),
        health_url="http://127.0.0.1:1/api/version",
        health_insecure_tls=False,
        active_path=str(active),
        status_path=str(root / "restart-status.json"),
        lock_path=str(root / "activation.lock"),
        claim_lock_path=str(root / "claim.lock"),
        created_at="2026-01-01T00:00:00+00:00",
        shutdown_timeout_secs=0.1,
        transition_timeout_secs=0.1,
        health_timeout_secs=0.1,
    )


class FakeLaunchctl:
    configuration_matches = helper.Launchctl.configuration_matches
    require_configuration = helper.Launchctl.require_configuration

    def __init__(self, manifest):
        self.manifest = manifest
        self.signals = []

    def inspect(self):
        pid = 100 if not self.signals else 101
        return helper.LoadedJob(
            "running",
            pid,
            True,
            self.manifest.plist_path,
            self.manifest.binary_path,
            (str(self.manifest.socket_service),),
        )

    def signal_hup(self, expected_pid):
        if expected_pid != 100:
            raise AssertionError("wrong expected PID")
        self.signals.append("HUP")
        return 100

    def wait_for_new_pid(self, previous_pid):
        if previous_pid != 100:
            raise AssertionError("wrong previous PID")
        return 101


class RestartHelperTests(unittest.TestCase):
    def test_signal_targets_inspected_pid_without_unloading_target(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            run = mock.Mock(return_value=subprocess.CompletedProcess(
                [],
                0,
                (
                    f"path = {manifest.plist_path}\n"
                    "state = running\n"
                    f"program = {manifest.binary_path}\n"
                    "pid = 100\n"
                    "service name = 1\n"
                    "properties = keepalive | runatload\n"
                ),
                "",
            ))
            kill = mock.Mock()

            signal_pid = helper.Launchctl(manifest, run=run, kill=kill).signal_hup(100)

            self.assertEqual(100, signal_pid)
            run.assert_called_once_with(
                ["launchctl", "print", f"gui/{manifest.uid}/{manifest.label}"],
                capture_output=True,
                text=True,
            )
            kill.assert_called_once_with(100, signal.SIGHUP)

    def test_inspection_failure_preserves_launchctl_diagnostic(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            run = mock.Mock(return_value=subprocess.CompletedProcess(
                [],
                64,
                "",
                "launchctl domain temporarily unavailable",
            ))

            with self.assertRaisesRegex(
                helper.RestartError,
                "domain temporarily unavailable",
            ):
                helper.Launchctl(manifest, run=run).inspect()

    def test_short_restart_identity_is_rejected_before_target_disruption(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = dataclasses.replace(
                make_manifest(root),
                expected=helper.Identity("2.0.0", "a" * 12),
            )
            manifest_path = root / "manifest.json"
            manifest_path.write_text(json.dumps(dataclasses.asdict(manifest)))
            argv = [
                "launchd_restart_helper.py",
                "restart",
                "--manifest",
                str(manifest_path),
                "--helper-label",
                manifest.helper_label,
                "--uid",
                str(manifest.uid),
            ]

            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "restart") as restart, \
                 mock.patch.object(helper, "request_helper_bootout"), \
                 mock.patch.object(sys, "stderr", new_callable=io.StringIO) as stderr:
                self.assertEqual(1, helper.main())

            restart.assert_not_called()
            self.assertIn("requires a full lowercase git SHA", stderr.getvalue())
            self.assertFalse(Path(manifest.status_path).exists())

    def test_identity_probe_requires_runtime_socket_activation(self):
        response = io.BytesIO(json.dumps({
            "version": "2.0.0",
            "git_sha": "a" * 40,
            "socket_activated": False,
        }).encode())
        with mock.patch.object(helper.urllib.request, "urlopen", return_value=response):
            with self.assertRaisesRegex(helper.RestartError, "socket activation"):
                helper.fetch_identity("http://127.0.0.1/version", 1.0, False)

    def test_restart_preserves_installed_artifacts_and_commits_exact_identity(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            binary_before = Path(manifest.binary_path).read_bytes()
            plist_before = Path(manifest.plist_path).read_bytes()
            sha_before = Path(manifest.deployed_sha_path).read_bytes()
            launchctl = FakeLaunchctl(manifest)

            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(helper, "fetch_identity", return_value=manifest.expected), \
                 mock.patch.object(helper, "wait_for_identity") as wait:
                state = helper.restart(manifest)

            self.assertEqual("committed", state)
            self.assertEqual(["HUP"], launchctl.signals)
            wait.assert_called_once_with(manifest, manifest.expected)
            self.assertEqual(binary_before, Path(manifest.binary_path).read_bytes())
            self.assertEqual(plist_before, Path(manifest.plist_path).read_bytes())
            self.assertEqual(sha_before, Path(manifest.deployed_sha_path).read_bytes())
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("committed", status["state"])
            self.assertEqual(100, status["previous_pid"])
            self.assertEqual(101, status["running_pid"])

    def test_signal_rejects_pid_rebound_after_identity_verification(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            run = mock.Mock(return_value=subprocess.CompletedProcess(
                [],
                0,
                (
                    f"path = {manifest.plist_path}\n"
                    "state = running\n"
                    f"program = {manifest.binary_path}\n"
                    "pid = 101\n"
                    "service name = 1\n"
                    "properties = keepalive | runatload\n"
                ),
                "",
            ))
            kill = mock.Mock()

            with self.assertRaisesRegex(
                helper.RestartError,
                "PID changed between identity verification and signaling",
            ):
                helper.Launchctl(manifest, run=run, kill=kill).signal_hup(100)

            kill.assert_not_called()

    def test_replacement_deadline_includes_bounded_shutdown_budget(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = dataclasses.replace(
                make_manifest(Path(td)),
                shutdown_timeout_secs=0.2,
                transition_timeout_secs=0.1,
            )
            replacement = helper.LoadedJob(
                "running",
                101,
                True,
                manifest.plist_path,
                manifest.binary_path,
                (str(manifest.socket_service),),
            )
            monotonic = mock.Mock(side_effect=[0.0, 0.15])
            launchctl = helper.Launchctl(
                manifest,
                monotonic=monotonic,
                sleep=mock.Mock(),
            )

            with mock.patch.object(launchctl, "inspect", return_value=replacement):
                self.assertEqual(101, launchctl.wait_for_new_pid(100))

    def test_restart_rejects_identity_verified_against_a_later_pid(self):
        class ReplacedDuringHealthCheck(FakeLaunchctl):
            def inspect(self):
                pid = 100 if not self.signals else 102
                return dataclasses.replace(super().inspect(), pid=pid)

        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = ReplacedDuringHealthCheck(manifest)

            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(helper, "fetch_identity", return_value=manifest.expected), \
                 mock.patch.object(helper, "wait_for_identity"):
                state = helper.restart(manifest)

            self.assertEqual("restart_failed", state)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("restart_failed", status["state"])
            self.assertIn("PID changed during identity verification", status["failure"])

    def test_artifact_change_is_rejected_before_signal(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            Path(manifest.plist_path).write_bytes(b"changed")
            launchctl = FakeLaunchctl(manifest)

            with mock.patch.object(helper, "Launchctl", return_value=launchctl):
                with self.assertRaisesRegex(helper.RestartError, "plist checksum mismatch"):
                    helper.restart(manifest)

            self.assertEqual([], launchctl.signals)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("precondition_failed", status["state"])

    def test_artifact_change_during_identity_probe_is_rejected_before_signal(self):
        class RevalidatingLaunchctl(FakeLaunchctl):
            signal_hup = helper.Launchctl.signal_hup

            def __init__(self, manifest):
                super().__init__(manifest)
                self.kill = mock.Mock()

        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = RevalidatingLaunchctl(manifest)

            def replace_binary_during_probe(*_args, **_kwargs):
                Path(manifest.binary_path).write_bytes(b"replaced")
                return manifest.expected

            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(
                     helper,
                     "fetch_identity",
                     side_effect=replace_binary_during_probe,
                 ):
                with self.assertRaisesRegex(
                    helper.RestartError,
                    "binary checksum mismatch",
                ):
                    helper.restart(manifest)

            launchctl.kill.assert_not_called()
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("precondition_failed", status["state"])

    def test_loaded_job_without_keepalive_is_rejected_before_signal(self):
        class NoKeepAliveLaunchctl(FakeLaunchctl):
            def inspect(self):
                return dataclasses.replace(super().inspect(), keep_alive=False)

        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = NoKeepAliveLaunchctl(manifest)

            with mock.patch.object(helper, "Launchctl", return_value=launchctl):
                with self.assertRaisesRegex(helper.RestartError, "keepalive=False"):
                    helper.restart(manifest)

            self.assertEqual([], launchctl.signals)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("precondition_failed", status["state"])

    def test_loaded_listener_mismatch_is_rejected_before_signal(self):
        class WrongListenerLaunchctl(FakeLaunchctl):
            def inspect(self):
                return dataclasses.replace(super().inspect(), socket_services=("2",))

        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = WrongListenerLaunchctl(manifest)

            with mock.patch.object(helper, "Launchctl", return_value=launchctl):
                with self.assertRaisesRegex(helper.RestartError, "socket_services"):
                    helper.restart(manifest)

            self.assertEqual([], launchctl.signals)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("precondition_failed", status["state"])

    def test_lock_open_failure_terminalizes_and_releases_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(root)
            Path(manifest.lock_path).mkdir()
            manifest_path = root / "manifest.json"
            manifest_path.write_text(json.dumps(dataclasses.asdict(manifest)))
            argv = [
                "launchd_restart_helper.py",
                "restart",
                "--manifest",
                str(manifest_path),
                "--helper-label",
                manifest.helper_label,
                "--uid",
                str(manifest.uid),
            ]

            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "request_helper_bootout") as bootout, \
                 mock.patch.object(sys, "stderr", new_callable=io.StringIO):
                self.assertEqual(1, helper.main())

            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("precondition_failed", status["state"])
            self.assertIn("directory", status["failure"])
            self.assertFalse(Path(manifest.active_path).exists())
            bootout.assert_called_once_with(manifest.uid, manifest.helper_label)

    def test_lock_acquisition_failure_terminalizes_and_releases_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(root)
            real_flock = helper.fcntl.flock

            def fail_activation_lock(lock, operation):
                if operation & helper.fcntl.LOCK_NB:
                    raise OSError("lock unavailable")
                return real_flock(lock, operation)

            manifest_path = root / "manifest.json"
            manifest_path.write_text(json.dumps(dataclasses.asdict(manifest)))
            argv = [
                "launchd_restart_helper.py",
                "restart",
                "--manifest",
                str(manifest_path),
                "--helper-label",
                manifest.helper_label,
                "--uid",
                str(manifest.uid),
            ]

            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper.fcntl, "flock", side_effect=fail_activation_lock), \
                 mock.patch.object(helper, "request_helper_bootout"), \
                 mock.patch.object(sys, "stderr", new_callable=io.StringIO):
                self.assertEqual(1, helper.main())

            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("precondition_failed", status["state"])
            self.assertIn("lock unavailable", status["failure"])
            self.assertFalse(Path(manifest.active_path).exists())

    def test_lock_failure_retains_claim_when_terminal_status_is_not_durable(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            manifest = make_manifest(root)
            Path(manifest.lock_path).mkdir()
            manifest_path = root / "manifest.json"
            manifest_path.write_text(json.dumps(dataclasses.asdict(manifest)))
            argv = [
                "launchd_restart_helper.py",
                "restart",
                "--manifest",
                str(manifest_path),
                "--helper-label",
                manifest.helper_label,
                "--uid",
                str(manifest.uid),
            ]

            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(helper, "write_status", side_effect=OSError("disk full")), \
                 mock.patch.object(helper, "request_helper_bootout"), \
                 mock.patch.object(sys, "stderr", new_callable=io.StringIO):
                self.assertEqual(1, helper.main())

            self.assertTrue(Path(manifest.active_path).exists())

    def test_failed_recovery_is_truthful_and_does_not_claim_rollback(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            launchctl = FakeLaunchctl(manifest)

            with mock.patch.object(helper, "Launchctl", return_value=launchctl), \
                 mock.patch.object(helper, "fetch_identity", return_value=manifest.expected), \
                 mock.patch.object(helper, "wait_for_identity", side_effect=helper.RestartError("wrong identity")):
                state = helper.restart(manifest)

            self.assertEqual("restart_failed", state)
            status = json.loads(Path(manifest.status_path).read_text())
            self.assertEqual("restart_failed", status["state"])
            self.assertIn("wrong identity", status["failure"])
            self.assertNotIn("rollback", status)

    def test_helper_releases_only_its_own_claim_after_terminal_status(self):
        with tempfile.TemporaryDirectory() as td:
            manifest = make_manifest(Path(td))
            active = Path(manifest.active_path)
            helper.write_status(manifest, "committed", running_pid=101)

            self.assertFalse(helper.release_claim(helper.dataclasses.replace(manifest, transaction_id="other")))
            self.assertEqual("restart-tx", active.read_text().strip())
            real_fsync = os.fsync
            with mock.patch.object(helper.os, "fsync", wraps=real_fsync) as fsync:
                self.assertTrue(helper.release_claim(manifest))
            self.assertEqual(1, fsync.call_count)
            self.assertFalse(active.exists())


class RestartCommandTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load(ROOT / "dev.py", "phoenix_dev_launchd_restart_test")

    def _isolated_operation_paths(self, root: Path):
        restart = root / "restart"
        deploy = root / "deploy"
        return mock.patch.multiple(
            self.dev,
            LAUNCHD_INSTALL_DIR=root / "install",
            LAUNCHD_PLIST_PATH=root / "service.plist",
            PROD_SHA_PATH=root / "deployed.sha",
            LAUNCHD_RESTART_DIR=restart,
            LAUNCHD_RESTART_TRANSACTIONS_DIR=restart / "transactions",
            LAUNCHD_RESTART_ACTIVE_PATH=restart / "active",
            LAUNCHD_DEPLOY_DIR=deploy,
            LAUNCHD_DEPLOY_STATUS_PATH=deploy / "status.json",
            LAUNCHD_DEPLOY_ACTIVE_PATH=deploy / "active",
            LAUNCHD_DEPLOY_CLAIM_LOCK_PATH=deploy / "claim.lock",
            LAUNCHD_DEPLOY_LOCK_PATH=deploy / "activate.lock",
        )

    def _installed_plist(self, binary: Path) -> bytes:
        return plistlib.dumps({
            "Label": self.dev.LAUNCHD_LABEL,
            "ProgramArguments": [str(binary)],
            "EnvironmentVariables": {"PHOENIX_PASSWORD": "installed-secret"},
            "Sockets": {"Listeners": {
                "SockFamily": "IPv4v6",
                "SockProtocol": "TCP",
                "SockServiceName": "9555",
                "SockType": "stream",
            }},
            "KeepAlive": True,
            "RunAtLoad": True,
        })

    def test_atomic_json_write_fsyncs_file_and_containing_directory(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "status.json"
            real_fsync = os.fsync

            with mock.patch.object(self.dev.os, "fsync", wraps=real_fsync) as fsync:
                self.dev._write_json_atomic(path, {"state": "committed"})

            self.assertEqual(2, fsync.call_count)
            self.assertEqual({"state": "committed"}, json.loads(path.read_text()))

    def test_durable_mkdir_fsyncs_new_ancestors_and_existing_parent(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            leaf = root / "restart" / "transactions" / "tx"

            with mock.patch.object(self.dev, "_fsync_directory") as fsync_directory:
                self.dev._mkdir_durable(leaf, mode=0o700)

            self.assertTrue(leaf.is_dir())
            self.assertEqual(
                [
                    mock.call(leaf),
                    mock.call(leaf.parent),
                    mock.call(leaf.parent.parent),
                    mock.call(root),
                ],
                fsync_directory.call_args_list,
            )

    def test_launchd_restart_hands_off_installed_state_without_build_or_env_reload(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            install = root / "install"
            install.mkdir()
            binary = install / "phoenix-ide"
            binary.write_bytes(b"installed")
            plist = root / "service.plist"
            plist.write_bytes(self._installed_plist(binary))
            deployed_sha = root / "deployed.sha"
            deployed_sha.write_text("a" * 40 + "\n")
            restart_dir = root / "restart"
            deploy_dir = root / "deploy"
            deploy_dir.mkdir()
            deploy_status = deploy_dir / "status.json"
            deploy_status.write_text('{"state":"committed","source_kind":"published_release"}\n')
            identity = self.dev.RuntimeIdentity("2.0.0", "a" * 40)
            commands = []

            def run(command, **_kwargs):
                commands.append([str(part) for part in command])
                if command[:2] == ["launchctl", "print"]:
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        (
                            f"path = {plist}\n"
                            "state = running\n"
                            f"program = {binary}\n"
                            "pid = 100\n"
                            "service name = 9555\n"
                            "properties = keepalive | runatload\n"
                        ),
                        "",
                    )
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, "2\n", "")
                return subprocess.CompletedProcess(command, 0, "", "")

            with self._isolated_operation_paths(root), \
                 mock.patch.object(self.dev, "LAUNCHD_RESTART_HELPER_SOURCE", ROOT / "scripts" / "launchd_restart_helper.py"), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=identity), \
                 mock.patch.object(self.dev, "_current_prod_identity", return_value=identity), \
                 mock.patch.object(self.dev, "_load_env_file") as load_env, \
                 mock.patch.object(self.dev, "prod_build") as build, \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                self.dev.launchd_prod_restart()

            load_env.assert_not_called()
            build.assert_not_called()
            statuses = list((restart_dir / "transactions").glob("*/status.json"))
            self.assertEqual(1, len(statuses))
            status = json.loads(statuses[0].read_text())
            self.assertEqual("prepared", status["state"])
            self.assertEqual("installed_restart", status["source_kind"])
            transactions = list((restart_dir / "transactions").iterdir())
            self.assertEqual(1, len(transactions))
            manifest = json.loads((transactions[0] / "manifest.json").read_text())
            self.assertEqual(str(binary), manifest["binary_path"])
            self.assertEqual(str(plist), manifest["plist_path"])
            self.assertEqual("http://localhost:9555/api/version", manifest["health_url"])
            self.assertEqual(
                self.dev.LAUNCHD_RESTART_SHUTDOWN_TIMEOUT_SECS,
                manifest["shutdown_timeout_secs"],
            )
            self.assertNotIn("installed-secret", json.dumps(manifest))
            self.assertEqual(
                '{"state":"committed","source_kind":"published_release"}\n',
                deploy_status.read_text(),
            )
            flattened = [part for command in commands for part in command]
            self.assertNotIn("codesign", flattened)
            self.assertNotIn("bootout", flattened)
            self.assertNotIn("kill", flattened)

    def test_install_change_after_validation_is_rejected_before_handoff(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            install = root / "install"
            install.mkdir()
            binary = install / "phoenix-ide"
            binary.write_bytes(b"installed")
            plist = root / "service.plist"
            plist.write_bytes(self._installed_plist(binary))
            deployed_sha = root / "deployed.sha"
            deployed_sha.write_text("a" * 40 + "\n")
            identity = self.dev.RuntimeIdentity("2.0.0", "a" * 40)
            commands = []

            def run(command, **_kwargs):
                commands.append([str(part) for part in command])
                if command[:2] == ["launchctl", "print"]:
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        (
                            f"path = {plist}\n"
                            "state = running\n"
                            f"program = {binary}\n"
                            "pid = 100\n"
                            "service name = 9555\n"
                            "properties = keepalive | runatload\n"
                        ),
                        "",
                    )
                if "--protocol-version" in command:
                    binary.write_bytes(b"replaced after validation")
                    return subprocess.CompletedProcess(command, 0, "2\n", "")
                return subprocess.CompletedProcess(command, 0, "", "")

            with self._isolated_operation_paths(root), \
                 mock.patch.object(
                     self.dev,
                     "LAUNCHD_RESTART_HELPER_SOURCE",
                     ROOT / "scripts" / "launchd_restart_helper.py",
                 ), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=identity), \
                 mock.patch.object(self.dev, "_current_prod_identity", return_value=identity), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                with self.assertRaisesRegex(
                    SystemExit,
                    "binary changed during restart preparation",
                ):
                    self.dev.launchd_prod_restart()

            self.assertFalse(
                any(command[:2] == ["launchctl", "bootstrap"] for command in commands)
            )
            status_path = next(
                (root / "restart" / "transactions").glob("*/status.json")
            )
            self.assertEqual(
                "precondition_failed",
                json.loads(status_path.read_text())["state"],
            )
            self.assertFalse((root / "restart" / "active").exists())

    def test_interrupted_bootstrap_does_not_overwrite_helper_terminal_status(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            install = root / "install"
            install.mkdir()
            binary = install / "phoenix-ide"
            binary.write_bytes(b"installed")
            plist = root / "service.plist"
            plist.write_bytes(self._installed_plist(binary))
            deployed_sha = root / "deployed.sha"
            deployed_sha.write_text("a" * 40 + "\n")
            identity = self.dev.RuntimeIdentity("2.0.0", "a" * 40)
            installed = self.dev.InstalledLaunchdRuntime(
                binary=self.dev.ValidatedLaunchdArtifact(
                    binary,
                    self.dev._file_sha256(binary),
                ),
                plist=self.dev.ValidatedLaunchdArtifact(
                    plist,
                    self.dev._file_sha256(plist),
                ),
                deployed_sha=self.dev.ValidatedLaunchdArtifact(
                    deployed_sha,
                    self.dev._file_sha256(deployed_sha),
                ),
                identity=identity,
                pid=100,
                health_url="http://localhost:9555/api/version",
                health_insecure_tls=False,
            )
            real_write = self.dev._write_json_atomic

            def run(command, **_kwargs):
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, "2\n", "")
                if command[:2] == ["launchctl", "bootstrap"]:
                    status_path = next(
                        (root / "restart" / "transactions").glob("*/status.json")
                    )
                    status = json.loads(status_path.read_text())
                    status.update({
                        "state": "committed",
                        "running_pid": 101,
                        "updated_at": "2026-01-01T00:00:01+00:00",
                    })
                    real_write(status_path, status)
                    (root / "restart" / "active").unlink()
                    raise KeyboardInterrupt
                return subprocess.CompletedProcess(command, 0, "", "")

            with self._isolated_operation_paths(root), \
                 mock.patch.object(
                     self.dev,
                     "LAUNCHD_RESTART_HELPER_SOURCE",
                     ROOT / "scripts" / "launchd_restart_helper.py",
                 ), \
                 mock.patch.object(
                     self.dev,
                     "_installed_launchd_runtime_for_restart",
                     return_value=installed,
                 ), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                with self.assertRaises(KeyboardInterrupt):
                    self.dev.launchd_prod_restart()

            status_path = next(
                (root / "restart" / "transactions").glob("*/status.json")
            )
            self.assertEqual("committed", json.loads(status_path.read_text())["state"])
            self.assertFalse((root / "restart" / "active").exists())

    def test_restart_rejects_non_socket_activated_installation(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary = root / "phoenix-ide"
            binary.write_bytes(b"installed")
            plist = root / "service.plist"
            value = plistlib.loads(self._installed_plist(binary))
            value.pop("Sockets")
            plist.write_bytes(plistlib.dumps(value))

            with self._isolated_operation_paths(root), \
                 mock.patch.object(self.dev, "LAUNCHD_INSTALL_DIR", root):
                with self.assertRaisesRegex(SystemExit, "socket-activated"):
                    self.dev.launchd_prod_restart()

    def test_restart_rejects_loaded_job_without_keepalive(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary = root / "install" / "phoenix-ide"
            binary.parent.mkdir()
            binary.write_bytes(b"installed")
            plist = root / "service.plist"
            plist.write_bytes(self._installed_plist(binary))
            inspection = self.dev.LoadedLaunchdJob(
                state="running",
                pid=100,
                keep_alive=False,
                plist_path=str(plist),
                program_path=str(binary),
                socket_services=("9555",),
            )

            with self._isolated_operation_paths(root), \
                 mock.patch.object(
                     self.dev,
                     "_inspect_launchd_job",
                     return_value=inspection,
                 ):
                with self.assertRaisesRegex(SystemExit, "does not report KeepAlive"):
                    self.dev.launchd_prod_restart()

    def test_restart_rejects_loaded_listener_different_from_installed_plist(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            binary = root / "install" / "phoenix-ide"
            binary.parent.mkdir()
            binary.write_bytes(b"installed")
            plist = root / "service.plist"
            plist.write_bytes(self._installed_plist(binary))
            inspection = self.dev.LoadedLaunchdJob(
                state="running",
                pid=100,
                keep_alive=True,
                plist_path=str(plist),
                program_path=str(binary),
                socket_services=("9444",),
            )

            with self._isolated_operation_paths(root), \
                 mock.patch.object(
                     self.dev,
                     "_inspect_launchd_job",
                     return_value=inspection,
                 ):
                with self.assertRaisesRegex(SystemExit, "listener does not match"):
                    self.dev.launchd_prod_restart()

    def test_deploy_and_restart_claims_are_mutually_exclusive(self):
        with tempfile.TemporaryDirectory() as td, \
             self._isolated_operation_paths(Path(td)):
            self.dev._claim_launchd_restart("restart-one")
            with self.assertRaisesRegex(SystemExit, "restart-one"):
                self.dev._claim_launchd_deploy("deploy-two")
            self.dev._release_launchd_restart_claim("restart-one")
            self.dev._claim_launchd_deploy("deploy-two")
            with self.assertRaisesRegex(SystemExit, "deploy-two"):
                self.dev._claim_launchd_restart("restart-three")

    def test_deploy_rejected_by_active_restart_is_durably_recorded(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)

            with self._isolated_operation_paths(root):
                self.dev._claim_launchd_restart("restart-owner")
                with mock.patch.object(
                    self.dev,
                    "_launchd_candidate_env",
                    return_value=({}, None),
                ), mock.patch.object(
                    self.dev,
                    "_preflight_prod_bind_auth",
                ), mock.patch.object(self.dev, "prod_build") as build:
                    with self.assertRaisesRegex(
                        self.dev.ActiveLaunchdRestart,
                        "restart-owner",
                    ):
                        self.dev.launchd_prod_deploy()

                rejection = json.loads(
                    self.dev.LAUNCHD_DEPLOY_STATUS_PATH.read_text()
                )
                self.assertEqual("rejected_concurrent", rejection["state"])
                self.assertEqual("local_head", rejection["source_kind"])
                self.assertIn("restart-owner", rejection["failure"])
                self.assertEqual(
                    "restart-owner\n",
                    self.dev.LAUNCHD_RESTART_ACTIVE_PATH.read_text(),
                )
                self.assertFalse(self.dev.LAUNCHD_DEPLOY_ACTIVE_PATH.exists())
                build.assert_not_called()

    def test_concurrent_restart_rejection_is_durable_without_overwriting_owner(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            deploy = root / "deploy"
            deploy.mkdir()
            (deploy / "active").write_text("deploy-owner\n")

            with self._isolated_operation_paths(root):
                with self.assertRaisesRegex(SystemExit, "deploy-owner"):
                    self.dev.launchd_prod_restart()

            statuses = list((root / "restart" / "transactions").glob("*/status.json"))
            self.assertEqual(1, len(statuses))
            rejection = json.loads(statuses[0].read_text())
            self.assertEqual("rejected_concurrent", rejection["state"])
            self.assertIn("deploy-owner", rejection["failure"])
            self.assertEqual("deploy-owner\n", (deploy / "active").read_text())
            self.assertFalse((root / "restart" / "active").exists())

    def test_definitive_claim_lock_failure_is_terminalized(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            claim_lock = root / "deploy" / "claim.lock"
            claim_lock.mkdir(parents=True)

            with self._isolated_operation_paths(root):
                with self.assertRaisesRegex(SystemExit, "could not acquire"):
                    self.dev.launchd_prod_restart()

            statuses = list((root / "restart" / "transactions").glob("*/status.json"))
            self.assertEqual(1, len(statuses))
            failure = json.loads(statuses[0].read_text())
            self.assertEqual("precondition_failed", failure["state"])
            self.assertIn("claim", failure["failure"])
            self.assertFalse((root / "restart" / "active").exists())

    def test_ambiguous_claim_persistence_failure_retains_unresolved_fence(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            real_fsync_directory = self.dev._fsync_directory

            def fsync_directory(path):
                active = root / "restart" / "active"
                if path == active.parent and active.exists():
                    raise OSError("directory sync failed")
                return real_fsync_directory(path)

            with self._isolated_operation_paths(root), \
                 mock.patch.object(
                     self.dev,
                     "_fsync_directory",
                     side_effect=fsync_directory,
                 ):
                with self.assertRaisesRegex(OSError, "directory sync failed"):
                    self.dev.launchd_prod_restart()

            active = root / "restart" / "active"
            self.assertTrue(active.is_file())
            transaction_id = active.read_text().strip()
            status = json.loads(
                (root / "restart" / "transactions" / transaction_id / "status.json").read_text()
            )
            self.assertEqual("preparing", status["state"])

    def test_precondition_status_write_failure_retains_restart_claim(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            real_write = self.dev._write_json_atomic

            def write_status(path, value, mode=0o600):
                if value.get("state") == "precondition_failed":
                    raise OSError("disk full")
                return real_write(path, value, mode)

            with self._isolated_operation_paths(root), \
                 mock.patch.object(
                     self.dev,
                     "_installed_launchd_runtime_for_restart",
                     side_effect=SystemExit("invalid install"),
                 ), \
                 mock.patch.object(
                     self.dev,
                     "_write_json_atomic",
                     side_effect=write_status,
                 ):
                with self.assertRaisesRegex(OSError, "disk full"):
                    self.dev.launchd_prod_restart()

            active = root / "restart" / "active"
            self.assertTrue(active.is_file())
            transaction_id = active.read_text().strip()
            status = json.loads(
                (root / "restart" / "transactions" / transaction_id / "status.json").read_text()
            )
            self.assertEqual("preparing", status["state"])

    def test_restart_retention_preserves_current_and_active_transactions(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transactions = root / "restart" / "transactions"
            transactions.mkdir(parents=True)
            active = transactions / "active-owner"
            active.mkdir()
            current = transactions / "current-request"
            current.mkdir()
            for index in range(7):
                candidate = transactions / f"rejected-{index}"
                candidate.mkdir()
                os.utime(candidate, (index + 1, index + 1))
            (root / "restart" / "active").write_text("active-owner\n")

            with self._isolated_operation_paths(root):
                self.dev._prune_launchd_restart_transactions("current-request")

            self.assertTrue(active.is_dir())
            self.assertTrue(current.is_dir())
            retained_rejections = list(transactions.glob("rejected-*"))
            self.assertEqual(5, len(retained_rejections))

    def test_restart_status_uses_completion_chronology(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transactions = root / "restart" / "transactions"
            owner = transactions / "owner"
            rejected = transactions / "rejected"
            owner.mkdir(parents=True)
            rejected.mkdir()
            (owner / "status.json").write_text(json.dumps({
                "transaction_id": "owner",
                "state": "committed",
                "created_at": "2026-01-01T00:00:00+00:00",
                "updated_at": "2026-01-01T00:00:03+00:00",
            }))
            (rejected / "status.json").write_text(json.dumps({
                "transaction_id": "rejected",
                "state": "rejected_concurrent",
                "created_at": "2026-01-01T00:00:01+00:00",
                "updated_at": "2026-01-01T00:00:02+00:00",
            }))

            with self._isolated_operation_paths(root), \
                 mock.patch("builtins.print") as output:
                self.dev._print_launchd_restart_status()

            rendered = " ".join(str(call) for call in output.call_args_list)
            self.assertIn("Last restart: committed (owner)", rendered)

    def test_restart_status_surfaces_active_claim_with_unreadable_status(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            transaction = root / "restart" / "transactions" / "owner"
            transaction.mkdir(parents=True)
            (transaction / "status.json").write_text("not-json\n")
            (root / "restart" / "active").write_text("owner\n")

            with self._isolated_operation_paths(root), \
                 mock.patch("builtins.print") as output:
                self.dev._print_launchd_restart_status()

            rendered = " ".join(str(call) for call in output.call_args_list)
            self.assertIn("Active restart: status unavailable (owner)", rendered)
            self.assertIn("UNRESOLVED", rendered)
            self.assertIn("confirm no helper is running", rendered)

    def test_launchctl_failure_is_not_reported_as_not_loaded(self):
        failure = subprocess.CompletedProcess(
            [],
            64,
            "",
            "launchctl domain temporarily unavailable",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=failure):
            inspection = self.dev._inspect_launchd_job()

        self.assertIsInstance(inspection, self.dev.LaunchdJobInspectionFailed)
        self.assertEqual(64, inspection.exit_code)
        self.assertIn("temporarily unavailable", inspection.detail)

    def test_restart_identity_probe_rejects_non_socket_runtime(self):
        response = io.BytesIO(json.dumps({
            "version": "2.0.0",
            "git_sha": "a" * 40,
            "socket_activated": False,
        }).encode())
        with mock.patch.object(
            self.dev,
            "_launchd_health_probe",
            return_value=("http://127.0.0.1/version", False),
        ), mock.patch("urllib.request.urlopen", return_value=response):
            identity = self.dev._current_prod_identity(
                {},
                require_socket_activated=True,
            )

        self.assertIsNone(identity)

    def test_prod_status_surfaces_launchctl_failure_without_deploy_guidance(self):
        inspection = self.dev.LaunchdJobInspectionFailed(64, "permission denied")
        with mock.patch.object(self.dev, "_inspect_launchd_job", return_value=inspection), \
             mock.patch.object(self.dev, "_print_launchd_deploy_status"), \
             mock.patch.object(self.dev, "_print_launchd_restart_status"), \
             mock.patch("builtins.print") as output:
            self.dev.launchd_prod_status()

        rendered = " ".join(str(call) for call in output.call_args_list)
        self.assertIn("status unavailable", rendered)
        self.assertIn("permission denied", rendered)
        self.assertNotIn("prod deploy", rendered)

    def test_command_routes_launchd_without_touching_systemd_or_bare_linux(self):
        with mock.patch.object(self.dev, "detect_prod_env", return_value="launchd"), \
             mock.patch.object(self.dev, "launchd_prod_restart") as launchd, \
             mock.patch.object(self.dev, "prod_daemon_restart") as bare:
            self.dev.cmd_prod_restart()
        launchd.assert_called_once_with()
        bare.assert_not_called()


if __name__ == "__main__":
    unittest.main()
