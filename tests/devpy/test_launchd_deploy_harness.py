"""Fixture and production-boundary checks; this module never runs launchctl."""
import importlib.util
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("launchd_deploy_harness_test", ROOT / "tests/integration/launchd_deploy_harness.py")
harness = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = harness
spec.loader.exec_module(harness)


class FixtureTests(unittest.TestCase):
    def test_candidate_identity_is_exact_full_source_commit(self):
        fixture = harness.deployment_identity_fixture()
        self.assertIsNotNone(re.fullmatch(r"[0-9a-f]{40}", fixture["source_commit"]))
        self.assertEqual(fixture["source_commit"], fixture["expected"]["git_sha"])
        self.assertIsNotNone(re.fullmatch(r"[0-9a-f]{40}", fixture["previous"]["git_sha"]))

    def test_restart_identity_is_full_deployed_commit(self):
        fixture = harness.restart_identity_fixture()
        self.assertEqual(fixture["deployed_sha"], fixture["expected"]["git_sha"])
        self.assertIsNotNone(re.fullmatch(r"[0-9a-f]{40}", fixture["deployed_sha"]))

    def test_generated_candidate_scripts_compile_and_have_distinct_migration_action(self):
        for failure in (None, "migration", "health"):
            source = harness.server_script(harness.deployment_identity_fixture()["expected"],
                                           healthy=failure != "health", migration_failure=failure == "migration")
            compile(source, "candidate", "exec")
            self.assertIn('--build-identity', source)
            self.assertIn('--migrate-only', source)
            self.assertIn('migration_started', source)
            self.assertIn('raise SystemExit(7)', source)
        compile(harness.socket_activated_server_script(harness.restart_identity_fixture()["expected"]), "restart", "exec")

    def test_disposable_guard_requires_temp_root_test_label_and_nonproduction_port(self):
        with tempfile.TemporaryDirectory(prefix="phoenix-launchd-harness-") as td:
            root = Path(td)
            harness.refuse_live("test.phoenix-ide.server.unique", root, 12345)
            invalid = (
                (harness.LIVE_LABEL, root, 12345),
                ("another.application", root, 12345),
                ("test.phoenix-ide.server.unique", root, harness.LIVE_PORT),
                ("test.phoenix-ide.server.unique", root, 0),
                ("test.phoenix-ide.server.unique", harness.LIVE_HOME, 12345),
                ("test.phoenix-ide.server.unique", Path.home() / "Library/LaunchAgents", 12345),
                ("test.phoenix-ide.server.unique", Path("/etc"), 12345),
            )
            for label, path, port in invalid:
                with self.subTest(label=label, path=path, port=port), self.assertRaises(SystemExit):
                    harness.refuse_live(label, path, port)
            with tempfile.TemporaryDirectory(prefix="ordinary-temporary-") as ordinary:
                with self.assertRaises(SystemExit):
                    harness.refuse_live("test.phoenix-ide.server.unique", Path(ordinary), 12345)

    def test_harness_refuses_production_root_before_any_subprocess(self):
        with mock.patch.object(harness.subprocess, "run", side_effect=AssertionError("must not execute")), mock.patch.object(harness.subprocess, "Popen", side_effect=AssertionError("must not execute")):
            for action in (harness.run_scenario, harness.run_restart_scenario):
                with self.assertRaises(SystemExit):
                    action(harness.LIVE_HOME, f"gui/{os.getuid()}")


if __name__ == "__main__":
    unittest.main()
