import importlib.util
import json
import os
import re
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

    def setUp(self):
        identity = mock.patch.object(self.dev, "_systemd_current_identity", return_value=None)
        identity.start()
        self.addCleanup(identity.stop)

    def test_env_snapshot_round_trips_newlines_without_exposing_values_in_manifest(self):
        serialized = self.dev._serialize_env_snapshot({"TOKEN": "secret", "HEADERS": "a\nb"})
        self.assertEqual("TOKEN=secret\nHEADERS=a\\nb\n", serialized)

    def test_root_staging_rejects_transaction_path_escape_before_sudo(self):
        with tempfile.TemporaryDirectory() as td, \
             mock.patch.object(self.dev.subprocess, "run") as run:
            staging = Path(td)
            helper = staging / "helper.py"
            helper.write_text("trusted helper")
            with self.assertRaisesRegex(SystemExit, "exact transaction id"):
                self.dev._stage_systemd_root_handoff(
                    staging,
                    "../attacker-owned/bootstrap",
                    helper,
                    [("helper.py", helper)],
                )
        run.assert_not_called()

    def test_root_staging_failure_prevents_transient_activation(self):
        with tempfile.TemporaryDirectory() as td:
            staging = Path(td)
            artifact = staging / "candidate"
            artifact.write_text("candidate")
            receipt = staging / self.dev.SYSTEMD_PREPARED_RECEIPT
            receipt.write_text("authenticated receipt")
            receipt_sha256 = self.dev._file_sha256(receipt)
            helper_sha256 = self.dev._file_sha256(artifact)
            results = [
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, f"{helper_sha256}  helper.py\n", ""),
                subprocess.CompletedProcess([], 1, "", "denied"),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with mock.patch.object(self.dev.subprocess, "run", side_effect=results) as run:
                with self.assertRaisesRegex(SystemExit, "handoff staging failed"):
                    self.dev._stage_systemd_root_handoff(
                        staging,
                        "a" * 32,
                        artifact,
                        [
                            ("candidate-binary", artifact),
                            ("helper.py", artifact),
                            (receipt.name, receipt),
                        ],
                        expected_preparation_receipt_sha256=receipt_sha256,
                    )
            self.assertTrue(all("systemd-run" not in call.args[0] for call in run.call_args_list))
            stage_command = run.call_args_list[3].args[0]
            digest_index = stage_command.index("--bundle-sha256") + 1
            self.assertEqual(
                self.dev._file_sha256(staging / "bundle.json"),
                stage_command[digest_index],
            )
            receipt_index = stage_command.index("--expected-preparation-receipt-sha256") + 1
            self.assertEqual(receipt_sha256, stage_command[receipt_index])

    def test_root_staging_rejects_bootstrap_outside_exact_controller_digest(self):
        with tempfile.TemporaryDirectory() as td:
            staging = Path(td)
            helper = staging / "helper.py"
            helper.write_text("substituted helper")
            helper_sha256 = self.dev._file_sha256(helper)
            expected_helper_sha256 = "a" * 64
            results = [
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, f"{helper_sha256}  helper.py\n", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with mock.patch.object(self.dev.subprocess, "run", side_effect=results) as run:
                with self.assertRaisesRegex(SystemExit, "bootstrap helper hash mismatch"):
                    self.dev._stage_systemd_root_handoff(
                        staging,
                        "a" * 32,
                        helper,
                        [("helper.py", helper)],
                        expected_helper_sha256=expected_helper_sha256,
                    )
            self.assertFalse(any("python3" in call.args[0] for call in run.call_args_list))
            bundle = json.loads((staging / "bundle.json").read_text())
            self.assertEqual(expected_helper_sha256, bundle["files"][0]["sha256"])

    def test_ordinary_root_staging_preserves_legacy_helper_cli(self):
        with tempfile.TemporaryDirectory() as td:
            staging = Path(td)
            helper = staging / "helper.py"
            helper.write_text("trusted helper")
            helper_sha256 = self.dev._file_sha256(helper)
            results = [
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, f"{helper_sha256}  helper.py\n", ""),
                subprocess.CompletedProcess([], 1, "", "denied"),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            with mock.patch.object(self.dev.subprocess, "run", side_effect=results) as run:
                with self.assertRaisesRegex(SystemExit, "handoff staging failed"):
                    self.dev._stage_systemd_root_handoff(
                        staging,
                        "a" * 32,
                        helper,
                        [("helper.py", helper)],
                        bind_bundle=False,
                    )
            stage_command = run.call_args_list[3].args[0]
            self.assertNotIn("--bundle-sha256", stage_command)

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
            identity = self.dev.RuntimeIdentity("2.0.0", "b" * 40)
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
                     side_effect=lambda _staging, _tx, helper, files, *, noninteractive=False, bind_bundle=True: captured.update(
                         helper_text=helper.read_text(),
                         noninteractive=noninteractive,
                         bind_bundle=bind_bundle,
                         manifest=json.loads(dict(files)["manifest.json"].read_text()),
                         manifest_text=dict(files)["manifest.json"].read_text(),
                         socket_text=dict(files)["candidate.socket"].read_text(),
                         files=files,
                     ) or root_manifest,
                 ) as stage, \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run):
                self.dev.native_prod_deploy()

            prepare.assert_called_once_with(target="x86_64-unknown-linux-musl")
            source_file.assert_any_call(
                "b" * 40,
                "scripts/systemd_deploy_helper.py",
                mock.ANY,
                "local_head",
            )
            self.assertEqual((ROOT / "scripts/systemd_deploy_helper.py").read_text(), captured["helper_text"])
            files = captured["files"]
            manifest = captured["manifest"]
            self.assertFalse(captured["noninteractive"])
            self.assertFalse(captured["bind_bundle"])
            self.assertNotIn("value", captured["manifest_text"])
            self.assertEqual(identity.as_dict(), manifest["expected"])
            self.assertEqual(1, manifest["manifest_version"])
            for field in ("mode", "registry", "database_path", "controller_source_commit", "controller_helper", "release_asset_name", "release_asset_sha256"):
                self.assertNotIn(field, manifest)
            self.assertNotIn("migrations.rs", dict(files))
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

    def test_controller_skips_host_systemd_redetection(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v2.0.0",
            expected_full_commit="a" * 40,
            backend="systemd",
        )
        with mock.patch.object(self.dev, "check_systemd_available") as available, \
             mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
             mock.patch.object(self.dev, "_read_systemd_installed_env", return_value={"PHOENIX_PASSWORD": "installed"}), \
             mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
             mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
             mock.patch.object(self.dev, "_prepare_release_candidate", side_effect=SystemExit("stop here")):
            with self.assertRaisesRegex(SystemExit, "stop here"):
                self.dev.native_prod_deploy("v2.0.0", controller=controller)
        available.assert_not_called()

    def test_controller_requires_noninteractive_sudo_before_disruption(self):
        controller = self.dev.ProdDeployControllerOptions(
            enabled=True,
            exact_release_tag="v2.0.0",
            expected_full_commit="a" * 40,
            transaction_id="1" * 32,
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
            transaction_id="1" * 32,
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

    def protected_options(self, **changes):
        values = dict(enabled=True, backend="systemd", exact_release_tag=self.dev.SYSTEMD_PROTECTED_TAG,
                      expected_full_commit=self.dev.SYSTEMD_PROTECTED_COMMIT,
                      expected_asset_name=self.dev.SYSTEMD_PROTECTED_ASSET,
                      expected_asset_sha256=self.dev.SYSTEMD_PROTECTED_ASSET_SHA256, controller_source_commit="c" * 40,
                      systemd_current_schema_upgrade=True)
        values.update(changes)
        return self.dev.ProdDeployControllerOptions(**values)

    def test_protected_mode_rejects_every_unpinned_or_wrong_backend_route_before_work(self):
        import platform
        cases = [dict(enabled=False), dict(backend="bare_linux"), dict(backend="launchd"),
                 dict(prepared_artifact=Path("/prepared")), dict(current_schema_upgrade=True),
                 dict(paired_database_upgrade=True), dict(exact_release_tag=None),
                 dict(expected_full_commit=None), dict(expected_full_commit="B" * 40),
                 dict(expected_asset_name=None), dict(expected_asset_name="other-asset"),
                 dict(expected_asset_sha256=None), dict(expected_asset_sha256="D" * 64),
                 dict(controller_source_commit=None), dict(controller_source_commit="C" * 40),
                 dict(controller_source_commit=self.dev.SYSTEMD_PROTECTED_COMMIT),
                 dict(exact_release_tag="v0.13.0-rc.3"), dict(expected_full_commit="d" * 40),
                 dict(expected_asset_sha256="d" * 64)]
        with mock.patch.object(sys, "platform", "linux"), \
             mock.patch.object(platform, "machine", return_value="x86_64"), \
             mock.patch.object(self.dev, "_require_noninteractive_sudo_ready") as sudo, \
             mock.patch.object(self.dev, "_prepare_release_candidate") as prepare, \
             mock.patch.object(self.dev, "cmd_check") as check:
            for changes in cases:
                with self.subTest(changes=changes), self.assertRaises(SystemExit):
                    self.dev.cmd_prod_deploy(self.dev.SYSTEMD_PROTECTED_TAG, controller=self.protected_options(**changes))
            for release in (None, "latest", "v-other"):
                with self.subTest(release=release), self.assertRaises(SystemExit):
                    self.dev.cmd_prod_deploy(release, controller=self.protected_options(exact_release_tag=release))
        sudo.assert_not_called()
        prepare.assert_not_called()
        check.assert_not_called()
        for host, arch in (("darwin", "x86_64"), ("linux", "aarch64")):
            with mock.patch.object(sys, "platform", host), mock.patch.object(platform, "machine", return_value=arch):
                with self.assertRaises(SystemExit):
                    self.protected_options().require_systemd_current("v2.0.0")

    def test_protected_provenance_split_binds_candidate_registry_and_explicit_controller(self):
        import platform
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "release"
            binary.write_text("published binary")
            options = self.protected_options(expected_asset_sha256=self.dev._file_sha256(binary))
            commit = self.dev.SYSTEMD_PROTECTED_COMMIT
            tag = self.dev.SYSTEMD_PROTECTED_TAG
            candidate = self.dev.PreparedCandidate(binary, self.dev.ProdSourceKind.PUBLISHED_RELEASE,
                                                   commit, self.dev.RuntimeIdentity(self.dev.SYSTEMD_PROTECTED_VERSION, commit),
                                                   release_tag=tag, release_commit=commit)
            captured = {}
            def materialize(commit, source, destination, kind):
                destination.write_bytes((ROOT / source).read_bytes())
            def stage(_staging, tx, helper_path, files, **kwargs):
                captured.update(json.loads(dict(files)["manifest.json"].read_text()))
                captured["stage_kwargs"] = kwargs
                captured["helper_contents"] = helper_path.read_bytes()
                captured["registry_contents"] = dict(files)["migrations.rs"].read_bytes()
                return self.dev.SYSTEMD_TRANSACTION_ROOT / tx / "manifest.json"
            with mock.patch.object(sys, "platform", "linux"), \
                 mock.patch.object(platform, "machine", return_value="x86_64"), \
                 mock.patch.object(self.dev, "SYSTEMD_PROTECTED_ASSET_SHA256", options.expected_asset_sha256), \
                 mock.patch.object(self.dev, "SYSTEMD_PROTECTED_REGISTRY_SHA256", self.dev._file_sha256(ROOT / "crates/phoenix-db/src/migrations.rs")), \
                 mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
                 mock.patch.object(self.dev, "_read_systemd_installed_env", return_value={"SECRET": "installed"}), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
                 mock.patch.object(self.dev, "_prepare_release_candidate", return_value=candidate) as prepare, \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize) as fetch, \
                 mock.patch.object(self.dev, "_stage_systemd_root_handoff", side_effect=stage), \
                 mock.patch.object(self.dev, "_launch_systemd_activation"), \
                 mock.patch.object(self.dev.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "2\n", "")):
                self.dev.native_prod_deploy(tag, controller=options)
            fetch.assert_has_calls([
                mock.call("c" * 40, "scripts/systemd_deploy_helper.py", mock.ANY, "published_release"),
                mock.call(commit, "crates/phoenix-db/src/migrations.rs", mock.ANY, "published_release"),
            ])
            self.assertEqual(2, fetch.call_count)
            self.assertEqual(commit, captured["source_commit"])
            self.assertEqual("c" * 40, captured["controller_source_commit"])
            self.assertEqual(2, captured["manifest_version"])
            self.assertEqual("systemd_schema65_to128", captured["mode"])
            self.assertFalse(captured["stage_kwargs"]["bind_bundle"])
            import hashlib
            self.assertEqual(hashlib.sha256(captured["helper_contents"]).hexdigest(), captured["controller_helper"]["sha256"])
            self.assertEqual(hashlib.sha256(captured["registry_contents"]).hexdigest(), captured["registry"]["sha256"])
            self.assertEqual(str(self.dev.SYSTEMD_DB_PATH), captured["database_path"])
            prepare.assert_called_once_with(tag, mock.ANY, expected_full_commit=commit,
                                            expected_asset_name=options.expected_asset_name,
                                            expected_asset_sha256=options.expected_asset_sha256)

    def test_protected_controller_rejects_registry_byte_mutation_before_handoff(self):
        import platform
        with tempfile.TemporaryDirectory() as td:
            binary = Path(td) / "binary"
            binary.write_text("inert candidate oracle")
            tag, commit = self.dev.SYSTEMD_PROTECTED_TAG, self.dev.SYSTEMD_PROTECTED_COMMIT
            options = self.protected_options(expected_asset_sha256=self.dev._file_sha256(binary))
            candidate = self.dev.PreparedCandidate(binary, self.dev.ProdSourceKind.PUBLISHED_RELEASE, commit,
                                                   self.dev.RuntimeIdentity(self.dev.SYSTEMD_PROTECTED_VERSION, commit),
                                                   release_tag=tag, release_commit=commit)
            def materialize(commit, source, destination, kind):
                destination.write_bytes((ROOT / source).read_bytes() + b"\n")
            with mock.patch.object(sys, "platform", "linux"), \
                 mock.patch.object(platform, "machine", return_value="x86_64"), \
                 mock.patch.object(self.dev, "SYSTEMD_PROTECTED_ASSET_SHA256", options.expected_asset_sha256), \
                 mock.patch.object(self.dev, "SYSTEMD_PROTECTED_REGISTRY_SHA256", self.dev._file_sha256(ROOT / "crates/phoenix-db/src/migrations.rs")), \
                 mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
                 mock.patch.object(self.dev, "_read_systemd_installed_env", return_value={}), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
                 mock.patch.object(self.dev, "_prepare_release_candidate", return_value=candidate), \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize), \
                 mock.patch.object(self.dev, "_stage_systemd_root_handoff") as stage, \
                 mock.patch.object(self.dev, "_launch_systemd_activation") as activate:
                with self.assertRaisesRegex(SystemExit, "registry hash is not the reviewed release"):
                    self.dev.native_prod_deploy(tag, controller=options)
            stage.assert_not_called()
            activate.assert_not_called()

    def test_reviewed_protected_pins_are_literal_and_helper_agrees(self):
        import runpy
        helper = runpy.run_path(str(ROOT / "scripts/systemd_deploy_helper.py"))
        pins = {
            "TAG": "v0.13.0-rc.4", "COMMIT": "3f0ba4602faa71ebad5088e1ffc856f4c7d511d9",
            "VERSION": "0.13.0-rc.4", "ASSET": "phoenix_ide-x86_64-unknown-linux-musl",
            "ASSET_SHA256": "bcb74d565c828843596e5109ea9034a2b0e505aab4ba6c18c5cca8e116f5677e",
            "REGISTRY_SHA256": "700162dd8e41305546cf2acd2763bad5603c642840f5be851fb788353879a36a",
        }
        helper_names = {"TAG": "RELEASE_TAG", "COMMIT": "RELEASE_COMMIT", "VERSION": "RELEASE_VERSION", "ASSET": "ASSET_NAME"}
        for name, value in pins.items():
            self.assertEqual(value, getattr(self.dev, "SYSTEMD_PROTECTED_" + name))
            self.assertEqual(value, helper["PROTECTED_" + helper_names.get(name, name)])

    def test_linux_prepared_pins_are_literal_and_helper_agrees(self):
        import runpy
        helper = runpy.run_path(str(ROOT / "scripts/systemd_deploy_helper.py"))
        pins = {
            "COMMIT": "237100c79d9ce672867b284167a099fa0b7a2bcf",
            "VERSION": "0.13.0-rc.4", "TARGET": "x86_64-unknown-linux-musl",
            "REGISTRY_SHA256": "a4eb047e59b21997ea068f1bd5e97c9c943f74a4c5670cdd8ff8ac10ffd5eee6",
            "OPERATION": "prepare-incident-237100-linux-x86_64",
            "PAYLOAD": "phoenix_ide-x86_64-unknown-linux-musl-prepared-237100c79d9c",
            "RECEIPT": "LINUX-PREPARATION-RECEIPT-x86_64-unknown-linux-musl.json",
        }
        for name, value in pins.items():
            self.assertEqual(value, getattr(self.dev, "SYSTEMD_PREPARED_" + name))
            self.assertEqual(value, helper["PREPARED_" + name])
        self.assertEqual(3, helper["PREPARED_HANDOFF_PROTOCOL_VERSION"])
        self.assertEqual(2, helper["PROTECTED_HANDOFF_PROTOCOL_VERSION"])

    def linux_prepared_options(self, directory, **changes):
        receipt = directory / self.dev.SYSTEMD_PREPARED_RECEIPT
        values = dict(
            enabled=True,
            backend="systemd",
            systemd_current_schema_upgrade=True,
            prepared_artifact=directory,
            expected_preparation_receipt_sha256=(
                self.dev._file_sha256(receipt) if receipt.is_file() else "e" * 64
            ),
            expected_full_commit=self.dev.SYSTEMD_PREPARED_COMMIT,
            controller_source_commit="c" * 40,
        )
        values.update(changes)
        return self.dev.ProdDeployControllerOptions(**values)

    def linux_prepared_directory(self, base):
        binary = base / self.dev.SYSTEMD_PREPARED_PAYLOAD
        binary.write_bytes(b"inert Linux prepared payload")
        receipt = base / self.dev.SYSTEMD_PREPARED_RECEIPT
        receipt.write_text(json.dumps({
            "schema": 1, "operation": self.dev.SYSTEMD_PREPARED_OPERATION,
            "target": self.dev.SYSTEMD_PREPARED_TARGET, "version": self.dev.SYSTEMD_PREPARED_VERSION,
            "commit": self.dev.SYSTEMD_PREPARED_COMMIT, "controller_commit": "c" * 40,
            "registry_sha256": self.dev.SYSTEMD_PREPARED_REGISTRY_SHA256,
            "run_id": 42, "run_attempt": 1, "event": "workflow_dispatch", "ref": "refs/heads/main",
            "sha256": {binary.name: self.dev._file_sha256(binary)},
            "checks": {"clean_source": "verified", "static_elf": "verified", "embedded_identity": "verified"},
        }))
        return binary, receipt

    def test_linux_prepared_controller_checkout_must_be_exact_and_clean_before_sudo(self):
        import platform
        expected = "c" * 40

        for head, status in (("d" * 40, ""), (expected, "?? local-change")):
            with self.subTest(head=head, status=status), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=[
                     subprocess.CompletedProcess([], 0, head + "\n", ""),
                     subprocess.CompletedProcess([], 0, status + "\n", ""),
                 ]), self.assertRaisesRegex(SystemExit, "clean checkout"):
                self.dev._require_exact_clean_controller_checkout(expected)

        with mock.patch.object(self.dev.subprocess, "run", side_effect=[
                 subprocess.CompletedProcess([], 0, expected + "\n", ""),
                 subprocess.CompletedProcess([], 0, "", ""),
             ]) as run:
            self.dev._require_exact_clean_controller_checkout(expected)
        self.assertEqual(["git", "rev-parse", "HEAD"], run.call_args_list[0].args[0])
        self.assertEqual(["git", "status", "--porcelain"], run.call_args_list[1].args[0])

        with tempfile.TemporaryDirectory() as td, mock.patch.object(sys, "platform", "linux"), \
             mock.patch.object(platform, "machine", return_value="x86_64"), \
             mock.patch.object(self.dev, "_require_exact_clean_controller_checkout",
                               side_effect=SystemExit("wrong controller")) as checkout, \
             mock.patch.object(self.dev, "_require_noninteractive_sudo_ready") as sudo, \
             mock.patch.object(self.dev, "_prepare_linux_artifact") as prepare:
            with self.assertRaisesRegex(SystemExit, "wrong controller"):
                self.dev.native_prod_deploy(controller=self.linux_prepared_options(Path(td)))
        checkout.assert_called_once_with(expected)
        sudo.assert_not_called()
        prepare.assert_not_called()
        with tempfile.TemporaryDirectory() as td, mock.patch.object(sys, "platform", "linux"), \
             mock.patch.object(platform, "machine", return_value="x86_64"), \
             mock.patch.object(self.dev, "_require_exact_clean_controller_checkout"), \
             mock.patch.object(
                 self.dev,
                 "_materialize_verified_systemd_helper",
                 return_value=mock.sentinel.verified_helper,
             ), \
             mock.patch.object(
                 self.dev,
                 "_prepare_linux_artifact",
                 side_effect=SystemExit("receipt authority mismatch"),
             ) as prepare, \
             mock.patch.object(self.dev, "_require_noninteractive_sudo_ready") as sudo, \
             mock.patch.object(self.dev, "_read_systemd_installed_env") as read_env:
            with self.assertRaisesRegex(SystemExit, "receipt authority mismatch"):
                self.dev.native_prod_deploy(controller=self.linux_prepared_options(Path(td)))
        prepare.assert_called_once()
        sudo.assert_not_called()
        read_env.assert_not_called()


    def test_linux_prepared_options_reject_mixed_modes_before_sudo_or_preparation(self):
        import platform
        with tempfile.TemporaryDirectory() as td, mock.patch.object(sys, "platform", "linux"), \
             mock.patch.object(platform, "machine", return_value="x86_64"), \
             mock.patch.object(self.dev, "_require_noninteractive_sudo_ready") as sudo, \
             mock.patch.object(self.dev, "_prepare_linux_artifact") as prepare, \
             mock.patch.object(self.dev, "cmd_check") as check:
            base = Path(td)
            cases = [dict(enabled=False), dict(backend="launchd"), dict(prepared_artifact=None),
                     dict(expected_full_commit="a" * 40), dict(controller_source_commit=None),
                     dict(controller_source_commit="C" * 40),
                     dict(controller_source_commit=self.dev.SYSTEMD_PREPARED_COMMIT),
                     dict(expected_preparation_receipt_sha256=None),
                     dict(expected_preparation_receipt_sha256="A" * 64),
                     dict(transaction_id="../attacker-owned/bootstrap"),
                     dict(exact_release_tag=self.dev.SYSTEMD_PROTECTED_TAG),
                     dict(expected_asset_name=self.dev.SYSTEMD_PROTECTED_ASSET),
                     dict(expected_asset_sha256=self.dev.SYSTEMD_PROTECTED_ASSET_SHA256),
                     dict(systemd_current_schema_upgrade=False), dict(current_schema_upgrade=True),
                     dict(paired_database_upgrade=True)]
            for changes in cases:
                with self.subTest(changes=changes), self.assertRaises(SystemExit):
                    self.dev.cmd_prod_deploy(controller=self.linux_prepared_options(base, **changes))
            with self.assertRaises(SystemExit):
                self.dev.cmd_prod_deploy(self.dev.SYSTEMD_PROTECTED_TAG,
                                         controller=self.linux_prepared_options(base))
            sudo.assert_not_called()
            prepare.assert_not_called()
            check.assert_not_called()

    def test_linux_receipt_shape_tamper_symlinks_and_identity_are_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            base = root / "source"
            staging_root = root / "staging"
            base.mkdir()
            staging_root.mkdir()
            binary, receipt = self.linux_prepared_directory(base)
            expected = self.dev.RuntimeIdentity(self.dev.SYSTEMD_PREPARED_VERSION, self.dev.SYSTEMD_PREPARED_COMMIT)
            verified_helper = self.dev.VerifiedSystemdControllerHelper(
                source=(ROOT / "scripts/systemd_deploy_helper.py").read_bytes()
            )

            def prepare(expected_receipt_sha256=None):
                staging = Path(tempfile.mkdtemp(dir=staging_root))
                return self.dev._prepare_linux_artifact(
                    base,
                    self.dev.SYSTEMD_PREPARED_COMMIT,
                    "c" * 40,
                    expected_receipt_sha256 or self.dev._file_sha256(receipt),
                    staging,
                    verified_helper,
                )

            invoked = []

            def identity(path):
                invoked.append(path)
                binary.write_bytes(b"mutable source changed after private copy")
                return expected

            trusted_import = __import__

            def reject_ambient_helper_import(name, *args, **kwargs):
                if name == "scripts.systemd_deploy_helper":
                    raise AssertionError("prepared receipt parsing used mutable ambient helper")
                return trusted_import(name, *args, **kwargs)

            with mock.patch("builtins.__import__", side_effect=reject_ambient_helper_import), \
                 mock.patch.object(self.dev, "_binary_identity", side_effect=identity):
                candidate = prepare()
            self.assertEqual([candidate.binary], invoked)
            self.assertNotEqual(binary, candidate.binary)
            self.assertNotEqual(receipt, candidate.receipt)
            self.assertEqual(self.dev._file_sha256(candidate.receipt), candidate.receipt_sha256)
            binary.write_bytes(b"inert Linux prepared payload")
            authenticated_receipt_sha256 = self.dev._file_sha256(receipt)
            original_receipt = receipt.read_text()
            binary.write_bytes(b"jointly substituted payload")
            substituted = json.loads(original_receipt)
            substituted["sha256"][binary.name] = self.dev._file_sha256(binary)
            receipt.write_text(json.dumps(substituted))
            with self.assertRaisesRegex(SystemExit, "independently authenticated digest"):
                prepare(authenticated_receipt_sha256)
            binary.write_bytes(b"inert Linux prepared payload")
            receipt.write_text(original_receipt)
            with mock.patch.object(self.dev, "_binary_identity", return_value=expected):
                for field, value in (("schema", True), ("event", "push"), ("run_id", 0),
                                     ("run_attempt", False), ("controller_commit", "d" * 40),
                                     ("registry_sha256", "0" * 64)):
                    original = receipt.read_text()
                    raw = json.loads(original)
                    raw[field] = value
                    receipt.write_text(json.dumps(raw))
                    with self.subTest(field=field), self.assertRaises(SystemExit):
                        prepare()
                    receipt.write_text(original)
                for key in ("surplus", "checks", "sha256"):
                    original = receipt.read_text()
                    raw = json.loads(original)
                    if key == "surplus":
                        raw[key] = 1
                    else:
                        raw[key]["extra"] = "verified"
                    receipt.write_text(json.dumps(raw))
                    with self.subTest(key=key), self.assertRaises(SystemExit):
                        prepare()
                    receipt.write_text(original)
                original = receipt.read_text()
                raw = json.loads(original)
                raw.pop("checks")
                receipt.write_text(json.dumps(raw))
                with self.assertRaises(SystemExit):
                    prepare()
                receipt.write_text(original)
                receipt.write_text(original.replace('"schema": 1,', '"schema": 1, "schema": 1,'))
                with self.assertRaises(SystemExit):
                    prepare()
                receipt.write_text(original)
                raw = json.loads(original)
                raw["sha256"][binary.name] = "A" * 64
                receipt.write_text(json.dumps(raw))
                with self.assertRaises(SystemExit):
                    prepare()
                receipt.write_text(original)
                (base / "extra-payload").write_bytes(b"extra")
                with self.assertRaisesRegex(SystemExit, "only exact"):
                    prepare()
                (base / "extra-payload").unlink()
                binary.write_bytes(b"changed")
                with self.assertRaisesRegex(SystemExit, "checksum"):
                    prepare()
                binary.write_bytes(b"inert Linux prepared payload")
                with mock.patch.object(self.dev, "_binary_identity", return_value=self.dev.RuntimeIdentity("wrong", expected.git_sha)):
                    with self.assertRaisesRegex(SystemExit, "embedded identity"):
                        prepare()
                binary.unlink()
                binary.symlink_to(receipt)
                with self.assertRaisesRegex(SystemExit, "open prepared systemd artifact"):
                    prepare()
                candidate.binary.write_bytes(b"changed retained copy")
                with self.assertRaisesRegex(SystemExit, "changed"):
                    self.dev._require_linux_prepared_unchanged(candidate)

    def test_linux_prepared_directory_swap_cannot_retarget_entry_opens(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source = root / "source"
            forged = root / "forged"
            staging = root / "staging"
            source.mkdir()
            forged.mkdir()
            staging.mkdir()
            self.linux_prepared_directory(source)
            forged_binary, forged_receipt = self.linux_prepared_directory(forged)
            forged_binary.write_bytes(b"forged payload")
            forged_raw = json.loads(forged_receipt.read_text())
            forged_raw["sha256"][forged_binary.name] = self.dev._file_sha256(forged_binary)
            forged_receipt.write_text(json.dumps(forged_raw))
            expected = self.dev.RuntimeIdentity(
                self.dev.SYSTEMD_PREPARED_VERSION, self.dev.SYSTEMD_PREPARED_COMMIT
            )
            real_listdir = os.listdir

            def swap_after_enumeration(directory_fd):
                names = real_listdir(directory_fd)
                original = root / "original-source"
                source.rename(original)
                original_stat = original.stat()
                os.utime(
                    original,
                    ns=(original_stat.st_atime_ns, original_stat.st_mtime_ns + 1_000_000_000),
                )
                source.symlink_to(forged, target_is_directory=True)
                return names

            with mock.patch.object(os, "listdir", side_effect=swap_after_enumeration), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=expected) as identity:
                with self.assertRaisesRegex(SystemExit, "directory changed"):
                    self.dev._prepare_linux_artifact(
                        source,
                        self.dev.SYSTEMD_PREPARED_COMMIT,
                        "c" * 40,
                        self.dev._file_sha256(source / self.dev.SYSTEMD_PREPARED_RECEIPT),
                        staging,
                        self.dev.VerifiedSystemdControllerHelper(
                            source=(ROOT / "scripts/systemd_deploy_helper.py").read_bytes()
                        ),
                    )
            identity.assert_not_called()

    def test_linux_prepared_manifest_retains_receipt_and_separate_provenance(self):
        import platform
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            binary, receipt = self.linux_prepared_directory(base)
            from scripts import systemd_deploy_helper as linux_helper
            registry_source = (ROOT / "crates/phoenix-db/src/migrations.rs").read_text()
            registry_match = re.search(r"(const MIGRATIONS: &\[Migration\] = &\[)(.*?)(\n\];)", registry_source, re.DOTALL)
            self.assertIsNotNone(registry_match)
            historical_entries, removed = re.subn(
                r"    Migration\s*\{\s*version:\s*129,.*?\n    \},", "",
                registry_match.group(2), count=1, flags=re.DOTALL,
            )
            self.assertEqual(removed, 1)
            historical_registry = base / "qualified-128-migrations.rs"
            historical_registry.write_text(registry_source[:registry_match.start(2)] + historical_entries
                                           + registry_source[registry_match.end(2):])
            registry_sha = self.dev._file_sha256(historical_registry)
            raw = json.loads(receipt.read_text())
            raw["registry_sha256"] = registry_sha
            receipt.write_text(json.dumps(raw))
            captured = {}
            def materialize(commit, source, destination, kind):
                captured.setdefault("sources", []).append((commit, source, kind))
                fixture_source = historical_registry if source == "crates/phoenix-db/src/migrations.rs" else ROOT / source
                destination.write_bytes(fixture_source.read_bytes())
                return self.dev._file_sha256(destination)
            def stage(_staging, tx, helper_path, files, **kwargs):
                captured["manifest"] = json.loads(dict(files)["manifest.json"].read_text())
                captured["stage_kwargs"] = kwargs
                captured["files"] = dict(files)
                captured["receipt_bytes"] = dict(files)[self.dev.SYSTEMD_PREPARED_RECEIPT].read_bytes()
                return self.dev.SYSTEMD_TRANSACTION_ROOT / tx / "manifest.json"
            expected = self.dev.RuntimeIdentity(self.dev.SYSTEMD_PREPARED_VERSION, self.dev.SYSTEMD_PREPARED_COMMIT)
            def run_verified_helper(command, **kwargs):
                if "--parse-prepared-receipt" in command:
                    return subprocess.CompletedProcess(command, 0, json.dumps(raw).encode(), b"")
                return subprocess.CompletedProcess(command, 0, b"3\n", b"")
            with mock.patch.object(sys, "platform", "linux"), \
                 mock.patch.object(platform, "machine", return_value="x86_64"), \
                 mock.patch.object(self.dev, "_binary_identity", return_value=expected), \
                 mock.patch.object(self.dev, "SYSTEMD_PREPARED_REGISTRY_SHA256", registry_sha), \
                 mock.patch.object(linux_helper, "PREPARED_REGISTRY_SHA256", registry_sha), \
                 mock.patch.object(self.dev, "_require_exact_clean_controller_checkout") as checkout, \
                 mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
                 mock.patch.object(self.dev, "_read_systemd_installed_env", return_value={"SECRET": "installed"}), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize), \
                 mock.patch.object(self.dev, "_stage_systemd_root_handoff", side_effect=stage), \
                 mock.patch.object(self.dev, "_launch_systemd_activation"), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run_verified_helper) as run:
                self.dev.native_prod_deploy(controller=self.linux_prepared_options(base))
            manifest = captured["manifest"]
            self.assertEqual("prepared_artifact", manifest["source_kind"])
            self.assertEqual(3, manifest["manifest_version"])
            self.assertTrue(captured["stage_kwargs"]["bind_bundle"])
            self.assertEqual(
                self.dev._file_sha256(ROOT / "scripts/systemd_deploy_helper.py"),
                captured["stage_kwargs"]["expected_helper_sha256"],
            )
            self.assertNotIn("release_asset_name", manifest)
            self.assertNotIn("release_asset_sha256", manifest)
            self.assertNotIn("release_tag", manifest)
            self.assertNotIn("release_commit", manifest)
            self.assertEqual("c" * 40, manifest["controller_source_commit"])
            self.assertEqual(self.dev._file_sha256(receipt), manifest["prepared_artifact_receipt"]["sha256"])
            self.assertNotEqual(receipt, captured["files"][self.dev.SYSTEMD_PREPARED_RECEIPT])
            self.assertEqual(receipt.read_bytes(), captured["receipt_bytes"])
            self.assertEqual(self.dev._file_sha256(binary), manifest["candidate"]["binary"]["sha256"])
            self.assertEqual([("c" * 40, "scripts/systemd_deploy_helper.py", "prepared_artifact"),
                              (self.dev.SYSTEMD_PREPARED_COMMIT, "crates/phoenix-db/src/migrations.rs", "prepared_artifact")],
                             captured["sources"])
            parse_call = next(
                call for call in run.call_args_list
                if "--parse-prepared-receipt" in call.args[0]
            )
            self.assertEqual([sys.executable, "-I", "-S", "-"], parse_call.args[0][:4])
            protocol_call = next(
                call for call in run.call_args_list
                if "--protected-prepared-protocol-version" in call.args[0]
            )
            self.assertEqual("-", protocol_call.args[0][1])
            self.assertEqual(
                (ROOT / "scripts/systemd_deploy_helper.py").read_bytes(),
                protocol_call.kwargs["input"],
            )
            checkout.assert_called_once_with("c" * 40)

    def test_ordinary_old_release_pending_database_refuses_before_handoff_using_candidate_bytes(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            binary = base / "binary"
            binary.write_text("old RC candidate")
            database = base / "prod.db"
            with sqlite3.connect(database) as db:
                db.execute("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT)")
                db.execute("INSERT INTO _migrations VALUES (65, 'source65')")
            tag, commit = self.dev.SYSTEMD_PROTECTED_TAG, self.dev.SYSTEMD_PROTECTED_COMMIT
            candidate = self.dev.PreparedCandidate(binary, self.dev.ProdSourceKind.PUBLISHED_RELEASE, commit,
                                                   self.dev.RuntimeIdentity(self.dev.SYSTEMD_PROTECTED_VERSION, commit),
                                                   release_tag=tag, release_commit=commit)
            options = self.dev.ProdDeployControllerOptions(enabled=True, backend="systemd", exact_release_tag=tag,
                                                          expected_full_commit=commit)
            def run(command, **kwargs):
                if command[-1] == "--migrate-only":
                    self.assertEqual("candidate-binary", Path(command[0]).name)
                    self.assertEqual(binary.read_bytes(), Path(command[0]).read_bytes())
                    self.assertEqual({"PATH", "HOME", "PHOENIX_DB_PATH"}, set(kwargs["env"]))
                    target = Path(kwargs["env"]["PHOENIX_DB_PATH"])
                    self.assertNotEqual(database, target)
                    self.assertFalse(target.exists())
                    with sqlite3.connect(target) as db:
                        db.execute("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT)")
                        db.executemany("INSERT INTO _migrations VALUES (?, ?)", [(65, "source65"), (128, "target128")])
                    return subprocess.CompletedProcess(command, 0, "", "")
                self.assertEqual("gh", command[0])
                self.assertIn("?ref=" + commit, command[2])
                self.assertNotIn("migrations.rs", command[2])
                return subprocess.CompletedProcess(command, 0, b"# old release helper\n", b"")
            with mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
                 mock.patch.object(self.dev, "_read_systemd_installed_env", return_value={"PHOENIX_DB_PATH": str(database)}), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
                 mock.patch.object(self.dev, "_prepare_release_candidate", return_value=candidate), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run), \
                 mock.patch.object(self.dev, "_stage_systemd_root_handoff") as stage, \
                 mock.patch.object(self.dev, "_launch_systemd_activation") as activate:
                with self.assertRaisesRegex(SystemExit, "pending migration interval"):
                    self.dev.native_prod_deploy(tag, controller=options)
            stage.assert_not_called()
            activate.assert_not_called()

    def test_ordinary_future_exact_target_preserves_strict_old_helper_manifest_shape(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            binary = base / "binary"
            binary.write_text("future candidate")
            database = base / "prod.db"
            with sqlite3.connect(database) as db:
                db.execute("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT)")
                db.executemany("INSERT INTO _migrations VALUES (?, ?)", [(1, "one"), (129, "future")])
            candidate = self.dev.PreparedCandidate(binary, self.dev.ProdSourceKind.PUBLISHED_RELEASE, "b" * 40,
                                                   self.dev.RuntimeIdentity("3.0.0", "b" * 40),
                                                   release_tag="v3.0.0", release_commit="b" * 40)
            old_fields = {"manifest_version", "transaction_id", "unit_name", "service_user", "source_kind", "source_commit",
                          "release_tag", "release_commit", "expected", "previous", "expected_health_url", "previous_health_url",
                          "candidate", "rollback", "targets", "status_path", "active_path", "activation_lock_path", "claim_lock_path",
                          "previous_deployed_sha", "created_at"}
            def materialize(commit, source, destination, kind):
                self.assertEqual("b" * 40, commit)
                self.assertEqual("published_release", kind)
                self.assertFalse(source.endswith("migrations.rs"), "Rust registry syntax is not an authority")
                destination.write_text("# retained old helper\n")
            def run(command, **kwargs):
                if command[-1] == "--migrate-only":
                    self.assertEqual(binary.read_bytes(), Path(command[0]).read_bytes())
                    target = Path(kwargs["env"]["PHOENIX_DB_PATH"])
                    self.assertFalse(target.exists())
                    with sqlite3.connect(target) as db:
                        db.execute("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT)")
                        db.executemany("INSERT INTO _migrations VALUES (?, ?)", [(1, "one"), (129, "future")])
                return subprocess.CompletedProcess(command, 0, "1\n", "")
            def old_helper_stage(staging, tx, helper, files, **kwargs):
                manifest = json.loads(dict(files)["manifest.json"].read_text())
                self.assertEqual(old_fields, set(manifest))
                self.assertEqual(1, manifest["manifest_version"])
                self.assertNotIn("migrations.rs", dict(files))
                self.assertEqual("# retained old helper\n", helper.read_text())
                return self.dev.SYSTEMD_TRANSACTION_ROOT / tx / "manifest.json"
            options = self.dev.ProdDeployControllerOptions(enabled=True, backend="systemd", exact_release_tag="v3.0.0", expected_full_commit="b" * 40)
            with mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
                 mock.patch.object(self.dev, "_read_systemd_installed_env", return_value={"PHOENIX_DB_PATH": str(database)}), \
                 mock.patch.object(self.dev, "_preflight_prod_bind_auth"), \
                 mock.patch.object(self.dev, "detect_service_user", return_value="nobody"), \
                 mock.patch.object(self.dev, "_prepare_release_candidate", return_value=candidate), \
                 mock.patch.object(self.dev, "_materialize_source_file", side_effect=materialize), \
                 mock.patch.object(self.dev.subprocess, "run", side_effect=run), \
                 mock.patch.object(self.dev, "_stage_systemd_root_handoff", side_effect=old_helper_stage) as stage, \
                 mock.patch.object(self.dev, "_launch_systemd_activation") as activate:
                self.dev.native_prod_deploy("v3.0.0", controller=options)
            stage.assert_called_once()
            activate.assert_called_once()

    def test_ordinary_missing_database_requires_no_predecessor_artifact_or_identity(self):
        import contextlib
        import sqlite3
        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            base = Path(temporary)
            paths = {"PROD_INSTALL_DIR": base / "install", "SYSTEMD_SERVICE_PATH": base / "service",
                     "SYSTEMD_SOCKET_PATH": base / "socket", "PROD_ENV_FILE": base / "env",
                     "SYSTEMD_DEPLOYED_SHA_PATH": base / "deployed.sha"}
            for name, value in paths.items():
                stack.enter_context(mock.patch.object(self.dev, name, value))
            binary = base / "candidate"
            binary.write_text("candidate")
            candidate = self.dev.PreparedCandidate(binary, self.dev.ProdSourceKind.LOCAL_HEAD, "a" * 40,
                                                   self.dev.RuntimeIdentity("1.0.0", "a" * 40))
            live = base / "live.sqlite"
            environment = {"PHOENIX_DB_PATH": str(live)}
            with mock.patch.object(self.dev.subprocess, "run", side_effect=AssertionError("no candidate invocation")):
                self.dev._systemd_ordinary_database_preflight(candidate, base, environment)
                artifacts = [paths["PROD_INSTALL_DIR"] / "phoenix-ide", *[paths[name] for name in paths if name != "PROD_INSTALL_DIR"]]
                for artifact in artifacts:
                    with self.subTest(artifact=artifact):
                        artifact.parent.mkdir(exist_ok=True)
                        artifact.write_text("installed predecessor evidence")
                        with sqlite3.connect(live) as unlinked:
                            unlinked.execute("CREATE TABLE data(value)")
                            unlinked.commit()
                            live.unlink()
                            with self.assertRaisesRegex(SystemExit, "not a first install"):
                                self.dev._systemd_ordinary_database_preflight(candidate, base, environment)
                            self.assertEqual([], unlinked.execute("SELECT * FROM data").fetchall())
                        artifact.unlink()
                with mock.patch.object(self.dev, "_systemd_current_identity", return_value=self.dev.RuntimeIdentity("1.0.0", "b" * 40)):
                    with self.assertRaisesRegex(SystemExit, "not a first install"):
                        self.dev._systemd_ordinary_database_preflight(candidate, base, environment)
                self.dev._systemd_ordinary_database_preflight(candidate, base, environment)
            self.assertFalse(live.exists())

    def test_ordinary_candidate_target_gate_rejects_failed_invalid_or_foreign_key_broken_candidate_databases(self):
        import sqlite3
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            binary = base / "candidate"
            binary.write_text("exact candidate bytes")
            live = base / "live.sqlite"
            with sqlite3.connect(live) as db:
                db.execute("CREATE TABLE _migrations(version INTEGER PRIMARY KEY, name TEXT)")
                db.execute("INSERT INTO _migrations VALUES (1, 'one')")
            before = live.read_bytes()
            candidate = self.dev.PreparedCandidate(binary, self.dev.ProdSourceKind.LOCAL_HEAD, "a" * 40,
                                                   self.dev.RuntimeIdentity("1.0.0", "a" * 40))
            for failure in ("spawn", "nonzero", "missing", "corrupt", "empty", "duplicate", "foreign_key", "candidate_changed", "renamed", "extra"):
                def run(command, **kwargs):
                    self.assertEqual([str(binary), "--migrate-only"], command)
                    self.assertEqual("/", kwargs["cwd"])
                    self.assertEqual({"PATH", "HOME", "PHOENIX_DB_PATH"}, set(kwargs["env"]))
                    target = Path(kwargs["env"]["PHOENIX_DB_PATH"])
                    self.assertNotEqual(live, target)
                    self.assertFalse(target.exists())
                    if failure == "spawn":
                        raise OSError("cannot execute")
                    if failure == "nonzero":
                        raise subprocess.CalledProcessError(1, command)
                    if failure == "corrupt":
                        target.write_bytes(b"not sqlite")
                    elif failure != "missing":
                        with sqlite3.connect(target) as db:
                            db.execute("CREATE TABLE _migrations(version INTEGER, name TEXT)")
                            if failure != "empty":
                                db.execute("INSERT INTO _migrations VALUES (1, ?)", ("renamed" if failure == "renamed" else "one",))
                            if failure == "duplicate":
                                db.execute("INSERT INTO _migrations VALUES (1, 'one')")
                            if failure == "extra":
                                db.execute("INSERT INTO _migrations VALUES (2, 'two')")
                            if failure == "foreign_key":
                                db.executescript("CREATE TABLE p(id PRIMARY KEY); CREATE TABLE c(id REFERENCES p(id)); INSERT INTO c VALUES(1);")
                    if failure == "candidate_changed":
                        binary.write_bytes(b"different candidate")
                    return subprocess.CompletedProcess(command, 0, "", "")
                with self.subTest(failure=failure), \
                     mock.patch.object(self.dev.subprocess, "run", side_effect=run), \
                     mock.patch.object(self.dev, "_materialize_source_file", side_effect=AssertionError("registry parser is not authoritative")), \
                     self.assertRaises(SystemExit):
                    self.dev._systemd_ordinary_database_preflight(candidate, base, {"PHOENIX_DB_PATH": str(live), "SECRET": "must not leak"})
                self.assertEqual(before, live.read_bytes())
                self.assertEqual([], list(base.glob("ordinary-candidate-target-*")))
                binary.write_text("exact candidate bytes")

    def test_recover_systemd_uses_retained_helper_in_independent_root_unit(self):
        tx = "e" * 32
        with mock.patch.object(sys, "platform", "linux"), \
             mock.patch.object(self.dev, "_require_noninteractive_sudo_ready"), \
             mock.patch.object(self.dev.subprocess, "run") as run:
            self.dev.cmd_prod_recover_systemd_current(tx)
        command = run.call_args.args[0]
        self.assertIn("recover-current", command)
        self.assertIn(str(self.dev.SYSTEMD_TRANSACTION_ROOT / tx / "helper.py"), command)
        self.assertIn("systemd-run", command)
        self.assertEqual(["sudo", "-n"], command[:2])


if __name__ == "__main__":
    unittest.main()
