# ADR-150: Concrete Coordinator reporting and blocked-work recovery

- **Status:** Accepted
- **Date:** 2026-10-26
- **Affects:** REQ-GR-011

## Context

After ADR-149, a check-in treated an owner waiting for continuation and an obsolete parent question as harmless because other work was progressing. The user also found coordination terms such as “lane,” “gated,” “qualification,” and “reconciliation” hard to read. They requested concrete bad/good examples while keeping the prompt no longer than before.

## Decision

Replace overlapping prompt prose rather than append warnings. Both system variants require diagnosis of blocked or waiting work, the smallest owner-scoped recovery permitted by existing authority, and verification that work resumed. Independent progress does not discharge blocked obligations. Reconcile subsequent decisions and completed work before replaying questions or answers; preserve genuine pauses and gates.

Choose whether to speak before choosing wording. Routine check passes and message deliveries stay quiet; unsolicited updates require needed user action, a meaningful goal-level result, or a new blocker/risk changing the plan. Requested check-ins retain an executive view. Hypothetical examples illustrate silence for intermediate checks and sends, dependencies described as actual work, accepted resume versus verified restart, and a verified feature outcome. Report needed input, what changed, why it matters, and what happens next without a mandatory template. The button reinforces the policy without repeating the examples.

## Options considered

- Append more warnings: increases prompt length and leaves overlapping rules competing.
- Ban a vocabulary list: loses useful technical precision without teaching explanation.
- Remove selective intervention: invites redundant nudges and abandons the original user goal.

The user had already explicitly rejected routine updates about delivered instructions (October 12) and test counts (October 21), asking instead for a low-context executive view. This refinement recovers that preference rather than treating it solely as a vocabulary issue.

## Consequences

Each system variant and the button retain independent word and character ceilings from the preceding prompts. These measure text size, not provider token counts. Static tests check instruction content, few-shot structure, button submission and ceilings; they do not prove live model judgment. No runtime, lifecycle authority, polling, tool, merge or deployment changes are introduced. Archived evaluation fixtures remain unchanged; no paid evaluation is part of this refinement.
