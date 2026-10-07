import importlib.util
import re
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def load_harness():
    path = ROOT / "tests/integration/launchd_deploy_harness.py"
    spec = importlib.util.spec_from_file_location("launchd_deploy_harness_test", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


harness = load_harness()
FULL_SHA = re.compile(r"[0-9a-f]{40}")
LEGACY_SHA = re.compile(r"[0-9a-f]{12}")


class IdentityFixtureTests(unittest.TestCase):
    def test_candidate_identity_is_exact_full_source_commit(self):
        fixture = harness.deployment_identity_fixture(healthy_candidate=True)

        self.assertIsNotNone(FULL_SHA.fullmatch(fixture["source_commit"]))
        self.assertEqual(fixture["source_commit"], fixture["expected"]["git_sha"])
        self.assertIsNotNone(FULL_SHA.fullmatch(fixture["previous_deployed_sha"]))

    def test_failed_candidate_keeps_legacy_runtime_only_for_rollback(self):
        fixture = harness.deployment_identity_fixture(healthy_candidate=False)

        self.assertIsNotNone(LEGACY_SHA.fullmatch(fixture["previous"]["git_sha"]))
        self.assertEqual(
            harness.LEGACY_ROLLBACK_RUNTIME_SHA,
            fixture["previous"]["git_sha"],
        )
        self.assertIsNotNone(FULL_SHA.fullmatch(fixture["previous_deployed_sha"]))
        self.assertNotEqual(
            fixture["previous"]["git_sha"],
            fixture["previous_deployed_sha"],
        )

    def test_restart_identity_is_full_deployed_commit(self):
        fixture = harness.restart_identity_fixture()

        self.assertIsNotNone(FULL_SHA.fullmatch(fixture["expected"]["git_sha"]))
        self.assertEqual(fixture["deployed_sha"], fixture["expected"]["git_sha"])


if __name__ == "__main__":
    unittest.main()
