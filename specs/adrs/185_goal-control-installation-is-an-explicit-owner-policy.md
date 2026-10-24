# ADR-185: Goal-control installation is an explicit owner policy

- **Status:** Accepted
- **Date:** 2026-10-20
- **Affects:** REQ-GOAL-011 through REQ-GOAL-014, REQ-GR-015, REQ-CEVT-006; refines ADR-184's installation boundary

## Context

ADR-184 chose transactional local goal-watch capture. Existing watch recorders
receive a transaction but not its owning database's explicit installation policy.
Normal startup remains goal-services-uninstalled and default OFF. Migration 135
is reserved for capture storage, not permission to install its consumer.
`Database` and `WorkflowRepository` already carry `GoalControlInstallation`;
normal constructors default to Disabled and repository construction propagates
that policy. Only isolated test/test-support installation is available.

## Options considered

1. Infer installation from tables, current goals, latch rows or environment.
   Rejected: applying migrations would silently activate dormant consumers.
2. Add an independent boolean or ambient policy on every `WorkflowTx`.
   Rejected: duplicated authority can diverge and widens unrelated transactions.
3. Require the existing owner policy explicitly at every watch boundary.
   Chosen: missing propagation becomes a type error without a new installer.

## Decision

Propagate `GoalControlInstallation` from the owning Database/WorkflowRepository
through terminal, wait, creation, steering, summary and settled-mutation helpers.
Do not change production constructors or normal startup. Record four distinct
local event-time cases: LegacyNotCaptured, NotCapturedDisabled, CapturedNoGoal
and CapturedSavedGoal. Captured usage remains explicitly unavailable.

Disabled leaves existing execution/wait outbox behavior unchanged except its
immutable disabled marker. It performs no goal SQL, creates no goal-capture child
row and adds no standalone GoalChanged event. Standalone fanout returns before
querying. Installed captures only newly inserted eligible local subscription
events, never enriches a deduplication conflict, and does not alter mandatory
Close or federation payloads. Read-time installation cannot reclassify an older
event; legacy and disabled capture never mean NoGoal.

Migration 135 supplies constrained event markers and event-owned capture, not
activation. No table-existence probe, startup installer, new worker, environment
switch or automatic follow-through is introduced. Existing owning-transaction
storage failure policy from ADR-184 applies to installed capture; disabled
consumers cannot fail because goal storage is unavailable.

## Consequences

- Mandatory policy arguments touch existing boundary helpers and their callers,
  but make omissions visible rather than relying on comments or default args.
- Disabled and legacy evidence require explicit projection cases. This is honest
  absence, not a compatibility fallback that reconstructs current goals.
- Real boundary tests must cover default constructors, repository propagation,
  unavailable goal SQL, no capture rows/extra events, installed failures,
  deduplication immutability, and unchanged 503/private-install behavior.
- Reservation/registration and source authoring do not constitute installation,
  execution qualification or authorization to activate production.

## References

- [ADR-184](184_local-goal-watch-facts-belong-to-their-causal-event.md).
- [Goal requirements](../conversation-goals/requirements.md), REQ-GOAL-011–014.
- `GoalControlInstallation`, `Database::workflow_repository`,
  `WorkflowRepository::new`, `capture_runtime_control_tx` and
  `install_goal_controls_for_tests` are the existing policy seams.
