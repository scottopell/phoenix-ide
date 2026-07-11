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
        ):
            selected = self.dev._configure_compiler_cache(requested)
            return selected, os.environ.copy()

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
        self.assertEqual("kache", env["RUSTC_WRAPPER"])
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
        self.assertEqual("kache", env["RUSTC_WRAPPER"])

    def test_cli_selection_takes_precedence_over_environment(self):
        selected, env = self.configure(
            "sccache",
            env={"PHOENIX_COMPILER_CACHE": "kache"},
            installed={"kache", "sccache"},
        )
        self.assertEqual("sccache", selected)
        self.assertEqual("sccache", env["RUSTC_WRAPPER"])

    def test_unavailable_explicit_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "kache.*not installed"):
            self.configure("kache")

    def test_invalid_environment_backend_fails(self):
        with self.assertRaisesRegex(SystemExit, "invalid compiler cache"):
            self.configure(env={"PHOENIX_COMPILER_CACHE": "bogus"})


if __name__ == "__main__":
    unittest.main()
