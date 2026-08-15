# Build the Phoenix adversarial review skill

Create a new `phoenix-adversarial-review` skill that turns real review failures into a repeatable, evidence-backed workflow for challenging agent-authored code before it lands.

## Outcome

The skill will use two primary evidence sources:

1. Internal Phoenix production records for sessions where agents reviewed their own code, including traces and persisted conversation/tool activity where available.
2. GitHub Codex review findings and the associated patches, discussions, and outcomes.

It will distill recurring miss patterns into an adversarial review procedure without copying secrets, personal data, repository-specific confidential content, or transient examples into the durable skill.

## Investigation

- Read the active delivery roadmap and applicable skill-authoring/repository guidance before implementation.
- Inventory existing review-oriented skills and conventions, especially skill structure, trigger wording, progressive disclosure, supporting scripts/references, validation, and tests.
- Establish the production-data access path and inspect bounded, narrow samples first. Prefer TraceQL against the local VictoriaTraces service; identify candidate trace IDs before fetching full traces. Correlate traces with persisted Phoenix records only where needed.
- Establish the GitHub corpus: locate Codex-authored review comments/findings, the code they evaluated, author responses, subsequent patches, and whether each finding was accepted, rejected, or unresolved.
- Define an explicit evidence schema so the two sources can be compared consistently: finding category, severity, code context, reviewer reasoning, self-review miss, eventual disposition, false-positive signals, and reusable heuristic.
- Sample enough cases to distinguish repeated failure modes from anecdotes. Record aggregate patterns and sanitized exemplars, not raw sensitive production payloads.

## Skill design

- Specify when the skill should trigger and what inputs it requires (diff/PR scope, specs, tests, and available review history).
- Build a staged adversarial workflow that separates:
  - authority discovery (requirements, Allium, ADRs, task/PR intent),
  - change and blast-radius reconstruction,
  - correctness/invariant challenges,
  - persistence and wire-contract checks,
  - concurrency, recovery, and time-based smell checks,
  - security and boundary checks,
  - test-quality and negative-path challenges,
  - YAGNI/complexity challenges,
  - evidence-based severity and confidence calibration.
- Require reviewers to attempt falsification rather than summarize the patch, trace claims to concrete evidence, distinguish defects from questions, and explicitly report when no actionable finding survives scrutiny.
- Add anti-pattern guidance learned from the corpora: self-review blind spots, rubber-stamping, speculative findings, style noise, duplicated findings, missing spec checks, and severity inflation.
- Define a concise, actionable output contract with findings ordered by severity, precise anchors, failure scenarios, evidence, and suggested verification; keep summaries secondary to findings.
- Use supporting references or scripts only where they materially improve repeatability. Avoid embedding internal data or creating a parallel generic code-review framework.

## Implementation and validation

- Add the skill in the repository’s established skill location with a focused `SKILL.md` and any justified reference assets.
- Add or update discovery metadata/tests so the exact skill name and triggering descriptions are recognized.
- Exercise the skill against a held-out set of Phoenix self-review and Codex-review cases that were not used to formulate the heuristics.
- Evaluate whether it recovers known valid findings, avoids known false positives, produces source-grounded feedback, and remains useful on a clean patch.
- Run targeted skill validation/tests, then the relevant `./dev.py check` lanes (or full check if required by touched paths).
- Review the finished artifact for confidentiality: no production identifiers, secrets, private code excerpts, personal data, or raw conversation content may be committed.
- Update any applicable documentation/task status, commit the completed unit, and report the evidence sources queried, sanitized pattern counts, validation results, and limitations.

## Guardrails

- Production and GitHub records are evidence, not normative requirements; repository specs and ADRs remain authoritative.
- Use bounded queries and minimum necessary data access. Do not persist raw production records in the worktree.
- Treat finding disposition as essential training evidence: accepted findings are not automatically correct, and rejected findings are not automatically useless.
- Do not tune solely to Phoenix-specific examples; extract general review moves while retaining Phoenix-specific authority and architecture checks where appropriate.
- If access to either primary corpus is unavailable, document the blocker and do not silently substitute assumptions or synthetic evidence.
