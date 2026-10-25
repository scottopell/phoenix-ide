# ADR-182: Immutable conversation goals use owner-version fences

- **Status:** Accepted
- **Date:** 2026-10-19
- **Affects:** REQ-GOAL-001 through REQ-GOAL-008 (`specs/conversation-goals/requirements.md`)

## Context

Durable goals were commissioned as ordinary stable ProductConversation-owned state, separate from transcript execution, WorkScope and coding artifacts. An initial source prototype allowed objective edits with objective revisions as well as lifecycle versions. The primary user then preferred immutable objectives and explicit new-goal supersession, removing edit/reopen complexity. Complete and limit reached were distinct terminal outcomes; Blocked could return to Underway on the same identity.

The prototype was never registered or applied. The approved bounded source increment could replace that shape directly without a compatibility shim or data backfill. Concurrent user/Coordinator/agent changes and late execution reports still needed deterministic fences, concrete attribution and deletion compatibility. Parent coordination assigned goals migration 132 after federation migrations 130/131, with this ADR number checked against the shared chain. This source stack did not qualify federation or authorize deployment.

## Options considered

1. **Mutable objectives with objective revisions plus lifecycle versions.** This distinguished two kinds of edits but retained mutation/reopen rules the user explicitly removed.
2. **Immutable goals with a writable current record plus duplicated history snapshots.** This was familiar CRUD, but made the same current semantic state independently writable in two locations.
3. **Immutable objective identity rows plus goal-specific owner-version history.** New identities supersede current state through one history append; one owner-version fence covers status changes and replacements without objective revisions or a generic event framework.

## Decision

Choose option 3. Store the objective once on an immutable goal identity row owned by an ordinary ProductConversation. Store attributed status changes and new-identity supersessions as normalized goal-specific history rows. The indexed latest owner-history version identifies current state; no second writable current record is introduced.

All actors use the observed monotonic owner version for compare-and-set writes, including Global and future UI calls. New goals get new non-reusable identities and atomically supersede the previous current identity; supersession preserves factual terminal outcomes and does not fabricate successful completion. Complete and Exhausted are terminal and distinct. Blocked → Underway retains identity. There is no objective-edit, revision or clear-first API.

Use exact action identity plus full mutation/actor/binding comparison for idempotent accepted retries. Keep requested-by and applied-by distinct, with concrete user-channel, owning-agent, Coordinator and generated-host variants. Validate persisted kind/membership and assistant ToolUse consistency, but leave exact invocation assignment to trusted runtime context. Attribution is not a commission grant or biological-human authentication.

Persist immutable Goal/NoGoal observations for exact workflow/turn executions. Reports require stored binding equality, current goal identity and owner-version equality. Keep the storage capture primitive crate-private; it cannot prove the caller captured at admission. Actual runtime integration must capture before generation atomically with turn admission, never attach a current goal during late report dispatch.

Retain immutable evidence locators even when their source rows disappear, instead of foreign keys that pin source transcript/execution deletion. Cascade all goal-specific data with its ordinary owner, while guarding standalone mutation/deletion of retained history and observations. Register the single goal SQL source through normal numbered migration 132; fresh initialization already replays that same migration, so do not duplicate goal DDL in the base schema.

Keep this increment dormant: registration permits storage, not automatic work, new tools/endpoints/UI/watch consumers or user Stop release. Goal status alone is not a stable-owner Stop fence. Do not pack speculative future control columns into migration 132; any necessary durable Stop control gets a separately coordinated migration when its runtime contract is concrete, preserving the immutable imported migration identity.

## Consequences

- Objective immutability removes revision counters and objective-edit/reopen machinery; one owner version fences both status races and new-goal supersession, including replacement with identical text.
- Current reads join immutable objective identity to the latest owner history. Historical action reconstruction uses its own immutable identity, not the latest objective. History growth is goal-specific and reads are bounded.
- Normal fresh/upgrade migration coverage and supported hard-delete coverage are required, including a populated report/history/binding cycle. Source authoring alone does not qualify these behaviors; hosted execution at an exact candidate is separate evidence.
- Structured question/task-approval waiting is orthogonal to goal status. Waiting leaves Underway; an explicit Blocked report is separate. Existing watch attention conveys facts, not authority. No event/wait/lifetime-budget or general authorization framework is introduced.
- Settled-boundary mutation admission, stable-owner Stop control, trusted runtime adapters, local follow-through and public surfaces remain later integration work. Registration is not their activation or production migration authority.
