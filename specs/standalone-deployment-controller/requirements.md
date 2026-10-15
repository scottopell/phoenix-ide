# Standalone Deployment Controller

## User Story

As the operator responsible for replacing a running Phoenix installation, I need
deployment orchestration to make resource, ownership, cleanup, and recovery
decisions from durable typed evidence rather than shell-process accidents, so an
interruption or PID race cannot silently turn uncertainty into permission to
continue.

## Domain

The standalone controller is an independent policy core. It owns deployment
attempt state and emits typed operating-system requests only after producing the
checkpoint that must be durable first. Platform adapters supply process identity,
process-tree observations, resource measurements, signals, and candidate artifact
verification. The controller has no implicit authority over launchd or
production-deployment entry points; each entry point can use it only through an
explicit adapter.

The controller refines the compatibility policy in
`specs/compatibility/requirements.md`, the independent-owner boundary in
`specs/production-deployment/requirements.md`, and launchd activation behavior in
`specs/launchd-deployment/requirements.md`. It does not create a downgrade,
database rollback, or live-resource replacement guarantee.

---

## Requirements

### REQ-SDC-001: Durable Phase and Effect Ordering

WHEN a deployment attempt changes phase or records new evidence
THE SYSTEM SHALL produce a complete attempt checkpoint before producing any
operating-system mutation requested by that transition.

WHEN execution is interrupted after a checkpoint
THE SYSTEM SHALL recover from that checkpoint without assuming that an emitted
operating-system request completed.

WHEN a checkpoint has an unfinished child-enumeration, identity-observation, birth-probe,
or resource-sample request
THE SYSTEM SHALL retain and reissue that exact obligation during recovery before
advancing past its owning phase.

---

### REQ-SDC-002: Measured Resource Admission

WHEN the controller decides whether resource conditions admit deployment work
THE SYSTEM SHALL use an ordered configured window of measured capacity and
contention samples.

THE SYSTEM SHALL NOT use process names, a process blacklist, or equality of a
global process-table snapshot as resource-admission authority.

WHEN contention exceeds policy in only part of the required window
THE SYSTEM SHALL NOT classify that transient sample as sustained contention.

---

### REQ-SDC-003: Stable Process Identity

WHEN the controller records an owned process
THE SYSTEM SHALL identify it by native PID and a high-resolution process birth
token.

WHEN a PID appears in a child listing but no birth token has been acquired
THE SYSTEM SHALL record a distinct discovery epoch rather than an owned process.

WHEN a discovery epoch has recorded any failure before birth identity acquisition
THE SYSTEM SHALL NOT permit a later process at that PID to satisfy that epoch,
even when a reconstructed platform adapter observes the expected parent relationship.

---

### REQ-SDC-004: Truthful Observation Uncertainty

WHEN birth-identity acquisition reports no-such-process, permission denial,
temporary unavailability, or malformed metadata
THE SYSTEM SHALL retain the discovery as unresolved typed evidence
AND SHALL NOT convert that observation into verified process absence.

WHEN an unresolved discovery reaches the attempt's absolute operation deadline
THE SYSTEM SHALL stop bounded reconciliation in an
`administrative_closeout_required` phase rather than retrying indefinitely or
claiming verified cleanup.

---

### REQ-SDC-005: Exact Owned-Tree Mutation

WHEN the controller requests a process signal
THE SYSTEM SHALL name a previously acquired PID-plus-birth identity that a fresh
observation still matches.

WHEN an owned process is reparented after identity acquisition
THE SYSTEM SHALL retain ownership of that exact identity.

WHEN an owned PID is reused by a different birth identity
THE SYSTEM SHALL treat the original identity as absent
AND SHALL NOT signal or adopt the replacement without separate ownership evidence.

---

### REQ-SDC-006: Verified Cleanup

WHEN the controller reports `verified_clean`
THE SYSTEM SHALL have verified absence of every acquired owned identity
AND SHALL have no unresolved discovery epoch.

AN empty PID scan, a missing process group, or an identity probe that failed before
birth acquisition SHALL NOT by itself establish verified owned-tree cleanup.

---

### REQ-SDC-007: Administrative Closeout

WHEN explicit administrative risk acceptance identifies an attempt in
`administrative_closeout_required`
THE SYSTEM SHALL close administrative scheduling ownership without claiming
verified cleanup, requesting another process signal, or resuming that deployment.

THE accepted record SHALL retain the unresolved ownership evidence that required
administrative closeout.

---

### REQ-SDC-008: Interruption Reconciliation

WHEN a pre-migration attempt recovers from interruption
THE SYSTEM SHALL freshly observe every acquired owned identity before requesting a
new mutation
AND SHALL preserve the original absolute operation deadline.

THE SYSTEM SHALL NOT request any owned-process mutation until every acquired owned
identity has completed that recovery observation barrier.

WHEN reconciliation is itself interrupted
THE SYSTEM SHALL retain the original resume target and restart the complete
observation barrier.

WHEN reconciliation completes at or after the absolute operation deadline
THE SYSTEM SHALL NOT restore process-mutation authority unless cleanup is already
verified clean.

THE SYSTEM SHALL NOT replay a signal solely because the interrupted checkpoint
shows that the signal was previously planned or emitted.

---

### REQ-SDC-009: Forward Recovery Boundary

WHEN candidate migration invocation has been durably recorded
THE SYSTEM SHALL recover only by reverifying and running the exact retained
candidate identified by full source commit and artifact digest.

THE SYSTEM SHALL NOT start the predecessor or imply that an older binary is
compatible with the migrated database.

WHEN candidate migration invocation is already durable
THE SYSTEM SHALL reject an attempt to replace its retained candidate identity
AND SHALL allow recovery to reissue exact-candidate verification from both the
ready migration checkpoint and a forward-recovery checkpoint.

---

### REQ-SDC-010: Platform Boundary and Qualification

THE policy core SHALL receive process and resource facts through a typed
operating-system boundary
AND SHALL keep durable telemetry free of command lines, environment values,
credentials, and raw secret-bearing operating-system errors.

WHEN controller behavior is qualified
THE SYSTEM SHALL cover deterministic policy and race cases with a fake operating
system
AND SHALL separately qualify platform adapters with harmless isolated native
labels and resources before production adoption.

---

### REQ-SDC-011: Independent Normalized Persistence

THE standalone controller SHALL own a SQLite database whose lifecycle is independent
of the Phoenix application database under migration.

THE controller database SHALL normalize attempts, resource samples, owned identities,
pending child scans, discovery epochs, and retained candidates with relational
constraints rather than storing those child collections in an aggregate blob.

WHEN controller SQLite authority cannot classify whether a checkpoint committed
THE SYSTEM SHALL fail stop rather than publish effects or continue admission.

WHEN a checkpoint replaces an existing attempt row
THE SYSTEM SHALL compare the checkpoint's durable revision with the stored revision
in the same transaction
AND SHALL reject stale or same-ID replacement authority.

WHEN a checkpoint is loaded after controller restart
THE SYSTEM SHALL return affine recovery evidence that cannot request process mutation
until the required interruption and fresh-reconciliation checkpoints are committed.

WHEN persistence or load authority cannot classify durable state
THE SYSTEM SHALL invalidate that database handle for further admission until the
controller reopens the database and classifies its durable evidence.

---

### REQ-SDC-012: Owned-Child macOS Mutation

WHEN the macOS adapter creates process-mutation authority
THE SYSTEM SHALL spawn a direct child as an isolated process-group leader and retain
that child unreaped until group cleanup and final wait.

THE general observation adapter SHALL NOT signal an arbitrary process solely from a
PID-plus-birth observation.

THE SYSTEM SHALL expose descendant group escape, deliberate same-session group join,
and controller exit as unresolved supervision risks
AND SHALL treat supervision lost across controller exit as unresolved risk unless a
surviving supervisor handoff is positively identified.

WHEN a launchd-owned guardian has retained supervised children and its policy
controller exits
THE SYSTEM SHALL fence the ended controller session while allowing only already
started child work to continue
AND SHALL retain child results and native ownership without starting another step or
sending another signal.

WHEN a replacement policy controller reconnects
THE SYSTEM SHALL verify the same live guardian instance and its retained child slots
AND SHALL commit interruption and fresh reconciliation before authorizing any new
mutation
AND SHALL preserve the original absolute operation deadline.

WHEN the original deadline expires before recovery completes, or when the retained
guardian instance is lost or replaced,
THE SYSTEM SHALL preserve unresolved supervision risk and SHALL NOT adopt historical
PIDs, process groups, socket paths, labels, or heartbeat expiry as mutation authority.

---

### REQ-SDC-013: Affine Dispatch and Completion Correlation

WHEN a transition has successfully persisted its checkpoint
THE SYSTEM SHALL allow consumption of that `CommittedTransition` to mint one
non-cloneable `DispatchBatch` of non-cloneable `DispatchTicket` values.

EACH ticket SHALL carry exactly one `OsEffect` and a correlation key consisting
of the attempt ID, the issuing committed persistence revision, and the effect's
zero-based ordinal in the committed plan. Sibling effects SHALL have distinct
ordinals, including when their effect values are equal.

THE SYSTEM SHALL prevent executable ticket or batch fabrication from diagnostic
checkpoint or effect values. Uncommitted transitions, failed persistence, and
crash-loaded evidence SHALL expose no dispatch authority. Consuming a committed
plan through a diagnostic decomposition SHALL discard its dispatch authority.

`AttemptStore` implementations used by the dispatch boundary SHALL be trusted durable
issuers and SHALL return success only after committing the supplied checkpoint.
Dispatch tickets SHALL gate admission to `EffectExecutor`; `ControllerOs` remains an
adapter interface rather than a global operating-system capability sandbox.

WHEN the executor consumes one ticket
THE SYSTEM SHALL make exactly one matching `ControllerOs` call and return an
`ExecutedEffect` that structurally binds the original key and effect identity to
the corresponding typed `EffectCompletion` outcome, including typed failures.

THE executor SHALL NOT retry, escalate, roll back, infer phase, apply results to
policy state, fabricate evidence, or authorize activation. Exact retained-candidate
reverification SHALL remain an observation, not activation permission. Dropping a
batch or unexecuted ticket SHALL make no operating-system call.

THE dispatch boundary SHALL NOT claim exactly-once execution across controller
crash or derive replay permission from a previously issued ticket.

---

### REQ-SDC-014: Safe Standalone Store Commands

THE standalone binary SHALL accept only `status --store PATH --attempt U128` and
`closeout --store PATH --attempt U128`, with each flag supplied once in either
order. The attempt ID SHALL be unsigned ASCII decimal within the full `u128`
range. Unknown commands, unknown or duplicate flags, missing values, and malformed
IDs SHALL fail before opening a store.

THE commands SHALL open only an existing controller store without creating a
missing database or initializing an unversioned one. Missing stores SHALL remain
absent. The commands SHALL preserve schema admission, compare-and-swap revision
checks, and load/persistence fail-stop poisoning.

WHEN `status` loads an attempt
THE SYSTEM SHALL consume only affine loaded evidence for inspection and print
one JSON object followed by a newline without changing the checkpoint or requesting
recovery. The summary SHALL contain only `attempt_id` (canonical decimal string),
`persistence_revision` (positive integer), `phase` (the snake-case phase name), and
`cleanup_outcome` (the snake-case cleanup outcome name), without exposing a live
checkpoint, effects, or native process/resource details.

WHEN `closeout` loads an attempt in `administrative_closeout_required`
THE SYSTEM SHALL consume its stable evidence, create matching
`AdministrativeRiskAcceptance`, persist the administrative closeout, verify that
the committed batch has zero effects, and report `administratively_closed` through
the same summary shape. Administrative closeout SHALL preserve unresolved evidence
and SHALL NOT claim verified cleanup or authorize deployment continuation.

WHEN `closeout` loads any other phase
THE SYSTEM SHALL fail before recovery preparation or persistence without changing
the durable revision or executing an effect.

THE binary SHALL NOT expose native run, reconcile, deploy, activation, installer,
or schema-migration commands, and SHALL NOT invoke a native deployment adapter.

THE commands SHALL emit success JSON only on stdout and fixed category diagnostics
only on stderr. Diagnostics SHALL NOT include arguments, paths, raw SQL or SQLite
errors, command lines, or environment values. Exit codes and stderr categories
SHALL be:

| Exit code | Meaning | Stderr category |
| --- | --- | --- |
| 0 | Successful summary | None |
| 2 | Invalid command or arguments | `usage_error` |
| 3 | Store open, schema, load, or persistence failure | `store_error` |
| 4 | Attempt not found in the existing store | `attempt_not_found` |
| 5 | Closeout rejected for the loaded phase | `wrong_phase` |
| 6 | Internal invariant, JSON encoding, or stdout I/O failure | `internal_error` |
