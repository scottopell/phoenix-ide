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

    def setUp(self):
        identity = mock.patch.object(self.dev, "_systemd_current_identity", return_value=None)
        identity.start()
        self.addCleanup(identity.stop)

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
