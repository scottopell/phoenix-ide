# Knock out three bounded reliability papercuts

Complete three independent ready tasks that are still reproducible in the current tree:

- **13036 — worktree cleanup failure visibility:** replace the silent `let _ = run_git(...)` in terminal worktree cleanup with explicit outcome handling. Preserve best-effort cleanup, but log failures with the worktree path and error.
- **13037 — tmux drain failure visibility:** replace `collect_drain`'s wildcard fallback with explicit timeout, cancellation, and panic branches. Log dropped-output paths at an appropriate level, with panic treated more severely than expected timeout/cancellation.
- **76003 — E2E port allocation TOCTOU:** make the E2E harness recover when its preselected port is taken before Phoenix binds. Prefer a bounded retry with a newly allocated port because it is local to the harness and avoids broad server startup protocol changes.

## Implementation plan

1. Read the relevant tool/runtime specifications before changing each existing tool or lifecycle path.
2. Add focused regression coverage where practical:
   - exercise or assert the cleanup/drain error classification without introducing timer-dependent tests;
   - cover bounded E2E startup retry behavior using a deterministic simulated bind failure rather than relying on a real port race.
3. Keep all three fixes behavior-preserving outside their failure paths; do not add new product features or generalized abstractions.
4. Run targeted tests first, then `./dev.py check`.
5. Transition tasks 13036, 13037, and 76003 to `done` once their acceptance criteria are met; transition this batch task to `done` as the umbrella record.
6. Commit the completed batch, using separate commits if the E2E harness change is materially independent from the Rust observability fixes.

## Acceptance criteria

- Failed terminal worktree removal is no longer silent and remains best-effort.
- Tmux drain timeout, cancellation, and panic are distinguishable in logs; successful output collection is unchanged.
- The E2E runner retries an `AddrInUse` startup failure with a fresh port, stops after a documented finite bound, and cleans up failed child processes.
- Regression tests are deterministic and avoid sleep-based race assertions.
- `./dev.py check` passes.
- Original tasks 13036, 13037, and 76003 are marked done.

## Excluded candidates

- Task 58012 is stale: SPA routes are now centralized in `api/spa_routes.rs`, and `/chains/:rootConvId` is already registered and auth-exempt.
- Tasks 13039 and 13040 require broader wire-type migration or a product/type-shape decision, so they are not part of this quick batch.
