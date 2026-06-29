import importlib.util
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

spec = importlib.util.spec_from_file_location("phoenix_dev_py", ROOT / "dev.py")
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

    def test_adr_chain_reports_numbering_gap(self):
        with tempfile.TemporaryDirectory() as td:
            specs = Path(td) / "specs"
            self.write_valid_adrs(specs)
            (specs / "adrs" / "002_second-decision.md").write_text(ADR_TEXT.replace("ADR-000", "ADR-002"))
            with (specs / "adrs" / "README.md").open("a") as f:
                f.write("| [002](002_second-decision.md) | Second | Accepted | methodology-level |\n")

            result = dev._validate_spears_v2_shape(specs)

            self.assertTrue(any("sequential without gaps" in e for e in result.errors))


if __name__ == "__main__":
    unittest.main()
