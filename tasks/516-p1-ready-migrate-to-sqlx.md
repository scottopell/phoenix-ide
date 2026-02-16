---
created: 2025-02-07
priority: p1
status: ready
---

# Migrate from rusqlite to sqlx

## Summary

Replace the synchronous rusqlite database layer with sqlx for native async support and compile-time query verification.

## Context

Currently phoenix-ide uses rusqlite wrapped in `spawn_blocking` for async compatibility. This adds overhead and complexity. The recovered rustey-shelley codebase demonstrates that sqlx works well for this use case:

- Native async queries without spawn_blocking
- Compile-time SQL verification (catches typos, schema mismatches)
- Built-in migration support
- Same SQLite backend

rustey-shelley Cargo.toml reference:
```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate", "chrono"] }
```

## Benefits

1. **Cleaner async code**: No more `spawn_blocking` wrappers
2. **Compile-time safety**: SQL errors caught at build time via `sqlx::query!` macros
3. **Migration tooling**: `sqlx migrate` CLI for schema management
4. **Better error messages**: sqlx provides more context on failures
5. **Connection pooling**: Built-in pool management

## Acceptance Criteria

- [ ] Replace rusqlite dependency with sqlx in Cargo.toml
- [ ] Convert `src/db.rs` to use sqlx types and async queries
- [ ] Move schema to `migrations/` directory using sqlx format
- [ ] Update all database calls to use native async (remove spawn_blocking)
- [ ] Ensure compile-time query checking works (may need `DATABASE_URL` env var)
- [ ] All existing tests pass
- [ ] No performance regression on conversation list/message queries

## Migration Strategy

1. Add sqlx dependency alongside rusqlite temporarily
2. Create parallel sqlx-based Database impl
3. Migrate one query at a time, testing each
4. Remove rusqlite once all queries migrated
5. Clean up spawn_blocking remnants

## Reference Implementation

See `/home/exedev/rustey-github-upstream-copy/src/db/` for working sqlx patterns:
- `models.rs` - Type definitions
- `queries.rs` - Async query implementations
- `/home/exedev/rustey-github-upstream-copy/migrations/` - Migration files

## Notes

- sqlx requires `DATABASE_URL` environment variable for compile-time checking
- Can use `sqlx prepare` to generate offline query data for CI builds without DB
- The `migrate` feature auto-runs migrations on connection
- Consider using `sqlx::query_as!` for type-safe row mapping
