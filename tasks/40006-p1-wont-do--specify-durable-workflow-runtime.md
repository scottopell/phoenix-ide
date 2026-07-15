# Specify the Durable Workflow Runtime

## Mission

Define the normative runtime-wide protocol by which Phoenix commits workflow state, durably schedules external effects, recovers after crashes, fences stale workers, derives completion, cancels in-flight work, and reconciles ambiguous external outcomes.

This specification is the entry point for the **Durable Workflow Runtime** initiative. It must stand independently of the conversation that motivated it. Conversation creation is the first adopter, but the resulting contracts must also fit LLM dispatch, runtime lifecycle, tools/processes, cleanup, and notifications.

## Context

The durable async conversation-creation work proved that claim/lease/generation fencing and deterministic simulation are valuable, but repeated review findings exposed recurring architectural gaps:

- State could persist without the resulting effect being durably scheduled.
- Authority checks could be separated from mutations or destructive effects.
- “Complete” had several competing meanings.
- Cleanup mixed durable ownership, filesystem inference, and Git serialization.
- Cancellation required immediate authority revocation plus durable reconciliation.
- New states required scattered negative capability lists in API, runtime, SQL, and UI.

The new runtime must eliminate these bug classes structurally rather than encode more conventions.

## Fixed design decisions

These decisions are normative and must not be reopened casually:

1. The scope is runtime-wide, not creation-specific.
2. Extend Phoenix’s existing pure reducer contract; do not introduce a parallel authoritative workflow reducer.
3. A committed transition atomically persists its user-visible state, append-only transition record, typed effect intents, dependencies, and completion barrier.
4. External work is represented as a durable typed effect DAG.
5. Reducer transitions have one serialized writer enforced by workflow-version CAS; independent eligible effect nodes may execute concurrently.
6. Every effect executes under fenced claim/token/lease/generation authority.
7. Lost acknowledgements use inspect-then-reconcile: perform, adopt, repair/compensate, conflict, or request manual resolution.
8. Every effect type declares exactly one ambiguity/recovery capability: observable reconciliation, externally enforced idempotency, safe repeatability, or durable manual resolution.
9. Typed receipts return to the same reducer. The engine records execution truth; the reducer owns product meaning.
10. Completion is derived from a typed required-effect barrier. Optional effects cannot block success.
11. Cancellation atomically advances generation, publishes cancelled state, revokes old authority, and emits an explicit compensation DAG.
12. Compensation effects are ordinary durable/reconcilable effects, not hidden rollback logic.
13. Durable storage uses normalized current snapshots plus append-only transition/effect history; full event sourcing is not required.
14. Effect vocabulary uses typed families with registered versioned codecs, not arbitrary opaque plugin strings and not one monolithic central enum.
15. Reducer-owned exhaustive capability projections govern compose, terminal, runtime start, cancel, retry, delete, prompt hydration, and similar affordances. Capabilities are derived, not separately persisted.
16. Rollout is a creation-first vertical slice delivered through follow-up PRs; do not expand PR #442.

## Required artifacts

Create:

```text
specs/durable-workflows/requirements.md
specs/durable-workflows/durable-workflows.allium
specs/durable-workflows/executive.md
```

Add project ADRs, using the next available ADR numbers rather than assuming numbers:

- Durable transitions commit effect intents atomically
- External effects use inspect-and-reconcile recovery
- Workflows use normalized snapshots plus append-only history

Update `specs/adrs/README.md`.

## Requirements coverage

The spEARS requirements must define timeless needs for:

- Atomic transition-plan commit
- Workflow version CAS and generation fencing
- Durable typed effect DAGs and dependency eligibility
- Effect claims, renewal, expiry, takeover, and stale-result rejection
- Mandatory recovery capability
- Observation and reconciliation decisions
- Typed attempts, observations, receipts, conflicts, and manual resolution
- Required/optional/compensation effect classification
- Completion barriers
- Receipt-driven reducer events
- Immediate cancellation and durable compensation
- Resource-specific physical serialization in addition to DB authority
- Reducer-owned capabilities
- Normalized snapshots and append-only history
- Deterministic virtual-time verification
- Creation shadow parity and migration compatibility
- Explicit non-claim of universal exactly-once external execution

## Allium model

The Allium model must precisely cover:

- Workflow snapshot/version/generation
- Transition plan commit and version conflict
- Effect intent lifecycle
- Dependency readiness
- Claim, renewal, expiry, crash, and takeover
- Observation before external mutation
- Lost acknowledgement and reconciliation outcomes
- Receipt persistence and delivery to reducer
- Required-effect barrier satisfaction
- Optional effects after workflow completion
- Cancellation generation change
- Old-generation fencing
- Reducer-emitted compensation effects
- Durable conflict and manual resolution
- Invariants connecting snapshot, transition history, intents, receipts, and barriers

Include implementation guidance only where operation ordering is safety-critical. Keep all timeless artifacts free of task/PR references and rollout-status language.

## Core invariants to specify

At minimum:

```text
A committed workflow version has exactly one transition-history record.
Every effect declared by a committed transition exists durably in the same commit.
Only one transition may advance workflow version N.
Only a live effect authority may persist an attempt result or receipt.
Old workflow generations cannot affect current workflow state.
An effect is claimable only after all required dependencies have compatible receipts.
A completion barrier is satisfied only by its declared required receipts.
Cancellation is visible in the same commit that revokes prior-generation authority.
A destructive effect requires both durable authority and its resource-specific lock.
Every registered effect has one explicit ambiguity policy.
```

## Non-goals

- Implementing the engine
- Migrating current creation jobs
- Rewriting the whole runtime
- Claiming exactly-once execution
- Designing UI appearance
- Creating generic arbitrary plugins
- Replacing SQLite

## Verification

- Run `allium check specs/durable-workflows/durable-workflows.allium`.
- Run the pre-flight checklist in `specs/AUTHORING.md` before push.
- Verify ADR ordering/indexing and cross-spec references.
- Commission independent review focused on ambiguity, crash boundaries, and contradictions with the existing state-machine/runtime specs.

## Acceptance criteria

- [ ] Requirements, Allium, executive, ADRs, and ADR index exist and validate.
- [ ] All fixed decisions above are represented normatively.
- [ ] State/effect atomicity and authority invariants are unambiguous.
- [ ] Every lifecycle transition has defined crash and cancellation behavior.
- [ ] Completion and failure ownership are clearly divided between engine and reducer.
- [ ] Creation-first migration constraints are specified without status-relative language.
- [ ] No unresolved Allium questions remain.
- [ ] The spec is sufficient for an agent to implement the pure engine without relying on hidden conversation context.

## Follow-up dependency

The next task is **Build the Pure Durable Workflow Engine**. It must treat these artifacts as normative.


## Umbrella authority and dependency

Task 47003 is the sole authority for shared-engine ownership, sequencing, migration gates, and release criteria. This task preserves its historical design context and narrower acceptance detail, but any conflicting creation-first, wake-only, bespoke-scheduler, or rollout direction is superseded. Implementations SHALL follow `specs/durable-workflows/requirements.md` and ADR-013 through ADR-014.

This specification task is superseded by task 47003 Milestone 1; its body remains design input.
