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
    spec = importlib.util.spec_from_file_location("devpy_bare_deploy_test", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class BareDeployCommandTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dev = load_dev()

    def test_release_dispatch_reaches_bare_supervisor_without_checks(self):
        with mock.patch.object(self.dev, "detect_prod_env", return_value="daemon"), \
             mock.patch.object(self.dev, "cmd_check") as check, \
             mock.patch.object(self.dev, "prod_daemon_deploy") as deploy:
            self.dev.cmd_prod_deploy("v2.0.0")
        check.assert_not_called()
        deploy.assert_called_once_with("v2.0.0")

    def test_stop_routes_through_supervisor_socket_not_pid_file(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            socket = root / "run/supervisor.sock"
            socket.parent.mkdir()
            socket.touch()
            layout = {
                "root": root,
                "supervisor": root / "bin/supervisor.py",
                "socket": socket,
            }
            with mock.patch.object(self.dev, "_bare_layout", return_value=layout), \
                 mock.patch.object(
                     self.dev.subprocess,
                     "run",
                     return_value=subprocess.CompletedProcess([], 0, json.dumps({"ok": True, "child": None}), ""),
                 ) as run:
                self.dev.prod_daemon_stop()
            command = run.call_args.args[0]
            self.assertIn("stop", command)
            self.assertNotIn("prod.pid", " ".join(command))

    def test_reboot_persistence_installs_idempotent_owner_crontab_entry(self):
        root = Path("/tmp/phoenix owner")
        layout = {"root": root, "supervisor": root / "bin/phoenix-supervisor.py"}
        existing = "MAILTO=user@example.test\n@reboot old-command # phoenix-ide persistent supervisor\n"
        calls = [
            subprocess.CompletedProcess([], 0, existing, ""),
            subprocess.CompletedProcess([], 0, "", ""),
        ]
        with mock.patch.object(self.dev.shutil, "which", return_value="/usr/bin/crontab"), \
             mock.patch.object(self.dev.subprocess, "run", side_effect=calls) as run:
            configured = self.dev._configure_bare_reboot_persistence(layout)

        self.assertTrue(configured)
        installed = run.call_args_list[1].kwargs["input"]
        self.assertEqual(1, installed.count("# phoenix-ide persistent supervisor"))
        self.assertIn("MAILTO=user@example.test", installed)
        self.assertIn("@reboot", installed)
        self.assertIn("phoenix-supervisor.py", installed)

    def test_reboot_persistence_prints_exact_rc_guidance_without_crontab(self):
        root = Path("/tmp/phoenix owner")
        layout = {"root": root, "supervisor": root / "bin/phoenix-supervisor.py"}
        with mock.patch.object(self.dev.shutil, "which", return_value=None), \
             mock.patch("builtins.print") as output:
            configured = self.dev._configure_bare_reboot_persistence(layout)

        self.assertFalse(configured)
        text = "\n".join(" ".join(str(value) for value in call.args) for call in output.call_args_list)
        self.assertIn("Reboot persistence: not configured", text)
        self.assertIn("same-user boot/rc mechanism", text)
        self.assertIn("phoenix-supervisor.py", text)
        self.assertIn("--root", text)
        self.assertIn("run", text)

    def test_deploy_stages_only_transaction_reference_to_supervisor(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "phoenix"
            layout = {
                "root": root,
                "supervisor": root / "bin/phoenix-supervisor.py",
                "binary": root / "bin/phoenix-ide",
                "environment": root / "config/phoenix.env",
                "transactions": root / "deploy/transactions",
                "status": root / "deploy/status.json",
                "socket": root / "run/supervisor.sock",
                "deployed_sha": root / "deployed.sha",
            }
            candidate_binary = Path(td) / "candidate"
            candidate_binary.write_text("candidate")
            candidate = self.dev.PreparedCandidate(
                binary=candidate_binary,
                source_kind=self.dev.ProdSourceKind.PUBLISHED_RELEASE,
                source_commit="b" * 40,
                identity=self.dev.RuntimeIdentity("2.0.0", "b" * 12),
                release_tag="v2.0.0",
                release_commit="b" * 40,
            )
            commands = []

            def run(command, *args, **kwargs):
                commands.append(command)
                if "--protocol-version" in command:
                    return subprocess.CompletedProcess(command, 0, "1\n", "")
                if "activate" in command:
                    return subprocess.CompletedProcess(command, 0, json.dumps({"ok": True, "state": "committed"}), "")
                return subprocess.CompletedProcess(command, 0, "", "")

            def materialize(_commit, _source, destination, _kind):
                destination.write_text("supervisor")
                destination.chmod(0o700)

            with mock.patch.object(self.dev, "_bare_layout", return_value=layout), \
                 mock.patch.object(self.dev, "_load_env_file", return_value=None), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_prepare_release_candidate", return_value=candidate), \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize), \
                 mock.patch.object(self.dev, "_start_bare_supervisor"), \
                 mock.patch.object(self.dev, "_configure_bare_reboot_persistence"), \
                 mock.patch.object(self.dev, "_current_prod_identity", return_value=None), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run), \
                 mock.patch("uuid.uuid4", return_value=mock.Mock(hex="d" * 32)):
                self.dev.prod_daemon_deploy("v2.0.0")

            activation = next(command for command in commands if "activate" in command)
            self.assertIn("--transaction-id", activation)
            self.assertIn("--manifest-sha256", activation)
            self.assertNotIn("candidate", " ".join(activation))
            manifest = json.loads((layout["transactions"] / ("d" * 32) / "manifest.json").read_text())
            self.assertNotIn("SECRET", json.dumps(manifest))
            self.assertEqual(candidate.identity.as_dict(), manifest["expected"])


if __name__ == "__main__":
    unittest.main()
