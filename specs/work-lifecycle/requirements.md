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

WHEN bedrock requests retirement inspection for one exact Close attempt
THE SYSTEM SHALL inspect every attached `WorkScope` that owns a Git-backed worktree
AND SHALL bind each evidence set to that exact Close attempt and exact attached `WorkScope` identity
AND SHALL inspect only state whose durability depends on that scope's attached worktree and owned resources
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
THE SYSTEM SHALL return an exact categorized inventory with one materialized loss row per exact `(attached_workscope_identity, category, item_identity)`
AND SHALL include every relevant path row and every detached-commit identity rather than collapsing multiple items into one category summary
AND SHALL require explicit discard confirmation before destructive retirement begins

WHEN no category is present
THE SYSTEM SHALL allow retirement to proceed without a discard confirmation

WHEN multiple attached `WorkScope`s own Git-backed worktrees
THE SYSTEM SHALL determine confirmation from the union of their exact per-scope inventories
AND SHALL NOT treat missing inspection evidence for any attached worktree-owning scope as a no-loss result

**Rationale:** Phoenix owns the disposable environment, not repository history. Loss inspection must warn exactly about worktree-only risk without conflating it with durable refs the user still owns.

---

### REQ-WL-002a: Retirement Inspection Binds Confirmation to One Exact Workspace Generation

WHEN retirement inspection completes for attached Git-backed `WorkScope`s
THE SYSTEM SHALL produce one inspection generation and workspace fingerprint with the categorized results for each exact attached worktree-owning scope

WHEN that inspection requires discard confirmation
THE SYSTEM SHALL expose one concrete user-facing discard-confirmation affordance that issues `UserConfirmsCloseAfterRetirementInspection(product_conversation, attempt_id, inspection_generation, inspection_fingerprint)`
AND SHALL expose that affordance only while the exact active Close obligation for that `product_conversation` remains in `awaiting_loss_confirmation`
AND SHALL bind that affordance to the exact active Close-attempt identity, inspection generation, and workspace fingerprint currently held on that Close obligation
AND SHALL NOT expose that affordance for any stale inspection, completed Close attempt, superseded Close attempt, or non-active transcript row within the same product conversation

WHEN the user confirms discard after a warning-producing inspection
THE SYSTEM SHALL bind that confirmation to the exact inspection generation and workspace fingerprint that justified the warning
AND SHALL route the confirmation through the same atomic recomputation boundary that decides whether destructive retirement may begin

WHEN the workspace changes after inspection and before destructive retirement begins
THE SYSTEM SHALL invalidate the outstanding confirmation
AND SHALL require reinspection before retirement may proceed

WHEN the user declines to continue from that warning state before destructive retirement begins
THE SYSTEM SHALL preserve bedrock's pre-retirement `UserCancelsClose(product_conversation, attempt_id)` cancellation path as the only cancel affordance
AND SHALL NOT reinterpret that cancellation as a discard confirmation

WHEN a product conversation has no attached `WorkScope` that owns a Git-backed worktree
THE SYSTEM SHALL skip worktree-loss inspection
AND SHALL emit the no-confirmation inspection outcome for that exact Close attempt
AND SHALL NOT require a discard confirmation that implies worktree-owned loss
AND SHALL NOT let the presence of other attached non-worktree scopes bypass inspection for any simultaneously attached worktree-owning scope

WHEN a discard-confirmation request arrives with a stale, mismatched, or no-longer-active generation/fingerprint pair
THE SYSTEM SHALL treat it as a typed inspection-mismatch path that returns the Close flow to reinspection rather than beginning destructive retirement

**Rationale:** The confirmation is only trustworthy for the exact inspected workspace. A changed workspace must not inherit stale approval to discard different state.

---

### REQ-WL-002b: Retirement Retires Owned Resources Stepwise, Idempotently, and Without Automatic Recovery Artifacts

WHEN bedrock requests resource retirement for one exact Close attempt
THE SYSTEM SHALL retire the owned worktree and WorkScope-scoped resources for every attached `WorkScope` targeted by that ProductConversation operation, including each worktree itself, bash/process-group resources, tmux resources, PTY/terminal resources, browser resources, and equivalent live execution resources owned by that exact WorkScope

THE SYSTEM SHALL treat the attached `WorkScope` as the owner of the retireable resources
AND SHALL derive cleanup authority only from the exact ProductConversation's committed Close retirement operation targeting that attached `WorkScope`

THE SYSTEM SHALL treat transcript rows and subordinate execution conversations within the same ordinary Open ProductConversation as participants in that one aggregate rather than as independent WorkScope owners
AND SHALL NOT let those subordinate participants independently own, veto, or delay destructive retirement of the ProductConversation's attached `WorkScope`

THE SYSTEM SHALL distinguish those same-aggregate participants from a genuinely separate ordinary Open ProductConversation that also resolves to the same `WorkScope`, or from unresolved ProductConversation-identity evidence that prevents Phoenix from proving whether another Open product aggregate exists
AND SHALL block destructive teardown only for that distinct-open-aggregate or unresolved-identity-conflict case

THE SYSTEM SHALL perform retirement as a stepwise idempotent operation that records per-owned-resource completion or residual-error evidence as each step is attempted so retries can safely continue from the exact prior state

WHEN retirement attempts one owned resource
THE SYSTEM SHALL record typed evidence for that exact retirement attempt before any all-retired completion is emitted
AND SHALL classify the attempted resource as one of: worktree, bash/process-group, tmux, PTY/terminal, browser, or equivalent live execution resource
AND SHALL carry a stable concrete `resource_identity` alongside `resource_kind` so multiple same-kind owned resources remain distinguishable across retries and restarts

WHEN a retirement step succeeds for one owned resource
THE SYSTEM SHALL record `RetiredResource` evidence for that resource bound to the exact retirement attempt, attached `WorkScope`, `resource_kind`, and stable `resource_identity`
AND SHALL treat a later retry that encounters the same resource already retired as an idempotent no-op rather than as a failure or a second completion

WHEN retirement succeeds overall
THE SYSTEM SHALL emit success only after every owned resource of every attached owned `WorkScope` has either produced `RetiredResource` evidence for that exact attempt and exact scope or been accepted as an idempotent already-retired no-op for that exact attempt and scope

WHEN retirement cannot retire every owned resource
THE SYSTEM SHALL report typed residual cleanup state and repair information rather than silently succeeding
AND SHALL bind every residual cleanup item to the exact retirement attempt, attached `WorkScope`, `resource_kind`, and stable `resource_identity`
AND SHALL preserve the previously recorded per-resource retirement evidence so the remaining residual set is explicit

WHEN the worktree is already absent
THE SYSTEM SHALL bind that absence evidence to the exact retirement attempt, attached `WorkScope`, `resource_kind`, and stable `resource_identity`
AND SHALL accept the absence only when retained identity and evidence show that the same requested retirement already removed it or is adopting that exact absence
AND SHALL otherwise report typed residual evidence rather than silently treating the absence as success

WHEN the attached `WorkScope` also owns attachments or other work-affine retained resources that are shared across transcript rows of the same open product conversation
THE SYSTEM SHALL retire or preserve those resources according to that same WorkScope ownership boundary rather than according to individual transcript-row ownership

CONFIRMED retirement SHALL NOT create a branch, tag, commit, stash, patch, diff snapshot, or other automatic recovery artifact

THE SYSTEM SHALL leave every branch, tag, stash, remote-tracking ref, and pull request untouched
AND SHALL NOT create, rename, move, fast-forward, merge, delete, push, close, or retarget any branch or pull request as a side effect of Close or retirement

**Rationale:** Retirement must reclaim exactly the resources Phoenix owns, converge safely across retries and restarts, and never disguise destructive teardown as repository management or automatic backup creation.

---

### REQ-WL-002c: Needs-Repair Retry Reuses the Same Exact Close Attempt

WHEN resource retirement for one exact Close attempt fails and leaves the product conversation in a visible needs-repair state
THE SYSTEM SHALL expose a retry affordance bound to that same exact `attempt_id`
AND SHALL issue `CloseRetirementRetryRequested(product_conversation, attempt_id)` from that visible needs-repair state rather than from a fresh local lifecycle mutation

WHEN the user invokes retry from needs-repair
THE SYSTEM SHALL request retirement again for that same exact Close attempt
AND SHALL preserve the attempt-bound retirement evidence and residual state already recorded for prior steps
AND SHALL NOT mint a new Close attempt, silently complete the Close obligation, or mutate project-surface lifecycle state outside the typed Close retry command

WHEN repair completes automatically through operator action or an idempotent external precondition change
THE SYSTEM SHALL converge by driving the same exact-attempt retry/completion authority rather than by fabricating an unbound success path that bypasses the visible needs-repair attempt

**Rationale:** A transient retirement failure should stay user-retryable on the exact visible Close attempt. Reusing the same attempt preserves evidence continuity and avoids hidden local lifecycle drift.

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
