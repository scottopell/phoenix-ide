#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Compiler-free, process-free opening harness regressions (no httpx import needed)."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import history_opening_browser as opening


class OpeningQualificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.app = self.root / "app"
        self.app.mkdir()
        (self.app / "ui/dist").mkdir(parents=True)
        (self.app / "ui/dist/index.html").write_text("independent built UI")
        (self.app / "Cargo.lock").write_text("lock")
        (self.app / "ui/pnpm-lock.yaml").write_text("ui lock")
        self.exe = self.root / "libtest"
        self.exe.write_text("independent artifact (identity selftest only)")
        self.exe.chmod(0o700)
        self.receipt = self.root / "receipt.json"
        self.value = {"app_source": opening.PIN, "app_tree": "a" * 40, "harness_source": "b" * 40,
            "executable_sha256": opening.shipping.digest(self.exe), "evidence_class": opening.CLASS,
            "cargo_lock_sha256": opening.shipping.digest(self.app / "Cargo.lock"),
            "ui_lock_sha256": opening.shipping.digest(self.app / "ui/pnpm-lock.yaml"),
            "ui_files": opening.shipping.ui_hashes(self.app), "rustc": "rustc 1.95.0 (build attestation)",
            "focused_test_exit": 0, "ignored_driver": opening.DRIVER,
            "compiler_artifact_selector": ["cargo", "test", "-p", "phoenix_ide", "--lib", "--no-run", "--message-format=json"]}
        self.write()

    def write(self):
        self.receipt.write_text(json.dumps(self.value))

    def git(self, root, *args):
        if args == ("rev-parse", "--show-toplevel"): return str(root)
        if args[0] == "status": return ""
        if args[-1] == "HEAD^{tree}": return "a" * 40
        return opening.PIN if root == self.app else "b" * 40

    def identity(self):
        with patch.object(opening.shipping, "git", self.git):
            return opening.identity(self.app, self.exe, self.receipt)

    def test_independent_hash_and_build_metadata(self):
        value = self.identity()
        self.assertEqual(value["evidence_class"], opening.CLASS)
        self.assertIn("attestation", value["source_provenance"])
        self.assertIn("tests/e2e/history_browser.py", value["harness_files"])

    def test_each_identity_dimension_fails_closed(self):
        for key in self.value:
            saved = self.value[key]
            self.value[key] = "wrong"
            self.write()
            with self.subTest(key=key), self.assertRaises(ValueError): self.identity()
            self.value[key] = saved
        self.write()
        for path in [self.exe, self.app / "ui/dist/index.html", self.app / "Cargo.lock", self.app / "ui/pnpm-lock.yaml"]:
            saved = path.read_bytes(); path.write_bytes(b"changed")
            with self.subTest(path=path), self.assertRaises(ValueError): self.identity()
            path.write_bytes(saved)

    def test_pin_dirty_and_symlink_rejected(self):
        for arg, value in [("HEAD", "wrong"), ("status", " M source.rs")]:
            def git(root, *args):
                if (arg == "HEAD" and args == ("rev-parse", "HEAD") and root == self.app) or args[0] == arg: return value
                return self.git(root, *args)
            with patch.object(opening.shipping, "git", git), self.assertRaises(ValueError):
                opening.identity(self.app, self.exe, self.receipt)
        link = self.root / "linked-exe"; link.symlink_to(self.exe)
        with patch.object(opening.shipping, "git", self.git), self.assertRaises(ValueError):
            opening.identity(self.app, link, self.receipt)

    def test_startup_requires_exact_compiled_and_owned_identity(self):
        home = self.root / "owned-home"; home.mkdir()
        log = self.root / "startup.log"
        data = {"git_sha": opening.PIN, "exe": str(self.exe), "home": str(home),
                "db": str(home / ".phoenix-ide/phoenix.db"), "bind": "127.0.0.1", "port": 34567,
                "instrumented_test_binary": True}
        def write(): log.write_text("OPENING_QUALIFIER " + json.dumps(data) + "\n")
        write(); self.assertEqual(opening.startup_identity(log, self.exe, self.root, 34567)["executable_sha256"], opening.shipping.digest(self.exe))
        for key, bad in [("git_sha", "other"), ("exe", "/tmp/other"), ("home", "/tmp"), ("db", "/tmp/shared.db"),
                         ("bind", "0.0.0.0"), ("port", 1), ("instrumented_test_binary", False)]:
            saved = data[key]; data[key] = bad; write()
            with self.subTest(key=key), self.assertRaises(ValueError): opening.startup_identity(log, self.exe, self.root, 34567)
            data[key] = saved
        log.write_text("")
        with self.assertRaises(ValueError): opening.startup_identity(log, self.exe, self.root, 34567)
        write(); log.write_text(log.read_text() * 2)
        with self.assertRaises(ValueError): opening.startup_identity(log, self.exe, self.root, 34567)

    def test_owned_process_cleanup_on_child_assertion_failure(self):
        child = types.SimpleNamespace(pid=1234)
        with patch.object(opening.shipping.subprocess, "Popen", return_value=child), patch.object(opening.shipping, "stop") as stop:
            with self.assertRaisesRegex(RuntimeError, "child failed"):
                with opening.shipping.owned_process(["never-executed"], self.root, {}, self.root / "child.log"):
                    raise RuntimeError("child failed")
            stop.assert_called_once_with(child)

    def test_no_compiler_or_shipping_server_invocation(self):
        source = Path(opening.__file__).read_text()
        self.assertNotIn("h._server()", source)
        self.assertNotIn("subprocess.run", source)
        self.assertNotIn("subprocess.check_output", source)
        self.assertIn("--ignored", source)
        self.assertIn("Fixture(h, base, case)", source)


if __name__ == "__main__": unittest.main()
