import contextlib
import importlib.util
import io
import multiprocessing
import os
import platform
import sys
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_devpy():
    spec = importlib.util.spec_from_file_location("devpy_cache_under_test", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class CompilerCacheTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()
        self.store_root = self.dev.tempfile.TemporaryDirectory()
        self.addCleanup(self.store_root.cleanup)
        self.environment = mock.patch.dict(
            os.environ,
            {"KACHE_CACHE_DIR": str(Path(self.store_root.name) / "cache")},
            clear=True,
        )
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.socket_directory = Path(self.store_root.name) / "sockets"
        self.socket_directory.mkdir()
        self.cache_directory = Path(self.store_root.name) / "durable-cache"
        self.cache_directory.mkdir()
        self.real_private_kache_cache_dir = self.dev._private_kache_cache_dir
        self.private_cache = mock.patch.object(
            self.dev, "_private_kache_cache_dir", return_value=self.cache_directory
        )
        self.private_cache.start()
        self.addCleanup(self.private_cache.stop)
        self.real_private_kache_socket_dir = self.dev._private_kache_socket_dir
        self.private_socket = mock.patch.object(
            self.dev,
            "_private_kache_socket_dir",
            return_value=self.socket_directory,
        )
        self.private_socket.start()
        self.addCleanup(self.private_socket.stop)

        file_identity = self.dev._file_identity
        host_identity = mock.patch.object(
            self.dev,
            "_file_identity",
            side_effect=lambda path: {"state": "absent"}
            if path == Path("/etc/kache/config.toml").resolve() else file_identity(path),
        )
        host_identity.start()
        self.addCleanup(host_identity.stop)

        def daemon_status(*_args, **_kwargs):
            return mock.Mock(
                returncode=0,
                stdout=self.dev.json.dumps({
                    "daemon_running": False,
                    "socket": os.environ.get("KACHE_SOCKET_PATH"),
                }),
                stderr="",
            )

        status_command = mock.patch.object(
            self.dev.subprocess, "run", side_effect=daemon_status
        )
        status_command.start()
        self.addCleanup(status_command.stop)
        no_daemon = mock.patch.object(
            self.dev.subprocess, "Popen", side_effect=AssertionError("unexpected daemon launch")
        )
        no_daemon.start()
        self.addCleanup(no_daemon.stop)

    def configure(self, requested=None, *, env=None, installed=(), **options):
        env = {} if env is None else env
        with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(
            self.dev.shutil,
            "which",
            side_effect=lambda name: f"/bin/{name}" if name in installed else None,
        ), mock.patch.object(
            self.dev,
            "_kache_version",
            return_value=("1.2.2", None),
        ), mock.patch.object(
            self.dev,
            "_usable_sccache",
            return_value=("sccache 0.18.0", None)
            if "sccache" in installed
            else (None, "not installed or not on PATH"),
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value=None):
            selected = self.dev._configure_compiler_cache(requested, **options)
            return selected, os.environ.copy()

    def test_all_cargo_lanes_enable_compiler_cache_setup(self):
        for lane in ("rust", "clippy", "e2e"):
            with self.subTest(lane=lane):
                self.assertTrue(self.dev._cargo_check_active({lane}))

    def test_non_cargo_lane_skips_compiler_cache_setup(self):
        self.assertFalse(self.dev._cargo_check_active({"vitest"}))

    def test_explicit_rustc_wrapper_wins(self):
        with mock.patch("builtins.print") as output:
            selected, env = self.configure("kache", env={"RUSTC_WRAPPER": "custom"})
        output.assert_called_once_with("  Compiler cache: explicit")
        self.assertEqual("explicit", selected)
        self.assertEqual("custom", env["RUSTC_WRAPPER"])

    def test_auto_prefers_qualified_kache(self):
        selected, env = self.configure(installed={"kache", "sccache"})

        self.assertEqual("kache", selected)
        self.assertEqual(
            str(self.dev.Path("/bin/kache").resolve()), env["RUSTC_WRAPPER"]
        )

    def test_explicit_kache_selects_backend(self):
        with mock.patch("builtins.print") as output:
            selected, env = self.configure(
                "kache",
                env={"KACHE_LOG_FILE": "kache=trace"},
                installed={"kache", "sccache"},
            )
        self.assertEqual("kache", selected)
        output.assert_any_call("  Compiler cache: kache 1.2.2")
        self.assertEqual(
            str(self.dev.Path("/bin/kache").resolve()), env["RUSTC_WRAPPER"]
        )
        self.assertEqual("kache=trace", env["KACHE_LOG_FILE"])

    def test_kache_host_support_is_limited_to_darwin_arm64(self):
        with mock.patch.object(self.dev.sys, "platform", "darwin"), mock.patch.object(
            self.dev.platform, "machine", return_value="arm64"
        ):
            self.assertIsNone(self.dev._kache_host_error())

        for system, machine in (("linux", "aarch64"), ("win32", "AMD64"), ("darwin", "x86_64")):
            with self.subTest(system=system, machine=machine), mock.patch.object(
                self.dev.sys, "platform", system
            ), mock.patch.object(self.dev.platform, "machine", return_value=machine):
                error = self.dev._kache_host_error()
                self.assertIn(f"unsupported host {system}/{machine.lower()}", error or "")
                self.assertIn("qualified host is darwin/arm64", error or "")

    def test_explicit_kache_rejects_unsupported_host_before_version_probe(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev, "_kache_host_error", return_value="unsupported host linux/x86_64"
        ), mock.patch.object(self.dev, "_kache_binary", return_value="/bin/kache"), mock.patch.object(
            self.dev, "_kache_version"
        ) as version:
            with self.assertRaisesRegex(SystemExit, "unsupported host linux/x86_64"):
                self.dev._configure_compiler_cache("kache")
        version.assert_not_called()

    def test_auto_reports_none_when_no_backend_is_installed(self):
        with mock.patch("builtins.print") as output:
            selected, env = self.configure(installed=set())
        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)
        output.assert_any_call("  Compiler cache: none")

    def test_sccache_limit_warning_reports_running_server_mismatch(self):
        completed = mock.Mock(
            returncode=0,
            stdout='{"max_cache_size": 21474836480}',
            stderr="",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed):
            warning = self.dev._sccache_limit_warning()

        self.assertIn("20.0 GiB", warning)
        self.assertIn("restart sccache", warning)

    def test_sccache_limit_warning_accepts_effective_limit(self):
        completed = mock.Mock(
            returncode=0,
            stdout='{"max_cache_size": 10737418240}',
            stderr="",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed):
            self.assertIsNone(self.dev._sccache_limit_warning())

    def test_sccache_limit_warning_honors_explicit_limit(self):
        completed = mock.Mock(
            returncode=0,
            stdout='{"max_cache_size": 21474836480}',
            stderr="",
        )
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed):
            self.assertIsNone(self.dev._sccache_limit_warning("20G"))

    def test_sccache_limit_warning_reports_invalid_config(self):
        warning = self.dev._sccache_limit_warning("twenty gigs")
        self.assertIn("unsupported cache size", warning)

    def test_auto_uses_kache_when_sccache_is_unavailable(self):
        with mock.patch.object(self.dev, "_kache_host_error", return_value=None):
            selected, env = self.configure(installed={"kache"})
            self.assertEqual("kache", selected)
            self.assertEqual(
                str(self.dev.Path("/bin/kache").resolve()), env["RUSTC_WRAPPER"]
            )

    def test_auto_falls_back_to_none_when_kache_and_sccache_are_unusable(self):
        selected, env = self.configure(installed=set())
        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)

    def test_none_disables_automatic_wrapper(self):
        selected, env = self.configure("none", installed={"kache", "sccache"})
        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)

    def test_environment_selects_backend(self):
        selected, env = self.configure(
            env={"PHOENIX_COMPILER_CACHE": "kache"}, installed={"kache"}
        )
        self.assertEqual("kache", selected)
        self.assertEqual(str(self.dev.Path("/bin/kache").resolve()), env["RUSTC_WRAPPER"])

    def test_cli_selection_takes_precedence_over_environment(self):
        selected, env = self.configure(
            "sccache",
            env={"PHOENIX_COMPILER_CACHE": "kache"},
            installed={"kache", "sccache"},
        )
        self.assertEqual("sccache", selected)
        self.assertEqual(str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"])

    def test_explicit_sccache_does_not_probe_kache(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil,
            "which",
            side_effect=lambda name: f"/bin/{name}",
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
        ), mock.patch.object(self.dev, "_kache_version") as probe:
            self.assertEqual("sccache", self.dev._configure_compiler_cache("sccache"))
        probe.assert_not_called()

    @unittest.skipUnless(
        sys.platform == "darwin" and platform.machine().lower() == "arm64",
        "Kache v1.2.2 is qualified only on macOS arm64",
    )
    def test_local_kache_binary_is_supported(self):
        with mock.patch.dict(
            os.environ, {"PHOENIX_KACHE_BIN": "/opt/local/kache"}, clear=True
        ), mock.patch.object(self.dev.Path, "is_file", return_value=True), mock.patch.object(
            self.dev.os, "access", return_value=True
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("1.2.2", None)
        ), mock.patch.object(
            self.dev, "_ensure_kache_daemon", return_value=None
        ) as ensure:
            selected = self.dev._configure_compiler_cache("kache")
            self.assertEqual("kache", selected)
            self.assertEqual("/opt/local/kache", os.environ["RUSTC_WRAPPER"])
            ensure.assert_called_once_with("/opt/local/kache", cargo_cwd=None)

    def test_minimal_kache_configuration_gets_generated_socket(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev, "_private_kache_socket_dir", return_value=self.socket_directory
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev, "_start_kache_daemon_locked", return_value=None
        ):
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
            self.assertRegex(
                os.environ["KACHE_SOCKET_PATH"],
                rf"^{self.socket_directory}/local-only-v1\.2\.2/[0-9a-f]{{16}}\.sock$",
            )

    def test_daemon_identity_is_stable_across_worktrees_with_same_effective_inputs(self):
        with mock.patch.dict(
            os.environ, {"KACHE_CACHE_DIR": "/cache/shared"}, clear=True
        ):
            first = self.dev._kache_daemon_identity(
                "/bin/kache", cargo_cwd=Path("/repo-a")
            )
            second = self.dev._kache_daemon_identity(
                "/bin/kache", cargo_cwd=Path("/repo-b")
            )

        self.assertEqual(first, second)

    def test_exact_kache_version_only(self):
        for output, accepted in (("kache 1.2.2", True), ("kache 1.2.1", False),
                                 ("kache 1.2.3", False), ("kache 1.2.2-rc.1", False)):
            with self.subTest(output=output), mock.patch.object(
                self.dev, "_command_version", return_value=(output, None)
            ):
                version, error = self.dev._kache_version("/bin/kache")
                self.assertEqual(version == "1.2.2", accepted)
                self.assertEqual(error is None, accepted)

    def test_forced_local_config_and_isolated_paths_ignore_remote_ambient(self):
        import tomllib

        with self.dev.tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "phoenix"
            root.mkdir()
            remote = Path(temporary) / "remote.toml"
            remote.write_text('[cache]\nlocal_only = false\n')
            ambient = Path(temporary) / "ambient"
            ambient.mkdir()
            (ambient / "queue").write_text("untouched")
            with mock.patch.dict(os.environ, {
                "KACHE_CONFIG": str(remote), "KACHE_HOST_CONFIG": str(remote),
                "KACHE_LOCAL_ONLY": "false", "KACHE_SOCKET_PATH": str(ambient / "daemon.sock"),
                "KACHE_CACHE_DIR": str(ambient), "KACHE_RUNTIME_DIR": str(ambient),
                "KACHE_TRUST_DOMAIN": "remote", "KACHE_REMOTE_URL": "https://remote.invalid",
                "KACHE_VOLUMES": "remote-volume", "KACHE_DISABLED": "0",
                "AWS_ACCESS_KEY_ID": "remote-key",
                "GOOGLE_APPLICATION_CREDENTIALS": str(remote), "AZURE_CLIENT_SECRET": "remote-secret",
            }, clear=True), mock.patch.object(
                self.dev, "_private_kache_socket_dir", return_value=root
            ), mock.patch.object(
                self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
            ), mock.patch.object(
                self.dev, "_effective_kache_socket",
                side_effect=lambda *_a, **_kw: (Path(os.environ["KACHE_SOCKET_PATH"]), None)
            ) as status, mock.patch.object(
                self.dev, "_start_kache_daemon_locked", return_value=None
            ) as start:
                self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
                self.assertEqual((ambient / "queue").read_text(), "untouched")
                config = Path(os.environ["KACHE_CONFIG"])
                self.assertEqual(str(config), os.environ["KACHE_HOST_CONFIG"])
                self.assertEqual(config.parent, root / "local-only-v1.2.2")
                cache = tomllib.loads(config.read_text())["cache"]
                self.assertEqual(cache, {
                    "local_only": True, "ignore_env": True,
                    "local_store": str(self.cache_directory / "store"),
                    "runtime_dir": str(config.parent / "runtime"),
                })
                self.assertEqual(os.environ["KACHE_LOCAL_ONLY"], "true")
                self.assertNotIn("KACHE_DISABLED", os.environ)
                self.assertEqual(os.environ["KACHE_CACHE_DIR"], cache["local_store"])
                self.assertEqual(os.environ["KACHE_RUNTIME_DIR"], cache["runtime_dir"])
                self.assertEqual(Path(os.environ["KACHE_SOCKET_PATH"]).parent, config.parent)
                self.assertFalse(str(cache["local_store"]).startswith(str(root)))
                for path in (cache["local_store"], cache["runtime_dir"]):
                    self.assertTrue(Path(path).is_dir())
                for key in ("KACHE_REMOTE_URL", "KACHE_VOLUMES", "KACHE_TRUST_DOMAIN"):
                    self.assertNotIn(key, os.environ)
                self.assertEqual(os.environ["AWS_ACCESS_KEY_ID"], "remote-key")
                self.assertEqual(os.environ["GOOGLE_APPLICATION_CREDENTIALS"], str(remote))
                self.assertEqual(os.environ["AZURE_CLIENT_SECRET"], "remote-secret")
                self.assertNotEqual(config, remote)
                self.assertEqual(status.call_count, 1)
                self.assertEqual(start.call_count, 1)

    def test_durable_cache_root_is_user_namespaced_and_private(self):
        with self.dev.tempfile.TemporaryDirectory() as home, mock.patch.object(
            self.dev.Path, "home", return_value=Path(home)
        ):
            root = self.real_private_kache_cache_dir()
            self.assertEqual(
                root, Path(home) / ".cache" / "phoenix-ide" / "kache" / "local-only-v1.2.2"
            )
            self.assertEqual(root.stat().st_mode & 0o777, 0o700)

    def test_credential_file_not_read_for_local_identity(self):
        with self.dev.tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "phoenix"
            root.mkdir()
            credentials = Path(temporary) / "credentials"
            credentials.write_text("secret-one")
            with mock.patch.object(self.dev, "_private_kache_socket_dir", return_value=root), \
                 mock.patch.dict(os.environ, {"AWS_SHARED_CREDENTIALS_FILE": str(credentials),
                                           "KACHE_CONFIG": str(credentials)}, clear=True), \
                 mock.patch.object(self.dev, "_file_identity", side_effect=AssertionError("read")):
                before = self.dev._kache_daemon_identity("/bin/kache", cargo_cwd=None)
                credentials.write_text("secret-two")
                os.environ["AWS_ACCESS_KEY_ID"] = "another-aws-key"
                os.environ["AZURE_CLIENT_SECRET"] = "another-azure-secret"
                os.environ["GOOGLE_APPLICATION_CREDENTIALS"] = str(credentials)
                self.assertEqual(before, self.dev._kache_daemon_identity("/bin/kache", cargo_cwd=None))
                self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)
                self.assertEqual(before, self.dev._kache_daemon_identity("/bin/kache", cargo_cwd=None))
                config = Path(os.environ["KACHE_CONFIG"])
                content = config.read_text()
                self.assertNotIn("secret", content)
                self.assertNotIn("aws-key", content)
                self.assertNotIn("AZURE_", content)
                self.assertNotIn("GOOGLE_", content)
                os.environ["AWS_ACCESS_KEY_ID"] = "third-aws-key"
                os.environ["AZURE_CLIENT_SECRET"] = "third-azure-secret"
                self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)
                self.assertEqual(content, config.read_text())
                self.assertEqual(before, self.dev._kache_daemon_identity("/bin/kache", cargo_cwd=None))
                self.assertEqual(os.environ["AWS_SHARED_CREDENTIALS_FILE"], str(credentials))
                self.assertEqual(os.environ["AWS_ACCESS_KEY_ID"], "third-aws-key")
                self.assertEqual(os.environ["AZURE_CLIENT_SECRET"], "third-azure-secret")
                self.assertEqual(os.environ["GOOGLE_APPLICATION_CREDENTIALS"], str(credentials))

    def test_local_config_is_durable_before_selection(self):
        synced_modes = []
        real_fsync = self.dev.os.fsync

        def fsync(descriptor):
            synced_modes.append(self.dev.os.fstat(descriptor).st_mode)
            real_fsync(descriptor)

        with mock.patch.object(self.dev.os, "fsync", side_effect=fsync):
            self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)

        self.assertEqual(2, len(synced_modes))
        self.assertTrue(self.dev.stat.S_ISREG(synced_modes[0]))
        self.assertTrue(self.dev.stat.S_ISDIR(synced_modes[1]))
        self.assertEqual(0o600, Path(os.environ["KACHE_CONFIG"]).stat().st_mode & 0o777)

    def test_local_config_retry_completes_failed_directory_durability_barrier(self):
        real_fsync = self.dev.os.fsync
        first_modes = []

        def fail_directory_fsync(descriptor):
            mode = self.dev.os.fstat(descriptor).st_mode
            first_modes.append(mode)
            if self.dev.stat.S_ISDIR(mode):
                raise OSError("directory sync failed")
            real_fsync(descriptor)

        with mock.patch.object(
            self.dev.os, "fsync", side_effect=fail_directory_fsync
        ), self.assertRaisesRegex(OSError, "directory sync failed"):
            self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)
        self.assertNotIn("KACHE_CONFIG", os.environ)

        retry_modes = []

        def record_fsync(descriptor):
            retry_modes.append(self.dev.os.fstat(descriptor).st_mode)
            real_fsync(descriptor)

        with mock.patch.object(self.dev.os, "fsync", side_effect=record_fsync):
            self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)

        self.assertTrue(any(self.dev.stat.S_ISDIR(mode) for mode in first_modes))
        self.assertEqual(1, len(retry_modes))
        self.assertTrue(self.dev.stat.S_ISDIR(retry_modes[0]))
        self.assertIn("KACHE_CONFIG", os.environ)

    def test_local_config_reuse_rejects_unsafe_mode_and_non_regular_file(self):
        self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)
        config = Path(os.environ["KACHE_CONFIG"])

        config.chmod(0o644)
        with self.assertRaisesRegex(
            self.dev.KacheLocalConfigError, "unsafe kache local config"
        ):
            self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)

        config.unlink()
        config.mkdir()
        with self.assertRaisesRegex(
            self.dev.KacheLocalConfigError, "unsafe kache local config"
        ):
            self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)

    def test_local_config_reuse_rejects_foreign_owner(self):
        self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)
        config = Path(os.environ["KACHE_CONFIG"])
        path_type = type(config)
        real_lstat = path_type.lstat

        def lstat(path):
            info = real_lstat(path)
            if path != config:
                return info
            fields = list(info)
            fields[4] = os.getuid() + 1
            return os.stat_result(fields)

        with mock.patch.object(
            path_type, "lstat", autospec=True, side_effect=lstat
        ), self.assertRaisesRegex(
            self.dev.KacheLocalConfigError, "unsafe kache local config"
        ):
            self.dev._prepare_local_kache("/bin/kache", cargo_cwd=None)

    def test_non_object_daemon_receipt_is_not_trusted(self):
        with self.dev.tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            socket = root / "operator.sock"
            socket.write_text("")
            with mock.patch.object(
                self.dev, "_private_kache_socket_dir", return_value=root
            ):
                self.dev._kache_daemon_receipt_path(socket).write_text("[]")
                self.assertFalse(
                    self.dev._kache_daemon_receipt_matches(socket, "a" * 64, 1234)
                )

    def test_daemon_receipt_contains_only_digest_pid_and_socket_incarnation(self):
        with self.dev.tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            socket = root / "operator.sock"
            socket.write_text("")
            with mock.patch.object(
                self.dev, "_private_kache_socket_dir", return_value=root
            ):
                self.assertIsNone(
                    self.dev._record_kache_daemon_receipt(
                        socket, "a" * 64, 1234
                    )
                )
                receipt_path = self.dev._kache_daemon_receipt_path(socket)
                receipt = receipt_path.read_text()

            self.assertNotIn("secret-access-key", receipt)
            self.assertEqual(0o600, receipt_path.stat().st_mode & 0o777)
            parsed = self.dev.json.loads(receipt)
            self.assertEqual(
                {"identity", "serving_pid", "socket_device", "socket_inode", "version"},
                set(parsed),
            )

    def test_daemon_receipt_rejects_missing_socket_receipt_or_wrong_process(self):
        socket = Path(self.store_root.name) / "operator.sock"
        self.assertFalse(self.dev._kache_daemon_receipt_matches(socket, "a" * 64, 1234))
        socket.write_text("mock socket")
        self.assertFalse(self.dev._kache_daemon_receipt_matches(socket, "a" * 64, 1234))
        self.assertIsNone(self.dev._record_kache_daemon_receipt(socket, "a" * 64, 1234))
        self.assertFalse(self.dev._kache_daemon_receipt_matches(socket, "b" * 64, 1234))
        self.assertFalse(self.dev._kache_daemon_receipt_matches(socket, "a" * 64, 5678))
        socket.unlink()
        self.assertFalse(self.dev._kache_daemon_receipt_matches(socket, "a" * 64, 1234))

    def test_daemon_receipt_rejects_socket_incarnation_change(self):
        with self.dev.tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            socket = root / "operator.sock"
            socket.write_text("first")
            with mock.patch.object(
                self.dev, "_private_kache_socket_dir", return_value=root
            ):
                self.assertIsNone(
                    self.dev._record_kache_daemon_receipt(socket, "a" * 64, 1234)
                )
                self.assertTrue(
                    self.dev._kache_daemon_receipt_matches(socket, "a" * 64, 1234)
                )
                replacement = root / "replacement.sock"
                replacement.write_text("second")
                replacement.replace(socket)
                self.assertFalse(
                    self.dev._kache_daemon_receipt_matches(socket, "a" * 64, 1234)
                )

    def test_effective_socket_omission_or_status_failure_never_starts_daemon(self):
        for status in (
            mock.Mock(returncode=0, stdout='{"daemon_running":false}', stderr=""),
            mock.Mock(returncode=2, stdout="", stderr="config denied"),
        ):
            with self.subTest(status=status.stdout), mock.patch.dict(
                os.environ, {"KACHE_SOCKET_PATH": str(self.socket_directory / "operator.sock")},
                clear=True,
            ), mock.patch.object(
                self.dev.subprocess, "run", return_value=status
            ), mock.patch.object(self.dev.subprocess, "Popen") as spawn, mock.patch.object(
                self.dev, "_kache_socket_lock"
            ) as lock:
                error = self.dev._ensure_kache_daemon("/bin/kache")
            self.assertEqual(
                "config denied" if status.returncode else "daemon status omitted effective socket",
                error,
            )
            spawn.assert_not_called()
            lock.assert_not_called()

    def test_socket_status_failure_is_secret_safe_and_preserves_selection_policy(self):
        secret = "mock-aws-secret"
        for requested in ("auto", "kache"):
            with self.subTest(requested=requested), mock.patch.dict(
                os.environ,
                {"AWS_SECRET_ACCESS_KEY": secret, "KACHE_TRUST_DOMAIN": "original-domain"},
                clear=True,
            ), mock.patch.object(
                self.dev, "_kache_host_error", return_value=None
            ), mock.patch.object(
                self.dev, "_kache_binary", return_value="/bin/kache"
            ), mock.patch.object(
                self.dev, "_kache_version", return_value=("1.2.2", None)
            ), mock.patch.object(
                self.dev.shutil, "which", return_value="/bin/sccache"
            ), mock.patch.object(
                self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
            ), mock.patch.object(
                self.dev.subprocess, "run",
                return_value=mock.Mock(returncode=2, stdout="", stderr=f"config denied: {secret}"),
            ), mock.patch("builtins.print") as output:
                if requested == "auto":
                    self.assertEqual("sccache", self.dev._configure_compiler_cache(requested))
                    self.assertEqual("original-domain", os.environ["KACHE_TRUST_DOMAIN"])
                    self.assertEqual(secret, os.environ["AWS_SECRET_ACCESS_KEY"])
                    self.assertNotIn("KACHE_SOCKET_PATH", os.environ)
                    detail = str(output.call_args_list)
                else:
                    with self.assertRaises(SystemExit) as failure:
                        self.dev._configure_compiler_cache(requested)
                    detail = str(failure.exception)
                    self.assertIn("kache daemon failed to start", detail)
                    self.assertNotIn("RUSTC_WRAPPER", os.environ)
                    self.assertEqual(secret, os.environ["AWS_SECRET_ACCESS_KEY"])
                    self.assertEqual("original-domain", os.environ["KACHE_TRUST_DOMAIN"])
                self.assertNotIn(secret, detail)
                self.assertIn("[redacted]", detail)

    def test_effective_pr_scoped_socket_drives_ownership(self):
        scoped = Path("/tmp/kache-pr-deadbeef.sock")
        with mock.patch.dict(
            os.environ,
            {"GITHUB_EVENT_NAME": "pull_request", "KACHE_PULL_REQUEST_PREFIX": "prs"},
            clear=True,
        ), mock.patch.object(
            self.dev,
            "_read_kache_daemon_status",
            return_value=(self.dev.KacheDaemonStatus.ABSENT, str(scoped), None),
        ):
            actual, error = self.dev._effective_kache_socket(
                "/bin/kache", cargo_cwd=Path("/repo")
            )

        self.assertIsNone(error)
        self.assertEqual(scoped.resolve(), actual)

    def test_bounded_startup_diagnostic_redacts_environment_secrets(self):
        secret = "super-secret-value"
        rendered = self.dev._safe_kache_startup_diagnostic(
            f"invalid credential {secret}\ntoken=unregistered-secret\nhttps://user:pass@example.test\n"
            + "x" * 10000,
            environment={"AWS_SECRET_ACCESS_KEY": secret},
        )
        self.assertNotIn(secret, rendered)
        self.assertNotIn("unregistered-secret", rendered)
        self.assertNotIn("user:pass", rendered)
        self.assertLessEqual(len(rendered), 2048)
        self.assertIn("[redacted", rendered)

    def test_startup_diagnostic_redacts_kache_access_key_and_quoted_values(self):
        access_key = "fixture-access-key"
        rendered = self.dev._safe_kache_startup_diagnostic(
            f'invalid {access_key}\npassword = "two word secret"\n'
            "token = '''multiline\nsecret'''\n",
            environment={"KACHE_S3_ACCESS_KEY": access_key},
        )
        self.assertNotIn(access_key, rendered)
        self.assertNotIn("two word secret", rendered)
        self.assertNotIn("multiline", rendered)
        self.assertEqual("[redacted backend diagnostic]", rendered)

    def test_startup_diagnostic_redacts_quoted_key_and_escaped_value(self):
        rendered = self.dev._safe_kache_startup_diagnostic(
            '"token" = "first\\\" fixture-escaped-suffix"',
            environment={},
        )
        self.assertEqual("[redacted backend diagnostic]", rendered)

    def test_startup_diagnostic_redacts_escaped_key_and_numbered_excerpt(self):
        for diagnostic in (
            '"to\\u006ben" = "fixture-escaped-key-secret"',
            '1 | "access\\u005fkey" = "fixture-access-credential"',
        ):
            with self.subTest(diagnostic=diagnostic):
                self.assertEqual(
                    "[redacted backend diagnostic]",
                    self.dev._safe_kache_startup_diagnostic(
                        diagnostic, environment={}
                    ),
                )

    def test_environment_secret_redaction_is_order_independent(self):
        secret = "fixture-secret-2-suffix"
        rendered = self.dev._safe_kache_startup_diagnostic(
            f"config denied: {secret}",
            environment={
                "AWS_MAX_ATTEMPTS": "2",
                "AWS_SECRET_ACCESS_KEY": secret,
            },
        )
        self.assertEqual("config denied: [redacted]", rendered)

    def test_daemon_start_receipts_only_the_spawned_serving_process(self):
        child = mock.Mock(pid=1234)
        child.poll.return_value = None
        child.wait.return_value = -15
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ) as spawn, mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=4321
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt"
        ) as record:
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )

        self.assertIn("different process won the daemon bind race", error or "")
        self.assertIn("spawned Kache child pid 1234 is no longer running", error or "")
        self.assertEqual(["/bin/kache", "daemon", "run"], spawn.call_args.args[0])
        child.terminate.assert_called_once_with()
        child.wait.assert_called_once_with(timeout=1.0)
        record.assert_not_called()

    def test_live_child_after_readiness_failure_is_settled(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.return_value = -15
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value="readiness timed out"
        ):
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )
        child.terminate.assert_called_once_with()
        child.kill.assert_not_called()
        child.wait.assert_called_once_with(timeout=1.0)
        self.assertIn("readiness timed out", error or "")
        self.assertIn("spawned Kache child pid 2468 is no longer running", error or "")
        self.assertNotIn("may still be live", error or "")

    def test_readiness_failure_escalates_only_spawned_child(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.side_effect = [self.dev.subprocess.TimeoutExpired("kache", 1.0), -9]
        settlement = self.dev._settle_spawned_kache_child(child)
        self.assertIsNone(settlement)
        child.terminate.assert_called_once_with()
        child.kill.assert_called_once_with()
        self.assertEqual(
            [mock.call(timeout=1.0), mock.call(timeout=1.0)],
            child.wait.call_args_list,
        )

    def test_readiness_failure_reports_owned_child_settlement_failure(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.side_effect = self.dev.subprocess.TimeoutExpired("kache", 1.0)
        settlement = self.dev._settle_spawned_kache_child(child)
        self.assertIn("pid 2468 remained live after SIGKILL", settlement or "")

    def test_long_readiness_failure_retains_child_settlement(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.return_value = -15
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value="x" * 2500
        ):
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )
        rendered = self.dev._safe_kache_startup_diagnostic(error or "", environment={})
        self.assertLessEqual(len(rendered), 2048)
        self.assertIn("spawned Kache child pid 2468 is no longer running", rendered)
        self.assertIn("after readiness failure", rendered)

    def test_credential_readiness_failure_retains_child_settlement(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.return_value = -15
        diagnostic = '1 | "to\\u006ben" = "fixture-secret"\n' + "x" * 2500
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=diagnostic
        ):
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )
        self.assertNotIn("fixture-secret", error or "")
        self.assertIn("[redacted backend diagnostic]", error or "")
        self.assertIn("spawned Kache child pid 2468 is no longer running", error or "")

    def test_diagnostic_thread_failure_settles_child(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.return_value = -15
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev.threading.Thread, "start", side_effect=RuntimeError("thread unavailable")
        ):
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )
        self.assertIn("thread unavailable", error or "")
        self.assertIn(
            "pid 2468 is no longer running after diagnostic setup failure", error or ""
        )
        child.terminate.assert_called_once_with()
        child.wait.assert_called_once_with(timeout=1.0)
        child.stderr.close.assert_called_once_with()

    def test_interrupt_surfaces_child_settlement_failure(self):
        for interruption_at in ("diagnostic", "readiness"):
            with self.subTest(interruption_at=interruption_at):
                child = mock.Mock(pid=2468)
                child.poll.return_value = None
                patches = [
                    mock.patch.object(
                        self.dev, "_kache_daemon_is_running", return_value=(False, None)
                    ),
                    mock.patch.object(self.dev.subprocess, "Popen", return_value=child),
                    mock.patch.object(
                        self.dev,
                        "_settle_spawned_kache_child",
                        return_value="spawned Kache child pid 2468 remained live after SIGKILL",
                    ),
                    mock.patch.object(
                        self.dev.threading.Thread,
                        "start",
                        side_effect=KeyboardInterrupt
                        if interruption_at == "diagnostic"
                        else None,
                    ),
                    mock.patch.object(
                        self.dev,
                        "_wait_for_kache_daemon",
                        side_effect=KeyboardInterrupt
                        if interruption_at == "readiness"
                        else AssertionError("readiness must not run"),
                    ),
                ]
                with contextlib.ExitStack() as stack:
                    for patch in patches:
                        stack.enter_context(patch)
                    with self.assertRaises(KeyboardInterrupt) as raised:
                        self.dev._start_kache_daemon_locked(
                            "/bin/kache",
                            cargo_cwd=Path("/repo"),
                            socket=Path("/tmp/kache.sock"),
                            identity="identity",
                        )
                self.assertIn("remained live after SIGKILL", " ".join(raised.exception.__notes__))
                child.stderr.close.assert_not_called()

    def test_interrupt_in_unready_failure_handling_settles_child(self):
        child = mock.Mock(pid=2468)
        child.poll.side_effect = KeyboardInterrupt
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev.threading.Thread, "start", return_value=None
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value="not ready"
        ), mock.patch.object(
            self.dev, "_remove_kache_daemon_receipt", return_value=None
        ) as remove, mock.patch.object(
            self.dev, "_settle_spawned_kache_child", return_value=None
        ) as settle:
            with self.assertRaises(KeyboardInterrupt):
                self.dev._start_kache_daemon_locked(
                    "/bin/kache",
                    cargo_cwd=Path("/repo"),
                    socket=Path("/tmp/kache.sock"),
                    identity="identity",
                )

        remove.assert_called_once_with(Path("/tmp/kache.sock"))
        settle.assert_called_once_with(child)

    def test_interrupt_during_diagnostic_construction_settles_child(self):
        child = mock.Mock(pid=2468)
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev.threading, "Thread", side_effect=KeyboardInterrupt
        ), mock.patch.object(
            self.dev, "_remove_kache_daemon_receipt", return_value=None
        ) as remove, mock.patch.object(
            self.dev, "_settle_spawned_kache_child", return_value=None
        ) as settle:
            with self.assertRaises(KeyboardInterrupt):
                self.dev._start_kache_daemon_locked(
                    "/bin/kache",
                    cargo_cwd=Path("/repo"),
                    socket=Path("/tmp/kache.sock"),
                    identity="identity",
                )

        remove.assert_called_once_with(Path("/tmp/kache.sock"))
        settle.assert_called_once_with(child)

    def test_system_exit_prints_child_ownership_uncertainty(self):
        child = mock.Mock(pid=2468)
        child.stderr = mock.Mock()
        interruption = SystemExit(7)
        with mock.patch.object(
            self.dev, "_remove_kache_daemon_receipt", return_value=None
        ), mock.patch.object(
            self.dev,
            "_settle_spawned_kache_child",
            return_value="spawned Kache child pid 2468 remained live after SIGKILL",
        ), mock.patch("builtins.print") as output:
            self.dev._cleanup_interrupted_kache_start(
                child, Path("/tmp/kache.sock"), interruption
            )

        self.assertIn("remained live after SIGKILL", " ".join(interruption.__notes__))
        self.assertIn("remained live after SIGKILL", output.call_args.args[0])
        self.assertIs(output.call_args.kwargs["file"], sys.stderr)
        child.stderr.close.assert_not_called()

    def test_interrupt_after_receipt_removes_receipt_and_settles_child(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        with mock.patch.object(
            self.dev,
            "_kache_daemon_is_running",
            side_effect=[(False, None), KeyboardInterrupt],
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev.threading.Thread, "start", return_value=None
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=2468
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt", return_value=None
        ) as record, mock.patch.object(
            self.dev, "_remove_kache_daemon_receipt", return_value=None
        ) as remove, mock.patch.object(
            self.dev, "_settle_spawned_kache_child", return_value=None
        ) as settle:
            with self.assertRaises(KeyboardInterrupt):
                self.dev._start_kache_daemon_locked(
                    "/bin/kache",
                    cargo_cwd=Path("/repo"),
                    socket=Path("/tmp/kache.sock"),
                    identity="identity",
                )

        record.assert_called_once()
        remove.assert_called_once_with(Path("/tmp/kache.sock"))
        settle.assert_called_once_with(child)
        child.stderr.close.assert_not_called()

    def test_live_child_after_receipt_failure_is_settled(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.return_value = -15
        with mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(False, None)
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=2468
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt", return_value="receipt write failed"
        ):
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )
        self.assertIn("receipt write failed", error or "")
        self.assertIn("spawned Kache child pid 2468 is no longer running", error or "")
        child.terminate.assert_called_once_with()
        child.wait.assert_called_once_with(timeout=1.0)

    def test_child_exit_after_receipt_fails_closed_and_removes_receipt(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = 7
        with mock.patch.object(
            self.dev,
            "_kache_daemon_is_running",
            side_effect=[(False, None), (True, None)],
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=2468
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt", return_value=None
        ) as record, mock.patch.object(
            self.dev, "_remove_kache_daemon_receipt", return_value=None
        ) as remove:
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )

        self.assertIn("cannot confirm spawned Kache child ownership", error or "")
        record.assert_called_once()
        remove.assert_called_once_with(Path("/tmp/kache.sock"))
        child.terminate.assert_not_called()

    def test_serving_pid_change_after_receipt_fails_closed_and_settles_child(self):
        child = mock.Mock(pid=2468)
        child.poll.return_value = None
        child.wait.return_value = -15
        with mock.patch.object(
            self.dev,
            "_kache_daemon_is_running",
            side_effect=[(False, None), (True, None)],
        ), mock.patch.object(
            self.dev.subprocess, "Popen", return_value=child
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", side_effect=[2468, 9999]
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt", return_value=None
        ) as record, mock.patch.object(
            self.dev, "_remove_kache_daemon_receipt", return_value=None
        ) as remove:
            error = self.dev._start_kache_daemon_locked(
                "/bin/kache",
                cargo_cwd=Path("/repo"),
                socket=Path("/tmp/kache.sock"),
                identity="identity",
            )

        self.assertIn("cannot confirm spawned Kache child ownership", error or "")
        record.assert_called_once()
        remove.assert_called_once_with(Path("/tmp/kache.sock"))
        child.terminate.assert_called_once_with()
        child.wait.assert_called_once_with(timeout=1.0)

    def test_malformed_utf8_proof_files_are_unverifiable(self):
        with self.dev.tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            socket = root / "kache.sock"
            socket.write_text("")
            socket.with_suffix(".state.json").write_bytes(b"\xff")
            with mock.patch.object(
                self.dev, "_private_kache_socket_dir", return_value=root
            ):
                self.dev._kache_daemon_receipt_path(socket).write_bytes(b"\xff")
                self.assertFalse(
                    self.dev._kache_daemon_receipt_matches(socket, "identity", 1234)
                )
                self.assertIsNone(self.dev._kache_daemon_serving_pid(socket))

    def test_pathological_numeric_proof_files_are_unverifiable(self):
        with self.dev.tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            socket = root / "kache.sock"
            socket.write_text("")
            invalid = b'{"phase":"ready","pid":' + b"9" * 4301 + b"}"
            socket.with_suffix(".state.json").write_bytes(invalid)
            with mock.patch.object(
                self.dev, "_private_kache_socket_dir", return_value=root
            ):
                self.dev._kache_daemon_receipt_path(socket).write_bytes(invalid)
                self.assertFalse(
                    self.dev._kache_daemon_receipt_matches(socket, "identity", 1234)
                )
                self.assertIsNone(self.dev._kache_daemon_serving_pid(socket))

    def test_full_daemon_boundary_preserves_lifecycle_facts_for_equals_socket(self):
        socket = Path("/tmp/run=one.sock")
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": str(socket)}, clear=True
        ), mock.patch.object(
            self.dev, "_effective_kache_socket", return_value=(socket, None)
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev,
            "_start_kache_daemon_locked",
            return_value=(
                "[redacted backend diagnostic]; spawned Kache child pid 2468 may still be "
                f"live on {socket} without a Phoenix ownership receipt"
            ),
        ):
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertIn("pid 2468 may still be live", error or "")
        self.assertIn(str(socket), error or "")
        self.assertIn("without a Phoenix ownership receipt", error or "")

    def test_second_status_failure_is_sanitized_at_full_daemon_boundary(self):
        socket = Path("/tmp/run=one.sock")
        secret = "fixture-secret"
        with mock.patch.dict(
            os.environ,
            {
                "KACHE_SOCKET_PATH": str(socket),
                "AWS_SECRET_ACCESS_KEY": secret,
            },
            clear=True,
        ), mock.patch.object(
            self.dev, "_effective_kache_socket", return_value=(socket, None)
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev,
            "_kache_daemon_is_running",
            return_value=(False, f'1 | "to\\u006ben" = "{secret}"'),
        ), mock.patch.object(self.dev.subprocess, "Popen") as spawn:
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertNotIn(secret, error or "")
        self.assertIn("[redacted backend diagnostic]", error or "")
        spawn.assert_not_called()

    def test_operator_socket_reuses_only_matching_phoenix_started_daemon(self):
        socket = Path("/tmp/operator.sock")
        events = []

        def status(*_args, **_kwargs):
            return (bool(events), None)

        def launch(*_args, **_kwargs):
            events.append("started")
            child = mock.Mock(pid=1234)
            child.poll.return_value = None
            return child

        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": str(socket)}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", side_effect=status
        ), mock.patch.object(
            self.dev.subprocess, "Popen", side_effect=launch
        ) as run, mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=1234
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt", return_value=None
        ) as record, mock.patch.object(
            self.dev, "_read_kache_daemon_status",
            return_value=(self.dev.KacheDaemonStatus.ABSENT, str(socket), None)
        ), mock.patch.object(
            self.dev, "_kache_daemon_receipt_matches", return_value=True
        ) as matches:
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))

        self.assertEqual(1, run.call_count)
        record.assert_called_once_with(mock.ANY, mock.ANY, 1234)
        matches.assert_called_once_with(mock.ANY, mock.ANY, 1234)

    def test_operator_socket_rejects_replaced_daemon(self):
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/operator.sock"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(True, None)
        ), mock.patch.object(
            self.dev, "_kache_daemon_receipt_matches", return_value=False
        ), mock.patch.object(self.dev.subprocess, "Popen") as run:
            error = self.dev._ensure_kache_daemon("/bin/kache")

        self.assertIn("environment or process identity cannot be verified", error or "")
        run.assert_not_called()

    def test_generated_socket_reuses_running_daemon(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev, "_generated_kache_socket", return_value=Path("/tmp/kache.sock")
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", return_value=contextlib.nullcontext()
        ), mock.patch.object(
            self.dev, "_start_kache_daemon_locked", return_value=None
        ) as start:
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))

        start.assert_called_once()
        self.assertEqual("/bin/kache", start.call_args.args[0])
        self.assertEqual(Path("/tmp/kache.sock").resolve(), start.call_args.kwargs["socket"])
        self.assertRegex(start.call_args.kwargs["identity"], r"^[0-9a-f]{64}$")

    def test_kache_lock_setup_failure_is_actionable(self):
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/unwritable/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", side_effect=OSError("permission denied")
        ):
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertEqual("cannot lock Kache socket setup: permission denied", error)

    def test_kache_lock_creation_failure_propagates(self):
        with mock.patch.object(
            self.dev.Path, "mkdir", side_effect=OSError("read-only filesystem")
        ):
            with self.assertRaisesRegex(OSError, "read-only filesystem"):
                with self.dev._kache_socket_lock(Path("/tmp/kache.sock")):
                    self.fail("lock body must not run")

    def test_kache_lock_acquisition_failure_closes_descriptor(self):
        with mock.patch.object(self.dev.os, "open", return_value=42), mock.patch.object(
            self.dev.fcntl, "flock", side_effect=OSError("lock unavailable")
        ), mock.patch.object(self.dev.os, "close") as close:
            with self.assertRaisesRegex(OSError, "lock unavailable"):
                with self.dev._kache_socket_lock(Path("/tmp/kache.sock")):
                    self.fail("lock body must not run")
        close.assert_called_once_with(42)

    def test_kache_daemon_holds_socket_lock_across_check_start_and_readiness(self):
        child = mock.Mock(pid=1234)
        child.poll.return_value = None
        events = []

        @contextlib.contextmanager
        def lock(_socket):
            events.append("lock")
            yield
            events.append("unlock")

        def status(*_args, **_kwargs):
            events.append("check")
            return events.count("check") > 1, None

        def start(*_args, **_kwargs):
            events.append("start")
            return child

        def ready(*_args, **_kwargs):
            events.append("ready")
            return None

        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_socket_lock", side_effect=lock
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", side_effect=status
        ), mock.patch.object(
            self.dev.subprocess, "Popen", side_effect=start
        ), mock.patch.object(
            self.dev, "_wait_for_kache_daemon", side_effect=ready
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=1234
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt", return_value=None
        ):
            self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
        self.assertEqual(
            ["lock", "check", "start", "ready", "check", "unlock"], events
        )

    @unittest.skipUnless("fork" in multiprocessing.get_all_start_methods(), "requires fork")
    def test_kache_daemon_rejects_running_process_with_unverifiable_environment(self):
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev, "_kache_daemon_is_running", return_value=(True, None)
        ), mock.patch.object(self.dev.subprocess, "Popen") as run:
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertIn("environment or process identity cannot be verified", error or "")
        run.assert_not_called()

    def test_kache_daemon_rejects_unknown_existing_state(self):
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev,
            "_kache_daemon_is_running",
            return_value=(False, "daemon readiness response had an invalid shape"),
        ), mock.patch.object(self.dev.subprocess, "Popen") as run:
            error = self.dev._ensure_kache_daemon("/bin/kache")
        self.assertIn("cannot verify existing daemon environment", error or "")
        run.assert_not_called()

    def test_kache_readiness_polls_until_running(self):
        starting = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":false,"socket":null}',
            stderr="",
        )
        running = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":true,"socket":"/tmp/kache.sock"}',
            stderr="",
        )
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", side_effect=(starting, running)
        ), mock.patch.object(self.dev.time, "sleep"):
            self.assertIsNone(self.dev._wait_for_kache_daemon("/bin/kache", timeout=1))

    def test_kache_readiness_sleeps_after_every_retryable_status(self):
        malformed = mock.Mock(returncode=0, stdout="not-json", stderr="")
        absent = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":false,"socket":"/tmp/kache.sock"}',
            stderr="",
        )
        running = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":true,"socket":"/tmp/kache.sock"}',
            stderr="",
        )
        clock = iter((0.0, 0.0, 0.1, 0.2))
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/kache.sock"}, clear=True
        ), mock.patch.object(
            self.dev.subprocess, "run", side_effect=(malformed, absent, running)
        ), mock.patch.object(
            self.dev.time, "monotonic", side_effect=lambda: next(clock)
        ), mock.patch.object(self.dev.time, "sleep") as sleep:
            self.assertIsNone(self.dev._wait_for_kache_daemon("/bin/kache", timeout=1))
        self.assertEqual([mock.call(0.05), mock.call(0.05)], sleep.call_args_list)

    def test_kache_status_nonzero_is_error_not_absence(self):
        status = mock.Mock(returncode=2, stdout="", stderr="socket permission denied")
        with mock.patch.object(self.dev.subprocess, "run", return_value=status):
            state, socket, error = self.dev._read_kache_daemon_status(
                "/bin/kache", cargo_cwd=None
            )
        self.assertIs(state, self.dev.KacheDaemonStatus.ERROR)
        self.assertIsNone(socket)
        self.assertEqual("socket permission denied", error)

    def test_kache_readiness_replaces_undecodable_output(self):
        status = mock.Mock(returncode=0, stdout="\ufffd", stderr="")
        clock = iter((0.0, 0.0, 1.0))
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", return_value=status
        ) as run, mock.patch.object(
            self.dev.time, "monotonic", side_effect=lambda: next(clock)
        ), mock.patch.object(self.dev.time, "sleep"):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=0.5)
        self.assertIn("Expecting value", error or "")
        self.assertEqual("replace", run.call_args.kwargs["errors"])

    def test_kache_readiness_rejects_non_object_status(self):
        status = mock.Mock(returncode=0, stdout="null", stderr="")
        clock = iter((0.0, 0.0, 1.0))
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", return_value=status
        ), mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: next(clock)), mock.patch.object(
            self.dev.time, "sleep"
        ):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=0.5)
        self.assertEqual("daemon readiness response had an invalid shape", error)

    def test_kache_readiness_rejects_non_string_socket(self):
        status = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":true,"socket":[]}',
            stderr="",
        )
        clock = iter((0.0, 0.0, 1.0))
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.subprocess, "run", return_value=status
        ), mock.patch.object(self.dev.time, "monotonic", side_effect=lambda: next(clock)), mock.patch.object(
            self.dev.time, "sleep"
        ):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=0.5)
        self.assertEqual("daemon readiness socket was not a string", error)

    def test_kache_readiness_rejects_wrong_socket(self):
        running = mock.Mock(
            returncode=0,
            stdout='{"daemon_running":true,"socket":"/tmp/other.sock"}',
            stderr="",
        )
        with mock.patch.dict(
            os.environ, {"KACHE_SOCKET_PATH": "/tmp/expected.sock"}, clear=True
        ), mock.patch.object(self.dev.subprocess, "run", return_value=running):
            error = self.dev._wait_for_kache_daemon("/bin/kache", timeout=1)
        self.assertIn("unexpected socket", error or "")

    def test_kache_readiness_rejects_ready_state_from_other_process(self):
        socket = Path("/tmp/expected.sock")
        with mock.patch.object(
            self.dev,
            "_read_kache_daemon_status",
            return_value=(self.dev.KacheDaemonStatus.RUNNING, str(socket), None),
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=9999
        ), mock.patch.object(self.dev.time, "sleep") as sleep:
            error = self.dev._wait_for_kache_daemon(
                "/bin/kache",
                expected_socket=socket,
                expected_pid=2468,
                timeout=1,
            )

        self.assertIn("different process", error or "")
        self.assertIn("expected pid 2468, found 9999", error or "")
        sleep.assert_not_called()

    def test_daemon_socket_uses_private_owned_directory(self):
        completed = mock.Mock(returncode=0, stdout="", stderr="")
        with self.subTest("socket path and permissions"), mock.patch.dict(
            os.environ, {"KACHE_CACHE_DIR": "/very/long/worktree/cache"}, clear=True
        ), mock.patch.object(
            self.dev,
            "_kache_daemon_is_running",
            side_effect=[(False, None), (True, None)],
        ), mock.patch.object(
            self.dev.subprocess, "Popen"
        ) as run, mock.patch.object(
            self.dev, "_wait_for_kache_daemon", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_daemon_serving_pid", return_value=1234
        ), mock.patch.object(
            self.dev, "_record_kache_daemon_receipt", return_value=None
        ):
            child = mock.Mock(pid=1234)
            child.poll.return_value = None
            run.return_value = child
            with self.dev.tempfile.TemporaryDirectory() as temporary, mock.patch.object(
                self.dev.tempfile, "gettempdir", return_value=temporary
            ), mock.patch.object(
                self.dev,
                "_private_kache_socket_dir",
                wraps=self.real_private_kache_socket_dir,
            ):
                self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))

                socket_path = Path(os.environ["KACHE_SOCKET_PATH"])
                self.assertEqual(
                    socket_path.parent,
                    Path("/tmp") / f"pk-{os.getuid()}" / "local-only-v1.2.2",
                )
                self.assertLess(len(str(socket_path.resolve()).encode()), 104)
                self.assertEqual(socket_path.parent.stat().st_mode & 0o777, 0o700)
                self.assertRegex(socket_path.name, r"^[0-9a-f]{16}\.sock$")
            run.assert_called_once()

    def test_auto_falls_back_to_usable_sccache_when_kache_is_unavailable(self):
        selected, env = self.configure(installed={"sccache"})
        self.assertEqual("sccache", selected)
        self.assertEqual(
            str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"]
        )

    def test_auto_falls_back_to_usable_sccache_when_kache_is_incompatible(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: f"/bin/{name}"
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=(None, "unsupported kache 0.9.0")
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
        ):
            selected = self.dev._configure_compiler_cache("auto")
            env = os.environ.copy()

        self.assertEqual("sccache", selected)
        self.assertEqual(
            str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"]
        )

    def test_auto_falls_back_to_usable_sccache_when_kache_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: f"/bin/{name}"
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("1.2.2", None)
        ), mock.patch.object(
            self.dev, "_ensure_kache_daemon", return_value="socket failed"
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
        ):
            selected = self.dev._configure_compiler_cache("auto")
            env = os.environ.copy()

        self.assertEqual("sccache", selected)
        self.assertEqual(
            str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"]
        )
        self.assertNotIn("KACHE_SOCKET_PATH", env)
        self.assertNotIn("KACHE_CONFIG", env)

    def test_auto_falls_back_to_none_when_kache_daemon_and_sccache_fail(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: f"/bin/{name}"
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("1.2.2", None)
        ), mock.patch.object(
            self.dev, "_ensure_kache_daemon", return_value="socket failed"
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=(None, "bad architecture")
        ):
            selected = self.dev._configure_compiler_cache("auto")
            env = os.environ.copy()

        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)
        self.assertNotIn("KACHE_SOCKET_PATH", env)
        self.assertNotIn("KACHE_CONFIG", env)

    def test_auto_falls_back_to_usable_sccache_on_unqualified_host(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: f"/bin/{name}"
        ), mock.patch.object(
            self.dev,
            "_kache_host_error",
            return_value="unsupported host linux/x86_64",
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
        ):
            selected = self.dev._configure_compiler_cache("auto")
            env = os.environ.copy()

        self.assertEqual("sccache", selected)
        self.assertEqual(
            str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"]
        )

    def test_explicit_kache_reports_invalid_configured_binary(self):
        with self.assertRaisesRegex(SystemExit, "PHOENIX_KACHE_BIN.*missing/kache"):
            self.configure(
                "kache",
                env={"PHOENIX_KACHE_BIN": "missing/kache"},
                installed=set(),
            )

    def test_kache_disabled_auto_falls_back_to_sccache(self):
        selected, env = self.configure(
            env={"KACHE_DISABLED": "1"}, installed={"kache", "sccache"}
        )
        self.assertEqual("sccache", selected)
        self.assertEqual(str(self.dev.Path("/bin/sccache").resolve()), env["RUSTC_WRAPPER"])

    def test_kache_disabled_explicit_fails(self):
        with self.assertRaisesRegex(SystemExit, "KACHE_DISABLED"):
            self.configure("kache", env={"KACHE_DISABLED": "1"}, installed={"kache"})

    def test_sccache_probe_requires_recognizable_version(self):
        for output in ("exit code 0", "", "other 0.18.0"):
            with self.subTest(output=output), mock.patch.object(
                self.dev,
                "_command_version",
                return_value=(output, None),
            ):
                version, error = self.dev._usable_sccache("/bin/sccache")
                self.assertIsNone(version)
                self.assertIn("unrecognized version output", error or "")

    def test_explicit_sccache_rejects_failed_version_probe(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", return_value="/bin/sccache"
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=(None, "bad architecture")
        ):
            with self.assertRaisesRegex(SystemExit, "bad architecture"):
                self.dev._configure_compiler_cache("sccache")

    def test_explicit_kache_fails_when_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("1.2.2", None)
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value="socket failed"):
            with self.assertRaisesRegex(SystemExit, "kache daemon failed to start: socket failed"):
                self.dev._configure_compiler_cache("kache")

    def test_unavailable_explicit_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "kache.*not installed"):
            self.configure("kache")

    def test_command_version_replaces_undecodable_output(self):
        completed = mock.Mock(returncode=0, stdout="kache \ufffd", stderr="")
        with mock.patch.object(self.dev.subprocess, "run", return_value=completed) as run:
            detail, error = self.dev._command_version("/bin/kache")
        self.assertEqual("kache \ufffd", detail)
        self.assertIsNone(error)
        self.assertEqual("replace", run.call_args.kwargs["errors"])

    def test_explicit_kache_rejects_unsupported_release(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(
            self.dev, "_kache_host_error", return_value=None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=(None, "unsupported kache 0.25.0")
        ):
            with self.assertRaisesRegex(SystemExit, "incompatible.*unsupported kache 0.25.0"):
                self.dev._configure_compiler_cache("kache")

    def test_auto_skips_unsupported_kache_for_sccache(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: f"/bin/{name}"
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=(None, "unsupported kache 0.25.0")
        ), mock.patch.object(
            self.dev, "_usable_sccache", return_value=("sccache 0.18.0", None)
        ):
            self.assertEqual("sccache", self.dev._configure_compiler_cache("auto"))
            self.assertEqual(
                str(self.dev.Path("/bin/sccache").resolve()),
                os.environ["RUSTC_WRAPPER"],
            )

    def test_build_configures_cache_before_spawning_cargo(self):
        calls = []
        stderr = mock.Mock()
        stderr.__iter__ = mock.Mock(return_value=iter(()))
        process = mock.Mock(stderr=stderr, returncode=0)
        process.wait.return_value = 0
        process.poll.return_value = 0
        with mock.patch.object(
            self.dev,
            "_configure_compiler_cache",
            side_effect=lambda _requested, **_kwargs: calls.append("cache"),
        ), mock.patch.object(
            self.dev.subprocess,
            "Popen",
            side_effect=lambda *args, **kwargs: calls.append(("cargo", kwargs.get("env"))) or process,
        ), mock.patch.object(self.dev, "_begin_dev_span", return_value=None), mock.patch.object(
            self.dev, "_finish_dev_span"
        ):
            self.dev.build_rust()
        self.assertEqual("cache", calls[0])
        self.assertEqual("cargo", calls[1][0])
        self.assertIsNotNone(calls[1][1])

    def test_local_prod_deploy_reuses_one_compiler_cache_setup(self):
        setup = (
            "kache",
            {
                "RUSTC_WRAPPER": "/bin/kache",
                "KACHE_SOCKET_PATH": "/tmp/kache.sock",
            },
        )
        controller = self.dev.ProdDeployControllerOptions(backend="launchd")
        with mock.patch.object(
            self.dev, "detect_prod_env", return_value="launchd"
        ), mock.patch.object(
            self.dev, "_compiler_cache_subprocess_env", return_value=setup
        ) as configure, mock.patch.object(self.dev, "cmd_check") as check, mock.patch.object(
            self.dev, "launchd_prod_deploy"
        ) as deploy, mock.patch.object(self.dev, "native_prod_deploy") as native, mock.patch.object(
            self.dev, "prod_daemon_deploy"
        ) as daemon:
            self.dev.cmd_prod_deploy(controller=controller)

        configure.assert_called_once_with(cargo_cwd=self.dev.ROOT)
        self.assertIs(setup, check.call_args.kwargs["compiler_cache_setup"])
        self.assertIs(setup, deploy.call_args.kwargs["compiler_cache_setup"])
        native.assert_not_called()
        daemon.assert_not_called()

    def test_independent_prod_deploys_configure_independently(self):
        controller = self.dev.ProdDeployControllerOptions(backend="launchd")
        with mock.patch.object(
            self.dev, "detect_prod_env", return_value="launchd"
        ), mock.patch.object(
            self.dev,
            "_compiler_cache_subprocess_env",
            side_effect=[("kache", {"run": "one"}), SystemExit("existing daemon")],
        ) as configure, mock.patch.object(self.dev, "cmd_check"), mock.patch.object(
            self.dev, "launchd_prod_deploy"
        ), mock.patch.object(self.dev, "native_prod_deploy") as native, mock.patch.object(
            self.dev, "prod_daemon_deploy"
        ) as daemon:
            self.dev.cmd_prod_deploy(controller=controller)
            with self.assertRaisesRegex(SystemExit, "existing daemon"):
                self.dev.cmd_prod_deploy(controller=controller)
        self.assertEqual(2, configure.call_count)
        native.assert_not_called()
        daemon.assert_not_called()

    def test_subprocess_environment_reports_actual_backend_without_leaking(self):
        def configure(_requested, **_options):
            os.environ["RUSTC_WRAPPER"] = "/bin/sccache"
            return "sccache"

        with mock.patch.dict(os.environ, {"ORIGINAL": "yes"}, clear=True), mock.patch.object(
            self.dev, "_configure_compiler_cache", side_effect=configure
        ):
            selected, environment = self.dev._compiler_cache_subprocess_env("auto")
            self.assertEqual({"ORIGINAL": "yes"}, os.environ)
        self.assertEqual("sccache", selected)
        self.assertEqual("/bin/sccache", environment["RUSTC_WRAPPER"])

    def test_check_cache_overrides_are_backend_specific(self):
        configured = {
            "RUSTC_WRAPPER": "/bin/kache",
            "KACHE_CACHE_DIR": "/kache",
            "SCCACHE_DIR": "/sccache",
            "UNRELATED": "value",
        }
        self.assertEqual(
            {
                "RUSTC_WRAPPER": "/bin/kache",
                "KACHE_CACHE_DIR": "/kache",
            },
            self.dev._compiler_cache_overrides("kache", configured),
        )
        self.assertEqual(
            {
                "RUSTC_WRAPPER": "/bin/kache",
                "SCCACHE_DIR": "/sccache",
            },
            self.dev._compiler_cache_overrides("sccache", configured),
        )

    def test_only_cargo_owning_steps_receive_check_cache(self):
        self.assertTrue(self.dev._command_uses_compiler_cache(["cargo", "test"]))
        self.assertTrue(self.dev._command_uses_compiler_cache(["/opt/bin/cargo", "clippy"]))
        self.assertTrue(
            self.dev._command_uses_compiler_cache(["uv", "run", "tests/e2e/run.py"])
        )
        self.assertFalse(self.dev._command_uses_compiler_cache(["uv", "run", "tests.py"]))
        self.assertFalse(self.dev._command_uses_compiler_cache([]))

    def test_relative_wrapper_path_is_made_absolute(self):
        with mock.patch.object(self.dev.Path, "resolve", return_value=self.dev.Path("/workspace/bin/kache")):
            self.assertEqual(
                "/workspace/bin/kache", self.dev._absolute_executable("bin/kache")
            )

    def test_cache_paths_normalize_against_invoking_directory(self):
        with mock.patch.dict(
            os.environ,
            {
                "KACHE_CACHE_DIR": "cache/kache",
                "KACHE_SOCKET_PATH": "run/kache.sock",
                "KACHE_CONFIG": "config/kache.toml",
                "KACHE_HOST_CONFIG": "config/host.toml",
                "KACHE_RUNTIME_DIR": "run/kache",
                "KACHE_LOG_FILE": "kache=trace",
                "KACHE_LOG_FILE_PATH": "logs/kache.log",
                "SCCACHE_DIR": "cache/sccache",
                "SCCACHE_CONF": "config/sccache.toml",
                "SCCACHE_ERROR_LOG": "logs/sccache.log",
                "SCCACHE_GCS_KEY_PATH": "credentials/gcs.json",
            },
            clear=True,
        ):
            self.dev._normalize_cache_paths("kache", self.dev.Path("/workspace"))
            self.assertEqual("/workspace/cache/kache", os.environ["KACHE_CACHE_DIR"])
            self.assertEqual("/workspace/run/kache.sock", os.environ["KACHE_SOCKET_PATH"])
            self.assertEqual("/workspace/config/kache.toml", os.environ["KACHE_CONFIG"])
            self.assertEqual("/workspace/config/host.toml", os.environ["KACHE_HOST_CONFIG"])
            self.assertEqual("/workspace/run/kache", os.environ["KACHE_RUNTIME_DIR"])
            self.assertEqual("kache=trace", os.environ["KACHE_LOG_FILE"])
            self.assertEqual("/workspace/logs/kache.log", os.environ["KACHE_LOG_FILE_PATH"])
            self.assertEqual("cache/sccache", os.environ["SCCACHE_DIR"])
            self.assertEqual("config/sccache.toml", os.environ["SCCACHE_CONF"])
            self.assertEqual("logs/sccache.log", os.environ["SCCACHE_ERROR_LOG"])
            self.assertEqual("credentials/gcs.json", os.environ["SCCACHE_GCS_KEY_PATH"])
            self.dev._normalize_cache_paths("sccache", self.dev.Path("/workspace"))
            self.assertEqual("/workspace/cache/sccache", os.environ["SCCACHE_DIR"])
            self.assertEqual("/workspace/config/sccache.toml", os.environ["SCCACHE_CONF"])
            self.assertEqual("/workspace/logs/sccache.log", os.environ["SCCACHE_ERROR_LOG"])
            self.assertEqual(
                "/workspace/credentials/gcs.json",
                os.environ["SCCACHE_GCS_KEY_PATH"],
            )

    def test_sccache_cwd_paths_and_abstract_uds(self):
        with mock.patch.dict(
            os.environ,
            {
                "SCCACHE_STARTUP_NOTIFY": "run/notify.sock",
                "SCCACHE_EXTRAFILES": f"one.h{os.pathsep}nested/two.h",
                "SCCACHE_BASEDIRS": f".{os.pathsep}nested/source",
                "SCCACHE_SERVER_UDS": "run/sccache.sock",
            },
            clear=True,
        ):
            self.dev._normalize_cache_paths("sccache", Path("/workspace"))
            self.assertEqual("/workspace/run/notify.sock", os.environ["SCCACHE_STARTUP_NOTIFY"])
            self.assertEqual(
                f"/workspace/one.h{os.pathsep}/workspace/nested/two.h",
                os.environ["SCCACHE_EXTRAFILES"],
            )
            self.assertEqual(
                f"/workspace{os.pathsep}/workspace/nested/source",
                os.environ["SCCACHE_BASEDIRS"],
            )
            self.assertEqual("/workspace/run/sccache.sock", os.environ["SCCACHE_SERVER_UDS"])

        with mock.patch.dict(
            os.environ, {"SCCACHE_SERVER_UDS": r"\x00sccache.sock"}, clear=True
        ):
            self.dev._normalize_cache_paths("sccache", Path("/workspace"))
            self.assertEqual(r"\x00sccache.sock", os.environ["SCCACHE_SERVER_UDS"])

    def test_absolute_cache_paths_are_preserved(self):
        with mock.patch.dict(
            os.environ, {"KACHE_CACHE_DIR": "/owned/cache"}, clear=True
        ):
            self.dev._normalize_cache_paths("kache", self.dev.Path("/elsewhere"))
            self.assertEqual("/owned/cache", os.environ["KACHE_CACHE_DIR"])

    def test_invalid_environment_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "invalid compiler cache"):
            self.configure(env={"PHOENIX_COMPILER_CACHE": "bogus"})


if __name__ == "__main__":
    unittest.main()
