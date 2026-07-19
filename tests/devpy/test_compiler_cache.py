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

    def test_auto_preserves_sccache_first_behavior(self):
        selected, env = self.configure(installed={"kache", "sccache"})
        self.assertEqual("sccache", selected)
        self.assertEqual("sccache", env["RUSTC_WRAPPER"])
        self.assertEqual("20G", env["SCCACHE_CACHE_SIZE"])

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
        self.assertEqual("sccache", env["RUSTC_WRAPPER"])

    def test_local_kache_binary_is_supported(self):
        with mock.patch.dict(
            os.environ, {"PHOENIX_KACHE_BIN": "/opt/local/kache"}, clear=True
        ), mock.patch.object(self.dev.Path, "is_file", return_value=True), mock.patch.object(
            self.dev.os, "access", return_value=True
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

    def test_auto_falls_back_when_kache_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value="socket failed"):
            self.assertEqual("none", self.dev._configure_compiler_cache("auto"))
            self.assertNotIn("RUSTC_WRAPPER", os.environ)

    def test_explicit_kache_fails_when_daemon_fails(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(
            self.dev.shutil, "which", side_effect=lambda name: "/bin/kache" if name == "kache" else None
        ), mock.patch.object(self.dev, "_ensure_kache_daemon", return_value="socket failed"):
            with self.assertRaisesRegex(SystemExit, "kache daemon failed to start: socket failed"):
                self.dev._configure_compiler_cache("kache")

    def test_unavailable_explicit_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "kache.*not installed"):
            self.configure("kache")

    def test_invalid_environment_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "invalid compiler cache"):
            self.configure(env={"PHOENIX_COMPILER_CACHE": "bogus"})


if __name__ == "__main__":
    unittest.main()
