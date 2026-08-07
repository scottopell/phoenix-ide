# Build a durable composable offline operation journal

## User journey
A supported operation committed during unreliable connectivity survives reload, executes with its stable identity, and remains visible until the server durably accepts/reconciles it or an explicit terminal action is required.

## Scope
- Separate dedicated journal database from disposable UI/cache storage.
- Versioned discriminated operation kinds, payloads, states, indexes, transaction-complete enqueue, multi-tab leases, executor/reconciler registry, and UI projections.
- Consolidate duplicate localStorage message queue and IDB pendingOps into one authoritative message record.
- Preserve/migrate generic offline operations; enable automatic retry only after each server endpoint has an idempotency or reconciliation contract.
- Prioritize message send. Model conversation creation as browser-local only until the server durably accepts and returns a job/request ID; after that the server owns progress.
- Update normative queue requirements/Allium before changing the persistence owner.

## Acceptance
- Committed message intent survives reload and renders from one record.
- Two tabs cannot corrupt or silently lose operations; stale lease completions cannot regress state.
- Retry preserves exact idempotency key/payload and deletion waits for authoritative reconciliation.
- Legacy localStorage-only, pendingOps-only, matching duplicate, mismatch, malformed, and retired variants migrate without silent loss or double execution.
- Journal unavailability is explicit when durability is promised but never blocks ordinary online app startup.

## Dependency
Deferred until lifecycle-command authority and per-operation idempotency/reconciliation authority are settled. Server replay audit remains required for each operation: messages first; rename needs CAS/idempotency; Open/Close/Delete and aggregate operations need authoritative lifecycle reconciliation. This task does not block transport-only transcript/cache work.
