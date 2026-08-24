import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SKILL_DIR = ROOT / "skills" / "phoenix-adversarial-review"
DISCOVERY_LINK = ROOT / ".agents" / "skills" / "phoenix-adversarial-review"
SKILL = SKILL_DIR / "SKILL.md"


class PhoenixAdversarialReviewSkillTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = SKILL.read_text()

    def test_phoenix_skill_uses_canonical_source_and_discovery_symlink(self):
        self.assertTrue(SKILL_DIR.is_dir())
        self.assertTrue(DISCOVERY_LINK.is_symlink())
        self.assertEqual(
            DISCOVERY_LINK.readlink(), Path("../../skills/phoenix-adversarial-review")
        )
        self.assertEqual(DISCOVERY_LINK.resolve(), SKILL_DIR.resolve())

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

    def test_exact_target_requires_base_and_head_without_worktree_conflation(self):
        freeze = self.text.index("## 1. Freeze the review target")
        delta = self.text.index("## 5. Compare local review with Codex")
        self.assertLess(freeze, delta)
        self.assertIn("identical base and head SHAs", self.text)
        self.assertIn("unrelated worktree state does not affect", self.text)
        self.assertIn("Label that weaker evidence **near-match**", self.text)
        self.assertIn("otherwise label it **unpaired**", self.text)

    def test_independent_pass_requires_fresh_context_and_sealed_output(self):
        evidence = (SKILL_DIR / "references" / "evidence-method.md").read_text()
        self.assertIn("fresh context", self.text)
        self.assertIn("seal its output", self.text)
        self.assertIn("mark the review anchored", evidence)

    def test_near_match_requires_lineage_and_defect_continuity(self):
        evidence = (SKILL_DIR / "references" / "evidence-method.md").read_text()
        self.assertIn("the intervening diff `L..C` is inspected", evidence)
        self.assertIn("faulty lines or semantic mechanism already existed at `L`", evidence)
        self.assertIn("states the direction and commit distance", evidence)
        self.assertIn("cannot reveal self-review misses", evidence)

    def test_durable_doctrine_separates_publication_local_loss_and_external_ambiguity(self):
        probes = (SKILL_DIR / "references" / "probes.md").read_text()
        for concept in (
            "Transaction / publication",
            "Local SQLite authority loss",
            "exact-query-or-fail-stop",
            "External ambiguity",
            "committed authoritative SQLite rows and durable time",
        ):
            self.assertIn(concept, probes)
        self.assertIn("Task/PR intent supplies scope and non-goal context", self.text)
        self.assertIn("Do not apply this rule to genuine external ambiguity", self.text)

    def test_review_delta_keeps_all_four_dimensions_independent(self):
        for dimension in (
            "**Evidence tier**",
            "**Comparison outcome**",
            "**Disposition**",
            "**Independence**",
        ):
            self.assertIn(dimension, self.text)
        self.assertIn(
            "simultaneously be `near-match`, `Codex-only`, `disproved`, and `isolated`",
            self.text,
        )
        delta = self.text.split("## Review delta", 1)[1].split("```", 1)[0]
        self.assertNotIn("anchored N", delta.split("Comparison outcome:", 1)[0])
        self.assertNotIn("disputed", delta)
        self.assertIn("Disposition: validated N; disproved N", delta)
        self.assertIn("Independence: isolated N; anchored N", delta)

    def test_corpus_candidate_probes_link_sanitized_evidence_report(self):
        probes = (SKILL_DIR / "references" / "probes.md").read_text()
        report = SKILL_DIR / "references" / "evidence-report.md"
        self.assertIn("[evidence-report.md](evidence-report.md)", probes)
        self.assertTrue(report.is_file())
        report_text = report.read_text()
        for section in ("Sources and sample sizes", "Held-out outcomes", "Limitations"):
            self.assertIn(section, report_text)
        self.assertIn("no verified exact-target", report_text)

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
