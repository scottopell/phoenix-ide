#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx", "httpx-sse"]
# ///
"""Compiler-free opening regressions; HTTP/SSE authentication uses in-memory mocks."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
import history_opening_browser as opening
REAL_DIGEST = opening.shipping.digest


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
        for relative in opening.TELEMETRY_MODULES:
            path = self.app / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("hash-attested candidate module")
        self.exe = self.root / "libtest"
        self.exe.write_text("independent artifact (identity selftest only)")
        self.exe.chmod(0o700)
        self.receipt = self.root / "receipt.json"
        self.value = {"app_source": opening.PIN, "app_tree": opening.TREE_PIN,
            "app_base": opening.BASE_PIN, "harness_source": "b" * 40,
            "executable_sha256": opening.shipping.digest(self.exe), "evidence_class": opening.CLASS,
            "cargo_lock_sha256": opening.shipping.digest(self.app / "Cargo.lock"),
            "ui_lock_sha256": opening.shipping.digest(self.app / "ui/pnpm-lock.yaml"),
            "ui_files": opening.shipping.ui_hashes(self.app),
            "telemetry_modules": opening.TELEMETRY_MODULES,
            "rustc": "rustc 1.95.0 (build attestation)",
            "focused_test_exit": 0, "ignored_driver": opening.DRIVER,
            "compiler_artifact_selector": ["cargo", "test", "--no-run", "--message-format=json"]}
        self.write()

    def write(self):
        self.receipt.write_text(json.dumps(self.value))

    def git(self, root, *args):
        if args == ("rev-parse", "--show-toplevel"): return str(root)
        if args[0] == "status": return ""
        if args == ("rev-parse", "HEAD^{tree}"): return opening.TREE_PIN
        if args == ("merge-base", opening.PIN, opening.BASE_PIN): return opening.BASE_PIN
        return opening.PIN if root == self.app else "b" * 40

    def digest(self, path):
        path = Path(path)
        try:
            relative = str(path.relative_to(self.app))
        except ValueError:
            relative = None
        return opening.TELEMETRY_MODULES.get(relative, REAL_DIGEST(path))

    def identity(self):
        with patch.object(opening.shipping, "git", self.git), \
                patch.object(opening.shipping, "digest", self.digest):
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
        self.assertIn(opening.PIN, workflow)
        self.assertIn(opening.TREE_PIN, workflow)
        self.assertIn(opening.BASE_PIN, workflow)
        self.assertIn("'telemetry_modules': telemetry", workflow)
        self.assertIn("uv run --with opentelemetry-proto==1.39.0 tests/e2e/history_opening_browser.py", workflow)
        self.assertIn("--joint-telemetry", workflow)
        self.assertIn("command = [str(destination), 'opening_qualifier::', '--nocapture', '--test-threads=1']", workflow)
        self.assertIn("'11 passed; 0 failed; 1 ignored'", workflow)

    def test_actual_pinned_git_module_bytes_match_both_unmocked_hash_consumers(self):
        import hashlib
        import subprocess
        app = Path(os.environ.get("APP_ROOT", str(opening.ROOT / "target/history-hidden-geometry-repair")))
        source = (opening.ROOT / "ui/scripts/history-joint-telemetry.mjs").read_text()
        for relative, expected in opening.TELEMETRY_MODULES.items():
            actual = subprocess.check_output(["git", "-C", str(app), "show", f"{opening.PIN}:{relative}"])
            self.assertEqual(hashlib.sha256(actual).hexdigest(), expected)
            self.assertEqual(hashlib.sha256((app / relative).read_bytes()).hexdigest(), expected)
            if relative.endswith(".mjs"):
                self.assertIn(expected, source)

    def test_donor_relative_helper_is_staged_hashed_and_discovered(self):
        import ast
        import hashlib
        import textwrap
        workflow = (opening.ROOT / ".github/workflows/ci.yml").read_text()
        relative = "ui/scripts/view-telemetry-diagnostics.mjs"
        self.assertIn(relative, opening.TELEMETRY_MODULES)
        source = workflow.split("      - name: Genuine typed-contract codegen", 1)[1]
        code = source.split("python3 - <<'PY'", 1)[1].split("\n          PY", 1)[0]
        tree = ast.parse(textwrap.dedent(code))
        expression = next(node.value for node in tree.body if isinstance(node, ast.Assign)
                          and any(isinstance(target, ast.Name) and target.id == "telemetry" for target in node.targets))
        measured = eval(compile(ast.Expression(expression), "workflow telemetry hashes", "eval"),
                        {"root": self.app, "hashlib": hashlib})
        self.assertEqual(set(measured), set(opening.TELEMETRY_MODULES))
        self.assertIn("test -f ../pinned-app/" + relative, workflow)
        self.assertIn("scripts/view-telemetry-diagnostics.test.mjs", workflow)
        self.assertIn(str(self.app / relative), self.identity()["runtime_imports"])
        original_digest = self.digest
        def changed(path):
            return "changed-helper" if Path(path) == self.app / relative else original_digest(path)
        with patch.object(opening.shipping, "git", self.git), \
                patch.object(opening.shipping, "digest", changed):
            with self.assertRaisesRegex(ValueError, "module hash mismatch"):
                opening.identity(self.app, self.exe, self.receipt)

    def test_existing_hosted_job_qualifies_full_pinned_app_ui_before_capture(self):
        workflow = (opening.ROOT / ".github/workflows/ci.yml").read_text()
        start = workflow.index("      - name: Full UI source qualification at exact pinned app")
        stop = workflow.index("      - name: Prepare owned Chromium", start)
        step = workflow[start:stop]
        self.assertIn("working-directory: pinned-app", step)
        self.assertIn("PHOENIX_CHECK_ALL=1 PHOENIX_CHECK_WORKERS=2 ./dev.py check --lanes tsc,ui-lint,vitest", step)
        self.assertIn("ui_status=${PIPESTATUS[0]}", step)
        self.assertIn('exit "$ui_status"', step)
        self.assertNotIn("--no-file-parallelism", step)
        self.assertNotIn("renderFixture.test", step)
        self.assertIn("vitest_config_sha256", step)
        self.assertIn("pinned-app-full-ui.log", workflow)
        self.assertIn("pinned-app-full-ui-receipt.json", workflow)
        self.assertIn("runs-on: ubuntu-latest", workflow[workflow.index("  qualify-history-realserver:"):start])

    def test_ui_command_is_accepted_by_exact_app_lane_resolver_not_ci_group_alias(self):
        import ast, subprocess, sys, os
        app = Path(os.environ.get("HISTORY_PINNED_SOURCE", os.environ.get("APP_ROOT", "target/history-hidden-geometry-repair")))
        source = subprocess.check_output(["git", "-C", str(app), "show", f"{opening.PIN}:dev.py"], text=True)
        tree = ast.parse(source)
        selected = [node for node in tree.body if
                    isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id in {"_LANE_DEFS", "_LANE_ALIASES"} for t in node.targets)
                    or isinstance(node, ast.FunctionDef) and node.name == "_resolve_check_lanes"]
        scope = {"sys": sys}
        exec(compile(ast.Module(body=selected, type_ignores=[]), "exact-app-lane-resolver", "exec"), scope)
        scope["_all_lanes"] = lambda: {lane[0] for lane in scope["_LANE_DEFS"]}
        active, skipped = scope["_resolve_check_lanes"](gate=False, lanes="tsc,ui-lint,vitest")
        self.assertEqual(active, {"tsc", "ui-lint", "vitest"})
        self.assertEqual(skipped, {})
        with self.assertRaises(SystemExit):
            scope["_resolve_check_lanes"](gate=False, lanes="ui")

    def test_owned_home_fixture_is_executed_not_only_compiled_by_codegen(self):
        workflow = (opening.ROOT / ".github/workflows/ci.yml").read_text()
        start = workflow.index("      - name: Execute owned Coordinator continuation admission fixture")
        end = workflow.index("      - name: Verify and retain instrumented libtest", start)
        step = workflow[start:end]
        self.assertIn("'cargo','test','-p','phoenix-db','coordinator_continuation_commit_admits_automatic_work'", step)
        self.assertIn("'1 passed; 0 failed'", step)
        self.assertIn("completed.returncode == 0", step)
        self.assertIn("opening-coordinator-home-fixture.log", workflow[end:])
        self.assertIn("opening-coordinator-home-fixture-receipt.json", workflow[end:])

    def test_fixture_timeout_retains_partial_log_and_failed_receipt_before_reraise(self):
        import textwrap, subprocess
        workflow = (opening.ROOT / ".github/workflows/ci.yml").read_text()
        snippet = workflow.split("      - name: Execute owned Coordinator continuation admission fixture", 1)[1]
        snippet = textwrap.dedent(snippet.split("          python3 - <<'PY'\n", 1)[1].split("          PY", 1)[0])
        case = Path(self.temp.name) / "timeout-app"
        case.mkdir()
        error = subprocess.TimeoutExpired(["UNIT_NO_CARGO"], 900, output=b"partial stdout", stderr=b"partial stderr")
        with patch("subprocess.run", side_effect=error), patch("subprocess.check_output", return_value="unit-source\n"), patch.object(Path, "cwd", return_value=case):
            with self.assertRaises(subprocess.TimeoutExpired):
                exec(compile(snippet, "UNIT_ONLY_no_cargo_fixture_step", "exec"), {})
        log = case.parent / "opening-coordinator-home-fixture.log"
        receipt = json.loads((case.parent / "opening-coordinator-home-fixture-receipt.json").read_text())
        self.assertEqual(log.read_text(), "partial stdoutpartial stderr")
        self.assertTrue(receipt["timed_out"])
        self.assertIsNone(receipt["exit_status"])
        self.assertEqual(receipt["timeout_seconds"], 900)

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
            with patch.object(opening.shipping, "git", git), \
                    patch.object(opening.shipping, "digest", self.digest), self.assertRaises(ValueError):
                opening.identity(self.app, self.exe, self.receipt)
        link = self.root / "linked-exe"; link.symlink_to(self.exe)
        with patch.object(opening.shipping, "git", self.git), \
                patch.object(opening.shipping, "digest", self.digest), self.assertRaises(ValueError):
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

    def test_full_wire_identity_and_member_boundary_facts(self):
        from copy import deepcopy
        for kind in ("ordinary", "global"):
            endpoint = "/api/product-conversations/product" if kind == "ordinary" else "/api/global/coordinator/history/root"
            page = {"product_conversation_id": "product", "canonical_root": {"transcript_row_id": "root"},
                "requested_transcript_row_id": "root", "latest_transcript_row_id": "successor", "boundaries": [],
                "segments": [{"transcript_row_id": row, "segment_ordinal": i, "messages": []} for i, row in enumerate(["root", "successor"])]}
            self.assertEqual(opening.shipping.validate_wire_identity([page], ["root", "successor"], endpoint, kind), "product")
            for field in ["product_conversation_id", "requested_transcript_row_id", "latest_transcript_row_id"]:
                bad = deepcopy(page); bad[field] = "foreign"
                with self.subTest(kind=kind, field=field), self.assertRaises(AssertionError):
                    opening.shipping.validate_wire_identity([bad], ["root", "successor"], endpoint, kind, "product")
            bad = deepcopy(page); bad["segments"][0]["transcript_row_id"] = "foreign"
            with self.assertRaises(AssertionError): opening.shipping.validate_wire_identity([bad], ["root", "successor"], endpoint, kind)
        boundaries = opening.shipping.member_boundaries(["root", "successor"], [[{"message_id": "boundary", "message_type": "continuation"}], []])
        self.assertEqual(boundaries[0]["successor_transcript_row_id"], "successor")
        with self.assertRaises(AssertionError): opening.shipping.member_boundaries(["root", "successor"], [[], []])

    def test_owned_auth_credential_rejects_external_symlink_permissions_and_content(self):
        home = self.root / "auth-home"
        home.mkdir(mode=0o700)
        credential = home / "qualification-password"
        secret = "12345678-1234-4234-8234-123456789abc"
        credential.write_text(secret)
        credential.chmod(0o600)
        startup = {"home": str(home), "credential_file": str(credential)}
        self.assertEqual(opening.owned_qualification_password(startup, self.root / "credential-check.json"), secret)
        home.chmod(0o755)
        with self.assertRaises(ValueError): opening.owned_qualification_password(startup, self.root / "credential-check.json")
        facts = json.loads((self.root / "credential-check.json").read_text())
        self.assertEqual(facts["home_mode"], "0o755")
        self.assertFalse(facts["checks"]["home_mode_0700"])
        self.assertTrue(all(value for key, value in facts["checks"].items() if key != "home_mode_0700"))
        self.assertNotIn(secret, json.dumps(facts))
        home.chmod(0o700)
        credential.chmod(0o644)
        with self.assertRaises(ValueError): opening.owned_qualification_password(startup, self.root / "credential-check.json")
        credential.chmod(0o600)
        credential.write_text("x" * 36)
        with self.assertRaises(ValueError): opening.owned_qualification_password(startup, self.root / "credential-check.json")
        credential.unlink()
        outside = self.root / "outside-password"
        outside.write_text(secret); outside.chmod(0o600)
        credential.symlink_to(outside)
        with self.assertRaises(ValueError): opening.owned_qualification_password(startup, self.root / "credential-check.json")
        with self.assertRaises(ValueError): opening.owned_qualification_password({**startup, "credential_file": str(outside)}, self.root / "credential-check.json")

    def test_parent_session_login_and_sse_use_same_cookie_without_secret_receipts(self):
        import httpx
        from types import SimpleNamespace
        calls = []
        password = "12345678-1234-4234-8234-123456789abc"
        def serve(request):
            calls.append((request.method, request.url.path, request.headers.get("cookie")))
            if request.url.path == "/api/auth/status":
                return httpx.Response(200, json={"auth_required": True, "authenticated": "phoenix-auth=session" == request.headers.get("cookie")})
            if request.url.path == "/api/auth/login":
                self.assertEqual(json.loads(request.content), {"password": password})
                return httpx.Response(200, json={"ok": True}, headers={"set-cookie": "phoenix-auth=session; Path=/; HttpOnly"})
            self.assertEqual(request.headers.get("cookie"), "phoenix-auth=session")
            if request.url.path == "/api/models":
                return httpx.Response(200, json={"default": "mock-model"})
            return httpx.Response(200, text='event: init\ndata: ready\n\n', headers={"content-type": "text/event-stream"})
        with httpx.Client(base_url="http://127.0.0.1:12345", transport=httpx.MockTransport(serve)) as client:
            opening.authenticate_fixture(client, password)
            adapter = opening.AuthenticatedHarness(SimpleNamespace(marker="delegated"), client)
            self.assertEqual(adapter.marker, "delegated")
            self.assertEqual(adapter._default_model("http://127.0.0.1:12345"), "mock-model")
            observed = []
            adapter._wait_for_sse_signal("http://127.0.0.1:12345", "row",
                lambda kind, data: observed.append((kind, data)) or kind == "init", 20)
        self.assertEqual(observed, [("init", "ready")])
        self.assertNotIn(password, repr(calls))

    def test_parent_auth_disabled_wrong_boolean_and_failed_login_fail_closed(self):
        import httpx
        for initial in ({"auth_required": False, "authenticated": True},
                        {"auth_required": 1, "authenticated": False},
                        {"auth_required": True, "authenticated": 0}):
            methods = []
            def serve(request):
                methods.append(request.method)
                return httpx.Response(200, json=initial)
            with httpx.Client(base_url="http://127.0.0.1:12345", transport=httpx.MockTransport(serve)) as client:
                with self.assertRaises(ValueError): opening.authenticate_fixture(client, "must-not-log")
            self.assertEqual(methods, ["GET"])
        def reject(request):
            if request.method == "GET": return httpx.Response(200, json={"auth_required": True, "authenticated": False})
            return httpx.Response(401, text="private-login-body")
        with httpx.Client(base_url="http://127.0.0.1:12345", transport=httpx.MockTransport(reject)) as client:
            with self.assertRaises(ValueError) as caught: opening.authenticate_fixture(client, "private-password")
            self.assertNotIn("private", str(caught.exception))

    def test_cfgtest_auth_created_after_scrub_not_restored_from_ambient_password(self):
        app = Path(os.environ.get("APP_ROOT", str(opening.ROOT / "target/history-hidden-geometry-repair")))
        source = (app / "crates/phoenix-ide/src/opening_qualifier_tests.rs").read_text()
        self.assertLess(source.index('std::env::remove_var(key)'), source.index('let password = uuid::Uuid::new_v4().to_string()'))
        self.assertLess(source.index('let password = uuid::Uuid::new_v4().to_string()'), source.index('std::env::set_var("PHOENIX_PASSWORD", password)'))
        self.assertIn('.create_new(true)\n        .mode(0o600)', source)
        self.assertIn('.prefix("opening-server-")\n        .permissions(std::fs::Permissions::from_mode(0o700))\n        .tempdir_in(&parent)', source)
        self.assertIn('"credential_file":credential', source)
        self.assertNotIn('std::env::var("PHOENIX_PASSWORD")', source)

    def test_lifecycle_preserves_primary_and_nested_cleanup_failures(self):
        import contextlib
        failures = opening.LifecycleFailures()
        primary = AssertionError("browser report missing")

        @contextlib.contextmanager
        def broken_cleanup(message):
            try:
                yield object()
            finally:
                raise RuntimeError(message)

        with self.assertRaises(AssertionError) as caught:
            with failures.own(broken_cleanup("receiver cleanup"), "receiver"):
                with failures.own(broken_cleanup("server cleanup"), "server"):
                    raise primary
        self.assertIs(caught.exception, primary)
        receipt = failures.receipt()
        self.assertEqual(receipt["primary"]["message"], "browser report missing")
        self.assertEqual([item["message"] for item in receipt["cleanup"]],
                         ["server cleanup", "receiver cleanup"])

    def test_lifecycle_cleanup_only_and_suppression_remain_failures(self):
        import contextlib

        @contextlib.contextmanager
        def cleanup_failure():
            yield None
            raise RuntimeError("cleanup only")

        failures = opening.LifecycleFailures()
        with self.assertRaisesRegex(RuntimeError, "cleanup only"):
            with failures.own(cleanup_failure(), "server"):
                pass
        self.assertEqual(failures.receipt()["primary"]["message"], "cleanup only")
        self.assertEqual(failures.receipt()["cleanup"], [])
        failures = opening.LifecycleFailures()
        with self.assertRaisesRegex(AssertionError, "must not suppress"):
            with failures.own(contextlib.suppress(AssertionError), "browser"):
                raise AssertionError("must not suppress")

    def test_lifecycle_pre_finally_primary_survives_body_cleanup_mask(self):
        import contextlib
        failures = opening.LifecycleFailures()
        primary = AssertionError("original body")
        with self.assertRaises(AssertionError) as caught:
            with failures.own(contextlib.nullcontext(), "server"):
                try:
                    raise primary
                except BaseException as error:
                    failures.remember_primary("server_body", error)
                    raise
                finally:
                    raise RuntimeError("finally failed")
        self.assertIs(caught.exception, primary)
        self.assertEqual(failures.receipt()["cleanup"][0]["message"], "finally failed")

    def test_lifecycle_failure_receipt_is_bounded_and_marks_overflow(self):
        failures = opening.LifecycleFailures()
        failures.remember_primary("case", AssertionError("x" * 2048))
        for index in range(10):
            failures.remember_cleanup("server", RuntimeError(str(index)))
        receipt = failures.receipt()
        self.assertEqual(len(receipt["primary"]["message"]), 1024)
        self.assertTrue(receipt["primary"]["message_truncated"])
        self.assertEqual(len(receipt["cleanup"]), 8)
        self.assertTrue(receipt["cleanup_overflow"])

    def test_lifecycle_receipt_write_failure_cannot_mask_primary(self):
        failures = opening.LifecycleFailures()
        primary = AssertionError("primary before receipt write")
        failures.remember_primary("case", primary)
        with patch.object(opening.shipping, "write_json", side_effect=OSError("receipt failed")):
            with self.assertRaises(AssertionError) as caught:
                failures.write_receipt(self.root / "lifecycle.json")
        self.assertIs(caught.exception, primary)
        self.assertEqual(failures.receipt()["cleanup"][0]["stage"], "failure_receipt")
        failures = opening.LifecycleFailures()
        with patch.object(opening.shipping, "write_json", side_effect=OSError("receipt failed")):
            with self.assertRaises(OSError):
                failures.write_receipt(self.root / "lifecycle.json")

    def test_forced_shutdown_zero_is_not_normal_and_retains_control_error(self):
        from unittest.mock import Mock
        control = self.root / "fake-control.sock"
        control.touch()
        failures = opening.LifecycleFailures()
        with patch.object(opening.socket, "socket", side_effect=OSError("control failed")):
            opening.forced_server_shutdown(Mock(poll=Mock(return_value=0)),
                                           self.root, control, failures)
        receipt = json.loads((self.root / "forced-server-shutdown.json").read_text())
        self.assertEqual(receipt["status"], "forced_shutdown_attempt")
        self.assertEqual(receipt["returncode_after"], 0)
        self.assertFalse(receipt["stop_response_received"])
        self.assertEqual(receipt["error_type"], "OSError")
        self.assertEqual(failures.receipt()["cleanup"][0]["stage"], "server_control_stop")
        self.assertFalse((self.root / "server-completion.json").exists())

    def test_actual_qualify_failure_path_preserves_primary_and_forced_zero(self):
        import contextlib
        from unittest.mock import Mock
        output = self.root / "qualification"
        output.mkdir()
        server = Mock(poll=Mock(return_value=0))

        @contextlib.contextmanager
        def process(*args):
            try:
                yield server
            finally:
                raise RuntimeError("process cleanup failed")

        with patch.object(opening.shipping, "load_app_harness", return_value=Mock()), \
                patch.object(opening.shipping, "owned_process", side_effect=process), \
                patch.object(opening, "readiness", side_effect=AssertionError("startup primary")):
            with self.assertRaisesRegex(AssertionError, "startup primary"):
                opening.qualify(self.root, self.exe, output)
        case = output / "ordinary"
        completion = json.loads((case / "server-completion.json").read_text())
        self.assertEqual(completion, {"status": "force_cleanup_required", "returncode": 0})
        forced = json.loads((case / "forced-server-shutdown.json").read_text())
        self.assertEqual(forced["status"], "forced_shutdown_attempt")
        self.assertEqual(forced["returncode_after"], 0)
        failures = json.loads((case / "lifecycle-failures.json").read_text())
        self.assertEqual(failures["primary"]["message"], "startup primary")
        self.assertEqual(failures["cleanup"][0]["message"], "process cleanup failed")
        self.assertFalse((output / "global").exists())

    def test_main_partial_forced_receipt_preserves_original_and_publishes_failed_summary(self):
        from unittest.mock import Mock
        output = self.root / "main-partial"
        primary = AssertionError("startup primary")
        writer = opening.shipping.write_json

        def partial_write(path, value):
            if path.name == "forced-server-shutdown.json":
                path.write_text('{"status":')
                raise OSError("partial forced receipt")
            return writer(path, value)

        argv = ["qualify", "--app-root", str(self.root), "--instrumented-exe", str(self.exe),
                "--instrumented-receipt", str(self.root / "input.json"), "--output", str(output)]
        server = Mock(poll=Mock(return_value=0), pid=1234)
        with patch.object(opening.sys, "argv", argv), \
                patch.object(opening.signal, "signal"), patch.object(opening.signal, "alarm"), \
                patch.object(opening, "identity", return_value={}), \
                patch.object(opening.shipping, "load_app_harness", return_value=Mock()), \
                patch.object(opening.shipping.subprocess, "Popen", return_value=server), \
                patch.object(opening.shipping, "stop"), \
                patch.object(opening, "readiness", side_effect=primary), \
                patch.object(opening.shipping, "write_json", side_effect=partial_write):
            with self.assertRaises(AssertionError) as caught:
                opening.main()
        self.assertIs(caught.exception, primary)
        summary = json.loads((output / "result.json").read_text())
        self.assertEqual(summary["status"], "failed")
        self.assertIn("startup primary", summary["error"])
        self.assertEqual(summary["cases"]["ordinary"]["forced_shutdown"]["status"], "invalid_evidence")
        self.assertEqual(summary["aggregation_errors"][0]["file"], "forced-server-shutdown.json")
        self.assertEqual(summary["cases"]["ordinary"]["lifecycle_failures"]["cleanup"][0]["type"], "OSError")
        self.assertEqual(summary["cases"]["global"]["lifecycle_failures"]["status"], "not_observed")

    def test_main_invalid_receipt_cannot_promote_success(self):
        output = self.root / "main-invalid"
        def qualify(*args):
            case = output / "ordinary"
            case.mkdir()
            (case / "lifecycle-failures.json").write_text("[]")
        argv = ["qualify", "--app-root", str(self.root), "--instrumented-exe", str(self.exe),
                "--instrumented-receipt", str(self.root / "input.json"), "--output", str(output)]
        with patch.object(opening.sys, "argv", argv), \
                patch.object(opening.signal, "signal"), patch.object(opening.signal, "alarm"), \
                patch.object(opening, "identity", return_value={}), \
                patch.object(opening, "qualify", side_effect=qualify):
            with self.assertRaisesRegex(RuntimeError, "receipt aggregation failed"):
                opening.main()
        self.assertEqual(json.loads((output / "result.json").read_text())["status"], "failed")

    def test_main_summary_write_failure_preserves_primary(self):
        output = self.root / "main-summary-write"
        primary = AssertionError("qualification primary")
        writer = opening.shipping.write_json
        def write(path, value):
            if path.name == "result.json":
                raise OSError("summary failed")
            return writer(path, value)
        argv = ["qualify", "--app-root", str(self.root), "--instrumented-exe", str(self.exe),
                "--instrumented-receipt", str(self.root / "input.json"), "--output", str(output)]
        with patch.object(opening.sys, "argv", argv), \
                patch.object(opening.signal, "signal"), patch.object(opening.signal, "alarm"), \
                patch.object(opening, "identity", return_value={}), \
                patch.object(opening, "qualify", side_effect=primary), \
                patch.object(opening.shipping, "write_json", side_effect=write):
            with self.assertRaises(AssertionError) as caught:
                opening.main()
        self.assertIs(caught.exception, primary)
        self.assertIsInstance(primary.__cause__, OSError)

    def test_normal_completion_primary_survives_partial_receipt_failure(self):
        import contextlib
        from unittest.mock import Mock
        for code in (None, 1):
            failures = opening.LifecycleFailures()
            server = Mock(returncode=code)
            if code is None:
                server.wait.side_effect = opening.shipping.subprocess.TimeoutExpired("fake", 65)
            def write_partial(path, value):
                path.write_text('{"status":')
                raise OSError("partial normal receipt")
            with patch.object(opening.shipping, "write_json", side_effect=write_partial):
                with self.assertRaises((TimeoutError, RuntimeError)) as caught:
                    with failures.own(contextlib.nullcontext(server), "server"):
                        opening.normal_server_completion(server, self.root,
                            {"home": str(self.root / "missing")}, self.root / "missing.sock", failures)
            self.assertIs(failures.primary[1], caught.exception)
            self.assertIs(type(caught.exception), TimeoutError if code is None else RuntimeError)
            self.assertEqual(failures.receipt()["cleanup"][0]["stage"], "normal_completion_receipt")

    def test_normal_server_completion_requires_zero_and_owned_cleanup(self):
        from unittest.mock import Mock
        home = self.root / "removed-driver-home"
        control = self.root / "removed-control.sock"
        server = Mock(returncode=0)
        receipt = opening.normal_server_completion(server, self.root, {"home": str(home)}, control, opening.LifecycleFailures())
        server.wait.assert_called_once_with(timeout=65)
        self.assertEqual(receipt["status"], "success")
        self.assertEqual(json.loads((self.root / "server-completion.json").read_text()), receipt)

    def test_normal_server_completion_signal_nonzero_and_timeout_fail(self):
        from unittest.mock import Mock
        for code, status in [(-15, "signal_exit"), (1, "nonzero_exit"), (None, "timeout")]:
            server = Mock(returncode=code)
            if code is None:
                server.wait.side_effect = opening.shipping.subprocess.TimeoutExpired("owned-fake", 65)
            with self.subTest(code=code), self.assertRaises((RuntimeError, TimeoutError)):
                opening.normal_server_completion(server, self.root, {"home": str(self.root / "missing")}, self.root / "missing.sock", opening.LifecycleFailures())
            receipt = json.loads((self.root / "server-completion.json").read_text())
            self.assertEqual(receipt["status"], status)
            self.assertEqual(receipt["returncode"], code)
            server.wait.assert_called_once_with(timeout=65)

    def test_normal_server_completion_cleanup_overrides_success_or_signal(self):
        from unittest.mock import Mock
        home = self.root / "owned-driver-home"
        control = self.root / "owned-control.sock"
        for remaining in (home, control):
            remaining.mkdir() if remaining == home else remaining.touch()
            for code in (0, -15):
                with self.subTest(remaining=remaining, code=code), self.assertRaisesRegex(RuntimeError, "owned resources"):
                    opening.normal_server_completion(Mock(returncode=code), self.root, {"home": str(home)}, control, opening.LifecycleFailures())
                receipt = json.loads((self.root / "server-completion.json").read_text())
                self.assertEqual(receipt["status"], "cleanup_failed")
                self.assertEqual(receipt["exit_status"], "success" if code == 0 else "signal_exit")
            remaining.rmdir() if remaining == home else remaining.unlink()

    def test_normal_wait_precedes_final_forcecleanup_and_result_requires_receipt(self):
        import ast
        source = Path(opening.__file__).read_text()
        qualify = next(node for node in ast.parse(source).body if isinstance(node, ast.FunctionDef) and node.name == "qualify")
        normal = next(n for n in ast.walk(qualify) if isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "normal_server_completion")
        owned = next(n for n in ast.walk(qualify) if isinstance(n, ast.With) and 'as server' in ast.unparse(n.items[0]))
        self.assertGreater(normal.lineno, owned.lineno)
        self.assertLess(normal.lineno, owned.end_lineno)
        self.assertIn('"status": "force_cleanup_required"', source)
        self.assertIn('"server_completion":', source)

    def test_source_tree_base_and_runtime_module_hashes_fail_closed(self):
        for wanted in (("rev-parse", "HEAD^{tree}"),
                       ("merge-base", opening.PIN, opening.BASE_PIN)):
            def wrong_git(root, *args):
                return "0" * 40 if args == wanted else self.git(root, *args)
            with self.subTest(git=wanted), patch.object(opening.shipping, "git", wrong_git), \
                    patch.object(opening.shipping, "digest", self.digest), self.assertRaises(ValueError):
                opening.identity(self.app, self.exe, self.receipt)
        changed = next(iter(opening.TELEMETRY_MODULES))
        def wrong_digest(path):
            try:
                relative = str(Path(path).relative_to(self.app))
            except ValueError:
                relative = None
            return "0" * 64 if relative == changed else self.digest(path)
        with patch.object(opening.shipping, "git", self.git), \
                patch.object(opening.shipping, "digest", wrong_digest), self.assertRaises(ValueError):
            opening.identity(self.app, self.exe, self.receipt)

    def test_telemetry_environment_drops_ambient_and_requires_owned_quartet(self):
        run_id = "history-ordinary-1234"
        receiver = types.SimpleNamespace(
            endpoint="http://127.0.0.1:4318/v1/traces",
            service=f"phoenix-view-qualification-{run_id}")
        receiver.env = {"PHOENIX_TRACE_EXPORTER": "otlp",
                        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL": "http/protobuf",
                        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT": receiver.endpoint,
                        "OTEL_SERVICE_NAME": receiver.service}
        value = opening.telemetry_environment({"PATH": "/bin", "OTEL_SDK_DISABLED": "true",
            "DD_API_KEY": "ambient", "VENDOR_OTLP_HEADERS": "secret"}, receiver)
        self.assertEqual(set(value), {"PATH"} | opening.TELEMETRY_KEYS)
        for key, bad in [("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://localhost:4318/v1/traces"),
                         ("OTEL_SERVICE_NAME", "shared-service")]:
            changed = dict(receiver.env); changed[key] = bad
            with self.subTest(key=key), self.assertRaises(ValueError):
                opening.telemetry_environment({"PATH": "/bin"},
                    types.SimpleNamespace(endpoint=receiver.endpoint, service=receiver.service,
                                          env=changed))
        changed = dict(receiver.env); changed["OTEL_EXPORTER_OTLP_HEADERS"] = "secret"
        with self.assertRaises(ValueError):
            opening.telemetry_environment({"PATH": "/bin"},
                types.SimpleNamespace(endpoint=receiver.endpoint, service=receiver.service,
                                      env=changed))

    def test_joint_fixture_uses_actual_ordinary_and_unique_global_identities(self):
        ordinary = {"kind": "ordinary", "route": "/conversation/ordinary-latest",
            "endpoint": "/api/product-conversations/ordinary-product",
            "members": ["ordinary-root", "ordinary-latest"],
            "latest_transcript_row_id": "ordinary-latest",
            "product_conversation_id": "ordinary-product"}
        global_ = {"kind": "global", "route": "/global/global-latest",
            "endpoint": "/api/global/coordinator/history/global-root",
            "members": ["global-root", "global-latest"],
            "latest_transcript_row_id": "global-latest",
            "product_conversation_id": "global-product"}
        identity = {"appSource": opening.PIN, "frontendSource": opening.PIN,
                    "serverInstanceId": "server"}
        result = opening.joint_fixture(identity, [global_, ordinary])
        self.assertEqual(set(result), {"identity", "aggregates", "lifecycle", "forbidden"})
        self.assertEqual(result["lifecycle"]["route"], ordinary["route"])
        self.assertEqual(result["lifecycle"]["productConversationId"], "ordinary-product")
        self.assertIn("statebar-slug", result["lifecycle"]["spaSelector"])
        for field, bad in [("route", "/global/old-root"),
                           ("product_conversation_id", "global-root")]:
            broken = dict(global_); broken[field] = bad
            with self.subTest(field=field), self.assertRaises(ValueError):
                opening.joint_fixture(identity, [ordinary, broken])
        broken = dict(global_); broken["persisted_rows"] = float("nan")
        with self.assertRaises(ValueError):
            opening.joint_fixture(identity, [ordinary, broken])

    def test_actual_server_identity_uses_deployment_and_typed_owned_database(self):
        database = self.root / "identity.db"
        import sqlite3
        with sqlite3.connect(database) as connection:
            connection.execute("CREATE TABLE instance_identity (singleton_key INTEGER, instance_id TEXT)")
            instance = str(opening.shipping.uuid.uuid4())
            connection.execute("INSERT INTO instance_identity VALUES (1, ?)", (instance,))
        fixture = types.SimpleNamespace(request=lambda method, path: {
            "build": {"git_sha": opening.PIN}})
        startup = {"git_sha": opening.PIN, "db": str(database)}
        self.assertEqual(opening.actual_server_identity(startup, fixture), {
            "appSource": opening.PIN, "frontendSource": opening.PIN,
            "serverInstanceId": instance})
        with sqlite3.connect(database) as connection:
            connection.execute("UPDATE instance_identity SET instance_id = 'not-typed'")
        with self.assertRaises(ValueError):
            opening.actual_server_identity(startup, fixture)
        fixture.request = lambda method, path: {"build": {"git_sha": "0" * 40}}
        with self.assertRaises(ValueError):
            opening.actual_server_identity(startup, fixture)

    def test_executed_helper_bytes_are_in_before_after_identity(self):
        helper = opening.ROOT / "ui/scripts/history-joint-telemetry.mjs"
        baseline = self.identity()
        self.assertIn("ui/scripts/history-joint-telemetry.mjs", baseline["harness_files"])
        self.assertIn("ui/scripts/history-postfault-baseline.mjs", baseline["harness_files"])
        original_digest = self.digest
        def changed_digest(path):
            return "changed-helper-bytes" if Path(path) == helper else original_digest(path)
        with patch.object(self, "digest", changed_digest):
            changed = self.identity()
        self.assertNotEqual(baseline, changed)
        self.assertIn('assert identity(app, exe, args.instrumented_receipt) == before, "artifact identity changed"', Path(opening.__file__).read_text())

    def test_early_receiver_expiry_and_stopped_worker_fail_liveness(self):
        for expired, serving in [(True, True), (True, False), (False, False)]:
            with self.subTest(expired=expired, serving=serving), self.assertRaises(RuntimeError):
                opening.receiver_serving_check(types.SimpleNamespace(expired=expired, is_serving=lambda: serving))
        opening.receiver_serving_check(types.SimpleNamespace(expired=False, is_serving=lambda: True))

    def test_receiver_liveness_is_observed_inside_context_after_normal_app_completion(self):
        source = Path(opening.__file__).read_text()
        self.assertIn("lifetime_seconds=650", source)
        normal = source.index("completion = normal_server_completion(")
        check = source.index("receiver_serving_check(receiver)", normal)
        exit_check = source.index("receiver_stopped = receiver_is_stopped(receiver)", check)
        self.assertLess(normal, check)
        self.assertLess(check, exit_check)
        self.assertIn('"receiver_expired": receiver.expired', source)
        self.assertIn('"receiver_serving": receiver.is_serving()', source)

    def test_joint_source_orders_app_then_receiver_stop_before_final_assertion(self):
        import ast
        qualify = next(node for node in ast.parse(Path(opening.__file__).read_text()).body
                       if isinstance(node, ast.FunctionDef) and node.name == "qualify")
        calls = [node for node in ast.walk(qualify) if isinstance(node, ast.Call)]
        startup = next(node for node in calls if isinstance(node.func, ast.Name)
                       and node.func.id == "receiver_startup_check")
        normal = next(node for node in calls if isinstance(node.func, ast.Name)
                      and node.func.id == "normal_server_completion")
        stopped = next(node for node in calls if isinstance(node.func, ast.Name)
                       and node.func.id == "receiver_is_stopped")
        final = next(node for node in calls if isinstance(node.func, ast.Attribute)
                     and node.func.attr == "assertPhoenixTelemetry")
        receiver_with = next(node for node in ast.walk(qualify) if isinstance(node, ast.With)
                             and any(isinstance(item.context_expr, ast.Call)
                                     and isinstance(item.context_expr.func, ast.Attribute)
                                     and item.context_expr.func.attr == "own"
                                     and isinstance(item.context_expr.args[0], ast.Name)
                                     and item.context_expr.args[0].id == "receiver_context"
                                     for item in node.items))
        self.assertLess(startup.lineno, normal.lineno)
        self.assertLess(normal.lineno, receiver_with.end_lineno)
        self.assertGreater(stopped.lineno, receiver_with.end_lineno)
        self.assertGreater(final.lineno, stopped.lineno)
        source = ast.unparse(qualify)
        self.assertIn("'app_stopped': app_stopped", source)
        self.assertIn("'receiver_stopped': receiver_stopped", source)
        self.assertIn("telemetry/telemetry-browser-result.json", source)
        self.assertNotIn("'app_stopped': True", source)
        self.assertNotIn("'receiver_stopped': True", source)

    def test_missing_or_invalid_joint_browser_producer_receipt_fails(self):
        path = self.root / "telemetry" / "telemetry-browser-result.json"
        with self.assertRaisesRegex(RuntimeError, "producer receipt is missing"):
            opening.telemetry_browser_result(path)
        path.parent.mkdir()
        path.write_text(json.dumps({"runId": "missing-journeys"}))
        with self.assertRaisesRegex(ValueError, "producer receipt is invalid"):
            opening.telemetry_browser_result(path)
        expected = {"runId": "run", "journeys": []}
        path.write_text(json.dumps(expected))
        self.assertEqual(opening.telemetry_browser_result(path), expected)

    def test_no_compiler_or_shipping_server_invocation(self):
        source = Path(opening.__file__).read_text()
        self.assertNotIn("h._server()", source)
        self.assertNotIn("subprocess.run", source)
        self.assertNotIn("subprocess.check_output", source)
        self.assertIn("--ignored", source)
        self.assertIn("Fixture(h, base, case)", source)


class FixtureSettlementTests(unittest.TestCase):
    def setUp(self):
        import sqlite3
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.db = self.root / "owned.db"
        self.instance = str(opening.shipping.uuid.uuid4())
        with opening.contextlib.closing(sqlite3.connect(self.db)) as db, db:
            db.executescript("CREATE TABLE instance_identity(singleton_key INTEGER, instance_id TEXT);"
                             "CREATE TABLE durable_turns(conversation_id TEXT, canonical_message_id TEXT,"
                             "disposition TEXT, terminal_kind TEXT, owns_conversation INTEGER);"
                             "CREATE TABLE steering_messages(conversation_id TEXT);")
            db.execute("INSERT INTO instance_identity VALUES(1, ?)", (self.instance,))
            db.execute("INSERT INTO durable_turns VALUES('row', 'row:key', 'Runtime', NULL, 1)")
        self.idle = True
        self.correlation = "row:key"
        self.calls = []
        self.fixture = types.SimpleNamespace(request=self.request)
        self.observer = opening.OwnedFixtureCompletion(
            {"db": str(self.db), "git_sha": opening.PIN}, self.fixture)
        self.data = {"conversation": {"state": {"type": "idle"}},
                     "messages": [{"message_id": "row:key", "message_type": "user"}]}

    def request(self, method, path, body=None):
        self.calls.append((method, path, body))
        if path == "/api/deployment":
            return {"build": {"git_sha": opening.PIN}}
        return {"conversation_idle": self.idle, "entries": [
            {"message_id": self.correlation, "status": "persisted"}]}

    def sql(self, statement, args=()):
        import sqlite3
        with opening.contextlib.closing(sqlite3.connect(self.db)) as db, db:
            db.execute(statement, args)

    def test_correlated_terminal_release_and_live_idle_are_both_required(self):
        self.assertFalse(self.observer("row", False, "row:key", self.data))
        self.sql("UPDATE durable_turns SET terminal_kind='Completed', owns_conversation=0")
        self.idle = False
        self.assertFalse(self.observer("row", False, "row:key", self.data))
        self.idle = True
        self.assertTrue(self.observer("row", False, "row:key", self.data))
        self.assertFalse(self.observer("row", False, "row:foreign", self.data))
        self.sql("INSERT INTO durable_turns VALUES('row', 'row:next', 'Runtime', NULL, 1)")
        self.assertFalse(self.observer("row", False, "row:key", self.data))

    def test_empty_owner_set_does_not_hide_backlog_or_foreign_message(self):
        self.sql("UPDATE durable_turns SET terminal_kind='Completed', owns_conversation=0")
        self.sql("INSERT INTO steering_messages VALUES('row')")
        with self.assertRaisesRegex(AssertionError, "steering backlog"):
            self.observer("row", False, "row:key", self.data)
        self.sql("DELETE FROM steering_messages")
        self.correlation = "row:foreign"
        with self.assertRaisesRegex(ValueError, "reconciliation identity"):
            self.observer("row", False, "row:key", self.data)

    def test_opening_wait_resolves_persisted_canonical_identity_and_requires_terminal(self):
        self.assertFalse(self.observer("row", False, None, self.data))
        self.sql("UPDATE durable_turns SET terminal_kind='Completed', owns_conversation=0")
        self.assertTrue(self.observer("row", False, None, self.data))
        self.sql("UPDATE durable_turns SET terminal_kind='Failed'")
        self.assertFalse(self.observer("row", False, None, self.data))
        self.sql("UPDATE durable_turns SET terminal_kind='Completed', conversation_id='foreign'")
        self.assertFalse(self.observer("row", False, None, self.data))
        self.assertFalse(self.observer("row", False, None, {"messages": []}))

    def test_exhausted_requires_failed_exact_turn_and_released_owner(self):
        self.assertFalse(self.observer("row", True, "row:key", self.data))
        self.sql("UPDATE durable_turns SET terminal_kind='Completed', owns_conversation=0")
        self.assertFalse(self.observer("row", True, "row:key", self.data))
        self.sql("UPDATE durable_turns SET terminal_kind='Failed'")
        self.assertTrue(self.observer("row", True, "row:key", self.data))

    def test_database_errors_and_identity_changes_fail_closed(self):
        import sqlite3
        self.sql("UPDATE instance_identity SET instance_id='foreign'")
        with self.assertRaisesRegex(ValueError, "server identity"):
            self.observer("row", False, "row:key", self.data)
        self.sql("UPDATE instance_identity SET instance_id=?", (self.instance,))
        self.sql("DROP TABLE durable_turns")
        with self.assertRaises(sqlite3.OperationalError):
            self.observer("row", False, "row:key", self.data)
        self.db.unlink()
        with self.assertRaises(FileNotFoundError):
            self.observer("row", False, "row:key", self.data)
        self.assertFalse(self.db.exists())

    def test_existing_wait_cannot_return_from_idle_and_agent_before_real_completion(self):
        reads = []
        harness = types.SimpleNamespace(_terminal_event=lambda *a: True,
            _context_exhausted_event=lambda *a: True,
            _wait_for_sse_signal=lambda *a: None,
            _state_str=lambda state: state["type"], _has_agent_after_message=lambda *a: True)
        fixture = opening.shipping.Fixture(harness, "http://127.0.0.1:1", self.root,
                                          completion=self.observer)
        self.addCleanup(fixture.client.close)
        def request(method, path, body=None):
            if method == "POST":
                self.assertEqual(path, "/api/conversations/row/chat")
                self.assertEqual(body["message_id"], "key")
                return {"queued": True, "steering": False}
            reads.append(path)
            if len(reads) == 2:
                self.sql("UPDATE durable_turns SET terminal_kind='Completed', owns_conversation=0")
            self.idle = len(reads) >= 3
            return self.data
        fixture.request = request
        with patch.object(opening.shipping.uuid, "uuid4", return_value="key"), \
                patch.object(opening.shipping.time, "sleep") as no_elapsed_delay:
            result = fixture.send("row", "fixture turn")
        self.assertIs(result, self.data)
        self.assertEqual(len(reads), 3)
        self.assertEqual(no_elapsed_delay.call_count, 2)
        self.assertEqual(len([c for c in self.calls if c[1].endswith("/messages/reconcile")]), 2)


if __name__ == "__main__": unittest.main()
