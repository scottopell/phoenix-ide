# Repair continuation SQLite writer admission

Production continuation START/atomic COMMIT can fail immediately with SQLite code 5 or extended code 517 because the operation begins a deferred transaction, reads ownership/state, then attempts its first write after another WAL writer has committed or is active. Preserve operation identity, summary/message/state/turn atomicity, and duplicate/stale/retry semantics while reserving writer admission before transaction-local validation, consistent with merged PR #703. Add deterministic file-backed two-connection regressions through the actual continuation START and atomic COMMIT APIs, including concurrent writer admission, duplicate settlement, rollback/ambiguous retry convergence, and phase/extended-code observability. Do not globally rewrite transaction policy, add blanket timeouts, deploy, restart, or modify production data.

## Causal postmortem and production reachability

Deployed/current baseline `19fe992c72e5c044bc18c3f09b2e2cc4e40e0e2c` began continuation write transactions deferred. In WAL mode, the actual APIs read operation/ownership state and then wrote message, state, workflow/turn terminalization. A concurrent writer therefore made the first write fail immediately with primary code 5 while active, or extended code 517 after committing and invalidating the reader snapshot. This matches the incident's sub-millisecond statement-phase failures and the two-connection SQLite reproduction.

The accepted durable runtime calls `WorkflowRepository::settle_failed_continuation_start_atomically`, `settle_continuation_direct_turn_atomically`, and startup `reconcile_legacy_continuation_atomically`; all now execute `BEGIN IMMEDIATE` inside the existing transaction-acquisition telemetry boundary before validation reads. No helper opens a second pooled connection while the writer transaction is held.

`DatabaseStorage` also exposes raw `Database::begin_continuation` / `recover_continuation_start` and `commit_continuation` through the runtime effects trait. They delegate to `Database::persist_continuation_start` and `commit_continuation`, so they remain production-reachable even though the durable direct-turn path is preferred. Their transaction starts now also use `BEGIN IMMEDIATE`. A file-backed two-database regression covers their actual public START and COMMIT APIs under an independently held writer and verifies exact retry convergence.

All other transactions retain their prior policy; this is not a blanket SQLite mode change and adds no timeout, scan, polling, or retry loop.

The deterministic cut regression distinguishes failure timing precisely:

- before-commit injected failure rolls the transaction back; awaiting state, no message, generation 0, and turn ownership remain, and exact retry applies once;
- after-commit ambiguous failure returns an error after durable commit; completed state, one message, terminal generation 1, and released ownership are already atomic, and exact retry returns `Duplicate` without another transition.

Focused evidence: 18 continuation tests passed, including file-backed two-connection START/COMMIT/legacy contention, primary+extended code 5 at `transaction_acquisition`, raw Database wrapper contention, and rollback/post-commit retry convergence. `./dev.py check` passed all 17 applicable checks.
