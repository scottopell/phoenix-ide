#!/usr/bin/env python3

import subprocess
import sys
import unittest
from pathlib import Path

from release_version import ReleaseVersion, latest_supported, parse_tag


class ReleaseVersionTest(unittest.TestCase):
    def test_parses_supported_stable_and_rc_versions(self) -> None:
        self.assertEqual(str(ReleaseVersion.parse("0.13.0")), "0.13.0")
        self.assertEqual(str(ReleaseVersion.parse("1.2.3-rc.98")), "1.2.3-rc.98")
        self.assertEqual(str(parse_tag("v1.2.3-rc.4")), "1.2.3-rc.4")

    def test_rejects_unsupported_syntax(self) -> None:
        for value in (
            "v1.2.3",
            "1.2",
            "1.2.3.4",
            "1.2.3-rc1",
            "1.2.3-rc.01",
            "1.2.3-beta.1",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            " 1.2.3",
        ):
            with self.subTest(value=value), self.assertRaises(ValueError):
                ReleaseVersion.parse(value)

    def test_enforces_component_and_rc_bounds(self) -> None:
        for value in (
            "9999.0.0",
            "1.100.0",
            "1.2.99",
            "1.2.3-rc.0",
            "1.2.3-rc.99",
            "0.12.98-rc.1",
        ):
            with self.subTest(value=value), self.assertRaises(ValueError):
                ReleaseVersion.parse(value)
        self.assertEqual(str(ReleaseVersion.parse("9998.99.98-rc.98")), "9998.99.98-rc.98")

    def test_default_bump_advances_patch_or_rc(self) -> None:
        self.assertEqual(str(ReleaseVersion.parse("1.2.3").next_default()), "1.2.4")
        self.assertEqual(str(ReleaseVersion.parse("1.2.3-rc.3").next_default()), "1.2.3-rc.4")
        with self.assertRaises(ValueError):
            ReleaseVersion.parse("1.2.98").next_default()
        with self.assertRaises(ValueError):
            ReleaseVersion.parse("1.2.3-rc.98").next_default()

    def test_latest_supported_orders_rcs_and_final_stable(self) -> None:
        self.assertEqual(str(latest_supported(["v0.12.0", "v0.13.0-rc.1", "junk"])), "0.13.0-rc.1")
        self.assertEqual(str(latest_supported(["v0.13.0-rc.2", "v0.13.0-rc.10"])), "0.13.0-rc.10")
        self.assertEqual(str(latest_supported(["v0.13.0-rc.10", "v0.13.0"])), "0.13.0")
        self.assertIsNone(latest_supported(["junk", "v1.2.3-beta.1"]))

    def test_next_from_tags_handles_rc_history(self) -> None:
        helper = Path(__file__).with_name("release_version.py")
        result = subprocess.run(
            [sys.executable, str(helper), "next-from-tags"],
            input="v0.12.0\nv0.13.0-rc.1\nv0.13.0-rc.2\n",
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.stdout, "0.13.0-rc.3\n")

    def test_new_version_must_follow_release_history(self) -> None:
        helper = Path(__file__).with_name("release_version.py")
        history = "v0.12.0\nv0.13.0-rc.2\n"
        accepted = subprocess.run(
            [sys.executable, str(helper), "validate-new-from-tags", "0.13.0-rc.3"],
            input=history,
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(accepted.stdout, "0.13.0-rc.3\n")
        for rejected in ("0.12.1", "0.13.0-rc.1", "0.13.0-rc.2"):
            with self.subTest(rejected=rejected):
                result = subprocess.run(
                    [sys.executable, str(helper), "validate-new-from-tags", rejected],
                    input=history,
                    capture_output=True,
                    text=True,
                )
                self.assertNotEqual(result.returncode, 0)
        after_final = subprocess.run(
            [sys.executable, str(helper), "validate-new-from-tags", "0.13.0-rc.3"],
            input="v0.13.0\n",
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(after_final.returncode, 0)

    def test_preserves_historical_stable_apple_mapping(self) -> None:
        version = ReleaseVersion.parse("0.12.98")
        self.assertEqual(version.apple_marketing_version(), "0.12.98")
        self.assertEqual(version.apple_build_version(), "1.12.98")

    def test_maps_rc_capable_stable_and_rc_apple_versions(self) -> None:
        rc = ReleaseVersion.parse("0.13.0-rc.1")
        stable = ReleaseVersion.parse("0.13.0")
        later_rc = ReleaseVersion.parse("3.4.5-rc.98")
        later_stable = ReleaseVersion.parse("3.4.5")
        self.assertEqual(rc.apple_marketing_version(), "0.13.0")
        self.assertEqual(rc.apple_build_version(), "1.13.1")
        self.assertEqual(stable.apple_marketing_version(), "0.13.0")
        self.assertEqual(stable.apple_build_version(), "1.13.99")
        self.assertEqual(later_rc.apple_build_version(), "4.4.598")
        self.assertEqual(later_stable.apple_build_version(), "4.4.599")

    def test_cli_outputs_values_and_rejects_invalid_input(self) -> None:
        helper = Path(__file__).with_name("release_version.py")
        result = subprocess.run(
            [sys.executable, str(helper), "apple-build", "1.2.3-rc.4"],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.stdout, "2.2.304\n")
        invalid = subprocess.run(
            [sys.executable, str(helper), "validate-tag", "1.2.3"],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(invalid.returncode, 0)


if __name__ == "__main__":
    unittest.main()
