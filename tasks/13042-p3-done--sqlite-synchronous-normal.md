# SQLite: set `synchronous=NORMAL` under WAL to eliminate fsync-per-commit stalls

## Symptom

During `./dev.py check`, sqlx emits 1.3s slow-statement WARN logs for
single-row INSERTs that should take microseconds:

```
WARN slow statement: execution time exceeded alert threshold
  db.statement: "INSERT INTO turn_usage (...) VALUES (?1..?8)"
  rows_affected: 1
  elapsed: "1.307234115s"
  slow_threshold: "1s"
  target: sqlx::query

WARN slow statement: ...
  db.statement: "INSERT INTO messages (..., display_data, ...) VALUES (?1..?8)"
  elapsed: "1.333047294s"
```

First surfaced by the new e2e lane (`tests/e2e/run.py`) running
concurrently with `lane_rust`'s cargo workload.

## Diagnosis

Under isolated load (sequential server, 5 multi_tool + 3 plain_text
runs in a row) the warnings do not appear. They only manifest when
many lanes hit disk concurrently (cargo compile + vite build + tsc +
vitest + e2e all writing fsync-bound data simultaneously).

`crates/phoenix-ide/src/db.rs:104-108` opens the pool with:

```rust
SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))?
    .journal_mode(SqliteJournalMode::Wal)
    .busy_timeout(std::time::Duration::from_secs(5))
    .foreign_keys(true);
```

No explicit `synchronous` setting, so SQLite defaults to `FULL` — every
COMMIT triggers an fsync on the WAL file. On a busy ext4 mount that
fsync can take 1+ second. The `add_message_with_seq` loop in
`runtime/executor.rs:2044` serializes tool-result persistence per
checkpoint, so the second of N tool results waits behind the first's
fsync.

`display_data` size is not the issue: bash tool payloads observed in
e2e (ls -la src/, git log --oneline -5) are well under 1 KB.

## Proposed change

Add `.synchronous(SqliteSynchronous::Normal)` to both `Database::open`
and `Database::open_in_memory` (so dev/test/prod all share the same
durability setting).

`NORMAL` under WAL is the standard choice for write-heavy applications:
SQLite still fsyncs on `PRAGMA wal_checkpoint` (which the WAL auto-runs
periodically), and data committed to WAL is durable across process
crashes. The only loss window is a power failure / hard reset between
WAL append and the next checkpoint fsync — for a coding assistant where
state is reconstructible from conversation history and the worst case
is "the last assistant turn might need to be re-streamed", this
tradeoff is the right one.

## Acceptance

- [ ] `synchronous=NORMAL` applied in `Database::open` and `open_in_memory`
- [ ] Stress-run `./dev.py check` 3x, confirm zero `slow statement` WARNs
- [ ] No new test failures (the existing in-memory tests should be
      unaffected — in-memory DBs don't fsync regardless)
- [ ] Note in commit: durability tradeoff under WAL, why it's safe here

## Not in scope

- Batching `add_message_with_seq` calls per checkpoint into a single
  transaction. That's a separate optimization (would also help) and a
  larger blast radius. Worth its own task only if `NORMAL` doesn't
  fully eliminate the contention.
- Tuning `max_connections` on the pool. Current defaults are fine for
  a single-user IDE; revisit if/when the app grows multi-tenant.
