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
