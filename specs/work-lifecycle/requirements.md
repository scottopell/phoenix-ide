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
AND SHALL let bedrock classify confirmed conversation-and-process shutdown as History with `cleanup_attention`; uncertainty before any positive shutdown/History commit remains Open with `CloseIncomplete`, while later uncertainty on an already historical conversation preserves History, retained positive shutdown evidence, and truthful cleanup attention

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
AND SHALL NOT continue to a later step, replay the failed step, or invoke automatic repair, except for the single fresh completion-only successor narrowly allowed by REQ-WL-002f when exact proof establishes all originally planned effects durably succeeded and no resource/destructive effects remain

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

**Rationale:** Retirement reclaims only proven Phoenix-owned disposable resources. A failed run ends rather than converging automatically, except for the single proof-gated completion-only successor in REQ-WL-002f; exact evidence constrains any explicitly admitted resource retry. Repository management and automatic backup creation are not Close effects.

---

### REQ-WL-002e: Durable External Writer Authority Evidence

WHEN an admitted Close run blocks worktree retirement because it observes an external writer or an inspection gap
THE SYSTEM SHALL persist an immutable child observation bound to the exact attempt, admitted run ordinal, WorkScope, inspection generation and fingerprint, and captured resource identity before returning that blocker
AND SHALL distinguish positive authority from an inspection gap

Positive evidence shall retain the detector, PID and process incarnation, executable when observable, matched path and object kind, access mode, and authority kind. A gap shall retain its detector, reason, and PID when attributable; it shall not fabricate process identity or writer authority. A reused PID shall not identify the same process without an equal incarnation.

Directory descriptors, including read-only directory descriptors, and process working directories inside the worktree confer path-relative write authority. Ordinary regular-file descriptors confer authority only when writable. A writable shared mapping confers authority independently of ordinary descriptors. A changing read position on a read-only regular-file descriptor shall not constitute writer authority or an inspection gap when its identity and access classification remain stable. Changed process incarnation or unstable descriptor identity shall be an inspection gap.

An unavailable or malformed process inventory, or malformed, inaccessible, or changing evidence for a same-user candidate, shall be a fail-closed inspection gap. Access denial for an unattributed ambient candidate shall not constitute positive writer evidence. A Linux candidate positively identified as a single-task zombie by unambiguous kernel State and Threads fields, a matching singleton task inventory, and stable reinspection shall not require an executable link. A zombie thread-group leader with remaining or uncertain sibling threads shall remain a fail-closed inspection gap. Missing or ambiguous state shall remain an inspection gap; transition from a live candidate during inspection shall remain unstable evidence.

Observations shall be normalized child rows, not embedded collections or a parallel JSON payload. Writes shall require the exact active run, captured scope, inspection and resource binding. An identical replay may succeed without changing the row; replacement, mutation, deletion and conflicting replay shall be rejected during the owning aggregate's lifetime. Authoritative permanent Delete of an ordinary History aggregate shall remove these children by the owning product's deletion cascade in the same transaction, only after no conversation members remain. Empty membership alone shall not authorize child deletion while the owning product still exists. Observations from earlier runs shall remain attached to their original ordinal and shall not authorize a retry or process termination.

IF the blocking observation cannot be persisted
THE SYSTEM SHALL preserve the target and fail the retirement step rather than deleting without durable evidence

A pre-admission retry probe shall not invent a run ordinal or write admitted-run evidence. These observations confer no authority to terminate an external process and require no browser API or UI surface.

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
AND SHALL retain uncertain process shutdown before any positive shutdown/History commit as Open with `CloseIncomplete` rather than newly claiming History; later uncertainty SHALL preserve already committed History and prior positive shutdown evidence with truthful cleanup attention, not reopen execution

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
AND SHALL issue `CloseSafeRetryRequested(product_conversation, attempt_id, failed_run_ordinal, safety_evidence)` only for the retained non-native user/Global modern retry adapter on an operation not yet bound to native admission
AND SHALL route native `close_conversation` retry exclusively through `CloseConversationRequested` and the atomic exact-plan/receipt/admission contract below, never the retained generic event
AND SHALL NOT treat that retained modern adapter as the legacy HTTP compatibility path or expand either adapter's permissions
AND SHALL create a fresh durable run with a new monotonically increasing run ordinal under that original `attempt_id`
AND SHALL retain the stopped run unchanged and bind new inspection, effects, failures, and outcomes to the fresh run ordinal
AND SHALL permit at most one running Close run for that operation
AND SHALL reject stale or repeated admission for the same failed run

THE SYSTEM SHALL restrict that fresh run to the exact remaining resources and effect kinds authorized by the original Close
AND SHALL freshly verify ownership, identity, shutdown, reconstructibility, loss risk, and resolved failure preconditions before dispatch
AND SHALL NOT repeat an unresolved failure, replay already-completed effects, expand the target set, reopen the conversation from History, or resume the failed run
AND SHALL stop the fresh run at its first failure under the same contract as normal Close

WHEN an operation first receives a native Close receipt or admission
THE SYSTEM SHALL atomically bind it monotonically as `native_managed`, serialized with concurrent retry admission and finalization
AND SHALL reject subsequent generic non-native retry admission against that operation instead of bypassing the exact native plan/receipt boundary
AND SHALL permit only the exact previously admitted non-native run to finish if its admission won the race before native adoption; adopting a receipt SHALL NOT revoke or redispatch that run
AND SHALL initialize this binding false for existing unadopted operations, without minting a missing automatic retry allowance or changing original resource authority
AND SHALL keep `UserRequestsCloseConversation` an intentionally separate legacy UI/HTTP initial ingress, never another native adapter

WHEN a retirement step fails or cannot satisfy an exact resource precondition
THE SYSTEM SHALL pass its actual typed authority and exact affected-resource/scope evidence to the shared run-stopping boundary in the same owned failure decision
AND SHALL create the normalized resource-failure children with the stopped primary failure and mandatory delivery, rather than merely declaring a resource relation
AND SHALL classify resource retirement/identity/absence/reconstructibility failure as `resource_failure` with nonempty exact expected-resource evidence, and missing live process permission as `source_authority_lost` with its actual affected-resource evidence
AND SHALL NOT derive authority or resource evidence by parsing residual strings or fabricate live identity from an ambiguous observation
AND SHALL distinguish an intentional no-resource-failure case from unknown outcomes; neither missing evidence nor uncertainty SHALL establish successful completion

WHEN safe retry would change loss risk, discard unique or uncertain work, expand effects, or require database surgery
THE SYSTEM SHALL stop without performing those effects
AND SHALL require a concrete proposal describing exact targets, evidence, risk, intended mutation, and required user decision
AND SHALL NOT derive that additional authority from the original Close, Global's failure receipt, or a generic retry request

**Rationale:** Original Close authority permits a narrowly bounded explicit safe retry, not automatic recovery or open-ended repair. Separate run ordinals make each stopped execution and its failure observable without rewriting history.

---

### REQ-WL-002f: Unified Close Is Idempotent with One Bounded Completion Retry

WHEN a trusted, proven current Coordinator source requests Close for an ordinary ProductConversation
THE SYSTEM SHALL provide one unified `close_conversation` operation for initial Close, exact retained-operation observation, and explicit safe retry of an exact failed run, without exposing `retry_close_safely` as a second admission authority
AND SHALL derive authority from server-verified source identity and the current Coordinator, never caller-supplied authorizer identity
AND for an initial request to an Open ProductConversation without retained work requiring observation or an exact reference, SHALL capture exactly one ordinary ProductConversation/latest-row admission snapshot and atomically persist the trusted source-derived request identity with its Close-attempt and run identity
AND SHALL make duplicate, concurrent, replayed, and lost-response observations return that same retained operation/result without allocating another identity, changing topology, or admitting another run
AND SHALL reject absent or stale references for a retained action without selecting a latest fallback or creating an operation
AND SHALL preserve existing active, failed, completed, and cancelled references as exact attempt, run ordinal, and ProductConversation identities

WHEN a supported transaction writer performs the first complete retirement capture for a Close attempt
THE SYSTEM SHALL atomically capture its full inventory and establish one immutable original-snapshot anchor for the exact inspection generation and fingerprint
AND a retry or resumed inspection with a later generation SHALL preserve that anchor rather than replace or adopt it
AND the schema SHALL enforce the complete sealed shape of an original anchor and the immutability of original anchors and negative exclusions
AND every attempt that predates this migration SHALL be negatively excluded from original-anchor provenance, including attempts with no retained inventory or other ambiguity
AND no positive anchor SHALL be backfilled for a pre-migration attempt or restored through native adoption
AND legacy shared capture, reinspection, observation, and generic retry operations SHALL continue normally, but SHALL NOT gain native original-anchor proof from those writes
AND partial repair and copied inventory writes SHALL NOT originate original-anchor proof
AND persistence SHALL refuse a supported fresh capture's anchor unless its exact captured scope set has a sealed inventory and matching expected resources, including each scope's WorkScope identity
AND this supported-writer transaction guarantee and schema shape SHALL NOT claim a temporal guarantee against arbitrary future SQL writers

WHEN a native initial Close finalizes successful retirement or replays its retained successful completion
THE SYSTEM SHALL verify the exact live and durable source, retained initial request, original run, and target before returning either result
AND fresh finalization SHALL require the anchored sealed original inventory, exact typed dispatch authority, and successful retirement or exact absence-adoption proof for every expected resource
AND successful replay SHALL read only the same latest completed initial run and its retained archived History outcome without rewriting evidence, terminal timestamps, messages, or lifecycle
AND generic legacy completion ingress SHALL NOT bypass native initial source authority, including on completed replay
AND legacy completion authority SHALL remain available to operations adopted only through observation, without treating native management alone as an initial-request binding
AND stopped, cancelled, mismatched, or superseded initial runs SHALL NOT qualify as successful completion replay.

WHEN a native initial Close records a resource failure or observes its exact stopped failure again
THE SYSTEM SHALL verify the same live and durable source, retained initial request, original run, and target before accepting either a fresh failure or its replay
AND SHALL bind the failed resource to the complete sealed original inventory and exact inspection snapshot, preserve every successful prefix, and retain exactly the unresolved resource tail with the primary resource Failed and other remaining resources Unattempted
AND SHALL atomically retain the failure, residual evidence, stopped run, outcome, and mandatory failure delivery, with exact replay read-only and conflicting replay refused
AND generic legacy failure ingress SHALL NOT bypass native initial source authority; source-owned inspection and interruption classification SHALL retain their separately proven authority boundaries
AND a resource-failure path SHALL NOT invent an unresolved resource when all resources already have successful proof.

WHEN an authorized explicit action requests retry of an exact retained failed Close run and fresh read-only safety proof establishes the retry is safe under the original authority
THE SYSTEM SHALL admit at most one exact successor run with the next ordinal under the original Close authority
AND duplicate observations SHALL report that retained successor rather than admit another run
AND SHALL retain the failed run's immutable evidence and exact snapshot, scope, loss, and effect boundaries
AND SHALL atomically persist the exact immutable typed remaining-effect plan or positive verified-completion provenance with the successor and request receipt, dispatching only that run-bound plan rather than generic initial settlement or inspection
AND SHALL keep verified-completion explicit retry observation-only with zero resource-effect rows; a resource-bearing explicit retry SHALL exclude every already completed effect and recheck the exact admitted safety/identity before each remaining effect
AND SHALL include independent `WorkScope` retirement in the typed remaining-effect vocabulary alongside tmux and worktree retirement, binding its sealed expected resource identity and exact original scope, original authority/gate generation, fresh safety evidence, plan order, and fingerprint
AND SHALL order each pending scope retirement after pending resource effects in that scope, satisfy prior successes from retained proof without redispatch, freshly verify no distinct live owner or live scoped resource before scope retirement, and persist its exact success/failure against its admitted row
AND SHALL support a scope-only remaining plan without redoing prior tmux/worktree success, and count the scope's independent durable outcome in full-plan completion and automatic-completion proof; pending scope retirement SHALL NOT qualify as zero-effect verified completion
AND SHALL intentionally exclude process-epoch stop effects from this retry vocabulary; missing original live in-memory permits SHALL remain fail-closed and SHALL NOT be recreated from durable identities, adoption, restart, or generic fallback

WHEN the original Close operation has a remaining automatic completion-retry budget
AND fresh exact retained proof establishes that every originally planned effect durably succeeded and no resource or destructive effects remain
AND a live admitted source remains authorized, unsuppressed, and freshly proves conversation-and-owned-process shutdown
THE SYSTEM SHALL atomically consume that budget while admitting at most one fresh completion-only successor run
AND SHALL make that budget durable and nonrenewable across explicit retry, repeated call, progress, continuation, restart, lost response, or Coordinator replacement
AND SHALL create the single allowance only with the original operation, never when adopting a retained operation without an original allowance
AND SHALL require the exact stopped latest run to have ordinal greater than one, typed `AttemptInterrupted` primary failure authority with zero resource-failure children (not zero primary failures or mandatory deliveries), and positive persisted provenance from a nonempty exact resource plan with every effect durably successful, or from a verified-completion plan with zero effects and preserved positive completion provenance under the same original operation
AND SHALL NOT infer eligibility from initial-run interruption, an interruption label alone, an empty unproven plan, unknown effect result, or incomplete proof
AND SHALL NOT create a hidden bootstrap retry to make an initial run eligible; only an independently authorized explicit exact-run retry may establish qualifying later-run evidence
AND SHALL NOT automatically admit resource-plan or destructive effects, replay a completed effect, or guess an unknown failure
AND SHALL remain observation-only at startup and SHALL NOT dispatch or rearm automatic retry there
AND SHALL leave legacy or adopted operations without an allowance unchanged and ineligible; startup bookkeeping SHALL be conditional on an existing allowance and SHALL NOT fabricate or backfill one
AND SHALL connect live completion-only admission to one owned read-only observation and a definite successful or failed finalization, never silently drop an admitted Running successor on authority/proof loss; inability to establish durable terminal state SHALL close process authority rather than guess success or retry

WHEN Stop, pause, or cancellation is durably admitted
THE SYSTEM SHALL disable pending automatic retry and fence racing callbacks
AND SHALL NOT allow a repeated Close request or newer request to clear suppression or rearm a cancelled attempt
AND SHALL truthfully report exhausted budget, ineligibility, or unavailable durable storage without blind retry loops
AND SHALL atomically stop an admitted but unfinished automatic successor with its coherent exact-run failure and mandatory once-per-run delivery if suppression wins, preserving completed effects and the consumed allowance
AND SHALL let a losing callback observe that retained terminal disposition rather than leave the successor running or finalize through suppression
AND SHALL bind these postconditions directly to exact Close cancellation and the owning durable Stop/pause/source-fence admission transaction, not an optional later notification; source replacement or loss of the admitted live permit SHALL terminalize any unfinished automatic successor through the same fenced failure boundary
AND SHALL apply cancellation to exact stopped retained and admitted completion-only runs without undoing successful effects, reopening History, or discarding truthful residual attention
AND for exact retained cancellation of an explicit successor, SHALL atomically settle lifecycle using the same stable shutdown proof as its failure while preserving the terminal cancelled outcome: prior History stays History; prior Open with positive conversation-and-owned-process shutdown enters History with cleanup attention; prior Open without that proof stays Open with typed CloseIncomplete
AND SHALL persist that lifecycle/attention disposition, exact failure/resource evidence and delivery with cancellation, publish History only on a real Open-to-History commit, and SHALL NOT rely on a later failure event whose classifier excludes completed operations to repair lifecycle

WHEN a completion-only successor commits its successful outcome
THE SYSTEM SHALL atomically record completion, clear matching cleanup attention, persist its exact outcome message, and publish entry to History only if that commit actually moves the ProductConversation from Open to History
AND SHALL retain History with truthful attention if a later failed successor has uncertain residual observations; it SHALL NOT reopen settled execution

Repeated observation SHALL NOT supply stop-work/loss confirmation, cancel-and-rearm authority, reopen History, or bypass existing sealed gates, live permits, fingerprints, loss guards, or once-per-run failure delivery. A new operation after cancellation SHALL require the exact cancelled predecessor and explicit new Close intent. Direct HTTP behavior SHALL remain unchanged, and Close SHALL NOT bypass Stop/pause admission, mutate branches or PRs, or create recovery artifacts.

**Rationale:** One retained operation identity makes tool retries safe without turning observation into fresh destructive authority. A single atomic, proof-gated completion-only retry handles the narrow case where all planned work is already durable, while preserving explicit authority for every resource/destructive retry.

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
