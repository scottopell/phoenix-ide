# ProductConversation Close authority/evidence foundation

## PR 1 boundary

This task owns **dormant persisted authority/evidence primitives and schema invariants only**. The branch must not wire live settlement, resource admission, workspace inspection, retirement, History projection, archive/delete cleanup, or finalization orchestration into production paths.

The foundation includes:

- typed ProductConversation, transcript, Close-attempt, WorkScope, loss, inventory, evidence, and terminal-outcome identities;
- normalized Close obligation, topology snapshot, WorkScope snapshot, inspection, exact inventory, expected-resource, and retirement-evidence tables;
- repository APIs that transactionally read/write those dormant records;
- schema constraints/triggers that make invalid persisted shapes, stale evidence, and partial multi-row topology mutations impossible;
- typed reads that preserve whether a completed obligation was `archived` or `cancelled`.

No production runtime/API code calls the Close repository APIs in this PR.

## Follow-up ownership

- Task 92033 / PR 2 solely owns live settlement and WorkScope retirement orchestration: aggregate-wide authority release, fresh workspace inspection, resource-creation admission, inventory capture, destructive retirement, evidence recording, retry, and crash recovery.
- Task 92032 / PR 3 solely owns History finalization and deletion: durable outcome message, FTS projection, aggregate History transition, exact finalization replay, History listing, and lifecycle-aware archive/delete cleanup.

## Review-finding classification

### Foundation-local

- Typed completed outcome: retain in this task so persisted terminal meaning is total in the domain/repository model.
- SQL statement atomicity: retain `RAISE(ABORT)` for sealed topology and WorkScope mutation guards.

### PR 2 orchestration

- Hold resource-creation admission through retirement/finalization.
- Prove aggregate-wide settlement authority release.
- Recompute current workspace evidence before retirement authorization.

### PR 3 finalization

- Exact archived-finalization replay after commit/result loss.

## Acceptance criteria

- [ ] The PR differs from its immutable merge base only in Close domain/persistence/migration files, dependency lockfile wiring, this task, and the two follow-up task files.
- [ ] No Close repository API has a production caller outside `phoenix-core` / `phoenix-db` tests.
- [ ] No cleanup-execution owner, resource-admission fence, archived finalizer, or History projection API remains in the PR.
- [ ] Completed obligations round-trip a typed `archived` or `cancelled` outcome.
- [ ] Sealed topology/WorkScope guards abort the whole multi-row SQL statement.
- [ ] Focused domain, migration, and Close repository tests pass.
- [ ] Strict clippy and the full local repository gate pass.
- [ ] One exact-head review judges the PR against this reduced dormant-foundation boundary.
