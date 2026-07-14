# Durable Workflows

## User Story

As a Phoenix user, I need accepted asynchronous work to survive process failure,
retry, takeover, cancellation, and restart without losing delivery or blindly
repeating an ambiguous external action. I need product behavior to remain
consistent with the conversation reducer while a shared engine provides durable
execution, recovery, and audit truth.

## Scope

This specification defines a general durable workflow engine and two normative
profiles: terminal-handle wake and conversation creation. The engine is not a
second product state machine. It executes durable plans emitted by the same
product reducer that owns conversation meaning. Profile-specific requirements
refine this contract together with `specs/wake-contracts/requirements.md` and
`specs/conversation-creation/requirements.md`.

## Requirements

### REQ-DWF-001: Singular Product-Semantic Authority

WHEN a product event, typed effect receipt, cancellation, or manual resolution
requires a product-state decision
THE SYSTEM SHALL apply it through the same product reducer that governs the
corresponding synchronous conversation behavior

THE workflow engine SHALL NOT independently decide user-visible success, failure,
readiness, cancellation meaning, conversation state, or subsequent product intent.

### REQ-DWF-002: Engine Execution Truth

THE engine SHALL be authoritative for workflow identity, profile and codec
versions, workflow version, generation, effect dependencies and eligibility,
claims, leases, deadlines, attempts, observations, receipts, completion barriers,
and execution scheduling.

A profile adapter or effect handler SHALL NOT maintain a competing execution
status or scheduling authority.

### REQ-DWF-003: Normalized Core and Typed Profile Ownership

THE engine SHALL persist queryable execution and authority facts as normalized
columns and child rows, including workflow snapshots, transitions, effects,
dependencies, claims, attempts, observations, receipts, and barrier membership.

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
edges declared by that transition, all completion barriers and memberships, and
any effects invalidated by the transition.

IF the expected workflow version is not current
THE SYSTEM SHALL commit none of that transition plan and SHALL return a typed
version-conflict outcome.

A committed workflow version SHALL correspond to exactly one transition record,
and every effect and barrier declared by it SHALL exist in that same commit.

### REQ-DWF-005: Typed Effect DAG and Barrier Semantics

EVERY effect SHALL have a stable identity, typed family and kind, codec version,
generation, and exactly one role of required, optional, or compensation.

THE SYSTEM SHALL reject cyclic effect dependencies. An effect SHALL become
eligible only after every declared dependency has a compatible terminal receipt.
Independent eligible effects MAY execute concurrently.

A completion barrier SHALL be satisfied only by receipts from the current workflow
generation, for the same declared effect, and in the receipt family declared by the
profile for all and only its normalized required-member rows. Compensation receipts
SHALL satisfy only compensation barriers and SHALL NOT substitute for required work. Optional effects SHALL NOT delay required
completion and MAY continue afterward. Product completion SHALL occur only when
the reducer accepts the barrier-derived event; barrier satisfaction alone is
execution truth, not product meaning.

### REQ-DWF-006: Leased Authority for Every Claimed External Step

WHEN a worker claims an eligible external-effect step
THE SYSTEM SHALL issue authority containing the workflow identity, workflow
version at declaration, current generation, effect identity, opaque claim token,
worker identity, and finite lease.

EVERY external inspection, execution, retry decision, observation commit, receipt
commit, and compensation step performed as claimed work SHALL require live
matching authority. Renewal SHALL extend only the same live authority. Expiry or
generation change SHALL permit takeover and SHALL invalidate stale authority.

A stale or late result SHALL be retained only as non-authoritative diagnostic
evidence when useful and SHALL NOT mutate current execution or product state.

### REQ-DWF-007: Attempts, Observations, and Receipts

THE SYSTEM SHALL append an immutable attempt before or as a claimed execution
begins, append immutable typed observations for external facts, and persist at
most one accepted terminal typed receipt for an effect. The receipt and exactly one
normalized reducer-delivery artifact SHALL commit atomically.

An observation SHALL describe evidence without asserting product success. A
receipt SHALL describe the engine's accepted terminal execution outcome and MAY
be produced by execution, adoption, reconciliation, or manual resolution. Only a
typed receipt event accepted by the product reducer SHALL acquire product meaning.

### REQ-DWF-008: Exactly One Ambiguity Policy per Effect Family

EVERY registered effect family SHALL declare exactly one ambiguity policy:
observable reconciliation, externally enforced idempotency, safe repeatability,
or manual resolution.

THE SYSTEM SHALL reject registration or execution of an effect family with no
policy or multiple policies. The engine SHALL NOT claim universal exactly-once
external execution.

After an acknowledgement may have been lost, observable reconciliation SHALL
inspect before replay; external idempotency SHALL use the stable external key;
safe repeatability SHALL permit another attempt only under the declared semantic
guarantee; and manual resolution SHALL prohibit automatic replay.

### REQ-DWF-009: Reconciliation Decisions

WHEN external outcome is absent, complete, partial, conflicting, or unknown
THE profile SHALL produce a typed decision to perform, adopt, repair or
compensate, report durable conflict, request manual resolution, retry an
infrastructure failure, or stop because authority was lost.

Destructive external work SHALL additionally hold the profile's physical
resource lock for the affected resource. Database authority alone SHALL NOT
serialize an external system that has a wider mutation boundary.

### REQ-DWF-010: Durable Deadlines and Optimization-Only Kicks

THE SYSTEM SHALL durably store every retry deadline, claim lease expiry, and
other time at which an effect can next become eligible, and SHALL schedule from
the earliest durable deadline.

A commit MAY emit a kick to reduce latency, but missed, duplicated, or reordered
kicks SHALL NOT affect correctness or eventual discovery. After restart, rows and
durable time alone SHALL be sufficient to recover all owed work without a hot
poll loop.

### REQ-DWF-011: Atomic Cancellation and Compensation

WHEN the product reducer accepts cancellation or deletion
THE SYSTEM SHALL atomically advance workflow generation, commit the reducer's
visible next state, revoke all prior-generation claims, invalidate incompatible
pending effects, and append the complete typed compensation DAG and its barriers.

Compensation effects SHALL use the same dependency, lease, attempt, observation,
receipt, ambiguity, deadline, and takeover contracts as other effects. An
old-generation external success MAY be observed and adopted by compensation but
SHALL NOT directly mutate current workflow state.

### REQ-DWF-012: Manual Resolution

WHEN an effect is irreversibly ambiguous, conflicts with a foreign resource, or
has a manual-resolution ambiguity policy
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

### REQ-DWF-014: Protocol and Codec Versioning

EVERY workflow SHALL record its profile identifier and profile protocol version.
Every typed event, effect intent, observation, and receipt family SHALL carry a
codec version whose decoder either accepts that version losslessly or rejects it
without execution.

An accepted workflow SHALL retain its protocol and semantic authority for its
lifetime. A software upgrade SHALL keep the executor and decoders required to
drain accepted versions or SHALL stop before accepting work it cannot safely
resume.

### REQ-DWF-015: Singular Authority and Shadow Safety

EVERY accepted workflow SHALL designate exactly one semantic authority: a legacy
protocol or an engine protocol. A shadow workflow SHALL be explicitly
non-authoritative and structurally unable to claim or execute external effects,
invoke the product reducer authoritatively, or publish user-visible state.

Shadow processing MAY mirror authoritative observations and receipts and record
typed semantic divergences, but SHALL NOT duplicate authoritative semantic values
as an alternative source of truth.

### REQ-DWF-016: Migration, Rollback, and Drain

WHEN a profile changes execution protocols
THE SYSTEM SHALL select the protocol and authority exactly once for each new
acceptance. It SHALL NOT reinterpret or translate an in-flight workflow to another
authority model.

Rollback SHALL alter selection only for future acceptances. Every accepted
protocol's executor SHALL remain available until durable evidence proves that no
nonterminal or owed-acceptance work remains under it. Legacy scheduling and
storage SHALL be retired only after that zero-authority drain is proven.

### REQ-DWF-017: Runtime Acceptance Boundary

WHEN a receipt requires later entry into a product runtime
THE SYSTEM SHALL durably represent that acceptance is owed until the same
transaction that persists the runtime's accepting product state marks the exact
obligation accepted or suppressed by a reducer-authorized terminal action.

Core MAY provide normalized owed-acceptance records for profiles that need this
boundary. Profiles that do not enter a separately scheduled runtime SHALL omit
that capability rather than simulate an acceptance record.

Duplicate, stale, or restarted delivery of an already accepted obligation SHALL
NOT begin another product action.

### REQ-DWF-018: Deterministic Verification

WHEN the engine or a profile is verified
THE SYSTEM SHALL exercise deterministic virtual-time schedules containing
concurrent transition writers, duplicate kicks, competing claims, lease renewal
and expiry, takeover, stale results, ambiguous external outcomes, retry deadlines,
cancellation at every step, compensation failure, restart, manual resolution,
and codec rejection.

THE verification SHALL check authority, atomicity, dependency, receipt, barrier,
and singular-authority invariants after every operation and retain minimized
counterexamples as regressions.

### REQ-DWF-019: Protocol Admission and Drain Proof

WHEN a profile protocol is registered
THE SYSTEM SHALL durably register its profile, protocol version, semantic authority,
executor, lossless codecs, runtime-acceptance capability, and acceptance selector
before that protocol can accept work.

THE selector SHALL designate exactly one active protocol for each new authoritative
acceptance. Rollback SHALL switch only future acceptance and SHALL permit legacy and
engine protocols to coexist while each accepted workflow retains its original
semantic authority. Draining SHALL atomically close new acceptance while retaining
its executor and codecs.

THE drain proof SHALL query, by profile and protocol, exactly these blocking
categories: nonterminal workflows, active or unexpired claims, eligible or retry
effects, uncompensated effects, unresolved manual resolutions, pending reducer
inbox items, owed runtime acceptances, and unresolved blocking divergences. Shadow
work SHALL be excluded as authority. Retirement SHALL require a complete proof with
zero rows in every category and an operator-readable report identifying the
protocol selector, authoritative query identity and version, authority, exact counts,
and blocking identities for every category.

### REQ-DWF-020: Divergence Classification and Operator Action

EVERY shadow divergence SHALL have a typed severity of blocking, actionable, or
informational and a typed required action of halt acceptance, retain authority and
investigate, or record only. Snapshot, transition, effect-plan, observation,
receipt, reducer-event, capability, and user-projection differences that can change user
semantics SHALL be blocking.

THE SYSTEM SHALL expose the authoritative and shadow protocol, compared workflow,
typed divergence kind, severity, codec versions, evidence identity, required action,
and resolution state to operators. A blocking divergence SHALL prevent authority
cutover and SHALL halt new engine acceptance when discovered after cutover until an
operator explicitly selects rollback or reauthorization.

### REQ-DWF-021: Evidence-Based Authority Cutover

WHEN engine authority is selected for new wake registrations or creation requests
THE SYSTEM SHALL require zero unresolved blocking divergences across all required
deterministic fault schedules and representative production schedule classes,
lossless codec and rollback-selector verification, mixed-authority user-semantic
parity, and explicit operator authorization.

Required deterministic classes SHALL include concurrency, duplicate and reordered
kicks, lease expiry and takeover, stale results, restart at every commit boundary,
ambiguous outcomes, cancellation and compensation, reducer redelivery, runtime
acceptance duplication, and codec rejection. Required production classes SHALL
include each supported substrate, profile mode, protocol version, lifecycle path,
and contention class observed by that profile. Elapsed soak time alone SHALL NOT
satisfy the gate.

A new profile SHALL NOT be admitted until its typed codecs, ambiguity policies,
barrier receipt families, runtime-acceptance capability, deterministic schedule
suite, production schedule-class inventory, selector rollback, drain query, and
operator explainability surfaces are registered and verified.

### REQ-DWF-022: Mixed-Authority Semantic Parity

WHILE legacy-authoritative and engine-authoritative workflows coexist
THE SYSTEM SHALL present identical user semantics for equivalent accepted intent,
including visible state, capabilities, cancellation and retry meaning, lifecycle
guards, terminal outcomes, and operator explanations. Protocol identity MAY be
shown diagnostically but SHALL NOT alter product meaning.

### REQ-DWF-029: Externally Retryable Acceptance

WHEN a profile permits acceptance requests to be retried across a client-server
boundary
THE profile SHALL require a client-supplied stable idempotency key, atomically
bind that key under the accepting profile plus client authority scope to exactly
one accepted workflow, and return a typed acceptance receipt that echoes the key
and identifies the durable workflow or product handle the client can observe.

The accepted protocol selection MAY be retained as replay evidence, but selector
roll-forward, rollback, or drain SHALL NOT change the binding key. A retry with
the same profile, authority scope, and idempotency key SHALL return the same
acceptance receipt without creating another workflow or repeating acceptance
effects. A conflicting request that reuses the key for different intent SHALL be
rejected. Profiles without an externally retryable acceptance boundary SHALL omit
this capability rather than fabricate a key.

### REQ-DWF-030: Cross-Client Projection Parity

THE SYSTEM SHALL expose each supported client's user-action capabilities and
user-visible workflow presentation from the same typed product-state and engine
projection enforced by API and runtime actions. Supported clients SHALL NOT
maintain an independent semantic policy for compose, cancel, retry, delete,
resolve, lifecycle transition, or runtime start.

Equivalent accepted intent SHALL produce equivalent visible state,
presentation detail, capabilities, and terminal meaning on every supported
client surface. A conversation holding pending wake obligations SHALL remain
runtime-idle when otherwise idle and SHALL expose those obligations through
presentation detail, capability guards, and lifecycle conflict surfaces rather
than a fabricated busy state.

### REQ-DWF-031: Independent Inbox Consumers

WHEN a durable reducer-inbox observation is offered to an additional delivery
consumer
THE SYSTEM SHALL give that consumer an independent durable disposition or cursor
without mutating, consuming, or serving as a second representation of reducer
delivery or runtime acceptance state.

A consumer retry or failure SHALL NOT block or duplicate reducer consumption,
runtime acceptance, or another consumer's progress. Ephemeral notification
transport SHALL NOT be the authority for whether the durable observation exists
or remains owed to another consumer.

### REQ-DWF-032: Durable-Workflow Adoption Boundary

WHEN accepted intent can create externally observable work that spans a crash
boundary and requires effect ambiguity handling, leased retry or takeover,
cancellation or compensation arbitration, durable deadlines, owed reducer or
runtime delivery, or protocol shadow and cutover
THE SYSTEM SHALL model that work as a durable-workflow profile or extend an
existing profile.

Work that completes as one synchronous local transaction without durable owed
work, external ambiguity, or independent retry authority SHALL remain outside
the workflow engine. A new profile SHALL satisfy protocol admission before it
can accept work; implementation effort alone SHALL NOT justify routing around
the engine.

## Wake Profile

### REQ-DWF-WAKE-001: Registration Mapping

WHEN an authorized agent registers a bounded bash or Phoenix-managed tmux wait
THE wake profile SHALL atomically create the workflow and its typed observation
effect before returning the provider-valid registration receipt. The complete
tool round SHALL persist normally, and the later terminal outcome SHALL be a
runtime observation correlated by contract identity, never a delayed tool result.

Registration SHALL preserve the timeout range, WorkScope authorization, and
receipt shape required by `specs/wake-contracts/requirements.md`. It SHALL
reject registration with a typed rejection when the requested timeout is out of
range, when the conversation is archived or terminal, when the shared
registration/lifecycle fence is closed, or when the wake profile selector is not
accepting new authoritative wake work.

### REQ-DWF-WAKE-002: Terminal Evidence and Deadline Precedence

The wake observation effect SHALL use observable reconciliation. For bash it
SHALL map terminal handle evidence or unknowable-after-restart evidence; for tmux
it SHALL map durable exit-marker, killed-window, and final-tail evidence by stable
window identity.

Durable terminal evidence with occurrence time at or before `expires_at` SHALL
win over later scheduler observation. In the absence of such evidence, an
evaluable handle at or after the deadline SHALL map to `Expired`, and an
unevaluable handle SHALL map to `Forgotten` using the contract-compatible
forgotten reasons refined by `specs/wake-contracts/requirements.md`. Startup
reconciliation SHALL forget an unrecoverable non-terminal wake before normal
serving resumes rather than waiting for the original deadline. Each accepted
contract SHALL yield exactly one terminal receipt.

### REQ-DWF-WAKE-003: Delivery, Coalescing, and Acceptance

A wake terminal receipt SHALL enter the existing product reducer, which SHALL
materialize exactly one durable inbox observation per contract receipt. Replayed,
duplicated, or restarted reducer delivery of the same accepted receipt SHALL
reuse that observation rather than append another one. Auto-resuming observations
for an idle conversation SHALL be coalesced in committed order into a bounded
runtime-acceptance obligation; while that obligation remains owed, duplicate kicks
or restart recovery SHALL preserve the same owed acceptance rather than creating
another one. Busy arrivals SHALL remain owed without overlapping LLM requests.

Runtime acceptance SHALL atomically persist the accepting `LlmRequesting` product
state, consume the exact reducer inbox carried by that obligation, and accept the
exact obligation before invoking the LLM. The owed/accepted lifecycle for that
runtime-start boundary SHALL have one writable authority, using the core durable-
workflows owed-acceptance record rather than a profile-local parallel status.
Items committed after the accepted snapshot SHALL remain for a later batch.

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
observations, and unaccepted runtime obligations to exactly one successor while
preserving contract identity, watched resource identity, registration WorkScope,
and deadline.

A transferred materialized observation SHALL exist in successor history under a
deterministic successor-safe identity without altering predecessor history or
duplicating semantic delivery.

## Conversation-Creation Profile

### REQ-DWF-CREATE-001: Shell-First Acceptance

WHEN a structurally valid creation request is accepted
THE creation profile SHALL atomically persist the user-visible conversation shell,
creation intent, profile protocol authority, initial workflow transition, effect
DAG, and barriers before filesystem, Git, attachment, runtime, or provider effects
begin.

Creation acceptance SHALL declare externally retryable acceptance. Its
client-supplied idempotency key SHALL therefore be load-bearing under
REQ-DWF-029 rather than incidental request metadata.

### REQ-DWF-CREATE-002: Creation Effect Mapping

The creation profile SHALL represent repository resolution, resource reservation,
worktree materialization or reconciliation, attachment finalization, metadata
commit, initial-message expansion, runtime bootstrap, and initial LLM dispatch as
typed effects with explicit dependencies and ambiguity policies.

Seeded-empty creation SHALL declare a typed required barrier that omits message
expansion and dispatch rather than fabricating those effects. Analytics and
notifications, when present, SHALL be optional.

### REQ-DWF-CREATE-003: Resource Reconciliation and Completion

Worktree effects SHALL reserve normalized ownership, hold the canonical repository
mutation lock for destructive work, and inspect before replay. Owned complete
resources MAY be adopted; owned partial resources MAY be repaired or compensated;
foreign or conflicting resources SHALL enter durable conflict or manual resolution
and SHALL NOT be removed.

Initial-turn creation SHALL reach execution completion only when its required
barrier includes a compatible durable dispatch receipt. Seeded-empty creation
SHALL complete only when its own required metadata and state barrier is satisfied.
The product reducer alone SHALL publish `Ready` or failure.

### REQ-DWF-CREATE-004: Retry, Cancellation, and Deletion Mapping

The creation profile SHALL preserve its bounded transient retry policy and
permanent-failure classification as durable effect deadlines and reducer events.
Claim loss SHALL NOT itself become user-visible failure.

Cancellation SHALL immediately preserve a visible cancelled shell and creation
intent while atomically declaring compensation for runtime revocation, owned
worktree removal, reservation release, and staged-attachment cleanup as applicable.
Deletion SHALL hide the shell immediately and retain a durable tombstone until the
required compensation barrier is accepted by the reducer.

Reducer publication from stale generation-bound creation effects or stale lifecycle
completion evaluations SHALL NOT overwrite a later cancel, delete, or deleted
outcome.

### REQ-DWF-CREATE-005: Creation Capabilities

The creation reducer SHALL derive provisioning as read-only with Cancel and
Delete, failed and cancelled as read-only with Start over and Delete, and ready or
idle capabilities according to normal conversation behavior. Provisioning,
failed, cancelled, or deletion-pending states SHALL NOT start a runtime merely
because a caller omitted a state guard.
