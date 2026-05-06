---
created: 2026-05-06
priority: p2
status: ready
artifact: src/runtime/executor.rs
---

## Phase 3 follow-up to task 03001/09001 ConvMode::Explore refactor

The cwd-fallback removal in Phase 2 (commit 624c2516) didn't reach
`src/runtime/executor.rs:2008-2015`:

```rust
// Load-bearing cwd reset: the worktree is gone by this point, but API
// handlers (search_files, list_skills, list_tasks, get_system_prompt)
// read conv.cwd for terminal conversations without a state guard.
// Resetting to repo_root gives them a valid directory rather than a
// deleted worktree path.
self.storage
    .update_conversation_cwd(conv_id, &repo_root)
    .await?;
```

This is a workaround for downstream API handlers that read `conv.cwd`
without first checking `state == Terminal`. The handlers should be the
ones to refuse (or fall back to repo_root) when they see a terminal
conversation, not the executor's job to lie about cwd to keep them
happy.

## Why this wasn't fixed in Phase 2

Phase 2 was scoped to "kill the cwd-fallbacks in the worktree-path
resolution path" (`src/terminal/ws.rs`, `src/api/handlers.rs` cascade).
The `update_conversation_cwd` here is a different problem — it's about
post-completion cleanup, not worktree-path resolution.

## Suggested fix

Audit the four API handlers (`search_files`, `list_skills`, `list_tasks`,
`get_system_prompt`) to make them gracefully handle a terminal
conversation whose cwd points at a deleted worktree. Then remove the
cwd-reset hack at `executor.rs:2008-2015`.

Related: see the cwd-reset hack at `executor.rs:2130-2134` too — same
file, same shape, but for a different lifecycle transition (Managed
approval). May or may not be the same fix.

## Cross-references

- Phase 1: commit `bb50f4e6` (orphaned Work/Branch → Terminal)
- Phase 2: commit `624c2516` (Option C `ConvMode::Explore` variant + migration 007)
- Task 24692 (terminal-conv API handler audit) — related, possibly same scope
