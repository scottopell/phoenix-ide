---
created: 2025-07-05
priority: p2
status: ready
artifact: src/api/assets.rs
---

# RustEmbed fails in worktrees without frontend build

## Summary

`cargo check`, `cargo test`, and `cargo clippy` all fail in any git worktree that hasn't run a frontend build (`npm run build` or `./dev.py up`). The `#[derive(RustEmbed)] #[folder = "ui/dist"]` macro in `src/api/assets.rs` requires the folder to exist at compile time.

## Context

This produces 5 cascading errors (1 derive error + 4 `Assets::get()` not found) that block compilation of the entire crate. You can't run `cargo test state_machine` or any Rust test without first building the frontend, even though the tests have no dependency on embedded assets.

Every worktree created for a task hits this. `./dev.py check` fails on clippy, cargo check musl, and cargo test.

## Acceptance Criteria

- [ ] `cargo check` succeeds in a worktree without `ui/dist/`
- [ ] `cargo test` runs state machine and other non-UI tests without a frontend build
- [ ] Production builds still embed the frontend via RustEmbed
- [ ] No runtime regression — asset serving works identically in dev and prod

## Notes

Common approaches:
- Conditional compilation: `#[cfg(feature = "embed-ui")]` with a feature flag, dev builds skip embedding
- Fallback empty struct: derive RustEmbed on an always-present empty folder, overlay with real assets at runtime
- Build script: `build.rs` creates an empty `ui/dist/` if missing so the derive succeeds (assets just return None)

The build script approach is simplest — `mkdir -p ui/dist` in `build.rs` costs nothing and makes the derive always succeed. Dev mode serves from Vite anyway, so empty embedded assets are fine.
