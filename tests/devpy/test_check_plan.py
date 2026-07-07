import importlib.util
import os
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load_devpy():
    spec = importlib.util.spec_from_file_location("devpy_under_test", ROOT / "dev.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class CheckPlanTests(unittest.TestCase):
    def setUp(self):
        self.dev = load_devpy()

    def test_ci_lane_inventory_covers_every_devpy_lane_once(self):
        self.assertEqual([], self.dev._ci_lane_inventory_errors())

    def test_pr_433_task_only_rename_activates_only_task_group(self):
        paths = {"tasks/45002-p1-done--deterministic-message-scroll-state-machine.md"}
        with mock.patch.dict(os.environ, {"CI": "1", "PHOENIX_CHECK_BASE": "origin/main"}, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value=paths):
            active, skipped = self.dev._resolve_check_lanes()

        self.assertEqual({"task"}, active)
        self.assertIn("rust", skipped)
        self.assertIn("clippy", skipped)
        self.assertIn("e2e", skipped)
        self.assertIn("ui-lint", skipped)
        self.assertEqual(
            {"clippy": False, "e2e": False, "fast": True, "rust": False, "ui": False},
            self.dev._ci_groups_for_active_lanes(active),
        )

    def test_ui_change_activates_ui_group_without_task_group(self):
        paths = {"ui/src/App.tsx"}
        with mock.patch.dict(os.environ, {"CI": "1", "PHOENIX_CHECK_BASE": "origin/main"}, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value=paths):
            active, _skipped = self.dev._resolve_check_lanes()

        self.assertIn("tsc", active)
        self.assertIn("ui-lint", active)
        self.assertIn("vitest", active)
        self.assertIn("pkglock", active)
        self.assertIn("task", active)
        groups = self.dev._ci_groups_for_active_lanes(active)
        self.assertTrue(groups["ui"])
        self.assertTrue(groups["fast"])

    def test_devpy_change_runs_every_ci_group(self):
        with mock.patch.dict(os.environ, {"CI": "1", "PHOENIX_CHECK_BASE": "origin/main"}, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value={"dev.py"}):
            active, skipped = self.dev._resolve_check_lanes()

        self.assertEqual(set(), set(skipped))
        self.assertEqual(self.dev._all_lanes(), active)
        self.assertTrue(all(self.dev._ci_groups_for_active_lanes(active).values()))

    def test_workflow_change_runs_every_ci_group(self):
        with mock.patch.dict(os.environ, {"CI": "1", "PHOENIX_CHECK_BASE": "origin/main"}, clear=False), \
             mock.patch.object(self.dev, "_changed_paths_vs_base", return_value={".github/workflows/ci.yml"}):
            active, skipped = self.dev._resolve_check_lanes()

        self.assertEqual(set(), set(skipped))
        self.assertEqual(self.dev._all_lanes(), active)
        self.assertTrue(all(self.dev._ci_groups_for_active_lanes(active).values()))

    def test_workflow_check_commands_match_ci_lane_groups(self):
        workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        for group, lanes in self.dev._CI_LANE_GROUPS.items():
            if group == "fast":
                expected = "./dev.py check --lanes task"
            else:
                expected = "./dev.py check --lanes " + ",".join(
                    lane for lane in workflow_order(group) if lane in lanes
                )
            self.assertIn(expected, workflow)


def workflow_order(group):
    return {
        "rust": ["rust", "cargo-fmt"],
        "clippy": ["clippy"],
        "e2e": ["e2e"],
        "ui": ["tsc", "ui-lint", "vitest", "ast-grep", "allium", "spec-shape", "spec-anchors", "pkglock"],
    }[group]


if __name__ == "__main__":
    unittest.main()
