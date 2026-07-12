# Cut Conversation Creation Over to Durable Workflows

## Mission

Move newly accepted conversation creation from the bespoke creation scheduler/protocol to the Durable Workflow Runtime after specification, simulation, persistence, and shadow parity are proven.

Depends on all prior Durable Workflow Runtime tasks, especially successful shadow parity. Do not begin based only on unit-test confidence.

## Versioned rollout boundary

Introduce a durable protocol/version discriminator at acceptance:

- Existing and in-flight legacy creation jobs continue under the existing worker.
- Newly accepted jobs may use the durable workflow engine when enabled.
- No row is reinterpreted through a different protocol after acceptance.
- Disabling new-engine acceptance does not invalidate already accepted new-engine workflows; their executor remains available until drained.

The migration must be forward-recoverable and rollback-safe at the acceptance boundary.

## Production executor

Implement registered creation effect handlers using the shared engine contracts:

- Resolve repository
- Reserve worktree
- Materialize/reconcile worktree
- Finalize attachments
- Commit metadata
- Expand initial message
- Dispatch initial LLM request
- Runtime lifecycle/notification effects if required
- Compensation effects for cancel/delete/failure

Handlers record execution truth only. Typed receipts return to the reducer, which owns visible state and next compensation/completion plans.

## External-effect safety

- Worktree destructive effects require live effect authority plus repository mutation lock.
- Lost acknowledgement observes before replay.
- Partial owned paths are repaired/rematerialized.
- Foreign resources become durable conflict/manual-resolution states and are never removed.
- Provider effects use their declared recovery capability; do not invent idempotency guarantees.
- Attachment ownership and cleanup remain conversation-scoped and durable.

## Completion

Use typed required-effect barriers:

- Seeded-empty creation completes only when its required metadata/state effects have compatible receipts.
- Initial-turn creation completes only at the normative durable dispatch boundary.
- Optional analytics/notifications do not block readiness.
- No handler or incidental state write marks creation complete directly.

## Cancellation and deletion

Cancellation transaction:

```text
advance workflow generation
commit visible cancelled state
revoke old effect authority
append compensation DAG
commit
```

Deletion hides immediately, emits deletion publication, and retains a durable tombstone until compensation/cleanup barriers complete.

Old workers/effects return authority loss and perform no stale durable writes. External successes discovered after cancellation are adopted and compensated.

## Capability integration

Replace scattered negative state lists with reducer-owned typed capability projection for creation lifecycle surfaces. Backend enforces runtime/API capabilities; generated wire types allow UI components to render positive permissions.

Do not persist capability snapshots as a second semantic authority.

## Cutover verification

Before enabling by default:

- Shadow parity meets an explicit zero-unresolved-divergence threshold for normative outcomes.
- Deterministic fault campaigns pass.
- Real SQLite contention/restart tests pass.
- Real Git worktree create/adopt/partial/conflict/cleanup tests pass.
- API/UI integration covers accept, reload, cancel, delete, start over, retry, seeded-empty, and first turn.
- Restart after every effect boundary reaches a correct terminal or recoverable state.
- Lost kicks recover from durable deadlines.
- Resource and scheduler load are measured.

## Legacy retirement

Only after all legacy creation jobs are drained and rollback policy permits:

- Stop accepting legacy creation jobs.
- Remove the bespoke scheduler/worker and duplicate protocol code.
- Retain migrations/readers needed for historical or terminal legacy rows until separately safe to remove.
- Prove no parallel representation remains for creation authority/completion.

## Non-goals

- Migrating unrelated LLM/tool workflows in the same PR
- Runtime-wide big-bang cutover
- Rewriting UI design
- Deleting legacy tables before compatibility is proven

## Acceptance criteria

- [ ] Durable version boundary separates legacy and engine workflows.
- [ ] New-engine creation executes end to end under typed effect authority.
- [ ] Completion, cancellation, deletion, and compensation obey the specs.
- [ ] No external effect blindly replays after ambiguous success.
- [ ] Capability projection replaces creation-specific negative lists.
- [ ] Existing legacy jobs drain safely.
- [ ] Rollout can be disabled without stranding accepted workflows.
- [ ] Full tests, deterministic fault campaigns, real Git/SQLite integration, and `./dev.py check` pass.
- [ ] Independent review has no unresolved correctness findings.

## Follow-up dependency

The next initiative task is **Adopt Durable Workflows Across the Runtime**.


## Umbrella authority and dependency

Task 47003 is the sole authority for shared-engine ownership, sequencing, migration gates, and release criteria. This task preserves its historical design context and narrower acceptance detail, but any conflicting creation-first, wake-only, bespoke-scheduler, or rollout direction is superseded. Implementations SHALL follow `specs/durable-workflows/requirements.md` and ADR-010 through ADR-012.

This task is blocked on engine-backed wake adoption and then serves the creation shadow or cutover milestone respectively.
