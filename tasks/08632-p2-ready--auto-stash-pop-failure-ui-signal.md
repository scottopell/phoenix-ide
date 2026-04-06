---
created: 2026-04-06
priority: p2
status: ready
artifact: src/api/handlers.rs
---

# Surface auto-stash pop failure to user

## Summary

When the auto-stash merge flow does stash push -> merge -> stash pop, and the
pop fails, the failure only logs a tracing::warn. The user's uncommitted work
is stranded in git stash list with no UI signal. Given this targets the exact
scenario where the user has uncommitted work they care about, silent stranding
is the worst outcome.

Also: the TOCTOU gap between the overlap check (in complete-task) and the
actual stash (in confirm-complete) means the overlap check can be stale.

## Done when

- [ ] Stash pop failure produces a system message or UI warning (not just a log)
- [ ] Warning tells the user to run `git stash pop` manually
- [ ] Consider: re-check overlap at stash time (inside the mutex), not just at pre-check time
