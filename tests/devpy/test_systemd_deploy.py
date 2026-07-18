import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_dev():
    spec = importlib.util.spec_from_file_location("devpy_systemd_deploy_test", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class SystemdDeployCommandTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load_dev()

    def test_env_snapshot_round_trips_newlines_without_exposing_values_in_manifest(self):
        serialized = self.dev._serialize_env_snapshot({"TOKEN": "secret", "HEADERS": "a\nb"})
        self.assertEqual("TOKEN=secret\nHEADERS=a\\nb\n", serialized)

    def test_root_staging_failure_prevents_transient_activation(self):
        with tempfile.TemporaryDirectory() as td:
            staging = Path(td)
            artifact = staging / "candidate"
            artifact.write_text("candidate")
            with mock.patch.object(
                self.dev.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 1, "", "denied"),
            ) as run:
                with self.assertRaisesRegex(SystemExit, "handoff staging failed"):
                    self.dev._stage_systemd_root_handoff(
                        staging,
                        "a" * 32,
                        artifact,
                        [("candidate-binary", artifact)],
                    )
            self.assertTrue(all("systemd-run" not in call.args[0] for call in run.call_args_list))

    def test_transient_launch_failure_abandons_staged_claim(self):
        manifest = Path("/root/transactions/tx/manifest.json")
        calls = [
            subprocess.CalledProcessError(1, ["systemd-run"]),
            subprocess.CompletedProcess([], 0, "", ""),
        ]
        with mock.patch.object(self.dev.subprocess, "run", side_effect=calls) as run:
            with self.assertRaisesRegex(SystemExit, "staged claim released"):
                self.dev._launch_systemd_activation("tx", manifest)
        abandon = run.call_args_list[1].args[0]
        self.assertIn("abandon", abandon)
        self.assertIn(str(manifest), abandon)

    def test_native_deploy_hands_root_manifest_to_transient_unit(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            binary = base / "binary"
            binary.write_text("binary")
            helper_source = base / "helper.py"
            helper_source.write_text("print(1)\n")
            identity = self.dev.RuntimeIdentity("2.0.0", "b" * 12)
            candidate = self.dev.PreparedCandidate(
                binary=binary,
                source_kind=self.dev.ProdSourceKind.LOCAL_HEAD,
                source_commit="b" * 40,
                identity=identity,
            )
            transaction_id = "d" * 32
            root_manifest = self.dev.SYSTEMD_TRANSACTION_ROOT / transaction_id / "manifest.json"
            commands = []
            captured = {}

            def run(command, *args, **kwargs):
                commands.append(command)
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, "1\n", "")
                return subprocess.CompletedProcess(command, 0, "", "")

            def materialize(_commit, _source, destination, _kind):
                destination.write_text((ROOT / "scripts/systemd_deploy_helper.py").read_text())
                destination.chmod(0o700)

            with mock.patch.object(self.dev, "check_systemd_available", return_value=True), \
                 mock.patch.object(
                     self.dev,
                     "_load_env_file",
                     side_effect=lambda env: env.update({"SECRET": "value", "PHOENIX_PORT": "9443", "PHOENIX_TLS": "auto"}) or ".phoenix-ide.env",
                 ), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
                 mock.patch("uuid.uuid4", return_value=mock.Mock(hex=transaction_id)), \
                 mock.patch.object(self.dev, "_linux_musl_target", return_value="x86_64-unknown-linux-musl"), \
                 mock.patch.object(self.dev, "_prepare_local_candidate", return_value=candidate) as prepare, \
                 mock.patch.object(self.dev, "_systemd_installed_runtime", return_value=(None, None)), \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize) as source_file, \
                 mock.patch.object(
                     self.dev,
                     "_stage_systemd_root_handoff",
                     side_effect=lambda _staging, _tx, helper, files, *, noninteractive=False: captured.update(
                         helper_text=helper.read_text(),
                         noninteractive=noninteractive,
                         manifest=json.loads(dict(files)["manifest.json"].read_text()),
                         manifest_text=dict(files)["manifest.json"].read_text(),
                         socket_text=dict(files)["candidate.socket"].read_text(),
                         files=files,
                     ) or root_manifest,
                 ) as stage, \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                self.dev.native_prod_deploy()

            prepare.assert_called_once_with(target="x86_64-unknown-linux-musl")
            source_file.assert_called_once_with(
                "b" * 40,
                "scripts/systemd_deploy_helper.py",
                mock.ANY,
                "local_head",
            )
            self.assertEqual((ROOT / "scripts/systemd_deploy_helper.py").read_text(), captured["helper_text"])
            files = captured["files"]
            manifest = captured["manifest"]
            self.assertFalse(captured["noninteractive"])
            self.assertNotIn("value", captured["manifest_text"])
            self.assertEqual(identity.as_dict(), manifest["expected"])
            self.assertEqual("https://localhost:9443/api/version", manifest["expected_health_url"])
            self.assertIn("ListenStream=9443", captured["socket_text"])
            activation = commands[-1]
            self.assertIn("systemd-run", activation)
            self.assertIn(str(root_manifest), activation)
            self.assertIn(str(root_manifest.parent / "helper.py"), activation)

    def test_candidate_and_previous_health_urls_use_their_own_snapshots(self):
        previous = {"PHOENIX_PORT": "8031"}
        candidate = {"PHOENIX_PORT": "9443", "PHOENIX_TLS": "auto"}
        self.assertEqual("https://localhost:9443/api/version", self.dev._prod_api_health_url(candidate))
        with tempfile.TemporaryDirectory() as td:
            env = Path(td) / "prod.env"
            binary = Path(td) / "binary"
            env.write_text("PHOENIX_PORT=8031\n")
            binary.write_text("runtime")
            with mock.patch.object(
                self.dev, "_binary_identity", return_value=self.dev.RuntimeIdentity("1.0.0", "a" * 12)
            ):
                identity, url = self.dev._installed_runtime(binary, env)
            self.assertEqual(self.dev.RuntimeIdentity("1.0.0", "a" * 12), identity)
            self.assertEqual("http://localhost:8031/api/version", url)
        self.assertEqual("http://localhost:8031/api/version", self.dev._prod_api_health_url(previous))

    def test_systemd_status_reads_root_owned_deployed_sha(self):
        with mock.patch.object(
            self.dev.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 0, "a" * 40 + "\n", ""),
        ) as run:
            self.assertEqual("a" * 40, self.dev._read_systemd_deployed_sha())
        self.assertEqual(["sudo", "cat", str(self.dev.SYSTEMD_DEPLOYED_SHA_PATH)], run.call_args.args[0])

    def test_runtime_identity_refuses_incomplete_status_values(self):
        identity = self.dev.RuntimeIdentity(None, "a" * 12)
        with self.assertRaisesRegex(ValueError, "exact runtime identity"):
            identity.as_dict()

    def test_status_uses_installed_systemd_environment(self):
        commands = [
            subprocess.CompletedProcess([], 0, "active\n", ""),
            subprocess.CompletedProcess([], 1, "", "missing"),
        ]
        with mock.patch.object(self.dev.subprocess, "run", side_effect=commands), \
             mock.patch.object(
                 self.dev,
                 "_read_systemd_installed_env",
                 return_value={"PHOENIX_PORT": "9443", "PHOENIX_DB_PATH": "/srv/phoenix.db"},
             ), \
             mock.patch.object(self.dev, "_open_prod_health", side_effect=OSError), \
             mock.patch.object(self.dev, "_read_systemd_deployed_sha", return_value=None), \
             mock.patch.object(self.dev, "_read_systemd_deploy_status", return_value=None), \
             mock.patch("builtins.print") as output:
            self.dev.native_prod_status()
        rendered = "\n".join(" ".join(str(value) for value in call.args) for call in output.call_args_list)
        self.assertIn("Port: 9443", rendered)
        self.assertIn("Database: /srv/phoenix.db", rendered)
        self.assertIn("http://localhost:9443", rendered)

    def test_linux_musl_target_tracks_host_architecture(self):
        import platform

        with mock.patch.object(platform, "machine", return_value="aarch64"):
            self.assertEqual("aarch64-unknown-linux-musl", self.dev._linux_musl_target())
        with mock.patch.object(platform, "machine", return_value="amd64"):
            self.assertEqual("x86_64-unknown-linux-musl", self.dev._linux_musl_target())

    def test_status_reader_returns_only_durable_status_document(self):
        value = {"transaction_id": "tx", "state": "committed"}
        with mock.patch.object(
            self.dev.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 0, json.dumps(value), ""),
        ) as run:
            self.assertEqual(value, self.dev._read_systemd_deploy_status())
        self.assertEqual(["sudo", "cat", str(self.dev.SYSTEMD_STATUS_PATH)], run.call_args.args[0])

    def test_release_dispatch_reaches_systemd_without_running_checks(self):
        with mock.patch.object(self.dev, "detect_prod_env", return_value="native"), \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev, "native_prod_deploy") as deploy:
            self.dev.cmd_prod_deploy("v2.0.0")
        check.assert_not_called()
        deploy.assert_called_once_with("v2.0.0")

    def test_controller_requires_noninteractive_sudo_before_disruption(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v2.0.0",
            expected_full_commit="a" * 40,
            transaction_id="tx-123",
        )
        with mock.patch.object(
            self.dev.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 1, "", "password required"),
        ) as run, mock.patch.object(self.dev, "check_systemd_available") as available:
            with self.assertRaisesRegex(SystemExit, "non-interactive sudo is required before disruption"):
                self.dev.native_prod_deploy("v2.0.0", controller=controller)
        self.assertEqual(["sudo", "-n", "true"], run.call_args.args[0])
        available.assert_not_called()

    def test_controller_uses_installed_systemd_env_and_transaction_id(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v2.0.0",
            expected_full_commit="a" * 40,
            transaction_id="tx-123",
        )
        with mock.patch.object(self.dev, "check_systemd_available", return_value=True), \
             mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
             mock.patch.object(self.dev, "_read_systemd_installed_env", return_value={"PHOENIX_PASSWORD": "installed", "PHOENIX_PORT": "9443"}), \
             mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
             mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
             mock.patch.object(self.dev, "_linux_musl_target", return_value="x86_64-unknown-linux-musl"), \
             mock.patch.object(self.dev, "_prepare_release_candidate", side_effect=SystemExit("stop here")) as prepare:
            with self.assertRaisesRegex(SystemExit, "stop here"):
                self.dev.native_prod_deploy("v2.0.0", controller=controller)
        prepare.assert_called_once_with(
                "v2.0.0",
                mock.ANY,
                expected_full_commit="a" * 40,
                expected_asset_name=None,
                expected_asset_sha256=None,
            )


if __name__ == "__main__":
    unittest.main()
