# Durable Workflows

## User Story

As a Phoenix user, I need accepted asynchronous work to survive process failure,
retry, cancellation, continuation, and restart without losing delivery or blindly
repeating an ambiguous external action. I need Phoenix to acknowledge only work it
can durably account for, while the conversation reducer remains the sole authority
for user-visible meaning.

## Scope

This specification defines the shared durable-workflow engine plus the normative
wake and conversation-creation profiles. The engine owns crash-spanning execution
truth for one Phoenix API-server process with one bundled SQLite database and one
scheduler authority. Profiles own typed domain meaning, external adapters,
reducer-event mapping, and user projections.

## Requirements

### REQ-DWF-001: Singular Product-Semantic Authority

WHEN a product event, typed effect receipt, cancellation, or manual resolution
requires a product-state decision
THE SYSTEM SHALL apply it through the same product reducer that governs the
corresponding synchronous conversation behavior

THE workflow engine SHALL NOT independently decide user-visible success, failure,
readiness, cancellation meaning, conversation state, or subsequent product intent.

### REQ-DWF-002: Engine Execution Truth

THE engine SHALL be authoritative for workflow identity, profile kind and version,
workflow version, generation, effect dependencies and eligibility, attempts,
optional reclaimable leases, deadlines, evidence, receipts, canonical delivery,
and execution scheduling.

A profile adapter or effect handler SHALL NOT maintain a competing execution
status, delivery lifecycle, or scheduling authority.

### REQ-DWF-003: Normalized Core and Typed Profile Ownership

THE engine SHALL persist queryable execution and authority facts as normalized
columns and child rows, including workflow snapshots, transitions, effects,
dependencies, attempts, evidence, receipts, delivery items, and schedule state.

A registered profile SHALL own typed domain state and event codecs, typed intent,
observation, and receipt families, external adapters, resource locks,
compensation meaning, reducer-event mapping, and user projections.

THE core and a profile SHALL NOT persist overlapping authoritative
representations of the same semantic value. A polymorphic payload MAY be an
indivisible versioned aggregate only when it is read and written whole and is
never queried field-wise.

### REQ-DWF-004: Atomic Versioned Transition Plan

WHEN the reducer accepts an event at an expected workflow version
THE SYSTEM SHALL atomically commit exactly one next snapshot and workflow version,
one append-only transition record, the complete typed effect DAG and dependency
edges declared by that transition, all schedule and delivery facts declared by
that transition, and any effects invalidated by the transition.

IF the expected workflow version is not current
THE SYSTEM SHALL commit none of that transition plan and SHALL return a typed
version-conflict outcome.

A committed workflow version SHALL correspond to exactly one transition record,
and every effect, schedule, and delivery item declared by it SHALL exist in that
same commit.

### REQ-DWF-005: Typed Effect DAG and Barrier Semantics

EVERY effect SHALL have a stable identity, typed family and kind, codec version,
generation, exactly one execution capability class, and exactly one role of
required, optional, or compensation.

THE SYSTEM SHALL reject cyclic effect dependencies. An effect SHALL become
eligible only after every declared dependency has a compatible terminal receipt.
Independent eligible effects MAY execute concurrently.

A completion barrier SHALL be satisfied only by receipts from the current workflow
generation, for the same declared effect, and in the receipt family declared by the
profile for all and only its normalized required-member rows. Compensation receipts
SHALL satisfy only compensation barriers and SHALL NOT substitute for required
work. Optional effects SHALL NOT delay required completion and MAY continue
afterward. Product completion SHALL occur only when the reducer accepts the
barrier-derived event; barrier satisfaction alone is execution truth, not product
meaning.

### REQ-DWF-006: Attempt Authority and Reclaimable Leasing

WHEN the scheduler begins an eligible effect step
THE SYSTEM SHALL issue universal attempt authority containing the workflow
identity, declared workflow version, current generation, effect identity, attempt
identity, and process incarnation.

IF an effect step belongs to a reclaimable phase
THE SYSTEM SHALL additionally issue finite lease authority for that attempt.

EVERY observation commit, receipt commit, retry decision, compensation step, and
other execution progress SHALL require matching live attempt authority. Lease
renewal SHALL extend only the same live lease. Generation change, attempt
replacement, or process-incarnation mismatch SHALL invalidate stale authority.
Lease expiry SHALL invalidate reclaimable local authority only; it SHALL NOT by
itself prove that an external action stopped or failed.

A stale or late result SHALL be retained only as non-authoritative diagnostic
evidence when useful and SHALL NOT mutate current execution or product state.

### REQ-DWF-007: Attempts, Evidence, Receipts, and Delivery

THE SYSTEM SHALL append an immutable attempt before or as an execution begins,
append immutable typed evidence for external facts, persist at most one accepted
terminal typed receipt for an effect, and atomically commit that receipt with
exactly one canonical delivery item when reducer or runtime delivery is owed.

Evidence SHALL describe external facts without asserting product success. A
receipt SHALL describe the engine's accepted terminal execution outcome and MAY
be produced by execution, adoption, reconciliation, schedule collapse,
cancellation arbitration, or manual resolution. Only a typed receipt event
accepted by the product reducer SHALL acquire product meaning.

### REQ-DWF-008: Exactly One Recovery Policy per Effect Family

EVERY registered effect family SHALL declare exactly one execution capability
class and exactly one matching recovery policy: idempotency-keyed submission,
externally observable reconciliation, safe repeatability, or manual resolution.

THE SYSTEM SHALL reject registration or execution of an effect family with no
policy or multiple policies. The engine SHALL NOT claim universal exactly-once
external execution.

After an acknowledgement may have been lost, idempotency-keyed submission SHALL
reuse the stable external key; externally observable work SHALL inspect before
replay; safe repeatability SHALL permit another attempt only under the declared
semantic guarantee; and manual resolution SHALL prohibit automatic replay.

### REQ-DWF-009: Reconciliation Decisions

WHEN external outcome is absent, complete, partial, conflicting, or unknown
THE profile SHALL produce a typed decision to perform, adopt, repair or
compensate, report durable conflict, request manual resolution, retry an
infrastructure failure, or stop because authority was lost.

Destructive external work SHALL additionally hold the profile's physical
resource lock for the affected resource. Database authority alone SHALL NOT
serialize an external system that has a wider mutation boundary.

### REQ-DWF-010: Durable Eligibility and Optimization-Only Kicks

THE SYSTEM SHALL durably store every retry deadline, reclaimable lease expiry,
next schedule eligibility, and other time at which an effect or schedule can next
become eligible, and SHALL schedule from those durable times.

A commit MAY emit a kick to reduce latency, but missed, duplicated, or reordered
kicks SHALL NOT affect correctness or eventual discovery. After restart, rows and
durable time alone SHALL be sufficient to recover all owed work without a hot
poll loop.

### REQ-DWF-011: Atomic Cancellation and Compensation

WHEN the product reducer accepts cancellation or deletion
THE SYSTEM SHALL atomically advance workflow generation, commit the reducer's
visible next state, revoke all prior-generation live authority, invalidate
incompatible pending effects, and append the complete typed compensation DAG and
its barriers.

Compensation effects SHALL use the same dependency, attempt, evidence, receipt,
recovery-policy, deadline, and reclaimable-lease contracts as other effects. An
old-generation external success MAY be observed and adopted by compensation but
SHALL NOT directly mutate current workflow state.

### REQ-DWF-012: Manual Resolution

WHEN an effect is irreversibly ambiguous, conflicts with a foreign resource, or
has a manual-resolution recovery policy
THE SYSTEM SHALL persist a durable manual-resolution requirement, stop automatic
execution of the affected dependency path, expose the evidence and permitted
choices, and retain restart-safe ownership until an authorized choice is committed.

A manual choice SHALL become a typed receipt or reducer event under workflow-version
CAS and SHALL remain auditable. It SHALL NOT bypass generation, barrier, or
resource-ownership rules.

### REQ-DWF-013: Derived Capabilities

THE profile reducer SHALL exhaustively derive user and runtime capabilities from
its typed product state and engine execution projection, including compose,
runtime start, cancel, retry, delete, lifecycle transition, and manual resolution
where applicable.

Capabilities SHALL NOT be persisted as a second semantic status. Every API and
runtime action SHALL enforce the same positive capability projection exposed to
the UI.

### REQ-DWF-014: Profile Kind, Version, and Migration Contract

EVERY workflow SHALL record its profile kind and profile version. Every typed
snapshot, event, intent, evidence, and receipt family SHALL carry a codec or
schema version whose decoder either accepts that version losslessly or rejects it
before execution.

A software upgrade SHALL migrate persisted intent and evidence into the current
profile version, or SHALL move incompatible active work into an explicit
reconciliation or manual-resolution state that preserves auditability and owed
user-visible handling. The steady-state engine SHALL NOT require permanent
protocol selectors, executor registries, shadow workflows, rollback selectors, or
exact-drain machinery as part of normal operation.

### REQ-DWF-017: Runtime Acceptance Boundary

WHEN a canonical delivery item requires later entry into a separately scheduled
product runtime
THE SYSTEM SHALL durably represent that acceptance is owed until the same
transaction that persists the runtime's accepting product state marks the exact
canonical delivery item accepted or suppressed by a reducer-authorized terminal
action.

Profiles that do not enter a separately scheduled runtime SHALL omit this
capability rather than simulate an acceptance record.

### REQ-DWF-018: Deterministic Verification

WHEN the engine or a profile is verified
THE SYSTEM SHALL exercise deterministic virtual-time schedules containing
concurrent transition writers, duplicate kicks, competing local execution,
reclaimable lease renewal and expiry, stale results, ambiguous external outcomes,
retry deadlines, cancellation at every step, compensation failure, restart,
manual resolution, and codec rejection.

THE verification SHALL check authority, atomicity, dependency, receipt, delivery,
barrier, and single-authority invariants after every operation and retain
minimized counterexamples as regressions.

### REQ-DWF-033: One Scheduler Authority per SQLite Database

THE SYSTEM SHALL operate with exactly one Phoenix scheduler authority for each
SQLite database. That authority SHALL be the only component permitted to claim or
advance engine work from that database.

Remote executors, clients, and external systems MAY supply evidence or durable
receipts through typed profile adapters, but they SHALL NOT independently claim
scheduler authority over Phoenix workflow rows.

### REQ-DWF-034: Durable Acknowledgement Boundary

WHEN Phoenix durably acknowledges accepted intent while still owing independently
recoverable execution, observation, compensation, reducer delivery, runtime
acceptance, or client-visible completion handling
THE SYSTEM SHALL represent that obligation as durable workflow state before the
acknowledgement becomes visible.

Work that completes as one synchronous local transaction with no owed crash-spanning
obligation SHALL remain outside the workflow engine.

### REQ-DWF-035: Canonical Durable Delivery

WHEN a receipt or barrier-derived reducer event must be delivered
THE SYSTEM SHALL represent that obligation through exactly one canonical durable
delivery lifecycle owned by the engine core.

Profiles MAY persist non-overlapping typed payload detail for presentation or
adapter needs, but they SHALL NOT maintain a second authoritative inbox,
outbox, obligation, or acceptance lifecycle for the same semantic delivery.
Duplicate, stale, or restarted delivery of an already accepted canonical item
SHALL NOT begin another product action.

### REQ-DWF-036: Submit-Then-Observe Remote Work

WHEN a profile models long-running remote work whose execution continues outside
the Phoenix process
THE SYSTEM SHALL represent that work as durable submission under a stable command
identity, durable receipt of the remote handle or submit acknowledgement when
available, reclaimable observation under the declared recovery policy, and a
terminal typed receipt.

A disconnect or crash before submit acknowledgement SHALL trigger inspection by
stable command identity or returned handle where supported. IF the remote system
cannot answer authoritatively whether submission happened
THE SYSTEM SHALL enter explicit ambiguity or manual resolution rather than blindly
submitting again.

### REQ-DWF-037: Execution Capability Classes

EVERY effect family SHALL declare one of these structural execution capability
classes:

- `ReclaimableObservation` for work whose in-progress observation may be safely
taken over after lease expiry;
- `IdempotentSubmission` for submission that may be retried only under the same
stable external key;
- `ObservableSubmission` for work that must be inspected or reconciled before any
repeated external action;
- `SafelyRepeatable` for work whose semantics explicitly permit another attempt;
- `ManualOnAmbiguity` for work that must stop automatic retry when execution is
uncertain.

Takeover, retry, and ambiguity handling SHALL be constrained by that declared
class.

### REQ-DWF-038: Direct Turns Use Durable Client Message Identity

WHEN Phoenix accepts a direct turn whose outcome may span a crash boundary before
terminal user-visible completion
THE SYSTEM SHALL durably bind the resolved target and client message identity to
exactly one accepted workflow or equivalent durable turn obligation before
returning accepted.

The durable binding SHALL be keyed by the same resolved target identity Phoenix
uses for runtime delivery and transcript reconciliation, not by incidental request
metadata or a later-derived projection. A replay of the same direct-turn request
under the same resolved target and client message identity SHALL return the same
durable acceptance result or terminal outcome. A conflicting replay under that
same resolved target and client message identity SHALL be rejected rather than
start another direct turn.

### Direct-Chat Profile

### REQ-DWF-CHAT-001: Resolved Target-Bound Message Identity

WHEN Phoenix accepts or replays a direct-chat turn
THE direct-chat profile SHALL resolve the target conversation or successor before
durable acceptance and SHALL bind the user message identity to that resolved
runtime target.

The same client-supplied message identifier reused against a different resolved
target SHALL be a different identity domain. Within one resolved target, the
profile SHALL treat that message identifier as a stable exact identifier for
acceptance replay, runtime reconciliation, and transcript correlation.

### REQ-DWF-CHAT-002: Prepared Immutable Payload Before Durable Acceptance

WHEN a direct-chat turn crosses the durable acknowledgement boundary
THE direct-chat profile SHALL first prepare an immutable accepted payload and a
stable fingerprint derived from the exact user-visible message content, resolved
target, and other semantics that affect runtime behavior.

Preparation SHALL resolve inline file references and skill invocation content
into the immutable accepted payload before durable acceptance. After durable
acceptance, later mutation of caller-owned input buffers, request objects,
transcript windows, file contents, skill definitions, or adapter-local
presentation state SHALL NOT change the accepted payload, fingerprint, or
conflict decision for that accepted turn.


### REQ-DWF-CHAT-003: Typed Direct-Turn Disposition

EVERY durable direct-chat acceptance lookup, replay, reconciliation, or conflict
resolution SHALL produce one typed disposition.

The disposition family SHALL distinguish at least RetryableUnaccepted,
QueuedSteering, RuntimeAccepted, ReplayQueuedSteering,
ReplayRuntimeAccepted, ConflictingKeyReuse, and TerminalRejected. The profile
SHALL NOT encode these cases as ambiguous booleans or by omitting fields whose
absence could mean either replay, steering, conflict, runtime acceptance,
conflict, or rejection.

### REQ-DWF-CHAT-004: Same-Key Convergence and Conflict

WHEN two submissions present the same resolved target and client message identity
THE direct-chat profile SHALL compare the prepared immutable fingerprint before a
second durable acceptance is permitted.

Equal fingerprints SHALL converge to the same accepted durable turn identity and
typed replay disposition. Different fingerprints under that same key SHALL produce
a durable same-key conflict disposition and SHALL NOT start another turn.

### REQ-DWF-CHAT-005: Different-Key Direct-Turn Concurrency

WHEN two direct-chat submissions target the same resolved conversation but carry
different client message identities
THE SYSTEM MAY accept them concurrently as distinct durable turn obligations.

Their concurrency SHALL be serialized only by the normal runtime and reducer
contracts for that target conversation. Different message identities racing on
one resolved target SHALL NOT both become durably RuntimeAccepted for the same
runtime-admission opportunity; reducer or runtime arbitration MAY leave one as a
distinct accepted turn whose committed outcome is durably QueuedSteering.
Acceptance replay or conflict for one key SHALL NOT consume, suppress, or
redefine acceptance for a different key.

### REQ-DWF-CHAT-006: Durable Exact Runtime Acceptance

WHEN a direct-chat turn progresses from durable acceptance into runtime work
THE direct-chat profile SHALL durably record exact runtime acceptance of that same
accepted turn identity.

Runtime start, retry, wakeup, or reducer delivery SHALL reconcile against that
exact accepted turn identity rather than an approximate transcript position,
window-relative offset, or best-effort latest message guess.

### REQ-DWF-CHAT-007: Secondary Effects Cannot Reverse Acceptance

WHEN follow-on reducer delivery, runtime startup, wake observation, notification,
or any other secondary effect fails, retries, races, or is suppressed
THE accepted direct-chat turn SHALL remain accepted.

Secondary effects MAY add typed owed work, typed failure, or typed suppression,
but they SHALL NOT retroactively revoke, remap, or replace the already accepted
turn identity or transform accepted into never-accepted.

### REQ-DWF-CHAT-008: Exact-ID Reconciliation Independent of Transcript Window

WHEN Phoenix reconciles a durable direct-chat turn with runtime or transcript
state after crash, restart, pagination, continuation, or transcript compaction
THE SYSTEM SHALL use the exact accepted turn identity rather than depend on the
message still appearing inside an arbitrary transcript window.

A missing message in the currently loaded transcript slice SHALL trigger lookup,
replay, steering, or typed reconciliation by exact identifier rather than a new
acceptance or a window-relative guess.

### REQ-DWF-CHAT-009: Independent Non-Atomic Fan-Out

WHEN a direct-chat request resolves to multiple target conversations
THE SYSTEM SHALL treat that coordinator fan-out as one independent ordinary
acceptance attempt per resolved target rather than one atomic multi-target
acceptance.

Failure, retry, duplication, or restart for one resolved target SHALL NOT imply
that another resolved target was unaccepted, unobserved, or must be replayed
under a new turn identity. This rule is about multi-target direct-chat fan-out;
generic additional durable consumers of one already accepted item remain covered
by REQ-DWF-041.

### REQ-DWF-CHAT-010: Capability Isolation

THE direct-chat profile SHALL derive acceptance, replay, steering, conflict,
rejection, runtime-start, and secondary-consumer capabilities from typed durable
state for that exact accepted turn.

Target resolution SHALL be an adapter that maps the request onto an ordinary
conversation target or successor without broadening the caller's ordinary
conversation permissions. A notification, indexing, or audit consumer SHALL NOT
gain authority to redefine direct acceptance, and transcript presentation state
SHALL NOT gain authority to start or reject runtime work.

### REQ-DWF-CHAT-011: Crash, Race, and Mutable-Input Verification

WHEN the direct-chat profile is verified
THE SYSTEM SHALL exercise deterministic cases covering crash after observation,
crash after durable acceptance, crash before and after runtime acceptance,
concurrent same-key submissions with equal and unequal fingerprints,
different-key concurrent submissions, steering conversion and replay, mutable
caller input after acceptance, mutable file contents or skill definitions after
acceptance, restart, runtime acceptance racing transcript visibility, and
exact-ID reconciliation when the accepted message lies outside the loaded
transcript window.

The verification SHALL prove that acceptance identity, typed disposition,
fingerprint comparison, runtime reconciliation, target-local runtime arbitration,
and independent multi-target fan-out remain stable under those schedules.

### REQ-DWF-039: Typed Profile Migrations Prevent Silent Loss

WHEN persisted work from an earlier profile version is encountered after upgrade
THE SYSTEM SHALL either migrate it losslessly into the current typed profile
shape, or preserve it in an explicit durable state that makes incomplete
migration, reconciliation need, or manual action visible.

The system SHALL NOT silently discard active persisted work, owed delivery,
acceptance bindings, or evidence because a prior version is no longer executable.

### REQ-DWF-040: Scheduled Loops Use Explicit CoalesceLatest Policy

WHEN a profile models recurring coordinator or observer work whose product meaning
is latest-state rather than every-missed-occurrence replay
THE SYSTEM SHALL represent that schedule with an explicit `CoalesceLatest`
policy.

Under `CoalesceLatest`, at most one active occurrence SHALL exist for a schedule,
downtime or repeated kicks SHALL coalesce into one due occurrence, and the next
occurrence SHALL be computed from current durable state after the active
occurrence reaches a terminal disposition.

### REQ-DWF-041: Additional Durable Consumers Are Independent Views

WHEN a canonical delivery item is offered to an additional durable consumer such
as notification, indexing, or audit export
THE SYSTEM SHALL give that consumer an independent durable cursor or disposition
referencing the canonical item without mutating, consuming, or duplicating the
canonical reducer-delivery or runtime-acceptance state.

A consumer retry or failure SHALL NOT block or duplicate reducer consumption,
runtime acceptance, or another consumer's progress.

### REQ-DWF-042: Migration Safety Without Permanent Authority

WHEN a specific migration uses temporary comparison, shadow execution, or drain
inventory to reduce rollout risk
THE SYSTEM SHALL scope that machinery to the migration, prove that steady-state
authority returns to the single canonical scheduler after the migration, and
forbid such machinery from becoming a permanent semantic authority.

Migration-local safety checks MAY compare outcomes or inventory owed work, but
they SHALL NOT redefine the durable engine contract for normal operation.

### REQ-DWF-032: Durable-Workflow Adoption Boundary

WHEN accepted intent can create externally observable or crash-spanning work that
requires ambiguity handling, recovery-policy enforcement, cancellation or
compensation arbitration, durable deadlines, canonical delivery, or durable
acceptance replay
THE SYSTEM SHALL model that work as a durable-workflow profile or extend an
existing profile.

Implementation effort alone SHALL NOT justify routing such work around the shared
engine.

## Wake Profile

### REQ-DWF-WAKE-001: Registration Mapping

WHEN an authorized agent registers a bounded bash, Phoenix-managed tmux, or
sub-agent terminal wait
THE wake profile SHALL atomically create the workflow, typed observation effect,
and durable registration binding before returning the provider-valid registration
receipt. The complete tool round SHALL persist normally, and the later terminal
outcome SHALL be a runtime observation correlated by contract identity, never a
delayed tool result.

Registration SHALL preserve the timeout range, WorkScope authorization, and
receipt shape required by `specs/wake-contracts/requirements.md`. It SHALL
reject registration with a typed rejection when the requested timeout is out of
range, when the conversation is archived or terminal, or when the shared
registration or lifecycle fence is closed.

### REQ-DWF-WAKE-002: Terminal Evidence and Deadline Precedence

The wake observation effect SHALL use `ReclaimableObservation`. For bash it SHALL
map terminal handle evidence or unknowable-after-restart evidence; for tmux it
SHALL map durable exit-marker, killed-window, and final-tail evidence by stable
window identity; for sub-agents it SHALL map the durable child terminal record.

Durable terminal evidence with occurrence time at or before `expires_at` SHALL
win over later scheduler observation. In the absence of such evidence, an
evaluable handle at or after the deadline SHALL map to `Expired`, and an
unevaluable handle SHALL map to `Forgotten` using the contract-compatible
forgotten reasons refined by `specs/wake-contracts/requirements.md`. Startup
reconciliation SHALL forget an unrecoverable non-terminal wake before normal
serving resumes rather than waiting for the original deadline. Each accepted
contract SHALL yield exactly one terminal receipt.

### REQ-DWF-WAKE-003: Canonical Delivery and Runtime Acceptance

A wake terminal receipt SHALL enter the existing product reducer, which SHALL
materialize exactly one canonical durable delivery item per accepted receipt.
Replayed, duplicated, or restarted delivery of the same accepted receipt SHALL
reuse that canonical item rather than append another one.

Auto-resuming observations for an idle conversation SHALL be coalesced in
committed order into one runtime-acceptance obligation over canonical delivery
items. While that obligation remains owed, duplicate kicks or restart recovery
SHALL preserve the same owed acceptance rather than creating another one. Busy
arrivals SHALL remain owed without overlapping LLM requests.

Runtime acceptance SHALL atomically persist the accepting `LlmRequesting`
product state and accept or suppress the exact canonical items carried by that
obligation before invoking the LLM. Items committed after the accepted snapshot
upper bound SHALL remain for a later batch.

### REQ-DWF-WAKE-004: Wake Cancellation and Lifecycle

Explicit wake cancellation SHALL use the generation-bump transition and produce a
`Cancelled` observation without scheduling an LLM turn solely for that
cancellation. It SHALL NOT terminate the watched process unless separately
requested through its owning profile.

Pending wake obligations SHALL derive lifecycle blocking without redefining an
idle conversation as runtime-busy. Destructive lifecycle actions SHALL conflict
until pending waits are explicitly resolved, with the guard serialized against
new registration.

### REQ-DWF-WAKE-005: Continuation Transfer

WHEN a conversation continues
THE wake profile SHALL atomically transfer pending contracts, unconsumed
canonical delivery items, and unaccepted runtime obligations to exactly one
successor while preserving contract identity, watched resource identity,
registration WorkScope, and deadline.

A transferred materialized observation SHALL exist in successor history under a
deterministic successor-safe identity without altering predecessor history or
duplicating semantic delivery.

## Conversation-Creation Profile

### REQ-DWF-CREATE-001: Shell-First Acceptance

WHEN a structurally valid creation request is accepted
THE creation profile SHALL atomically persist the user-visible conversation
shell, creation intent, initial workflow transition, effect DAG, barriers, and
acceptance binding before filesystem, Git, attachment, runtime, or provider
effects begin.

Creation acceptance SHALL declare externally retryable acceptance. Its
client-supplied idempotency key SHALL therefore be load-bearing under
REQ-DWF-029 and REQ-DWF-038 rather than incidental request metadata.

### REQ-DWF-CREATE-002: Creation Effect Mapping

The creation profile SHALL represent repository resolution, resource reservation,
worktree materialization or reconciliation, attachment finalization, metadata
commit, initial-message expansion, runtime bootstrap, and initial LLM dispatch as
typed effects with explicit dependencies and recovery policies.

Seeded-empty creation SHALL declare a typed required barrier that omits message
expansion and dispatch rather than fabricating those effects. Analytics and
notifications, when present, SHALL be optional.

### REQ-DWF-CREATE-003: Resource Reconciliation and Completion

Worktree effects SHALL reserve normalized ownership, hold the canonical
repository mutation lock for destructive work, and inspect before replay. Owned
complete resources MAY be adopted; owned partial resources MAY be repaired or
compensated; foreign or conflicting resources SHALL enter durable conflict or
manual resolution and SHALL NOT be removed.

Initial-turn creation SHALL reach execution completion only when its required
barrier includes a compatible durable dispatch receipt. Seeded-empty creation
SHALL complete only when its own required metadata and state barrier is
satisfied. The product reducer alone SHALL publish `Ready` or failure.

### REQ-DWF-CREATE-004: Retry, Cancellation, and Deletion Mapping

The creation profile SHALL preserve its bounded transient retry policy and
permanent-failure classification as durable effect deadlines and reducer events.
Attempt loss or reclaimable lease loss SHALL NOT itself become user-visible
failure.

Cancellation SHALL immediately preserve a visible cancelled shell and creation
intent while atomically declaring compensation for runtime revocation, owned
worktree removal, reservation release, and staged-attachment cleanup as
applicable. Deletion SHALL hide the shell immediately and retain a durable
tombstone until the required compensation barrier is accepted by the reducer.

Reducer publication from stale generation-bound creation effects or stale
lifecycle completion evaluations SHALL NOT overwrite a later cancel, delete, or
deleted outcome.

### REQ-DWF-CREATE-005: Creation Capabilities

The creation reducer SHALL derive provisioning as read-only with Cancel and
Delete, failed and cancelled as read-only with Start over and Delete, and ready or
idle capabilities according to normal conversation behavior. Provisioning,
failed, cancelled, or deletion-pending states SHALL NOT start a runtime merely
because a caller omitted a state guard.

### Deprecated Requirements

The following immutable identifiers remain reserved for historical traceability but
are superseded by the requirements above and SHALL NOT be used as current design
authority:

- REQ-DWF-015 Deprecated — selector-based one-scheduler authority semantics superseded by REQ-DWF-033
- REQ-DWF-016 Deprecated — permanent parallel-authority migration semantics superseded by REQ-DWF-034
- REQ-DWF-019 Deprecated — divergence-classification semantics removed from the current contract
- REQ-DWF-020 Deprecated — evidence-based authority-cutover semantics removed from the current contract
- REQ-DWF-021 Deprecated — mixed-authority semantic-parity semantics removed from the current contract
- REQ-DWF-022 Deprecated — shadow/drain client-boundary semantics superseded by REQ-DWF-029 through REQ-DWF-042

Current normative authority is REQ-DWF-017, REQ-DWF-029 through REQ-DWF-042,
REQ-DWF-WAKE-001 through REQ-DWF-WAKE-005, and REQ-DWF-CREATE-001 through
REQ-DWF-CREATE-005 as defined in this document.
