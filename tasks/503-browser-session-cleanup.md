---
created: 2026-02-07
priority: p2
status: ready
---

# Browser Session Cleanup Not Running

## Summary

The `cleanup_idle_sessions` method is defined in `BrowserSessionManager` but the cleanup task spawn is commented out, so idle browser sessions are never cleaned up.

## Context

From the continuation prompt:
> **Session cleanup** - `cleanup_idle_sessions` is defined but never called (the cleanup task spawn is commented out in `BrowserSessionManager::new()`).

This could lead to:
- Memory leaks from accumulating browser sessions
- Stale Chrome processes consuming resources
- Port exhaustion if many conversations use browser tools

## Relevant Files

- `src/tools/browser/session.rs` - `BrowserSessionManager::new()` and `cleanup_idle_sessions()`

## Acceptance Criteria

- [ ] Uncomment/implement the cleanup task spawn in `BrowserSessionManager::new()`
- [ ] Configure reasonable idle timeout (e.g., 5-10 minutes)
- [ ] Verify browser processes are properly terminated on cleanup
- [ ] Add logging for session cleanup events
- [ ] Consider cleanup on conversation end/archive

## Notes

Need to be careful about cleanup timing - don't want to kill a session that's about to be used. The idle timeout should be long enough that normal conversation flow doesn't trigger premature cleanup.
