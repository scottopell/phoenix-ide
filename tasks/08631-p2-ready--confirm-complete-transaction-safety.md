---
created: 2026-04-06
priority: p2
status: ready
artifact: src/api/handlers.rs
---

# Wrap confirm_complete DB writes in a transaction

## Summary

confirm_complete does 3 separate DB writes (state -> Terminal, mode -> Explore,
cwd -> repo root) with no transaction. If the process crashes between writes,
the conversation is left in an inconsistent state -- e.g., Terminal with stale
worktree cwd that reconcile_worktrees won't catch (it only checks Work convos).

Found by persona panel (Marcus, power user skeptic).

## Done when

- [ ] State, mode, and cwd updates are in a single SQLite transaction
- [ ] Same fix applied to abandon_task (same pattern)
- [ ] If any write fails, all are rolled back
