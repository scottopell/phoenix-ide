# Work Lifecycle: Explicit Close for Git-Backed Conversations

## User Story

As a developer using PhoenixIDE, I need one explicit way to close a Git-backed conversation so that I can retire finished or discarded work without Phoenix unexpectedly mutating branches, pull requests, or other repository state I still own.

## Scope

This spec governs the product-facing **Close conversation** action for Git-backed conversations and the WorkScope retirement contract consumed by that Close flow.

It owns:

- the exact retirement-inspection and loss-warning contract for Close on a Git-backed conversation;
- the exact content and ownership blockers that prevent destructive retirement;
- the requirement that Close retire the exact registered Phoenix-owned WorkScope resources and worktree without creating automatic recovery artifacts and without mutating repository refs or PRs;
- the exact blocker contract for data that Close cannot safely preserve or retire, including ignored/build content and active writers;
- the advisory use of PR state to guide the user toward closing when work appears shipped.

It does **not** own:

- conversation-state legality, root lifecycle topology, or durable cancellation sequencing — bedrock and the unified lifecycle specs own those authorities;
- PR feedback freshness, explicit active-PR targeting, auto-fix, and remediation context — the `pr-association` spec;
- UI placement, wording variants, or action-bar composition — the `work-actions-bar` spec owns those concerns.

Compatibility adapters for `abandon` and `mark_merged` SHALL map those inputs into the one Close flow, preserving its blocker, safe-retry, and lifecycle contract, and SHALL NOT expose them as writable lifecycle choices.

---

## Requirements

### REQ-WL-001: Close Is the Only User-Facing Terminal Action for Git-Backed Conversations

WHEN the user chooses to end a Git-backed conversation's active work
THE SYSTEM SHALL expose **Close conversation** as the only ordinary lifecycle action
AND SHALL move the conversation to read-only History only through that Close flow

WHEN legacy clients, stored intents, or migration adapters still reference `abandon` or `mark_merged`
THE SYSTEM SHALL treat those values only as deprecated compatibility inputs
AND SHALL require them to execute the same blocker inspection, cancellation, and finalization contract as Close
AND SHALL NOT expose those deprecated verbs as current writable lifecycle choices in ordinary product surfaces

**Rationale:** The user-facing distinction between “abandoned” and “merged” conflated repository interpretation with lifecycle. The durable product truth is simpler: the conversation is either still Open or the user explicitly closed it into History.

---

### REQ-WL-002: Retirement Inspection Classifies Exact Worktree-Loss Risk Before Destructive Teardown

WHEN bedrock requests retirement inspection for one exact Close attempt
THE SYSTEM SHALL inspect every attached `WorkScope` that owns a Git-backed worktree
AND SHALL bind each evidence set to that original Close authority, exact run ordinal, and exact attached `WorkScope` identity
AND SHALL inspect only state whose durability depends on that scope's attached worktree and owned resources
AND SHALL identify every content or commit that Close cannot preserve, including staged and unstaged tracked paths, conflicted or unmerged paths, untracked paths, ignored paths (including build output), dirty or untracked state in initialized submodules, and detached commits not reachable from any ref under `refs/heads/*`, `refs/remotes/*`, `refs/tags/*`, or `refs/stash`
AND SHALL treat LFS-tracked edits as ordinary tracked changes
AND SHALL treat local branches, remote-tracking branches, tags, and stash entries as durable refs rather than loss
AND SHALL treat reflog-only detached commits as at-risk detached commits
AND SHALL scope nested-repository inspection only to declared submodules

WHEN any content or commit cannot be shown to be preserved under the original Close authority
THE SYSTEM SHALL report an exact blocker inventory with one materialized row per exact `(attached_workscope_identity, category, item_identity)`
AND SHALL include every relevant path and detached-commit identity rather than collapsing multiple items into a category summary
AND SHALL block destructive retirement while those blockers remain
AND SHALL NOT provide an in-Close discard, salvage, or cleanup path for them
AND SHALL require any separate cleanup to be performed by its owner under separate authority, followed by fresh Close inspection
AND SHALL NOT infer permission to discard ignored or build content from `.gitignore`, its absence, or any other repository metadata

WHEN inspection is incomplete or identity is uncertain
THE SYSTEM SHALL preserve the target and report a concrete blocker rather than infer safe deletion

WHEN multiple attached `WorkScope`s own Git-backed worktrees
THE SYSTEM SHALL determine blockers from the union of their exact per-scope inventories
AND SHALL NOT treat missing inspection evidence for any attached worktree-owning scope as a no-loss result

**Rationale:** Phoenix owns the disposable environment, not repository history. Inspection must identify exact at-risk worktree content without treating durable refs as loss or metadata as discard permission.

---

### REQ-WL-002a: Blocker Projection Is Bound to the Exact Close Inspection

WHEN retirement inspection completes for attached Git-backed `WorkScope`s
THE SYSTEM SHALL produce one inspection generation and workspace fingerprint with exact blocker results for every attached worktree-owning scope
AND SHALL present Retry and Stop as the available choices when a blocker prevents progress
AND SHALL NOT expose an in-Close discard, salvage, or repair choice

WHEN the workspace or its ownership/permission evidence changes before a later mutation attempt
THE SYSTEM SHALL re-inspect and project newly observed failures as exact blockers
AND SHALL NOT treat a prior inspection or user choice as permission to mutate changed targets

WHEN a product conversation has no attached `WorkScope` that owns a Git-backed worktree
THE SYSTEM SHALL skip worktree-content inspection for that attempt
AND SHALL NOT let the absence of a worktree-owning scope bypass shutdown/retirement of any attached independently registered WorkScope

**Rationale:** Inspection informs the exact Close decision; it is neither discard authorization nor a guarantee that later mutations will succeed.

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
AND SHALL NOT continue to a later step, replay a failed step automatically, or invoke automatic repair
AND MAY move a definitely stopped conversation from Open to History only when shutdown is positively confirmed and every unresolved blocker is conspicuously retained as incomplete, with its exact evidence and the ordinary explicit safe-retry/Stop affordances
AND SHALL keep uncertain shutdown Open with `CloseIncomplete` before any positive shutdown/History commit; later uncertainty SHALL preserve already committed History, prior positive shutdown evidence, and truthful cleanup attention rather than reopen execution

WHEN mutation-boundary inspection cannot be completed or its evidence cannot be durably retained
THE SYSTEM SHALL preserve the target, stop further effects, and enter the exact run's typed failure boundary rather than silently leave it running or treat missing evidence as a clear inspection
AND if durable terminalization is unavailable SHALL close process authority and report the storage failure without guessing success or retrying effects


WHEN Phoenix starts with an interrupted Close run
THE SYSTEM SHALL observe its retained identities and residual state only
AND SHALL NOT reseal and dispatch cleanup, stop processes, remove resources, or resume the interrupted run

WHEN an explicit fresh safe retry is admitted under REQ-WL-002c
THE SYSTEM SHALL reinspect the exact remaining targets and establish that no unresolved blocker remains immediately before destructive worktree removal
AND SHALL safely retire a tmux server only when its sealed socket path and Phoenix-controlled server token identify the same server
AND SHALL leave process-epoch resources that have no live in-memory permit untouched
AND SHALL stop on ambiguous tmux, worktree, ownership, or shutdown evidence rather than guessing continuity

WHEN the attached `WorkScope` also owns attachments or other work-affine retained resources that are shared across transcript rows of the same open product conversation
THE SYSTEM SHALL retire or preserve those resources according to that same WorkScope ownership boundary rather than according to individual transcript-row ownership

CONFIRMED retirement SHALL NOT create a branch, tag, commit, stash, patch, diff snapshot, or other automatic recovery artifact

THE SYSTEM SHALL leave every branch, tag, stash, remote-tracking ref, and pull request untouched
AND SHALL NOT create, rename, move, fast-forward, merge, delete, push, close, or retarget any branch or pull request as a side effect of Close or retirement

**Rationale:** Close acts only on resources registered to the exact WorkScope and never treats a partial observation as a preservation guarantee. A failed run stops; unresolved data remains a visible blocker for an explicit retry or Stop decision. Repository management and automatic backup creation are not Close effects.

---

### REQ-WL-002e: Durable External Writer Authority Evidence

WHEN an admitted Close run observes positive external writer authority or an external process-inspection gap at the physical worktree boundary
THE SYSTEM SHALL persist a representative immutable child observation bound to the exact attempt, admitted run ordinal, WorkScope, inspection generation and fingerprint, and captured resource identity before deciding whether to continue
AND SHALL distinguish positive authority from an inspection gap and shall not let a gap mask independent verifiable positive evidence

WHEN native Close has confirmed shutdown of the exact Phoenix-owned resources and is authorized to physically retire the worktree
THE SYSTEM SHALL block on current positive target-bound writer authority
AND SHALL NOT block this admitted physical step solely because an external process or its inventory cannot be fully inspected
AND SHALL NOT classify such a gap as unrelated, clean, or proof that no external writer exists

The exception is limited to external process inspection. Uncertain shutdown, target inspection, loss checks, identity, permissions, custody, deadline, or evidence persistence still prevent unproven removal. Pre-admission retry checks and other strict retirement paths still block on gaps. A concurrent editor save or Git change by an undetected writer can be lost; removal of the checkout and exact Git registration does not prove immediate disk-space recovery when open handles remain.

Positive evidence shall retain the detector, PID and process incarnation, executable when observable, matched path and object kind, access mode, and authority kind. A gap shall retain its detector, reason, and PID when attributable; it shall not fabricate process identity or writer authority. A reused PID shall not identify the same process without an equal incarnation.

Directory descriptors, including read-only directory descriptors, and process working directories inside the worktree confer path-relative write authority. Ordinary regular-file descriptors confer authority only when writable. A writable shared mapping confers authority independently of ordinary descriptors. A changing read position on a read-only regular-file descriptor shall not constitute writer authority or an inspection gap when its identity and access classification remain stable. Changed process incarnation or unstable descriptor identity shall be an inspection gap.

An unavailable or malformed process inventory, or malformed, inaccessible, or changing evidence for a same-user candidate, shall be an inspection gap. Access denial for an unattributed ambient candidate shall not constitute positive writer evidence. A Linux candidate positively identified as a single-task zombie by unambiguous kernel State and Threads fields, a matching singleton task inventory, and stable reinspection shall not require an executable link. A zombie thread-group leader with remaining or uncertain sibling threads shall remain an inspection gap. Missing or ambiguous state shall remain an inspection gap; transition from a live candidate during inspection shall remain unstable evidence.

Observations shall be normalized child rows, not embedded collections or a parallel JSON payload. Writes shall require the exact active run, captured scope, inspection and resource binding. An identical replay may succeed without changing the row; replacement, mutation, deletion and conflicting replay shall be rejected during the owning aggregate's lifetime. Empty membership alone shall not authorize child deletion while the owning product still exists. Observations from earlier runs shall remain attached to their original ordinal and shall not authorize a retry or process termination.

IF the required writer observation cannot be persisted
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

WHEN the user explicitly requests a safe retry through native `close_conversation`
THE SYSTEM SHALL admit it only after fresh evidence establishes that the failed precondition has resolved and the exact remaining effects are safe under the original Close authority
AND SHALL create a fresh durable run with a new monotonically increasing run ordinal under that original `attempt_id`
AND SHALL retain the stopped run unchanged and bind new inspection, effects, failures, and outcomes to the fresh run ordinal
AND SHALL permit at most one running Close run for that operation
AND SHALL reject stale or repeated admission for the same failed run

THE SYSTEM SHALL restrict that fresh run to the exact remaining resources and effect kinds authorized by the original Close
AND SHALL freshly verify ownership, identity, shutdown, absence of unresolved blockers, and resolved failure preconditions before dispatch
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

### REQ-WL-002f: Unified Native Close Is Idempotent and Explicitly Retried

WHEN the trusted current Coordinator requests Close for an ordinary ProductConversation
THE SYSTEM SHALL expose one native `close_conversation` operation for initial Close, observation of its retained result, and an explicit safe retry of an exact stopped run
AND SHALL derive authority from server-verified Coordinator/source identity, never caller-supplied authorizer identity
AND SHALL atomically bind the request to the exact ProductConversation, captured latest-row identity, Close attempt, and run
AND SHALL make duplicate, concurrent, replayed, and lost-response calls idempotently return that same retained operation/result without changing topology or implicitly admitting a retry
AND SHALL reject absent or stale retained references without choosing a latest fallback or creating another operation
AND SHALL preserve exact attempt, run ordinal, request, and target identity across retries
AND SHALL present explicit **Retry** and **Stop** decisions for a stopped incomplete Close; neither observation, startup, elapsed time, metadata, nor external state change SHALL authorize retry

WHEN the Close inspection and retirement decision is made
THE SYSTEM SHALL produce one authoritative blocker decision and a user-facing projection from that same decision, naming the exact affected files or commits and the operational reason (writer/permission/layout/error/deadline)
AND SHALL recheck relevant identities and permissions at each mutation boundary and add newly observed failures as concrete blockers
AND SHALL NOT claim that an earlier snapshot guarantees later mutation success

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

WHEN the user explicitly requests retry of an exact retained failed Close run
THE SYSTEM SHALL create a fresh bounded run under the original Close authority only after fresh evidence establishes that the exact remaining effects are safe
AND SHALL retain the failed run unchanged and bind new inspection, effects, failures, and outcomes to a new monotonically increasing run ordinal
AND SHALL admit at most one running run, reject stale/repeated retry admission for the same failed run, and never replay a completed effect or expand the original target set
AND SHALL recheck exact ownership, identity, shutdown, loss blockers, writer/permission state, and resolved failure preconditions at each relevant mutation boundary
AND SHALL stop on any unresolved or newly observed blocker, retaining already completed effects and truthful residual state
AND SHALL NOT resume a stopped run or create automatic metadata-only completion, repair, salvage, or disposable-state classification
AND SHALL atomically persist the exact immutable typed remaining-effect plan or positive verified-completion provenance with the successor run and native request receipt
AND SHALL dispatch only that exact run-bound plan, never generic initial settlement or inspection as an alternative dispatch path
AND duplicate or lost-response observations SHALL return the retained successor and receipt without admitting another run
AND a verified-completion explicit retry SHALL be observation-only with zero resource-effect rows, supported by positive persisted proof of every original resource and independent WorkScope outcome and confirmed conversation-and-owned-process shutdown
AND SHALL NOT infer verified completion from an interruption label, an empty unproven plan, unknown outcomes, missing evidence, or an unfinished scope
AND SHALL include independent `WorkScope` retirement alongside tmux and worktree retirement in the typed remaining-effect vocabulary, binding each row's exact original scope, sealed expected resource identity, original authority/gate generation, fresh safety evidence, order, and fingerprint
AND SHALL order scope retirement after pending resource effects in that scope, satisfy prior successes from retained proof without redispatch, freshly verify no distinct live owner or live scoped resource, and persist exact success/failure against the admitted row
AND SHALL support a scope-only remaining plan without repeating tmux/worktree successes and count the independent scope outcome in full-plan completeness
AND SHALL exclude process-epoch stop effects from the retry vocabulary; missing original live in-memory permits SHALL remain fail-closed and SHALL NOT be recreated from durable identities, adoption, restart, or generic fallback
AND SHALL connect each admitted run to definite successful or failed terminalization; inability to persist terminal state SHALL close process authority and report the failure rather than leave an authorized Running run or guess success

WHEN the user chooses Stop for a stopped incomplete Close
THE SYSTEM SHALL keep unresolved targets and evidence intact, preserve truthful lifecycle state, and end further Close effects for that operation

WHEN Stop, pause, exact cancellation, or source replacement/loss revoking admitted authority is durably admitted for an exact explicit retry
THE SYSTEM SHALL fence racing effects, terminalize any unfinished exact run, and atomically retain its coherent failure, typed affected-resource evidence or intentional no-resource-failure case, residuals, cancelled outcome where applicable, lifecycle/attention disposition, and mandatory once-per-run failure outbox
AND exact cancellation of an already stopped retained explicit retry SHALL atomically settle that same shutdown-based lifecycle/attention and cancelled operation outcome while preserving the stopped run's immutable evidence and once-per-run delivery identity
AND SHALL bind those postconditions directly to the owning durable Stop/pause/cancellation/source-fence admission transaction rather than an optional later callback or failure event
AND SHALL preserve successful effects and prior History; prior Open with stable positive conversation-and-owned-process shutdown SHALL enter History with cleanup attention, while prior Open without that proof SHALL remain Open with typed `CloseIncomplete`
AND SHALL publish entry to History only on an actual Open-to-History commit and let losing callbacks observe the retained terminal disposition without finalizing through suppression
AND SHALL NOT let observation, a repeated Close request, or source replacement clear suppression or rearm a cancelled attempt
AND inability to persist that terminal disposition SHALL close process authority and report the storage failure, not permit further effects or assume terminal success

WHEN an explicitly admitted verified-completion run commits success
THE SYSTEM SHALL atomically record its completion, clear only matching cleanup attention, retain its exact outcome message, and publish entry to History only if that commit actually transitions Open to History

Repeated observation SHALL NOT supply stop-work/loss confirmation or retry authority, reopen History, or bypass existing sealed gates, live permits, fingerprints, blocker decisions, or once-per-run failure delivery. A new operation after cancellation SHALL require the exact cancelled predecessor and explicit new Close intent. Direct HTTP behavior SHALL remain unchanged, and Close SHALL NOT bypass Stop/pause admission, mutate branches or PRs, or create recovery artifacts.

**Rationale:** One retained operation identity makes repeated native calls safe without turning observation or metadata into fresh destructive authority. Every retry remains an explicit user decision bounded by original authority and fresh exact evidence.

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
