# Shadow Conversation Creation on the Durable Workflow Runtime

## Mission

Build a conversation-creation adapter for the Durable Workflow Runtime and run it in non-authoritative shadow mode beside the existing durable creation protocol. Establish deterministic and production shadow parity before any execution cutover.

Depends on:

- Durable workflow specifications
- Pure engine and simulator
- Atomic workflow/effect persistence
- The merged safe async conversation-creation baseline from PR #442 or its eventual main-branch equivalent

## Safety boundary

The existing creation worker remains authoritative. Shadow workflows may persist plans, observations derived from authoritative results, and comparison records, but must not mutate Git, files, runtime state, conversation state, attachments, or provider state.

A shadow failure must never fail or delay user creation.

## Creation effect DAG

Model creation as typed effects, with dependencies chosen from the normative spec. At minimum cover:

```text
ResolveRepository
ReserveWorktree
MaterializeOrReconcileWorktree
FinalizeAttachments
CommitMetadata
ExpandInitialMessage
DispatchInitialLlmRequest
```

Seeded-empty creation must have its own typed required set and completion contract; it must not fake message expansion or dispatch.

Model optional effects separately, such as analytics/notifications if included.

## Reconciliation contracts

Define typed intent, observation, decision, and receipt codecs for creation effects. Worktree reconciliation must distinguish:

- Absent path
- Valid owned worktree
- Partial owned directory
- Foreign Git root
- Conflicting branch/worktree
- Missing repository
- Transient infrastructure failure

Every destructive worktree decision requires both live durable effect authority and repository locking in the eventual executor, even though shadow mode does not execute it.

## Compensation DAGs

Model reducer-emitted cancellation/deletion/failure compensation:

```text
RevokeRuntime
RemoveOwnedWorktree
ReleaseReservation
DeleteStagedAttachments
FinishCancellationOrDeletion
```

Preserve product semantics:

- Cancel is immediately visible and retains intent for Start over/Delete.
- Delete hides immediately but retains an internal tombstone until cleanup.
- Old-generation results are fenced and later observed/adopted or compensated.

## Parity harness

Run equivalent generated histories through:

1. Existing creation protocol simulator/worker model
2. New Durable Workflow Runtime creation adapter

Compare externally meaningful outcomes:

- Visible conversation state
- Creation prompt/intent preservation
- Worktree ownership/reservation state
- Retry timing and exhaustion
- Cancellation/deletion visibility
- Completion boundary
- Cleanup obligations
- Conflicts/manual-resolution classification

Internal representation may differ; divergences in externally meaningful behavior require resolution, not normalization away.

## Production shadowing

For newly accepted creation jobs, optionally create a shadow workflow record keyed to the authoritative job. Record:

- Planned DAG
- Effect eligibility predictions
- Reconciliation decisions inferred from authoritative observations/results
- Completion prediction
- Compensation prediction
- Divergence diagnostics

Do not mirror the same semantic bytes in overlapping authoritative fields. Shadow records are explicitly diagnostic and cannot be mistaken for execution authority.

## Fault campaigns

Cover crash before/after each authoritative side-effect boundary, including:

- Reservation committed, path absent
- Partial directory
- Worktree created, acknowledgement lost
- Metadata committed, bootstrap not enqueued
- Message persisted, request not dispatched
- Cancellation during materialization
- Delete during queued bootstrap
- Cleanup failure and retry
- Lease takeover
- Lost scheduler kick

## Capability parity

Map creation states to reducer-owned capabilities and compare with current product behavior:

- Provisioning: read-only, runtime forbidden, Cancel/Delete
- Failed: read-only, runtime forbidden, Start over/Delete
- Cancelled: read-only, runtime forbidden, Start over/Delete
- Ready/Idle: writable/runtime allowed as appropriate

Remove no existing UI guards in shadow mode.

## Non-goals

- Executing new-engine effects
- Deleting/replacing current creation worker
- Migrating in-flight jobs
- LLM/tool runtime-wide adoption
- User-visible behavior changes except diagnostics unavailable to normal users

## Verification

- Deterministic simulator parity with checked-in divergence regressions.
- Real SQLite shadow persistence tests.
- Real Git observation tests with no shadow mutation.
- Production shadow records prove no authority leakage.
- Performance/storage overhead measured and bounded.
- Independent review focused on accidental double execution and representation overlap.

## Acceptance criteria

- [ ] Creation DAG, typed effects, receipts, completion barriers, and compensation plans exist.
- [ ] Shadow mode cannot execute external effects structurally.
- [ ] Generated parity campaigns cover all listed fault boundaries.
- [ ] Every observed divergence is resolved or documented as an intentional normative change with spec/ADR updates.
- [ ] Shadow instrumentation has bounded overhead and can be disabled.
- [ ] Existing creation behavior and full project checks remain green.

## Follow-up dependency

The next task is **Cut Conversation Creation Over to Durable Workflows**.


## Umbrella authority and dependency

Task 47003 is the sole authority for shared-engine ownership, sequencing, migration gates, and release criteria. This task preserves its historical design context and narrower acceptance detail, but any conflicting creation-first, wake-only, bespoke-scheduler, or rollout direction is superseded. Implementations SHALL follow `specs/durable-workflows/requirements.md` and ADR-010 through ADR-012.

This task is blocked on engine-backed wake adoption and then serves the creation shadow or cutover milestone respectively.
