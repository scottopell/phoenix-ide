import importlib.util
import os
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

    def configure(self, requested=None, *, env=None, installed=()):
        env = {} if env is None else env
        with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(
            self.dev.shutil,
            "which",
            side_effect=lambda name: f"/bin/{name}" if name in installed else None,
        ), mock.patch.object(
            self.dev,
            "_kache_version",
            return_value=("0.26.0", None),
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value=None):
            selected = self.dev._configure_compiler_cache(requested)
            return selected, os.environ.copy()

    def test_all_cargo_lanes_enable_compiler_cache_setup(self):
        for lane in ("rust", "clippy", "e2e"):
            with self.subTest(lane=lane):
                self.assertTrue(self.dev._cargo_check_active({lane}))

    def test_non_cargo_lane_skips_compiler_cache_setup(self):
        self.assertFalse(self.dev._cargo_check_active({"vitest"}))

    def test_explicit_rustc_wrapper_wins(self):
        selected, env = self.configure("kache", env={"RUSTC_WRAPPER": "custom"})
        self.assertEqual("explicit", selected)
        self.assertEqual("custom", env["RUSTC_WRAPPER"])

    def test_auto_prefers_supported_kache(self):
        selected, env = self.configure(installed={"kache", "sccache"})
        self.assertEqual("kache", selected)
        self.assertEqual("/bin/kache", env["RUSTC_WRAPPER"])
        self.assertNotIn("SCCACHE_CACHE_SIZE", env)

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
        selected, env = self.configure(installed={"kache"})
        self.assertEqual("kache", selected)
        self.assertEqual("/bin/kache", env["RUSTC_WRAPPER"])
        self.assertNotIn("SCCACHE_CACHE_SIZE", env)

    def test_none_disables_automatic_wrapper(self):
        selected, env = self.configure("none", installed={"kache", "sccache"})
        self.assertEqual("none", selected)
        self.assertNotIn("RUSTC_WRAPPER", env)

    def test_environment_selects_backend(self):
        selected, env = self.configure(
            env={"PHOENIX_COMPILER_CACHE": "kache"}, installed={"kache"}
        )
        self.assertEqual("kache", selected)
        self.assertEqual("/bin/kache", env["RUSTC_WRAPPER"])

    def test_cli_selection_takes_precedence_over_environment(self):
        selected, env = self.configure(
            "sccache",
            env={"PHOENIX_COMPILER_CACHE": "kache"},
            installed={"kache", "sccache"},
        )
        self.assertEqual("sccache", selected)
        self.assertEqual("/bin/sccache", env["RUSTC_WRAPPER"])

    def test_local_kache_binary_is_supported(self):
        with mock.patch.dict(
            os.environ, {"PHOENIX_KACHE_BIN": "/opt/local/kache"}, clear=True
        ), mock.patch.object(self.dev.Path, "is_file", return_value=True), mock.patch.object(
            self.dev.os, "access", return_value=True
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("0.26.0", None)
        ), mock.patch.object(
            self.dev, "_ensure_kache_daemon", return_value=None
        ) as ensure:
            selected = self.dev._configure_compiler_cache("kache")
            self.assertEqual("kache", selected)
            self.assertEqual("/opt/local/kache", os.environ["RUSTC_WRAPPER"])
            ensure.assert_called_once_with("/opt/local/kache")

    def test_daemon_socket_uses_private_owned_directory(self):
        completed = mock.Mock(returncode=0, stdout="", stderr="")
        with self.subTest("socket path and permissions"), mock.patch.dict(
            os.environ, {"KACHE_CACHE_DIR": "/very/long/worktree/cache"}, clear=True
        ), mock.patch.object(self.dev.subprocess, "run", return_value=completed) as run:
            with self.dev.tempfile.TemporaryDirectory() as temporary:
                with mock.patch.object(self.dev.tempfile, "gettempdir", return_value=temporary):
                    self.assertIsNone(self.dev._ensure_kache_daemon("/bin/kache"))
                socket_path = Path(os.environ["KACHE_SOCKET_PATH"])
                self.assertEqual(socket_path.parent.name, f"phoenix-kache-{os.getuid()}")
                self.assertEqual(socket_path.parent.stat().st_mode & 0o777, 0o700)
                self.assertRegex(socket_path.name, r"^[0-9a-f]{16}\.sock$")
            run.assert_called_once()

    def test_auto_falls_back_to_sccache_when_kache_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: f"/bin/{name}"
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("0.26.0", None)
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value="socket failed"):
            self.assertEqual("sccache", self.dev._configure_compiler_cache("auto"))
            self.assertEqual("/bin/sccache", os.environ["RUSTC_WRAPPER"])
            self.assertEqual("10G", os.environ["SCCACHE_CACHE_SIZE"])

    def test_auto_falls_back_to_none_when_kache_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("0.26.0", None)
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value="socket failed"):
            self.assertEqual("none", self.dev._configure_compiler_cache("auto"))
            self.assertNotIn("RUSTC_WRAPPER", os.environ)

    def test_explicit_kache_fails_when_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(
            self.dev, "_kache_version", return_value=("0.26.0", None)
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value="socket failed"):
            with self.assertRaisesRegex(SystemExit, "kache daemon failed to start: socket failed"):
                self.dev._configure_compiler_cache("kache")

    def test_unavailable_explicit_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "kache.*not installed"):
            self.configure("kache")

    def test_kache_version_accepts_released_series(self):
        with mock.patch.object(
            self.dev, "_command_version", return_value=("kache 0.26.0", None)
        ):
            self.assertEqual(("0.26.0", None), self.dev._kache_version("/bin/kache"))

    def test_explicit_kache_rejects_unsupported_series(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
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
        ):
            self.assertEqual("sccache", self.dev._configure_compiler_cache("auto"))
            self.assertEqual("/bin/sccache", os.environ["RUSTC_WRAPPER"])

    def test_build_configures_cache_before_spawning_cargo(self):
        calls = []
        stderr = mock.Mock()
        stderr.__iter__ = mock.Mock(return_value=iter(()))
        process = mock.Mock(stderr=stderr, returncode=0)
        process.wait.return_value = 0
        process.poll.return_value = 0
        with mock.patch.object(
            self.dev, "_configure_compiler_cache", side_effect=lambda: calls.append("cache")
        ), mock.patch.object(
            self.dev.subprocess, "Popen", side_effect=lambda *args, **kwargs: calls.append("cargo") or process
        ), mock.patch.object(self.dev, "_begin_dev_span", return_value=None), mock.patch.object(
            self.dev, "_finish_dev_span"
        ):
            self.dev.build_rust()
        self.assertEqual(["cache", "cargo"], calls)

    def test_subprocess_environment_reports_actual_backend(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev, "_configure_compiler_cache", return_value="sccache"
        ):
            os.environ["RUSTC_WRAPPER"] = "/bin/sccache"
            selected, environment = self.dev._compiler_cache_subprocess_env("auto")
        self.assertEqual("sccache", selected)
        self.assertEqual("/bin/sccache", environment["RUSTC_WRAPPER"])

    def test_invalid_environment_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "invalid compiler cache"):
            self.configure(env={"PHOENIX_COMPILER_CACHE": "bogus"})


if __name__ == "__main__":
    unittest.main()
