# Prevent transient SQLite busy snapshots from surfacing as PR-status 500s

## Production evidence

`GET /api/conversations/:id/pr-status` returned HTTP 500 in production with SQLite extended code 517 (`SQLITE_BUSY_SNAPSHOT`, "database is locked") after approximately two seconds. This occurred near, but was causally independent of, the late Stop failure addressed by PR #607.

## Investigation scope

Trace `get_conversation_pr_status` and its refresh/write path in `crates/phoenix-ide/src/api/git_handlers.rs` to identify the transaction that upgrades or holds a stale read snapshot while another writer commits. Determine whether the endpoint should use a shorter read transaction, a typed retry around an idempotent whole operation, or separate observation from refresh mutation.

## Acceptance criteria

- [ ] Deterministic concurrency regression reproduces the busy-snapshot path without sleeps.
- [ ] The endpoint does not surface transient `SQLITE_BUSY` / `SQLITE_BUSY_SNAPSHOT` as an unclassified HTTP 500.
- [ ] Any retry encloses the full idempotent transaction/operation rather than retrying a partial write.
- [ ] Retry is bounded and logged with attempt/outcome context.
- [ ] Non-retryable database errors remain visible and correctly classified.
- [ ] PR freshness and snapshot semantics remain consistent with the compact actionable-feedback contract.

## Non-goals

- Do not couple this fix to conversation cancellation or runtime state reconciliation.
- Do not add broad global retries that can replay non-idempotent operations.
