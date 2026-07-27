# Work Lifecycle: Explicit Close for Git-Backed Conversations

## User Story

As a developer using PhoenixIDE, I need one explicit way to close a Git-backed conversation so that I can retire finished or discarded work without Phoenix unexpectedly mutating branches, pull requests, or other repository state I still own.

## Scope

This spec governs the product-facing **Close conversation** action for Git-backed conversations and the WorkScope retirement contract consumed by that Close flow.

It owns:

- the exact retirement-inspection and loss-warning contract for Close on a Git-backed conversation;
- the exact loss categories that require confirmation before destructive retirement;
- the requirement that Close retire Phoenix-owned WorkScope resources and the worktree without creating automatic recovery artifacts and without mutating repository refs or PRs;
- the advisory use of PR state to guide the user toward closing when work appears shipped.

It does **not** own:

- conversation-state legality, root lifecycle topology, or durable cancellation sequencing — bedrock and the unified lifecycle specs own those authorities;
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

### REQ-WL-002: Retirement Inspection Classifies Exact Worktree-Loss Risk Before Destructive Teardown

WHEN bedrock requests retirement inspection for an attached Git-backed `WorkScope` for one exact Close attempt
THE SYSTEM SHALL inspect only state whose durability depends on the attached worktree and its owned resources
AND SHALL classify loss risk into these independent categories:
- staged tracked paths
- unstaged tracked paths, including conflicted or otherwise unmerged paths
- untracked non-ignored paths
- dirty or untracked state inside initialized submodule checkouts
- detached commits not reachable from any ref under `refs/heads/*`, `refs/remotes/*`, `refs/tags/*`, or `refs/stash`

THE SYSTEM SHALL exclude ignored paths from the loss inventory
AND SHALL treat LFS-tracked edits as ordinary tracked changes within the tracked-path categories
AND SHALL treat local branches, remote-tracking branches, tags, and stash entries as durable refs rather than as loss
AND SHALL treat reflog-only detached commits as at-risk detached commits
AND SHALL scope nested-repository inspection only to declared submodules rather than recursively inventing preservation rules for arbitrary nested repositories

WHEN any one or more of those categories are present
THE SYSTEM SHALL return an exact categorized inventory with the relevant path rows and detached-commit identities
AND SHALL require explicit discard confirmation before destructive retirement begins

WHEN no category is present
THE SYSTEM SHALL allow retirement to proceed without a discard confirmation

**Rationale:** Phoenix owns the disposable environment, not repository history. Loss inspection must warn exactly about worktree-only risk without conflating it with durable refs the user still owns.

---

### REQ-WL-002a: Retirement Inspection Binds Confirmation to One Exact Workspace Generation

WHEN retirement inspection completes for an attached Git-backed `WorkScope`
THE SYSTEM SHALL produce an inspection generation and workspace fingerprint together with the categorized results

WHEN the user confirms discard after a warning-producing inspection
THE SYSTEM SHALL bind that confirmation to the exact inspection generation and workspace fingerprint that justified the warning

WHEN the workspace changes after inspection and before destructive retirement begins
THE SYSTEM SHALL invalidate the outstanding confirmation
AND SHALL require reinspection before retirement may proceed

WHEN a product conversation has no attached `WorkScope` that owns a Git-backed worktree
THE SYSTEM SHALL skip worktree-loss inspection
AND SHALL emit the no-confirmation inspection outcome for that exact Close attempt
AND SHALL NOT require a discard confirmation that implies worktree-owned loss
AND SHALL NOT let the presence of other attached non-worktree scopes bypass inspection for any simultaneously attached worktree-owning scope

**Rationale:** The confirmation is only trustworthy for the exact inspected workspace. A changed workspace must not inherit stale approval to discard different state.

---

### REQ-WL-002b: Retirement Retires Owned Resources Stepwise, Idempotently, and Without Automatic Recovery Artifacts

WHEN bedrock requests resource retirement for an attached Git-backed `WorkScope` for one exact Close attempt
THE SYSTEM SHALL retire the owned worktree and WorkScope-scoped resources, including the worktree itself, bash/process-group resources, tmux resources, PTY/terminal resources, browser resources, and equivalent live execution resources owned by that same WorkScope

THE SYSTEM SHALL treat the attached `WorkScope` as the owner of the retireable resources
AND SHALL derive cleanup authority only from the root product conversation's committed Close retirement operation targeting that attached `WorkScope`

THE SYSTEM SHALL treat transcript rows and subordinate execution conversations within the same open product conversation as participants in that one aggregate rather than as independent WorkScope owners
AND SHALL NOT let those subordinate participants independently own, veto, or delay destructive retirement of the product conversation's attached `WorkScope`

THE SYSTEM SHALL distinguish those same-aggregate participants from a genuinely separate open product conversation that also resolves to the same `WorkScope`, or from unresolved conversation-identity evidence that prevents Phoenix from proving whether another open product aggregate exists
AND SHALL block destructive teardown only for that distinct-open-aggregate or unresolved-identity-conflict case

THE SYSTEM SHALL perform retirement as a stepwise idempotent operation that records per-owned-resource completion or residual-error evidence as each step is attempted so retries can safely continue from the exact prior state

WHEN retirement attempts one owned resource
THE SYSTEM SHALL record typed evidence for that exact retirement attempt before any all-retired completion is emitted
AND SHALL classify the attempted resource as one of: worktree, bash/process-group, tmux, PTY/terminal, browser, or equivalent live execution resource

WHEN a retirement step succeeds for one owned resource
THE SYSTEM SHALL record `RetiredResource` evidence for that resource bound to the exact retirement attempt and attached `WorkScope`
AND SHALL treat a later retry that encounters the same resource already retired as an idempotent no-op rather than as a failure or a second completion

WHEN retirement succeeds overall
THE SYSTEM SHALL emit success only after every owned resource has either produced `RetiredResource` evidence for that exact attempt or been accepted as an idempotent already-retired no-op for that exact attempt

WHEN retirement cannot retire every owned resource
THE SYSTEM SHALL report typed residual cleanup state and repair information rather than silently succeeding
AND SHALL bind every residual cleanup item to the exact retirement attempt and attached `WorkScope`
AND SHALL preserve the previously recorded per-resource retirement evidence so the remaining residual set is explicit

WHEN the worktree is already absent
THE SYSTEM SHALL bind that absence evidence to the exact retirement attempt and attached `WorkScope`
AND SHALL accept the absence only when retained identity and evidence show that the same requested retirement already removed it or is adopting that exact absence
AND SHALL otherwise report typed residual evidence rather than silently treating the absence as success

WHEN the attached `WorkScope` also owns attachments or other work-affine retained resources that are shared across transcript rows of the same open product conversation
THE SYSTEM SHALL retire or preserve those resources according to that same WorkScope ownership boundary rather than according to individual transcript-row ownership

CONFIRMED retirement SHALL NOT create a branch, tag, commit, stash, patch, diff snapshot, or other automatic recovery artifact

THE SYSTEM SHALL leave every branch, tag, stash, remote-tracking ref, and pull request untouched
AND SHALL NOT create, rename, move, fast-forward, merge, delete, push, close, or retarget any branch or pull request as a side effect of Close or retirement

**Rationale:** Retirement must reclaim exactly the resources Phoenix owns, converge safely across retries and restarts, and never disguise destructive teardown as repository management or automatic backup creation.

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
AND SHALL preserve Close as one product-conversation action rather than one lifecycle per PR

THE SYSTEM SHALL NOT automatically close a conversation because a PR appears merged, closed, missing, or stale
AND SHALL NOT treat PR state as ownership of the conversation lifecycle

**Rationale:** PR state helps the user understand whether work appears shipped, but Phoenix does not observe every repository event with enough authority to close work automatically. Close remains an explicit user decision.
