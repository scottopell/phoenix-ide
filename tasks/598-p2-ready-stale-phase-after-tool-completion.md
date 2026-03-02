---
created: 2025-07-13
priority: p2
status: ready
---

# UI phase indicators stale after tool completes

## Summary

After a tool (e.g. bash) completes successfully, the UI continues showing "tool running" indicators until the next SSE event arrives. This creates a contradictory state where:

- The tool result shows a green ✓ (completed)
- The Cancel button is still visible (implying work in progress)
- The breadcrumb bar shows the tool name as active
- The state bar shows `🟡 bash` (tool executing)

## Context

The phase transition from `tool_executing` → next state only happens when the server sends the next SSE event (e.g. the LLM starting to think). If there's any delay between tool completion and the next event (LLM latency, network), the UI is stuck in a stale state.

The user sees a completed tool result but all status indicators say "still running." This is confusing — SSE streaming implies real-time updates, but the phase lags behind the visible content.

## Possible Approaches

1. **Server-side:** Send an explicit phase transition event when a tool completes (before the LLM response starts). Something like `tool_completed` → `thinking` so the UI can show an intermediate state.
2. **Client-side:** When a tool result with success status is rendered but phase is still `tool_executing`, show a transitional indicator (e.g. `🟡 thinking...` instead of `🟡 bash`).
3. **Hybrid:** Server sends a `tool_result` event that the client uses to update phase optimistically.

## Acceptance Criteria

- [ ] After a tool result appears with ✓, status indicators no longer show that tool as "running"
- [ ] Intermediate state between tool completion and next LLM response is visually distinct (e.g. "thinking...")
- [ ] Cancel button behavior is correct for the actual state
