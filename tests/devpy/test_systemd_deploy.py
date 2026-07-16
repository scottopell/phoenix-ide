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
                destination.write_text("helper")
                destination.chmod(0o700)

            with mock.patch.object(self.dev, "check_systemd_available", return_value=True), \
                 mock.patch.object(self.dev, "_load_env_file", side_effect=lambda env: env.update({"SECRET": "value"}) or ".phoenix-ide.env"), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
                 mock.patch("uuid.uuid4", return_value=mock.Mock(hex=transaction_id)), \
                 mock.patch.object(self.dev, "_linux_musl_target", return_value="x86_64-unknown-linux-musl"), \
                 mock.patch.object(self.dev, "_prepare_local_candidate", return_value=candidate) as prepare, \
                 mock.patch.object(self.dev, "_systemd_current_identity", return_value=None), \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize) as source_file, \
                 mock.patch.object(
                     self.dev,
                     "_stage_systemd_root_handoff",
                     side_effect=lambda _staging, _tx, _helper, files: captured.update(
                         manifest=json.loads(dict(files)["manifest.json"].read_text()),
                         manifest_text=dict(files)["manifest.json"].read_text(),
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
            files = captured["files"]
            manifest = captured["manifest"]
            self.assertNotIn("value", captured["manifest_text"])
            self.assertEqual(identity.as_dict(), manifest["expected"])
            activation = commands[-1]
            self.assertIn("systemd-run", activation)
            self.assertIn(str(root_manifest), activation)
            self.assertIn(str(root_manifest.parent / "helper.py"), activation)

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


if __name__ == "__main__":
    unittest.main()
