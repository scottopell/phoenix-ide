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

### REQ-WL-002b: Retirement Seals One WorkScope Gate and Retires Its Owned Resources Without Automatic Recovery Artifacts

WHEN bedrock requests resource retirement for one exact Close attempt
THE SYSTEM SHALL retire the owned worktree and WorkScope-scoped resources for every attached `WorkScope` targeted by that ProductConversation operation, including each worktree itself, bash/process-group resources, tmux resources, PTY/terminal resources, browser resources, and equivalent live execution resources owned by that exact WorkScope

THE SYSTEM SHALL seal one admission gate for each exact attached `WorkScope` before retiring its currently owned resources
AND SHALL derive cleanup authority only from the exact ProductConversation's committed Close retirement operation targeting that attached `WorkScope`
AND SHALL reject new resource admission through that sealed gate until the attempt either completes or enters typed repair

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
THE SYSTEM SHALL preserve any safe leftover and route the exact Close attempt to `NeedsRepair`

Concurrent malicious mutation inside a Phoenix-owned private namespace, and mutation of resources outside Phoenix ownership, are outside the supported Close reliability boundary

WHEN retirement succeeds overall
THE SYSTEM SHALL emit success only after the sealed gate has stopped its currently owned process-epoch resources and the required durable tmux and worktree outcomes are recorded

WHEN retirement cannot retire a required durable resource or worktree
THE SYSTEM SHALL report typed residual cleanup state and repair information rather than silently succeeding

WHEN the worktree is already absent
THE SYSTEM SHALL bind that absence evidence to the exact retirement attempt and attached `WorkScope`
AND SHALL accept the absence only when retained worktree identity and same-attempt evidence show that the requested retirement removed it or is adopting that exact absence
AND SHALL otherwise report typed residual evidence rather than silently treating the absence as success

WHEN Phoenix resumes an interrupted Close attempt
THE SYSTEM SHALL reseal its exact WorkScope gate and reinspect the current worktree before destructive worktree removal
AND SHALL safely retire a tmux server only when its sealed socket path and Phoenix-controlled server token identify the same server
AND SHALL leave process-epoch resources that have no live in-memory permit untouched
AND SHALL route an ambiguous tmux server, worktree, or ownership record to `NeedsRepair`

WHEN the attached `WorkScope` also owns attachments or other work-affine retained resources that are shared across transcript rows of the same open product conversation
THE SYSTEM SHALL retire or preserve those resources according to that same WorkScope ownership boundary rather than according to individual transcript-row ownership

CONFIRMED retirement SHALL NOT create a branch, tag, commit, stash, patch, diff snapshot, or other automatic recovery artifact

THE SYSTEM SHALL leave every branch, tag, stash, remote-tracking ref, and pull request untouched
AND SHALL NOT create, rename, move, fast-forward, merge, delete, push, close, or retarget any branch or pull request as a side effect of Close or retirement

**Rationale:** Retirement must reclaim exactly the resources Phoenix owns, converge safely across retries and restarts, and never disguise destructive teardown as repository management or automatic backup creation.

---

### REQ-WL-002d: Durable Tmux Instance Authority

WHEN Phoenix creates a tmux server for a `WorkScope`
THE SYSTEM SHALL allocate a Phoenix-controlled server token before server creation
AND SHALL seal the server's socket path and token as the durable tmux identity for a Close attempt

WHEN Phoenix resumes retirement after a restart
THE SYSTEM SHALL retire a tmux server only when its sealed socket path and Phoenix-controlled server token prove the same server remains live
AND SHALL treat a socket path alone as insufficient authority

WHEN the sealed tmux identity proves that the requested server is absent and a distinct replacement owns the reused socket path
THE SYSTEM SHALL leave the replacement untouched and record the requested server's exact-attempt absence outcome

WHEN Phoenix cannot prove the sealed tmux identity
THE SYSTEM SHALL preserve the server and route the Close attempt to `NeedsRepair`

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
AND SHALL keep inaccessible observations in typed repair until a later exact observation proves `missing` or the resource is retired normally
AND SHALL otherwise route conflicts to typed repair rather than silently succeeding

**Rationale:** A missing registered worktree after restart is a repair-class ownership problem, not a hidden terminal lifecycle. Keeping the product aggregate Open preserves the one Close/reconciliation contract while typed evidence lets later retries reason from persisted ownership without guessing.

---

### REQ-PROJ-WS-001: WorkScope Has One Ordinary ProductConversation Owner

Each persisted `WorkScope` SHALL have exactly one ordinary `ProductConversation` owner through one authoritative writable representation. Work-affine resource ownership SHALL derive from that opaque persisted `work_scope_id`, not from transcript-row IDs, sub-agent IDs, working directories, or worktree paths. Continuation rows and subordinate execution conversations MAY refer to their owning aggregate's `WorkScope`, but SHALL NOT create a separate ownership relation. A distinct ordinary `ProductConversation` SHALL NOT share that `WorkScope`, even when an environment path happens to match. Legacy records that imply multiple or no ordinary owners SHALL be retained as repair input and SHALL NOT confer destructive authority.


---

### REQ-WL-002c: Needs-Repair Retry Reuses the Same Exact Close Attempt

WHEN resource retirement for one exact Close attempt fails and leaves the product conversation in a visible needs-repair state
THE SYSTEM SHALL expose a retry affordance bound to that same exact `attempt_id`
AND SHALL issue `CloseRetirementRetryRequested(product_conversation, attempt_id)` from that visible needs-repair state rather than from a fresh local lifecycle mutation

WHEN the user invokes retry from needs-repair
THE SYSTEM SHALL request retirement again for that same exact Close attempt
AND SHALL preserve the attempt-bound retirement evidence and residual state already recorded for prior steps
AND SHALL NOT mint a new Close attempt, silently complete the Close obligation, or mutate ProductConversation lifecycle state outside the typed Close retry command

WHILE the exact Close attempt is in `NeedsRepair`
THE SYSTEM SHALL require an explicit `CloseRetirementRetryRequested(product_conversation, attempt_id)` before resuming retirement
AND SHALL NOT interpret startup, migration, background observation, progress replay, operator filesystem repair, or an external precondition change as that retry request

WHEN Phoenix starts with an exact Close attempt already in `RetirementRequested`
THE SYSTEM MAY recover that already-authorized retirement without a new retry request
AND SHALL reseal the exact scope gates and revalidate live identity and writer safety before destructive work
AND SHALL NOT extend this startup authority to `NeedsRepair`

WHEN an explicit retry is accepted
THE SYSTEM SHALL return the same attempt to retirement inspection before authorizing resumed destructive work
AND SHALL retain prior generations as evidence rather than treating them as fresh discard confirmation
AND SHALL use a fresh inspection generation except for the specifically supported partial-generation shape in REQ-WL-005
AND SHALL NOT fabricate an unbound success path that bypasses the visible needs-repair attempt

WHEN a scope-specific observation or cleanup step fails
THE SYSTEM SHALL retain the exact failing scope through error propagation and atomically persist the typed residual resource and `NeedsRepair` transition for that scope
AND SHALL NOT substitute the first attached scope or an unscoped fallback identity

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


---

### REQ-WL-004: Fresh-Generation Cleanup Authority Is Adopted Atomically Within One Attempt

WHEN an authorized retry or interrupted retirement needs a retained worktree cleanup plan in its active inspection generation/fingerprint pair
THE SYSTEM SHALL bind adoption to the same exact Close attempt, captured WorkScope, worktree resource kind, identity kind, identity codec, and identity value
AND SHALL require that resource in the target generation's sealed expected-resource inventory
AND SHALL require fresh observation of the retained Git administrative-directory locator and incarnation before adoption
AND SHALL NOT infer continuity from a worktree path, branch name, or a different attempt

THE SYSTEM SHALL select the newest compatible prior-generation plan with its exact matching dispatch and inventory, excluding plans already consumed as a lineage source
AND SHALL order eligible sources by inventory capture time descending, then cleanup-plan row order descending
AND SHALL require its administrative-directory locator and incarnation to match the fresh observation
AND SHALL copy the complete cleanup payload, including any bound final-tombstone root and object identities

THE SYSTEM SHALL commit the target dispatch, target cleanup plan, and exact source-to-target adoption relation in one transaction
AND SHALL retain the source dispatch, plan, inspection, residual evidence, and prior lineage
AND SHALL enforce one incoming adoption per exact target and one outgoing adoption per exact source
AND SHALL freeze the source plan payload once it is adopted and prohibit lineage update or ordinary deletion

WHEN the identical target adoption is requested again
THE SYSTEM SHALL revalidate fresh administrative identity and exact source/target payload equality
AND SHALL return the existing plan without adding a second lineage edge or dispatch

WHEN source authority is missing, target authority is stale or unsealed, fresh identity differs, or a target dispatch, plan, or lineage conflicts
THE SYSTEM SHALL roll back all adoption writes
AND SHALL report typed evidence-invariant failure without destructive cleanup
AND SHALL NOT leave a dispatch committed without its plan or a plan committed without its lineage

WHEN a further explicit retry follows
THE SYSTEM SHALL extend the lineage from the newest eligible plan rather than rewriting an earlier source

WHEN a completed History aggregate is explicitly hard-deleted
THE SYSTEM MAY delete its adoption history only after all transcript members have been removed, within the authorized aggregate deletion transaction
AND SHALL remove lineage dependencies before deleting referenced plans and the Close obligation
AND SHALL NOT use this deletion exception during retry, startup recovery, or migration

---

### REQ-WL-005: Partial-Generation Foreign-Key Failure Has One Supported Recognizer

WHEN an exact Close obligation is in `awaiting_retirement_inspection` with a retained inspection generation/fingerprint pair
THE SYSTEM MAY resume that same pair as `RetirementRequested` only when all of the following retained conditions hold:

- At least one captured scope owns a worktree, and the total retained per-scope inspection count equals the captured worktree-owning scope count.
- Every captured scope has a sealed retirement inventory for the retained active pair, with no missing or extra active inventory.
- The active pair's residual-resource count equals the captured worktree-owning scope count; each such scope has its exact worktree residual with resource and identity kind `worktree`, identity codec `worktree_id_v1`, captured identity value, and reason `manual_repair_required`.
- Each exact residual detail equals the complete retained diagnostic `Database error: error returned from database: (code: 787) FOREIGN KEY constraint failed`; a prefix, suffix, generic FK-787 message, or cleanup-plan-prefixed message does not match.
- The active pair has no dispatch and no worktree cleanup plan for the attempt.
- Every captured worktree-owning scope has a cleanup plan and matching dispatch from a different generation/fingerprint pair of this same attempt, scope, and exact worktree resource identity.

THE SYSTEM SHALL evaluate this conjunction and the phase change atomically
AND SHALL retain all existing inspections, inventories, residuals, dispatches, plans, and identity bindings unchanged
AND SHALL make repeated recognition a no-op once the phase is no longer `awaiting_retirement_inspection`

WHEN that recognizer succeeds
THE SYSTEM SHALL reacquire the exact captured scope leases before runtime retirement
AND SHALL perform live administrative-directory, worktree/quarantine/tombstone identity validation and writer inspection before removal
AND SHALL adopt prior-generation cleanup authority only through REQ-WL-004
AND SHALL preserve ambiguous or replaced resources in typed repair

WHEN any recognizer condition fails
THE SYSTEM SHALL NOT use the retained pair as this compatibility shortcut
AND SHALL use ordinary fresh inspection or typed repair without deleting retained evidence

THE SYSTEM SHALL support only this enumerated foreign-key partial-generation shape
AND SHALL NOT treat an arbitrary SQLite error, generic manual-repair reason, resource absence, or unmatched historical cleanup plan as compatibility authority
AND SHALL NOT use recognition to leave `NeedsRepair` without the explicit retry required by REQ-WL-002c

---

### REQ-WL-006: Linux Writer Inventory Covers All Processes and All Tasks Fail-Closed

WHEN Linux retirement inspects whether a quarantined worktree or final-tombstone object has a process working directory within it
THE SYSTEM SHALL enumerate every numeric process entry in the visible process namespace and every numeric task entry for every enumerated process
AND SHALL inspect each task's cwd rather than only the thread-group leader's cwd
AND SHALL NOT restrict this inventory to Phoenix, its process groups, or its effective UID

THE SYSTEM SHALL bind process and task observations to their proc-directory device/inode and parsed stat identity, including numeric ID, start time, start-code, end-code, and start-stack values
AND SHALL revalidate process incarnations, task sets and incarnations, and the full process set before accepting a clean scan

WHEN any task cwd is within the retirement object
THE SYSTEM SHALL preserve the object and report residual writer evidence

WHEN enumeration, cwd or stat access is denied, missing, malformed, or incomplete
OR a process/task appears, disappears, or changes incarnation during the scan
THE SYSTEM SHALL classify the inventory as indeterminate and preserve the object in typed repair
AND SHALL NOT treat a vanished process, PID reuse, exec, an empty task set, or an unreadable non-Phoenix process as proof of a clean scan

THE SYSTEM SHALL NOT signal an ambient process solely because writer inspection finds it
AND SHALL limit this guarantee to the visible Linux process namespace and the private-directory reliability boundary of REQ-WL-002b

---

### REQ-WL-007: Final-Tombstone Recovery Reinspects Writers Before Deletion

WHEN retirement resumes with a retained final-tombstone binding
THE SYSTEM SHALL validate the Phoenix-owned owner-only tombstone root through a no-follow opened descriptor and its retained device/inode
AND SHALL validate the moved object relative to that descriptor against its retained device/inode and captured worktree fingerprint
AND SHALL freshly inspect writers at the actual current deletion location, including process cwd and open-descriptor inspection
AND SHALL revalidate the root, object, and worktree fingerprint after inspection and before deletion
AND SHALL NOT reuse a pre-rename, pre-crash, or prior-generation clean scan as permission to delete

WHEN writer evidence is present or indeterminate, an identity has changed, or an observation fails
THE SYSTEM SHALL leave the object untouched and route the exact attempt and failing scope to typed repair

WHEN a crash occurs after tombstone-root binding but before object binding
THE SYSTEM SHALL resume normal identity-checked cleanup only if the tombstone has no moved object and exactly one of the captured or quarantine paths contains the exact retained worktree
AND SHALL reinspect writers and revalidate that source before continuing
AND SHALL preserve an already-moved but unbound tombstone object for repair rather than guessing its identity

WHEN a bound tombstone object is absent after an interrupted deletion
THE SYSTEM SHALL accept completion only with same-attempt durable cleanup authority and verified absence of the captured and quarantine paths and of any tombstone object
AND SHALL accept a missing tombstone root only when the captured and quarantine paths are also proven missing
AND SHALL NOT interpret inaccessible paths, replacements, or partial identity bindings as absence

---

### REQ-WL-008: Cleanup-Lineage Upgrade Preserves Evidence Without Executing Retirement

WHEN migration 118 installs exact cleanup-plan adoption constraints and lineage storage
THE SYSTEM SHALL preserve every existing Close obligation, phase, captured member and scope, inspection, inventory, resource outcome/history, dispatch, cleanup plan, and bound tombstone identity
AND SHALL NOT synthesize adoption lineage for existing rows
AND SHALL NOT automatically retry an attempt, release a repair gate, rename or delete a worktree/quarantine/tombstone, or discard residual evidence

WHEN existing rows violate the required exact identity constraints
THE SYSTEM SHALL fail the migration rather than silently deleting, merging, or rewriting those rows

THE SYSTEM SHALL treat this as a forward-only feature-scoped upgrade under `specs/compatibility/requirements.md`
AND SHALL NOT infer downgrade, mixed-version database access, generic foreign-key repair, or arbitrary legacy recovery guarantees from it
