---
created: 2025-02-07
priority: p3
status: ready
---

# Investigate: SSE Stream agent_working Filter Logic

## ⚠️ INVESTIGATION ONLY

**This task is for investigation and documentation only. Do NOT implement any code fixes.**

Deliver findings as a report appended to this file. If vulnerabilities are found, document them and recommend fixes, but do not write implementation code.

## Summary

Compare SSE filtering logic between rustey-shelley and phoenix-ide. A subtle bug in rustey-shelley caused the UI to show "Agent working" forever. Goal: ensure phoenix-ide doesn't have similar logic inversions.

## The Bug in rustey-shelley

Commit `d578d62` fixed an SSE stream bug:

**Symptom:** UI showed "Agent working..." with Stop button even after agent finished.

**Root Cause:** The condition for skipping SSE updates was inverted:

```rust
// BROKEN - skips the exact update we need!
if event.sequence_id <= last_seq && !event.agent_working {
    continue;  // Skipped when agent_working became FALSE
}

// FIXED - only skip when nothing new AND still working
if event.sequence_id <= last_seq && event.agent_working {
    continue;
}
```

The bug skipped sending updates precisely when `agent_working` transitioned to `false` - the exact moment the UI needed to know!

## Investigation Tasks

### 1. Find phoenix-ide's SSE filtering logic

- [ ] Locate SSE stream handler in `src/api/` or `src/runtime/`
- [ ] Search for: `sequence_id`, `last_seq`, `agent_working`, `skip`, `continue`
- [ ] Document the exact filtering conditions

### 2. Analyze the logic

- [ ] What conditions cause an SSE event to be skipped?
- [ ] Are there any `!` or `not` operators that could be inverted?
- [ ] Trace through: "agent finishes, what events are sent?"

### 3. Test the scenario

- [ ] Start a conversation, send a message
- [ ] Watch network tab for SSE events
- [ ] Verify `agent_done` or equivalent event is received
- [ ] Check if UI correctly shows idle state after completion

### 4. Edge cases to verify

- [ ] Agent completes with no tool calls (just text response)
- [ ] Agent completes after tool execution
- [ ] Agent is cancelled mid-execution
- [ ] Network reconnection after agent completes

## Pit of Success Analysis

Can we make this bug unrepresentable?

1. **Explicit event types:** Instead of filtering by `sequence_id`, use explicit event types (`AgentStarted`, `AgentDone`)
2. **Always send state changes:** Never filter events that change `agent_working`
3. **Type-safe events:** Make the SSE event enum exhaustive so filtering logic is obvious
4. **Property test:** "If agent_working transitions, an event MUST be sent"

## Reference Files

**rustey-shelley:**
- `src/api/handlers.rs` - `stream()` function, around line 758
- Commit `d578d62` - the fix

**phoenix-ide:**
- `src/api/handlers.rs` or `src/api/mod.rs` - SSE handlers
- `src/runtime.rs` - `SseEvent` enum
- `ui/src/api.ts` or `ui/src/hooks/` - SSE client handling

## Success Criteria

- Document phoenix-ide's SSE filtering logic precisely
- Verify no similar inversion bugs exist
- If vulnerable, **document the vulnerability and propose a fix** (do not implement)
- Note any integration test gaps

---

## Investigation Findings

*(Append findings below this line)*
