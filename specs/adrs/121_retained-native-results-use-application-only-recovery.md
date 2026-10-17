# ADR-121: Retained native results use application-only recovery

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-SDC-013, REQ-SDC-015, REQ-SDC-017–020; completed-but-unapplied native sampling recovery, protocol v5

## Context

ADR-118, ADR-119, and ADR-120 establish controller durability, disconnect fencing,
and same-build native IPC. This decision refines those boundaries.

A native sampling runner can die after the guardian has executed and retained
`SampleResources` but before the controller store atomically applies the result.
The normalized issue remains `IssuedPendingApplication`, while the runner-local
`DispatchTicket`, issued-loop membership, and verified completion are gone.

The issue row proves that execution was admitted, not that it completed. The
wire snapshot proves that a result is retained, not that policy may apply it.
Reconstructing either a dispatch ticket or an ordinary policy loop from those
diagnostics would create a replay path and collapse execution authority into
stored evidence. Re-executing the sample would also abandon the guardian's
existing custody and could apply two different observations to one obligation.

## Options considered

1. **Reconstruct the original dispatch ticket or policy loop from SQLite.**
   This makes persisted diagnostics an alternate execution-authority factory
   and permits replay after process death.
2. **Discard or supersede the retained result and issue another sample.**
   This avoids ticket reconstruction but loses completed evidence and requires
   a separate durable settlement meaning before guardian custody can be
   released.
3. **Combine exact durable issuance with same-live retained provenance to mint
   application-only authority.** The capability can settle one already-executed
   result but cannot dispatch, activate reconciliation, or issue follow-up work.

## Decision

Use a distinct, affine application-only recovery capability.

A replacement runner first commits a fresh interruption and establishes an
exact reconciling session with the same live guardian before the original
absolute deadline. The controller store consumes crash-loaded evidence to
select the sole `IssuedPendingApplication` issue and current reconciling
binding. The guardian independently rereads that durable evidence and requires
an exact retained successful sample carrying the original issuing-session
provenance. Only their correlated result can construct the private recovered
application capability.

The store consumes that capability in one `BEGIN IMMEDIATE` transaction. It
rechecks the attempt revision, guardian identity, current reconciling session,
original issue, sampling sequence, and native monotonic deadline after lock
acquisition and immediately before commit. The bounded recovery accepts only a
nonterminal sample. It appends the sample to the interrupted
resource-sampling resume state and inserts the exact application row while the
top-level attempt remains `Interrupted`.

The operation returns no `DispatchTicket`, `ExecutedEffect`, runnable
transition, policy loop, activation permit, or acknowledgement authority. Once
the application is durable, the existing durable-proof acknowledgement path
may release the retained result even though fresh reconciliation remains
incomplete. Failed, terminal, missing, stale, mismatched, ambiguous, or expired
results remain retained and unapplied.

The dedicated recovery request changes the same-build wire contract. The
pre-adoption guardian protocol advances from v4 to v5 and explicitly rejects
v4 peers. The normalized controller schema remains v5 because the existing
issue, application, and acknowledgement rows already represent the durable
states without a new compatibility promise.

## Consequences

- A completed first resource sample survives runner death without native-effect
  replay or ticket reconstruction.
- Guardian custody plus durable issuance are both necessary and neither is
  sufficient alone.
- The recovered attempt stays fenced and interrupted; later progress requires
  another fresh recovery barrier.
- Death before application commit permits a new same-live verification. Death
  after commit uses acknowledgement-only recovery.
- Terminal resource decisions, failed observations, guardian replacement,
  cross-boot recovery, and general continuing orchestration remain separate
  design problems.
- Protocol v4/v5 replacement and downgrade compatibility are intentionally not
  promised under the pre-adoption same-build boundary.

## Supersession

This decision may be superseded if native orchestration gains a durable,
correct-by-construction owner that can preserve application authority across
runner replacement without making diagnostics executable. Any superseding ADR
must preserve non-replay, same-live provenance, absolute-deadline enforcement,
and explicit compatibility boundaries.
