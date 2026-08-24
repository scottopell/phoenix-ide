# Investigate SQLite live-ownership gaps under production symptoms

## Context

These are unproven hypotheses after merged PR #710, not current blockers and not evidence that the delivered SQLite workload attribution baseline is incorrect:

- Deferred transactions may report transaction admission before actual writer ownership. Investigate only if production shows low or zero admission waits while contention or statement latency is demonstrably high.
- Snapshots may omit currently active reads until completion. Investigate only if production samples show missing read concurrency during demonstrably long reads.

## First step

Inspect production SQLite workload reports and relevant traces under a real matching symptom. If evidence supports either hypothesis, add a discriminating test that reproduces the observed gap before changing code.

## Scope

Keep any response proportional to confirmed evidence. Do not assume or promise an architecture rewrite, general transaction framework, raw event history, or snapshot-gating redesign.

## Acceptance criteria

- Production reports/traces are inspected for a concrete matching symptom before implementation work begins.
- Each pursued hypothesis has a discriminating regression before code changes.
- Any resulting behavior or policy change is documented in the appropriate specification/ADR when required.
