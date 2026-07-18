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

    def test_controller_uses_installed_bare_env_and_transaction_id(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v2.0.0",
            expected_full_commit="a" * 40,
            transaction_id="tx-123",
        )
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "phoenix"
            environment = root / "config/phoenix.env"
            environment.parent.mkdir(parents=True)
            environment.write_text("PHOENIX_PASSWORD=installed\nPHOENIX_PORT=9443\n")
            layout = {
                "root": root,
                "supervisor": root / "bin/phoenix-supervisor.py",
                "binary": root / "bin/phoenix-ide",
                "environment": environment,
                "transactions": root / "deploy/transactions",
                "status": root / "deploy/status.json",
                "socket": root / "run/supervisor.sock",
                "deployed_sha": root / "deployed.sha",
            }
            with mock.patch.object(self.dev, "_bare_layout", return_value=layout), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_prepare_release_candidate", side_effect=SystemExit("stop here")) as prepare:
                with self.assertRaisesRegex(SystemExit, "stop here"):
                    self.dev.prod_daemon_deploy("v2.0.0", controller=controller)
        prepare.assert_called_once_with("v2.0.0", mock.ANY, expected_full_commit="a" * 40)

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

    def test_bare_health_url_tracks_specific_bind_and_maps_wildcards_to_loopback(self):
        self.assertEqual(
            "http://192.0.2.10:9443/api/version",
            self.dev._bare_api_health_url({"PHOENIX_BIND_ADDR": "192.0.2.10", "PHOENIX_PORT": "9443"}),
        )
        self.assertEqual(
            "http://127.0.0.1:8031/api/version",
            self.dev._bare_api_health_url({"PHOENIX_BIND_ADDR": "0.0.0.0"}),
        )
        self.assertEqual(
            "https://[2001:db8::10]:8031/api/version",
            self.dev._bare_api_health_url({"PHOENIX_BIND_ADDR": "2001:db8::10", "PHOENIX_TLS": "auto"}),
        )

    def test_changed_same_protocol_supervisor_refuses_without_stopping_production(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            socket = root / "run/supervisor.sock"
            socket.parent.mkdir(parents=True)
            socket.touch()
            installed = root / "bin/phoenix-supervisor.py"
            installed.parent.mkdir()
            installed.write_text("old supervisor")
            selected = root / "selected.py"
            selected.write_text("new supervisor")
            layout = {"root": root, "socket": socket, "supervisor": installed}

            with mock.patch.object(
                self.dev.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 0, json.dumps({"protocol_version": 1}), ""),
            ) as run, mock.patch.object(self.dev.subprocess, "Popen") as started:
                with self.assertRaisesRegex(SystemExit, "production was left running"):
                    self.dev._start_bare_supervisor(layout, "1", selected)

            self.assertEqual("old supervisor", installed.read_text())
            self.assertEqual(1, run.call_count)
            started.assert_not_called()
            self.assertTrue(socket.exists())

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

    def test_status_shows_durable_state_without_supervisor(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            status = root / "deploy/status.json"
            status.parent.mkdir()
            status.write_text(json.dumps({"state": "committed", "transaction_id": "tx"}))
            deployed_sha = root / "deployed.sha"
            deployed_sha.write_text("a" * 40 + "\n")
            layout = {
                "socket": root / "run/supervisor.sock",
                "status": status,
                "deployed_sha": deployed_sha,
            }
            with mock.patch.object(self.dev, "_bare_layout", return_value=layout), \
                 mock.patch("builtins.print") as output:
                self.dev.prod_daemon_status()
        rendered = "\n".join(" ".join(str(value) for value in call.args) for call in output.call_args_list)
        self.assertIn("Supervisor not running", rendered)
        self.assertIn("Deployment: committed (tx)", rendered)
        self.assertIn("a" * 40, rendered)

    def test_unclaimed_frozen_transaction_is_discarded_after_launch_failure(self):
        with tempfile.TemporaryDirectory() as td:
            transaction = Path(td) / "tx"
            transaction.mkdir(mode=0o700)
            artifact = transaction / "manifest.json"
            artifact.write_text("{}")
            artifact.chmod(0o400)
            transaction.chmod(0o500)
            self.dev._discard_unclaimed_bare_transaction(transaction)
            self.assertFalse(transaction.exists())

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
                if "status" in command:
                    return subprocess.CompletedProcess(command, 0, json.dumps({"protocol_version": 1, "child": None}), "")
                return subprocess.CompletedProcess(command, 0, "", "")

            def materialize(_commit, _source, destination, _kind):
                destination.write_text((ROOT / "scripts/bare_supervisor.py").read_text())
                destination.chmod(0o700)

            staged_supervisors = []

            def start(_layout, _protocol, selected_source):
                staged_supervisors.append(selected_source.read_text())

            with mock.patch.object(self.dev, "_bare_layout", return_value=layout), \
                 mock.patch.object(self.dev, "_load_env_file", return_value=None), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "_prepare_release_candidate", return_value=candidate), \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize) as source_file, \
                 mock.patch.object(self.dev, "_start_bare_supervisor", side_effect=start), \
                 mock.patch.object(self.dev, "_configure_bare_reboot_persistence"), \
                 mock.patch.object(self.dev, "_installed_bare_runtime", return_value=(None, None)), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run), \
                 mock.patch("uuid.uuid4", return_value=mock.Mock(hex="d" * 32)):
                self.dev.prod_daemon_deploy("v2.0.0")

            source_file.assert_called_once_with(
                "b" * 40,
                "scripts/bare_supervisor.py",
                mock.ANY,
                "published_release",
            )
            self.assertEqual([(ROOT / "scripts/bare_supervisor.py").read_text()], staged_supervisors)
            activation = next(command for command in commands if "activate" in command)
            self.assertIn("--transaction-id", activation)
            self.assertIn("--manifest-sha256", activation)
            self.assertNotIn("candidate", " ".join(activation))
            manifest = json.loads((layout["transactions"] / ("d" * 32) / "manifest.json").read_text())
            self.assertNotIn("SECRET", json.dumps(manifest))
            self.assertEqual(candidate.identity.as_dict(), manifest["expected"])
            installed_env = (layout["transactions"] / ("d" * 32) / "candidate.env").read_text()
            self.assertIn(f"HOME={Path.home()}", installed_env)
            self.assertIn(f"PATH={self.dev.os.environ.get('PATH', self.dev.os.defpath)}", installed_env)
            self.assertFalse(manifest["previous_running"])


if __name__ == "__main__":
    unittest.main()
