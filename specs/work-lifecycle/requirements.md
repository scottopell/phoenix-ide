# Work Lifecycle: Explicit Close for Git-Backed Conversations

## User Story

As a developer using PhoenixIDE, I need one explicit way to close a Git-backed conversation so that I can retire finished or discarded work without Phoenix unexpectedly mutating branches, pull requests, or other repository state I still own.

## Scope

This spec governs the product-facing **Close conversation** action for Git-backed conversations and the advisory repository facts that help the user decide when to use it.

It owns:

- the confirmation and loss-warning contract for Close on a Git-backed conversation;
- the requirement that Close release Phoenix-owned worktree resources while leaving repository refs and PRs untouched;
- the advisory use of PR state to guide the user toward closing when work appears shipped.

It does **not** own:

- conversation-state legality and durable cancellation sequencing — bedrock and the unified lifecycle specs own those authorities;
- PR feedback freshness, explicit active-PR targeting, auto-fix, and remediation context — the `pr-association` spec;
- UI placement, wording variants, or action-bar composition — the `work-actions-bar` spec owns those concerns.

Historical compatibility note: earlier Phoenix surfaces exposed **Abandon** and **Mark as merged** as separate lifecycle actions. That product split is deprecated. Any migration-time adapter that still recognizes those legacy verbs SHALL map them into the one Close flow while preserving Close's current confirmation and loss-safety contract; the legacy verbs are not current user-facing actions.

---

## Requirements

### REQ-WL-001: Close Is the Only User-Facing Terminal Action for Git-Backed Conversations

WHEN the user chooses to end a Git-backed conversation's active work
THE SYSTEM SHALL expose **Close conversation** as the only ordinary lifecycle action
AND SHALL move the conversation to read-only History only through that Close flow

WHEN legacy clients, stored intents, or migration adapters still reference `abandon` or `mark_merged`
THE SYSTEM SHALL treat those values only as deprecated compatibility inputs
AND SHALL require them to execute the same confirmation, loss inspection, cancellation, and finalization contract as Close
AND SHALL NOT expose those deprecated verbs as current writable lifecycle choices in ordinary product surfaces

**Rationale:** The user-facing distinction between “abandoned” and “merged” conflated repository interpretation with lifecycle. The durable product truth is simpler: the conversation is either still Open or the user explicitly closed it into History.

---

### REQ-WL-002: Close Releases Phoenix-Owned Resources but Never Mutates Branches or PRs

WHEN a Git-backed conversation successfully closes
THE SYSTEM SHALL release the conversation's Phoenix-owned worktree resources
AND SHALL remove the worktree only when no other live conversation still resolves to the same `WorkScope`
AND SHALL leave every branch, tag, stash, remote-tracking ref, and pull request untouched

THE SYSTEM SHALL NOT create, rename, move, fast-forward, merge, delete, push, close, or retarget any branch or pull request as a side effect of Close

WHEN the worktree contains local state whose loss depends on worktree removal
THE SYSTEM SHALL present Close's exact loss-warning contract before destructive teardown
AND SHALL require explicit user confirmation before discarding that state

**Rationale:** Phoenix owns the disposable environment, not repository history. Closing a conversation should reclaim Phoenix-owned resources while treating Git refs and PRs as user-owned facts Phoenix observes rather than lifecycle-owned artifacts Phoenix mutates.

---

### REQ-WL-003: Pull Request State Guides Close but Never Triggers It

WHEN a Git-backed conversation has one or more associated pull requests
AND Phoenix can observe their states
THE SYSTEM SHALL use that observed PR state only as advisory guidance on the Close surface

WHEN one associated pull request is confirmed merged
THE SYSTEM SHALL present that fact as a strong signal that the conversation may be ready to close

WHEN associated pull requests are open, draft, failing, pending, closed-unmerged, ambiguous, or unavailable
THE SYSTEM SHALL surface that truthfully without blocking Close solely on PR state

WHEN multiple associated pull requests exist
THE SYSTEM SHALL summarize their mixed states honestly
AND SHALL preserve Close as one WorkScope-owner action rather than one lifecycle per PR

THE SYSTEM SHALL NOT automatically close a conversation because a PR appears merged, closed, missing, or stale
AND SHALL NOT treat PR state as ownership of the conversation lifecycle

**Rationale:** PR state helps the user understand whether work appears shipped, but Phoenix does not observe every repository event with enough authority to close work automatically. Close remains an explicit user decision.
