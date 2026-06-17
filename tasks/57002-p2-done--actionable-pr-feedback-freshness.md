# Actionable PR feedback freshness

## Problem

The Work Actions bar can show `Address feedback` with a freshness badge like `at least 3 comments updated` when the only observed change is that existing GitHub review threads were resolved. That is misleading: resolved-only transitions remove work from the agent instead of creating new work.

Phoenix should not become a full GitHub PR comments/reviews/thread sync engine. The app only needs a compact projection of PR feedback for two product questions:

1. What actionable PR feedback should the agent address when the user clicks `Address feedback`?
2. Has unresolved/actionable feedback changed since Phoenix last captured a baseline?

## Scope

Stop at an **actionable feedback snapshot** boundary:

> Phoenix stores a compact baseline of **agent-actionable PR feedback**. GitHub remains the source of truth for the PR model. Phoenix does not mirror GitHub; it only snapshots enough to prepare an autofix prompt and avoid misleading freshness badges.

This principle should be added to the technical artifact constraints section of `AGENTS.md` so future PR-feedback work does not drift into a full GitHub PR data model.

## Plan

0. **Codify the boundary**
   - Add the scoped PR-feedback principle to the technical artifact constraints section of `AGENTS.md`:
     > Phoenix stores a compact baseline of **agent-actionable PR feedback**. GitHub remains the source of truth for the PR model. Phoenix does not mirror GitHub; it only snapshots enough to prepare an autofix prompt and avoid misleading freshness badges.
   - Keep this as a product/code constraint, not just a local comment, because it defines where Phoenix stops modeling GitHub.

1. **Define actionability explicitly**
   - Add or centralize an `is_actionable_feedback` helper for PR feedback items.
   - Treat `resolved == Some(true)` as non-actionable.
   - Keep unresolved and unknown-resolution items actionable so degraded/legacy sources do not disappear silently.

2. **Make freshness compare actionable feedback only**
   - Rename or refactor the existing freshness function toward `actionable_feedback_freshness_from_baseline` semantics.
   - Compute baseline identities/fingerprints from actionable items only.
   - Compare current actionable items only.
   - Exclude `resolved` from the content fingerprint so a resolution flip does not look like an edit.
   - Preserve current semantics for genuinely new actionable feedback and edited actionable feedback.

3. **Keep resolved-only deltas quiet in the UI**
   - If all changes since the baseline are resolved-only transitions, return no freshness signal and show no badge.
   - Do not add a `3 resolved` badge; it implies user/agent work when there is none.

4. **Use action-oriented copy**
   - Keep concise freshness labels, but avoid overclaiming GitHub data-model details.
   - Suggested visible labels:
     - `N new` for new actionable feedback.
     - `N updated` for changed actionable feedback.
     - Existing `at least` prefix still applies when fetch coverage is degraded.
   - Continue showing coverage/auth markers separately from content freshness.

5. **Make the autofix artifact honest about filtering**
   - The context bundle should present itself as an actionable snapshot, not a full PR feedback inventory.
   - Filter resolved review-thread comments out of the actionable feedback list, or place them only in an explicitly non-actionable/context section if needed.
   - Ensure the agent prompt language says the file contains failed CI details and **unresolved/actionable review feedback**, not every GitHub comment/thread.

6. **Tests**
   - Add backend tests proving:
     - new unresolved feedback yields `New` freshness;
     - edited unresolved feedback yields `Edited` freshness;
     - resolved-only transitions yield no freshness;
     - `resolved` is excluded from content fingerprints;
     - artifact/baseline generation uses actionable feedback semantics.
   - Add/update UI tests for badge copy:
     - `N new` remains;
     - edited actionable feedback renders `N updated`;
     - no badge renders for resolved-only changes;
     - degraded coverage still prefixes `at least`.

## Non-goals

- No full GitHub comment/review/thread synchronization.
- No durable normalized GitHub comment/thread tables.
- No webhook-style reconciliation loop.
- No UI badge for resolved-only deltas.
- No attempt to model all GitHub review state transitions.

## Acceptance criteria

- Resolving previously-captured review threads does not make `Address feedback` look like new work is available.
- The feedback freshness badge represents only new or materially changed unresolved/actionable feedback.
- The context artifact and agent prompt are honest that Phoenix provides an actionable snapshot, not a complete GitHub PR feedback inventory.
- Existing degraded-fetch coverage warnings remain separate from freshness.
