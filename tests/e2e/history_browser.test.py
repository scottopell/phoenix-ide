#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""Lightweight source-mock selftests; no backend, browser or Cargo build."""
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import tempfile
import types
import unittest
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location("qualification", Path(__file__).with_name("history_browser.py"))
hb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hb)


def page(segment, sequences):
    return {"segments": [{"segment_ordinal": segment, "messages": [
        {"sequence_id": n, "message_id": f"m-{segment}-{n}"} for n in sequences]}]}


class QualificationTests(unittest.TestCase):
    def test_delivered_chat_queue_ack_requires_exact_persisted_response(self):
        fixture = object.__new__(hb.Fixture)
        fixture.h = types.SimpleNamespace(_has_agent_after_message=lambda messages, mid: True)
        fixture.request = MagicMock(return_value={"queued": True})
        fixture.wait = MagicMock(side_effect=lambda row, exhausted, mid: {"messages": [{"message_id": mid}]})
        fixture.send("row", "probe")
        self.assertEqual(fixture.wait.call_count, 1)
        client_key = fixture.request.call_args.args[2]["message_id"]
        self.assertEqual(fixture.wait.call_args.args[2], f"row:{client_key}")
        fixture.request.return_value = {"queued": True, "steering": True}
        with self.assertRaises(AssertionError):
            fixture.send("row", "probe")
        self.assertEqual(fixture.wait.call_count, 1)

    def test_complete_paging_rejects_missing_interior_occurrence(self):
        full = page(0, [1, 2, 3])
        full["boundaries"] = []
        self.assertEqual(hb.complete_occurrences([full], [full["segments"][0]["messages"]]),
                         [(0, n, f"m-0-{n}") for n in (1, 2, 3)])
        dropped = page(0, [1, 3])
        dropped["boundaries"] = []
        with self.assertRaisesRegex(AssertionError, "authoritative member"):
            hb.complete_occurrences([dropped], [full["segments"][0]["messages"]])

    @staticmethod
    def instrumented_workflow_pin_matches(workflow, pin):
        job = workflow.split("  qualify-history-realserver:\n", 1)[1].split("\n  check-ui:", 1)[0]
        return (f"ref: {pin}\n          path: pinned-app" in job
                and f'test "$(git rev-parse HEAD)" = {pin}' in job)

    def test_workflow_checkout_and_build_guard_match_owning_instrumented_pin(self):
        import ast
        workflow = (hb.ROOT / ".github/workflows/ci.yml").read_text()
        source = ast.parse((hb.ROOT / "tests/e2e/history_opening_browser.py").read_text())
        pin = next(ast.literal_eval(node.value) for node in source.body if isinstance(node, ast.Assign)
                   and any(isinstance(target, ast.Name) and target.id == "PIN" for target in node.targets))
        self.assertEqual(hb.PIN, "d4c2772a95aa46da608b356c8fe7be074de1b7aa")
        self.assertEqual(len(pin), 40)
        self.assertNotEqual(pin, hb.PIN, "shipping receipt and integrated instrumented source are distinct authorities")
        self.assertTrue(self.instrumented_workflow_pin_matches(workflow, pin))
        self.assertFalse(self.instrumented_workflow_pin_matches(workflow, hb.PIN))
        wrong_checkout = workflow.replace(f"ref: {pin}\n          path: pinned-app", f"ref: {hb.PIN}\n          path: pinned-app")
        wrong_guard = workflow.replace(f'test "$(git rev-parse HEAD)" = {pin}', f'test "$(git rev-parse HEAD)" = {hb.PIN}')
        self.assertFalse(self.instrumented_workflow_pin_matches(wrong_checkout, pin))
        self.assertFalse(self.instrumented_workflow_pin_matches(wrong_guard, pin))

    def test_manual_equal_text_opening_is_not_a_represented_generated_opening(self):
        messages = [
            [{"sequence_id": 1, "message_id": "boundary", "content": "same"}],
            [{"sequence_id": 1, "message_id": "opening", "content": "same"}],
        ]
        full = {"segments": [{"segment_ordinal": n, "messages": member}
                             for n, member in enumerate(messages)],
                "boundaries": [{"represented_successor_opening_message_id": None}]}
        self.assertEqual(hb.complete_occurrences([full], messages),
                         [(0, 1, "boundary"), (1, 1, "opening")])
        dropped = {**full, "segments": full["segments"][:1]}
        with self.assertRaisesRegex(AssertionError, "authoritative member"):
            hb.complete_occurrences([dropped], messages)
        generated = {**dropped, "boundaries": [{"represented_successor_opening_message_id": "opening"}]}
        self.assertEqual(hb.complete_occurrences([generated], messages), [(0, 1, "boundary")])

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.app = Path(self.temp.name).resolve() / "app"
        self.app.mkdir()
        self.binary = self.app / "target/debug/phoenix_ide"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_text("independently supplied fake artifact for identity test only")
        self.binary.chmod(0o700)
        self.dist = self.app / "ui/dist"
        self.dist.mkdir(parents=True)
        (self.dist / "index.html").write_text("built UI")
        (self.dist / ".gitkeep").touch()
        self.receipt = Path(self.temp.name) / "build.json"
        hb.write_json(self.receipt, {"source_sha": hb.PIN, "binary_sha256": hb.digest(self.binary),
                                   "ui_files": hb.ui_hashes(self.app)})

    def git(self, root, *args):
        if args == ("rev-parse", "--show-toplevel"):
            return str(root)
        if args == ("status", "--porcelain", "--untracked-files=no"):
            return ""
        return hb.PIN

    def test_identity_checks_separate_full_pin_and_artifacts(self):
        with patch.object(hb, "git", self.git):
            value = hb.identity(self.app, self.receipt)
        self.assertEqual(value["source_sha"], hb.PIN)
        self.assertEqual(value["binary_sha256"], hb.digest(self.binary))
        self.assertEqual(value["ui_files"], {"index.html": hb.digest(self.dist / "index.html")})
        self.assertIn("attestation", value["source_provenance"])

    def test_identity_rejects_app_equal_harness(self):
        with patch.object(hb, "ROOT", self.app), patch.object(hb, "git", self.git):
            with self.assertRaisesRegex(ValueError, "independent"):
                hb.identity(self.app, self.receipt)

    def test_identity_rejects_wrong_sha_and_dirty_source(self):
        for answer, pattern in [("deadbeef", "full SHA"), (" M source.rs", "dirty")]:
            def mocked(root, *args):
                if args == ("rev-parse", "HEAD") and pattern == "full SHA":
                    return answer
                if args[0] == "status" and pattern == "dirty":
                    return answer
                return self.git(root, *args)
            with patch.object(hb, "git", mocked):
                with self.assertRaisesRegex(ValueError, pattern):
                    hb.identity(self.app, self.receipt)

    def test_receipt_rejects_changed_binary_changed_ui_and_extra_ui(self):
        for path in [self.binary, self.dist / "index.html", self.dist / "unexpected.js"]:
            saved = path.read_bytes() if path.exists() else None
            path.write_text("changed")
            with patch.object(hb, "git", self.git):
                with self.assertRaisesRegex(ValueError, "mismatch"):
                    hb.identity(self.app, self.receipt)
            if saved is None:
                path.unlink()
            else:
                path.write_bytes(saved)

    def test_no_missing_or_symlinked_binary_or_ui(self):
        target = self.binary.with_suffix(".real")
        self.binary.rename(target)
        self.binary.symlink_to(target)
        with patch.object(hb, "git", self.git):
            with self.assertRaisesRegex(ValueError, "standalone"):
                hb.identity(self.app, self.receipt)
        (self.dist / "index.html").unlink()
        with self.assertRaisesRegex(ValueError, "built"):
            hb.ui_hashes(self.app)

    def test_pages_three_chapters_and_sequence_restart(self):
        keys = hb.validate_pages([page(2, range(1, 80)), page(1, range(1, 80)), page(0, range(1, 80))])
        self.assertEqual(len(keys), 237)
        self.assertEqual(keys[0], (0, 1, "m-0-1"))
        self.assertEqual(keys[-1], (2, 79, "m-2-79"))

    def test_pages_reject_overlap_bad_local_and_cross_page_order(self):
        for pages in [[page(0, [2, 3]), page(0, [1, 2])], [page(0, [3, 1])],
                      [page(0, [1]), page(1, [1])]]:
            with self.assertRaises(AssertionError):
                hb.validate_pages(pages)

    def test_import_only_pinned_helpers_never_main(self):
        script = self.app / "tests/e2e/run.py"
        script.parent.mkdir(parents=True)
        script.write_text('from pathlib import Path\nROOT = Path(__file__).resolve().parents[2]\n'
                          'BINARY = ROOT / "target/debug/phoenix_ide"\n'
                          'if __name__ == "__main__": raise RuntimeError("must never invoke")\n')
        loaded = hb.load_app_harness(self.app)
        self.assertEqual(loaded.BINARY, self.binary)
        self.assertFalse((script.parent / "__pycache__").exists())

    def test_sse_terminal_is_not_enough_without_matching_persisted_send(self):
        # Terminal SSE may precede final persistence; old idle history cannot
        # count as proof of the newly accepted message.
        h = types.SimpleNamespace(_terminal_event=lambda kind, raw: True,
            _context_exhausted_event=lambda kind, raw: True,
            _state_str=lambda state: state,
            _has_agent_after_message=lambda messages, mid: any(
                m["message_id"] == mid for m in messages) and messages[-1]["message_type"] == "agent")
        h._wait_for_sse_signal = lambda base, row, predicate, timeout: self.assertTrue(predicate("agent_done", "{}"))
        fixture = object.__new__(hb.Fixture)
        fixture.h, fixture.base, fixture.out = h, "http://127.0.0.1:1", Path(self.temp.name)
        fixture.completion = None
        old = {"conversation": {"state": "idle"}, "messages": [{"message_id": "old", "message_type": "agent"}]}
        durable = {"conversation": {"state": "idle"}, "messages": [
            {"message_id": "new", "message_type": "user"}, {"message_id": "reply", "message_type": "agent"}]}
        fixture.request = MagicMock(side_effect=[old, durable])
        with patch.object(hb.time, "sleep"):
            self.assertEqual(fixture.wait("row", message_id="new"), durable)
        self.assertEqual(fixture.request.call_count, 2)
        self.assertTrue((fixture.out / "setup-sse.jsonl").exists())

    def test_process_cleanup_covers_exited_driver_descendants(self):
        proc = MagicMock(pid=99999)
        proc.poll.return_value = 1
        with patch.object(hb.os, "killpg") as kill:
            hb.stop(proc)
        self.assertEqual(kill.call_args_list[0].args, (99999, signal.SIGTERM))
        self.assertEqual(kill.call_args_list[1].args, (99999, signal.SIGKILL))
        proc.wait.assert_called_with(timeout=5)

    def test_owned_process_cleans_up_on_exception(self):
        proc = MagicMock(pid=99999)
        with patch.object(hb.subprocess, "Popen", return_value=proc) as popen, patch.object(hb, "stop") as stop:
            with self.assertRaisesRegex(RuntimeError, "test failure"):
                with hb.owned_process(["node", "driver.mjs"], self.app, {"HOME": "/isolated"}, Path(self.temp.name) / "process.log"):
                    raise RuntimeError("test failure")
        stop.assert_called_once_with(proc)
        self.assertTrue(popen.call_args.kwargs["start_new_session"])

    def test_source_mock_orchestration_uses_pinned_server_and_matching_preview(self):
        out = Path(self.temp.name) / "evidence"
        out.mkdir()
        h = types.SimpleNamespace()
        h.subprocess = hb.subprocess
        h._profile_dir_from_env = lambda: None
        h._free_port = lambda: 54321
        h._server_env = lambda temp, parent: parent | {"HOME": str(temp / "home"),
            "PHOENIX_DB_PATH": str(temp / "db"), "PHOENIX_DATA_DIR": str(temp / "data"),
            "PHOENIX_BIND_ADDR": "127.0.0.1", "DEFAULT_MODEL": "mock"}
        h._start_server_with_retries = lambda env, temp: (None, "http://127.0.0.1:54320", temp / "server.log", None)
        @contextlib.contextmanager
        def server():
            with tempfile.TemporaryDirectory() as root:
                root = Path(root)
                env = h._server_env(root)
                self.assertNotIn("ANTHROPIC_API_KEY", env)
                root.joinpath("server.log").write_text("owned backend log")
                h._start_server_with_retries(env, root)
                yield "http://127.0.0.1:54320", root / "server.log", 123, None
        h._server = server
        fake = MagicMock()
        fake.build.side_effect = [{"kind": "ordinary"}, {"kind": "global"}]
        commands = []
        @contextlib.contextmanager
        def process(command, cwd, env, log):
            commands.append(command)
            self.assertEqual(env["APP_ROOT"], str(self.app))
            if "preview" in command:
                config = json.loads((Path(cwd) / "preview.mjs").read_text().removeprefix("export default ").removesuffix(";\n"))
                self.assertEqual(config["preview"]["proxy"]["/api"]["target"], "http://127.0.0.1:54320")
                self.assertEqual(config["build"]["outDir"], str(self.dist))
            yield MagicMock(returncode=0, **{"poll.return_value": None})
        import httpx
        with patch.object(hb, "load_app_harness", return_value=h), patch.object(hb, "Fixture", return_value=fake), \
             patch.object(hb, "owned_process", process), patch.object(httpx, "get", return_value=types.SimpleNamespace(status_code=200, content=b"built UI")), \
             patch.dict(os.environ, {"ANTHROPIC_API_KEY": "must-not-leak", "PLAYWRIGHT_BROWSERS_PATH": self.temp.name}):
            hb.qualify(self.app, out, 600)
        self.assertEqual(len(commands), 2)
        self.assertTrue(all(command[0] == "node" for command in commands))
        self.assertEqual((out / "backend.log").read_text(), "owned backend log")
        self.assertEqual(json.loads((out / "fixture.json").read_text())["aggregates"][1]["kind"], "global")
        fake.client.close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
