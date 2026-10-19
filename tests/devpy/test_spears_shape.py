import importlib.util
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

spec = importlib.util.spec_from_file_location("phoenix_dev_py", ROOT / "dev.py")
assert spec is not None
dev = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(dev)

ADR_TEXT = """# ADR-000: Test decision

- **Status:** Accepted
- **Date:** 2026-06-29
- **Affects:** methodology-level

## Context

Context.

## Options considered

1. **A** — one.
2. **B** — two.

## Decision

Decision.

## Consequences

Consequences.
"""


class SpearsV2ShapeTests(unittest.TestCase):
    def write_valid_adrs(self, specs: Path) -> None:
        adrs = specs / "adrs"
        adrs.mkdir(parents=True)
        (adrs / "_TEMPLATE.md").write_text("# ADR-NNN: Template\n")
        (adrs / "000_test-decision.md").write_text(ADR_TEXT)
        (adrs / "README.md").write_text(
            "# Architecture Decision Records\n\n"
            "| ADR | Title | Status | Affects |\n"
            "| --- | --- | --- | --- |\n"
            "| [000](000_test-decision.md) | Test decision | Accepted | methodology-level |\n"
        )

    def test_v2_spec_without_design_passes(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            feature = specs / "feature"
            feature.mkdir(parents=True)
            (feature / "requirements.md").write_text("### REQ-FEA-001: Exists\n")
            (feature / "executive.md").write_text("| **REQ-FEA-001:** Exists | ✅ | |\n")
            self.write_valid_adrs(specs)

            result = dev._validate_spears_v2_shape(specs)

            self.assertEqual(result.errors, [])
            self.assertEqual(result.legacy_design_docs, [])

    def test_legacy_design_is_inventory_not_failure(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            feature = specs / "feature"
            feature.mkdir(parents=True)
            (feature / "design.md").write_text("legacy\n")
            self.write_valid_adrs(specs)

            result = dev._validate_spears_v2_shape(specs)

            self.assertEqual(result.errors, [])
            self.assertTrue(any(path.endswith("feature/design.md") for path in result.legacy_design_docs))

    def test_adr_chain_reports_missing_index_entry(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            self.write_valid_adrs(specs)
            (specs / "adrs" / "README.md").write_text("# Architecture Decision Records\n")

            result = dev._validate_spears_v2_shape(specs)

            self.assertTrue(any("missing index row/link for 000_test-decision.md" in e for e in result.errors))

    def test_adr_chain_reports_stale_index_link(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            self.write_valid_adrs(specs)
            (specs / "adrs" / "README.md").write_text(
                "# Architecture Decision Records\n\n"
                "| ADR | Title | Status | Affects |\n"
                "| --- | --- | --- | --- |\n"
                "| [000](000_test-decision.md) | Test decision | Accepted | methodology-level |\n"
                "| [001](001_missing-decision.md) | Missing decision | Accepted | methodology-level |\n"
            )

            result = dev._validate_spears_v2_shape(specs)

            self.assertTrue(any("index links to missing ADR file 001_missing-decision.md" in e for e in result.errors))

    def test_adr_chain_requires_numbering_to_start_at_zero(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            self.write_valid_adrs(specs)
            (specs / "adrs" / "000_test-decision.md").unlink()
            (specs / "adrs" / "001_test-decision.md").write_text(ADR_TEXT.replace("ADR-000", "ADR-001"))
            (specs / "adrs" / "README.md").write_text(
                "# Architecture Decision Records\n\n"
                "| ADR | Title | Status | Affects |\n"
                "| --- | --- | --- | --- |\n"
                "| [001](001_test-decision.md) | Test decision | Accepted | methodology-level |\n"
            )

            result = dev._validate_spears_v2_shape(specs)

            self.assertTrue(any("numbering must start at 000" in e for e in result.errors))

    def test_adr_chain_accepts_numbering_gap(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            self.write_valid_adrs(specs)
            (specs / "adrs" / "002_second-decision.md").write_text(ADR_TEXT.replace("ADR-000", "ADR-002"))
            with (specs / "adrs" / "README.md").open("a") as f:
                f.write("| [002](002_second-decision.md) | Second | Accepted | methodology-level |\n")

            result = dev._validate_spears_v2_shape(specs)

            self.assertEqual(result.errors, [])

    def test_adr_chain_rejects_duplicate_numeric_ids_with_distinct_slugs(self):
        for num in ("000", "002"):
            with self.subTest(num=num), tempfile.TemporaryDirectory() as td:
                specs = Path(td) / "specs"
                self.write_valid_adrs(specs)
                for slug in ("first", "second"):
                    filename = f"{num}_{slug}.md"
                    (specs / "adrs" / filename).write_text(ADR_TEXT)
                    with (specs / "adrs" / "README.md").open("a") as f:
                        f.write(f"| [{num}]({filename}) | Decision | Accepted | methodology-level |\n")

                result = dev._validate_spears_v2_shape(specs)

                self.assertTrue(any(f"duplicate ADR number {num}" in e for e in result.errors))
                self.assertFalse(any("missing index" in e for e in result.errors))

    def test_adr_chain_preserves_invalid_artifact_checks(self):
        cases = (
            ("_TEMPLATE.md", None, "_TEMPLATE.md is required"),
            ("README.md", None, "README.md is required"),
            ("000_test-decision.md", None, "at least one numbered ADR is required"),
            ("000_test-decision.md", ADR_TEXT.replace("- **Status:** Accepted", ""), "missing '- **Status:**"),
            ("000_test-decision.md", ADR_TEXT.replace("2026-06-29", "not-a-date"), "missing '- **Date:**"),
            ("000_test-decision.md", ADR_TEXT.replace("- **Affects:** methodology-level", ""), "missing '- **Affects:**"),
        )
        for filename, text, expected in cases:
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as td:
                specs = Path(td) / "specs"
                self.write_valid_adrs(specs)
                path = specs / "adrs" / filename
                if text is None:
                    path.unlink()
                else:
                    path.write_text(text)
                result = dev._validate_spears_v2_shape(specs)
                self.assertTrue(any(expected in e for e in result.errors), result.errors)

    def test_adr_chain_preserves_required_sections(self):
        for heading in ("## Context", "## Options considered", "## Decision", "## Consequences"):
            with self.subTest(heading=heading), tempfile.TemporaryDirectory() as td:
                specs = Path(td) / "specs"
                self.write_valid_adrs(specs)
                (specs / "adrs" / "000_test-decision.md").write_text(ADR_TEXT.replace(heading, ""))
                result = dev._validate_spears_v2_shape(specs)
                self.assertTrue(any(f"missing '{heading}'" in e for e in result.errors), result.errors)

    def test_adr_chain_preserves_filename_validation(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            self.write_valid_adrs(specs)
            (specs / "adrs" / "002_Bad-slug.md").write_text(ADR_TEXT)
            result = dev._validate_spears_v2_shape(specs)
            self.assertTrue(any("ADR filename must be NNN_<slug>.md" in e for e in result.errors))


if __name__ == "__main__":
    unittest.main()
