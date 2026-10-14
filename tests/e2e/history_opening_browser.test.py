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
            "compiler_artifact_selector": ["cargo", "test", "--no-run", "--message-format=json"]}
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

    def test_actual_workflow_ui_hash_producer_matches_identity_consumer(self):
        import ast
        import hashlib
        import textwrap
        assets = self.app / "ui/dist/assets"
        assets.mkdir()
        (assets / "owned.js").write_bytes(b"built asset")
        (self.app / "ui/dist/.gitkeep").write_bytes(b"excluded")
        workflow = (opening.ROOT / ".github/workflows/ci.yml").read_text()
        start = workflow.index("'ui_files':{") + len("'ui_files':")
        end = workflow.index("},", start) + 1
        expression = ast.parse(textwrap.dedent(workflow[start:end]), mode="eval")
        producer = eval(compile(expression, "actual-ci-ui-hash-producer", "eval"),
                        {"root": self.app, "hashlib": hashlib})
        self.assertEqual(producer, opening.shipping.ui_hashes(self.app))
        self.assertEqual(set(producer), {"index.html", "assets/owned.js"})
        self.value["ui_files"] = producer
        self.write()
        self.identity()
        self.value["ui_files"] = {"ui/dist/" + key: value for key, value in producer.items()}
        self.write()
        with self.assertRaises(ValueError):
            self.identity()

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

    def test_typed_exhausted_summary_preserves_exact_text(self):
        summary = "  actual summary\nwith punctuation: \"quoted\" \u2603  "
        self.assertEqual(opening.exhausted_summary(json.loads(json.dumps({
            "type": "context_exhausted", "summary": summary}))), summary)

    def test_exhausted_summary_rejects_invalid_or_guessed_shapes(self):
        for state in [None, "context_exhausted", '{"type":"context_exhausted","summary":"x"}',
                      [], {}, {"type": "idle", "summary": "x"},
                      {"type": "context_exhausted"}, {"type": "context_exhausted", "summary": None},
                      {"type": "context_exhausted", "summary": 42},
                      {"type": "context_exhausted", "summary": ""},
                      {"type": "context_exhausted", "summary": " \n"},
                      {"ContextExhausted": {"summary": "x"}}]:
            with self.subTest(state=state), self.assertRaises(ValueError):
                opening.exhausted_summary(state)

    def test_equal_text_post_and_sse_use_exact_persisted_summary(self):
        from unittest.mock import Mock
        fixture = Mock()
        summary = "  actual summary\nnot FAST or fabricated  "
        fixture.request.side_effect = [
            {"conversation": {"state": {"type": "context_exhausted", "summary": summary}}},
            {"status": "accepted", "conversation_id": "successor"},
            RuntimeError("stop before member refresh")]
        aggregate = {"members": ["old", "predecessor"]}
        with patch.object(opening.shipping.uuid, "uuid4", return_value="key"):
            with self.assertRaisesRegex(RuntimeError, "stop before member refresh"):
                opening.equal_text_fixture(fixture, aggregate)
        self.assertEqual(fixture.request.call_args_list[1].args, (
            "POST", "/api/conversations/predecessor/continue", {"handoff": summary, "message_id": "key"}))
        fixture.wait.assert_called_once_with("successor", message_id="successor:key")
        self.assertEqual([call.args[0] for call in fixture.send.call_args_list], ["predecessor", "successor"])
        self.assertTrue(all(call.args[2] is True for call in fixture.send.call_args_list))

    def test_source_ast_keeps_fixture_refresh_before_browser_and_gate(self):
        import ast
        tree = ast.parse(Path(opening.__file__).read_text())
        qualify = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "qualify")
        calls = [node for node in ast.walk(qualify) if isinstance(node, ast.Call)]
        refresh = next(node for node in calls if isinstance(node.func, ast.Name) and node.func.id == "equal_text_fixture")
        fixture_write = next(node for node in calls if isinstance(node.func, ast.Attribute)
            and node.func.attr == "write_json" and 'fixture.json' in ast.unparse(node))
        self.assertLess(refresh.lineno, fixture_write.lineno)
        source = opening.SCRIPT.read_text()
        self.assertLess(source.index('boundaryMessage.content.summary, openingMessage.content.text'),
                        source.index("command: 'arm'"))

    def test_no_compiler_or_shipping_server_invocation(self):
        source = Path(opening.__file__).read_text()
        self.assertNotIn("h._server()", source)
        self.assertNotIn("subprocess.run", source)
        self.assertNotIn("subprocess.check_output", source)
        self.assertIn("--ignored", source)
        self.assertIn("Fixture(h, base, case)", source)


if __name__ == "__main__": unittest.main()
