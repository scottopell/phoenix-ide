# Package production tool analysis as the phoenix-tool-review skill

## Summary

Create a repository skill at `skills/phoenix-tool-review/` that gives agents a repeatable, privacy-conscious workflow for reviewing Phoenix production tool usage. The skill should turn persisted tool calls/results, per-turn model attribution, retained deployment markers, and Git history into actionable failure histograms and before/after evaluations.

This is a side project. It is deliberately separate from the patch-tool improvement task so packaging the analysis workflow does not delay the product intervention.

## Context

A production review of `~/.phoenix-ide/prod.db` established a useful methodology:

- tool-protocol/execution failures are represented by tool messages with top-level `is_error=true`;
- ordinary shell outcomes, including exit code 1, remain `is_error=false` and should not be counted as agent tool misuse;
- tool results join to agent `tool_use` blocks through `conversation_id` plus `tool_use_id`;
- the nearest `turn_usage` timestamp provides reliable per-turn model attribution, but the query should report attribution coverage/proximity rather than assuming it;
- infrastructure/session failures such as server restarts and disk exhaustion must be separated from likely agent misuse;
- startup log records containing `Phoenix IDE starting`, `git_sha`, and `version` are exact deployment markers when retained, while `deployed.sha` identifies only the current deployment;
- rotated log retention can be incomplete, so the first observation of a new tool schema or operation in the database may be needed to bound a rollout;
- pre/post comparisons need call denominators, model controls, confidence intervals, and sensitivity windows that exclude development/dogfooding spikes.

The workflow currently exists only in conversation history and ad hoc SQL. It should become a discoverable project capability.

## Scope

- Add `skills/phoenix-tool-review/SKILL.md` with valid skill metadata and clear trigger language for production tool failure reviews, model comparisons, and deployment-impact analysis.
- Provide a read-only workflow for locating the production database and retained logs without assuming the browser and server share a filesystem.
- Define canonical extraction and classification steps:
  - total calls and results;
  - `is_error=true` failure cohort;
  - explicit exclusion or separate reporting of command exit codes and infrastructure failures;
  - per-tool counts and rates;
  - per-model counts and rates with attribution-quality checks;
  - normalized error classes that preserve an `other` bucket and expose classifier coverage.
- Define deployment-impact analysis using exact startup SHA markers when present, Git ancestry checks, schema/operation first-seen proxies when markers are missing, and sensitivity windows.
- Include statistical guidance: always show numerators/denominators, absolute and relative changes, confidence intervals or an equivalent uncertainty measure, and warnings for small samples and selection bias.
- Include privacy and safety rules: open SQLite read-only, emit aggregates by default, avoid printing prompts, source content, credentials, or raw tool payloads, and use only narrowly sampled/redacted errors when validating a classifier.
- Prefer a checked-in helper script or SQL resources when that materially reduces query drift; any helper must remain read-only and accept explicit database/log paths rather than hard-code one developer's home directory.
- Add the skill to `skills/README.md` and ensure project skill discovery exposes it consistently with the other repository skills.

## Acceptance Criteria

- [ ] `skills/phoenix-tool-review/SKILL.md` exists with valid YAML frontmatter and concise trigger language.
- [ ] Following the skill against a Phoenix production database produces per-tool and per-model call counts, marked-error counts, and failure rates while excluding ordinary non-zero shell exits from agent-misuse totals.
- [ ] The workflow reports infrastructure/session failures separately and reports the coverage of its normalized error classifier.
- [ ] Model attribution is performed at turn level and includes a quality check for unmatched or temporally distant `turn_usage` records.
- [ ] Deployment analysis prefers retained startup `git_sha` markers, verifies feature ancestry, and clearly labels database first-seen boundaries as proxies rather than exact deployment times.
- [ ] Before/after output includes denominators, uncertainty, model-mix controls where possible, and sensitivity windows that avoid development-period bias.
- [ ] The skill defaults to aggregate output and documents read-only and privacy constraints.
- [ ] Any scripts or SQL resources have focused tests or deterministic fixtures and do not depend on the production database in CI.
- [ ] `skills/README.md` indexes the new skill, and the normal project skill discovery path can find it.
- [ ] `./dev.py check` passes.

## Out of Scope

- Building a permanent analytics dashboard.
- Copying production message or source content into the repository.
- Changing tool behavior as part of the skill task.
- Treating text-pattern classification as ground truth without reporting exclusions and unclassified results.
