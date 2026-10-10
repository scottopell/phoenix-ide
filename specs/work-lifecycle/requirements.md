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

Compatibility adapters for `abandon` and `mark_merged` SHALL map those inputs into the one Close flow, preserving the confirmation and loss-safety contract, and SHALL NOT expose them as writable lifecycle choices.

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
AND SHALL bind each evidence set to that original Close authority, exact run ordinal, and exact attached `WorkScope` identity
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
AND fresh identity and reconstruction evidence proves that the targeted disposable state is reconstructible without losing unique work
THE SYSTEM SHALL allow retirement to proceed without a discard confirmation

WHEN reconstruction or loss evidence is uncertain
THE SYSTEM SHALL preserve the targeted state and require a concrete proposal and explicit discard decision before deleting it
AND SHALL NOT equate a clean status result, ignored-path exclusion, durable branch presence, or lack of inspection evidence with proof of reconstructibility

WHEN multiple attached `WorkScope`s own Git-backed worktrees
THE SYSTEM SHALL determine confirmation from the union of their exact per-scope inventories
AND SHALL NOT treat missing inspection evidence for any attached worktree-owning scope as a no-loss result

**Rationale:** Phoenix owns the disposable environment, not repository history. Loss inspection must warn exactly about worktree-only risk without conflating it with durable refs the user still owns.

---

### REQ-WL-002a: Retirement Inspection Binds Confirmation to One Exact Workspace Generation

WHEN retirement inspection completes for attached Git-backed `WorkScope`s
THE SYSTEM SHALL produce one inspection generation and workspace fingerprint with the categorized results for each exact attached worktree-owning scope

WHEN that inspection requires discard confirmation
THE SYSTEM SHALL expose one concrete user-facing discard-confirmation affordance that issues `UserConfirmsCloseAfterRetirementInspection(product_conversation, attempt_id, run_ordinal, inspection_generation, inspection_fingerprint)`
AND SHALL expose that affordance only while the exact active Close obligation for that `product_conversation` remains in `awaiting_loss_confirmation`
AND SHALL bind that affordance to the exact active Close-attempt identity, running run ordinal, inspection generation, and workspace fingerprint held on that Close obligation
AND SHALL NOT expose that affordance for any stale inspection, completed Close attempt, superseded Close attempt, or non-active transcript row within the same product conversation

WHEN the user confirms discard after a warning-producing inspection
THE SYSTEM SHALL bind that confirmation to the exact inspection generation and workspace fingerprint that justified the warning
AND SHALL route the confirmation through the same atomic recomputation boundary that decides whether destructive retirement may begin

WHEN the workspace changes after inspection and before destructive retirement begins
THE SYSTEM SHALL invalidate the outstanding confirmation
AND SHALL require reinspection before retirement may proceed

WHEN the user declines to continue from that warning state before destructive retirement begins
THE SYSTEM SHALL preserve bedrock's pre-retirement `UserCancelsClose(product_conversation, attempt_id, run_ordinal)` cancellation path as the only cancel affordance
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

### REQ-WL-002b: Retirement Seals One WorkScope Gate and Retires Its Owned Resources Without Automatic Recovery Artifacts

WHEN bedrock requests resource retirement for one exact Close attempt
THE SYSTEM SHALL retire the owned worktree and WorkScope-scoped resources for every attached `WorkScope` targeted by that ProductConversation operation, including each worktree itself, bash/process-group resources, tmux resources, PTY/terminal resources, browser resources, and equivalent live execution resources owned by that exact WorkScope

THE SYSTEM SHALL seal one admission gate for each exact attached `WorkScope` before retiring its owned resources
AND SHALL derive cleanup authority only from the exact ProductConversation's committed Close retirement operation targeting that attached `WorkScope`
AND SHALL reject new resource admission through that sealed gate while Close is in progress or stopped with unresolved shutdown or cleanup
AND SHALL NOT reopen the gate as a side effect of failure, notification receipt, restart, or retry

THE SYSTEM SHALL treat transcript rows and subordinate execution conversations within the same ordinary Open ProductConversation as participants in that one aggregate rather than as independent WorkScope owners
AND SHALL NOT let those subordinate participants independently own, veto, or delay destructive retirement of the ProductConversation's attached `WorkScope`

THE SYSTEM SHALL structurally assign each `WorkScope` to exactly one ordinary `ProductConversation`
AND SHALL permit continuation rows and subordinate execution conversations only as members of that same owning aggregate
AND SHALL reject a distinct ordinary `ProductConversation` attachment to that `WorkScope`
AND SHALL treat legacy conflicting ownership evidence as typed repair rather than electing an owner or beginning destructive teardown

THE SYSTEM SHALL stop resources held by the sealed gate's live process epoch through their in-memory ownership permits
AND SHALL treat bash/process groups, PTY sessions, browser sessions, and equivalent ordinary live execution resources as process-epoch resources rather than durable restart resources

WHEN retirement cannot confidently identify a resource as the resource owned by the sealed gate
THE SYSTEM SHALL leave that resource untouched
AND SHALL report typed repair information rather than silently succeeding

THE SYSTEM SHALL treat the WorkScope admission gate and Phoenix-created private resource directories as the trust boundary for normal Close reliability
AND SHALL perform final directory retirement only through a random Phoenix-owned private directory with owner-only permissions after descriptor-bound identity validation of that directory and the object being removed

WHEN a crash or external mutation makes that identity ambiguous
THE SYSTEM SHALL preserve any safe leftover and stop the exact Close run with typed failure
AND SHALL let bedrock classify confirmed conversation-and-process shutdown as History with `cleanup_attention`, or uncertain shutdown as Open with `CloseIncomplete`

Concurrent malicious mutation inside a Phoenix-owned private namespace, and mutation of resources outside Phoenix ownership, are outside the supported Close reliability boundary

WHEN retirement succeeds overall
THE SYSTEM SHALL emit success only after the sealed gate has stopped its owned process-epoch resources and the required durable tmux and worktree outcomes are recorded

WHEN retirement cannot retire a required durable resource or worktree
THE SYSTEM SHALL report typed residual cleanup state and repair information rather than silently succeeding

WHEN the worktree is already absent
THE SYSTEM SHALL bind that absence evidence to the exact retirement attempt and attached `WorkScope`
AND SHALL accept the absence only when retained worktree identity and exact-run evidence show that the requested retirement removed it or is adopting that exact absence through fresh observation
AND SHALL otherwise report typed residual evidence rather than silently treating the absence as success

WHEN a Close run encounters its first failure or is interrupted
THE SYSTEM SHALL stop all further Close effects for that run across all targeted scopes
AND SHALL retain exact successful steps, failed-step identity, residual resources, and confirmed or uncertain shutdown as durable evidence
AND SHALL NOT continue to a later step, replay the failed step, or invoke automatic repair

WHEN Phoenix starts with an interrupted Close run
THE SYSTEM SHALL observe its retained identities and residual state only
AND SHALL NOT reseal and dispatch cleanup, stop processes, remove resources, or resume the interrupted run

WHEN an explicit fresh safe retry is admitted under REQ-WL-002c
THE SYSTEM SHALL reinspect the exact remaining targets and prove reconstructibility immediately before destructive worktree removal
AND SHALL safely retire a tmux server only when its sealed socket path and Phoenix-controlled server token identify the same server
AND SHALL leave process-epoch resources that have no live in-memory permit untouched
AND SHALL stop on ambiguous tmux, worktree, ownership, or shutdown evidence rather than guessing continuity

WHEN the attached `WorkScope` also owns attachments or other work-affine retained resources that are shared across transcript rows of the same open product conversation
THE SYSTEM SHALL retire or preserve those resources according to that same WorkScope ownership boundary rather than according to individual transcript-row ownership

CONFIRMED retirement SHALL NOT create a branch, tag, commit, stash, patch, diff snapshot, or other automatic recovery artifact

THE SYSTEM SHALL leave every branch, tag, stash, remote-tracking ref, and pull request untouched
AND SHALL NOT create, rename, move, fast-forward, merge, delete, push, close, or retarget any branch or pull request as a side effect of Close or retirement

**Rationale:** Retirement reclaims only proven Phoenix-owned disposable resources. A failed run ends rather than converging automatically; exact evidence constrains any explicitly admitted fresh retry. Repository management and automatic backup creation are not Close effects.

---

### REQ-WL-002d: Durable Tmux Instance Authority

WHEN Phoenix creates a tmux server for a `WorkScope`
THE SYSTEM SHALL allocate a Phoenix-controlled server token before server creation
AND SHALL seal the server's socket path and token as the durable tmux identity for a Close attempt

WHEN Phoenix performs tmux retirement in an explicitly admitted Close run
THE SYSTEM SHALL retire a tmux server only when its sealed socket path and Phoenix-controlled server token prove the same server remains live
AND SHALL treat a socket path alone as insufficient authority

WHEN the sealed tmux identity proves that the requested server is absent and a distinct replacement owns the reused socket path
THE SYSTEM SHALL leave the replacement untouched and record the requested server's exact-attempt absence outcome

WHEN Phoenix cannot prove the sealed tmux identity
THE SYSTEM SHALL preserve the server and stop the run with typed failure
AND SHALL retain uncertain process shutdown as `CloseIncomplete` rather than claiming History

**Rationale:** tmux is intentionally process-persistent. Ordinary execution resources belong to one Phoenix process epoch and Close does not retain universal per-resource restart identity for them.

---

### REQ-PROJ-028a: Restart Reconciles a Missing Registered Worktree as Open Repair Evidence

WHEN Phoenix restarts and the latest Open row of a Git-backed `ProductConversation` still has a registered attached `WorkScope`/worktree tuple but that registered worktree path is missing on disk
THE SYSTEM SHALL keep the owning `ProductConversation` in lifecycle `Open`
AND SHALL record immutable typed restart repair evidence bound to that owning `ProductConversation`, its already-attached `WorkScope`, the attached hidden `GitRepository`, the retained worktree identity, the observed worktree path, the retained worktree fingerprint, one exact observation kind of `missing` or `inaccessible`, one repair-observation generation, and the exact observation time
AND SHALL NOT assign a removed row-terminal state to the latest row as restart classification

THE SYSTEM SHALL treat that typed condition as truthful persisted ownership evidence only
AND SHALL NOT fabricate a replacement `WorkScope`, worktree attachment, detached branch label, branch owner, fallback branch selection, or guessed path continuity for the conversation

THE SYSTEM SHALL integrate that typed condition with Close repair and retry semantics
AND SHALL allow later Close inspection, Close retry, or manual repair flows to adopt an exact `missing` observation idempotently only when exact identity evidence proves that adoption
AND SHALL NOT treat an `inaccessible` observation as absence authority
AND SHALL retain inaccessible observations as uncertainty until a later exact read-only observation proves `missing` or an explicitly admitted run retires the resource
AND SHALL otherwise route conflicts to typed repair rather than silently succeeding

**Rationale:** A missing registered worktree after restart is a repair-class ownership problem, not a hidden terminal lifecycle. Keeping the product aggregate Open preserves the one Close/reconciliation contract while typed evidence lets later retries reason from persisted ownership without guessing.

---

### REQ-PROJ-WS-001: WorkScope Has One Ordinary ProductConversation Owner

Each persisted `WorkScope` SHALL have exactly one ordinary `ProductConversation` owner through one authoritative writable representation. Work-affine resource ownership SHALL derive from that opaque persisted `work_scope_id`, not from transcript-row IDs, sub-agent IDs, working directories, or worktree paths. Continuation rows and subordinate execution conversations MAY refer to their owning aggregate's `WorkScope`, but SHALL NOT create a separate ownership relation. A distinct ordinary `ProductConversation` SHALL NOT share that `WorkScope`, even when an environment path happens to match. Legacy records that imply multiple or no ordinary owners SHALL be retained as repair input and SHALL NOT confer destructive authority.


---

### REQ-WL-002c: Explicit Safe Retry Creates a Fresh Bounded Run Under Original Close Authority

WHEN a Close run is stopped with `CloseIncomplete` or `cleanup_attention`
THE SYSTEM SHALL permit read-only investigation of its exact failure and remaining resources
AND SHALL NOT treat investigation, notification receipt, startup, elapsed time, or an external precondition change as retry authority

WHEN the user or Global explicitly requests a safe retry
THE SYSTEM SHALL admit it only after fresh read-only evidence proves that the failed precondition has resolved and the exact remaining effects are safe under the original Close authority
AND SHALL issue `CloseSafeRetryRequested(product_conversation, attempt_id, failed_run_ordinal, safety_evidence)`
AND SHALL create a fresh durable run with a new monotonically increasing run ordinal under that original `attempt_id`
AND SHALL retain the stopped run unchanged and bind new inspection, effects, failures, and outcomes to the fresh run ordinal
AND SHALL permit at most one running Close run for that operation
AND SHALL reject stale or repeated admission for the same failed run

THE SYSTEM SHALL restrict that fresh run to the exact remaining resources and effect kinds authorized by the original Close
AND SHALL freshly verify ownership, identity, shutdown, reconstructibility, loss risk, and resolved failure preconditions before dispatch
AND SHALL NOT repeat an unresolved failure, replay already-completed effects, expand the target set, reopen the conversation from History, or resume the failed run
AND SHALL stop the fresh run at its first failure under the same contract as normal Close

WHEN safe retry would change loss risk, discard unique or uncertain work, expand effects, or require database surgery
THE SYSTEM SHALL stop without performing those effects
AND SHALL require a concrete proposal describing exact targets, evidence, risk, intended mutation, and required user decision
AND SHALL NOT derive that additional authority from the original Close, Global's failure receipt, or a generic retry request

**Rationale:** Original Close authority permits a narrowly bounded explicit safe retry, not automatic recovery or open-ended repair. Separate run ordinals make each stopped execution and its failure observable without rewriting history.

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


---

### REQ-WL-004: Every Distinct Close Failure Durably Notifies Global Once

WHEN a Close run records a distinct failure, including partial cleanup or interruption
THE SYSTEM SHALL atomically retain a durable failure identity bound to the ProductConversation, original Close authority, run ordinal, failed step, shutdown evidence, and exact residual state
AND SHALL enqueue one mandatory Global event for that failure through the same unified durable delivery/outbox path used for conversation observations
AND SHALL deliver it regardless of watch enrollment or source transition to History
AND SHALL preserve it until durable delivery acceptance
AND SHALL deduplicate failure recording and delivery by that exact failure identity across repeated observations, reconnects, and restarts
AND SHALL NOT create a second mandatory event for the same failure when a watch also observes it
AND SHALL NOT depend on optional logging, an in-memory callback, or an independently managed notification path

WHEN a fresh Close retry run encounters a distinct failure
THE SYSTEM SHALL record its new run-bound failure and deliver its own once-per-failure Global event

WHEN Global receives a Close failure event
THE SYSTEM SHALL convey the difference between lifecycle ended and resources retired, exact remaining resources, failure reason, and allowed read-only investigation
AND SHALL NOT treat delivery acceptance as cleanup success, repair approval, or authority for further mutation
AND SHALL permit further effects only through REQ-WL-002c or a separately approved concrete proposal
