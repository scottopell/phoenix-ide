import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SKILL_DIR = ROOT / ".agents" / "skills" / "phoenix-adversarial-review"
SKILL = SKILL_DIR / "SKILL.md"


class PhoenixAdversarialReviewSkillTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = SKILL.read_text()

    def test_discovery_metadata_has_exact_name_and_review_triggers(self):
        frontmatter = self.text.split("---", 2)[1]
        self.assertIn("name: phoenix-adversarial-review", frontmatter)
        description = next(
            line.removeprefix("description: ")
            for line in frontmatter.splitlines()
            if line.startswith("description: ")
        )
        for trigger in ("adversarial", "exact-head", "self-review", "codex"):
            self.assertIn(trigger, description.lower())

    def test_same_head_identity_is_required_before_review_delta(self):
        freeze = self.text.index("## 1. Freeze the review target")
        independent = self.text.index("Do not read Codex findings before the independent pass")
        delta = self.text.index("## 5. Compare local review with Codex")
        self.assertLess(freeze, delta)
        self.assertLess(delta, independent)
        self.assertIn("external review `commit_id` are identical", self.text)
        self.assertIn("Label that weaker evidence **near-match**", self.text)
        self.assertIn("otherwise label it **unpaired**", self.text)

    def test_near_match_requires_lineage_and_defect_continuity(self):
        evidence = (SKILL_DIR / "references" / "evidence-method.md").read_text()
        self.assertIn("the intervening diff `L..C` is inspected", evidence)
        self.assertIn("faulty lines or semantic mechanism already existed at `L`", evidence)
        self.assertIn("states the direction and commit distance", evidence)
        self.assertIn("cannot reveal self-review misses", evidence)

    def test_finding_contract_requires_reachable_failure_and_counterevidence(self):
        for field in ("**Anchor:**", "**Trigger:**", "**Mechanism:**", "**Impact:**"):
            self.assertIn(field, self.text)
        self.assertIn("**Counterevidence check:**", self.text)
        self.assertIn("No actionable findings.", self.text)

    def test_progressive_disclosure_references_exist(self):
        references = re.findall(r"\(references/([^)]+)\)", self.text)
        self.assertEqual(set(references), {"probes.md", "evidence-method.md"})
        for reference in references:
            self.assertTrue((SKILL_DIR / "references" / reference).is_file())

    def test_committed_skill_contains_no_raw_internal_identifiers(self):
        combined = "\n".join(path.read_text() for path in SKILL_DIR.rglob("*.md"))
        forbidden = (
            r"@conv:",
            r"msg:[0-9a-f-]{16,}",
            r"trace[_ -]?id\s*[:=]",
            r"conversation[_ -]?id\s*[:=]",
            r"/Users/[^/\s]+/",
        )
        for pattern in forbidden:
            self.assertIsNone(re.search(pattern, combined, re.IGNORECASE), pattern)


if __name__ == "__main__":
    unittest.main()
