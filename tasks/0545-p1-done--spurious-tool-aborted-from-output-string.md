---
created: 2026-02-15
priority: p1
status: done
---

# Spurious ToolAborted Events From Output String Matching

## Summary

The executor sent `ToolAborted` events when tool output contained the string `"[command cancelled]"`, even when no cancellation was requested. This violated the state machine contract and caused conversations to become permanently stuck.

## The Bug

In `src/runtime/executor.rs`, the tool execution handler checked the **output string** to determine whether to send `ToolAborted`:

```rust
// THE BUG - checking output string instead of token state
if out.output.contains("[command cancelled]") {
    let _ = event_tx.send(Event::ToolAborted { tool_use_id }).await;
    return;
}
```

This was wrong because:

1. **State machine contract violation**: `ToolAborted` is only valid from `CancellingTool` state, which is entered when the user explicitly cancels (triggering `AbortTool` effect)
2. **From `ToolExecuting` state**, only `ToolComplete`, `SpawnAgentsComplete`, or `UserCancel` are valid events
3. **The transition was rejected**, leaving the conversation stuck in `ToolExecuting` forever

## Root Cause Analysis

The bash tool correctly returns `"[command cancelled]"` only when its cancellation token is signaled:

```rust
// In bash tool - this is correct
tokio::select! {
    () = ctx.cancel.cancelled() => {
        ToolOutput::error("[command cancelled]")
    }
    // ...
}
```

But the executor was using a **side effect** (output content) to infer **state** (was cancellation requested?). This is fundamentally unsound - the output string could theoretically appear for other reasons.

## The Fix

Check the cancellation token's state directly:

```rust
// Clone token before passing to tool context
let cancel_token_check = cancel_token.clone();

// After tool completes, check token state - NOT output string
if cancel_token_check.is_cancelled() {
    tracing::info!(tool_id = %tool_use_id, "Tool cancelled (token signaled)");
    let _ = event_tx.send(Event::ToolAborted { tool_use_id }).await;
    return;
}
```

The token is only signaled by `AbortTool` effect, which is only emitted after transitioning to `CancellingTool` state. This ensures proper state machine invariants.

## Why This Slipped Through

The irony: Phoenix IDE has extensive property-based testing of the state machine, including:
- `prop_cancelling_tool_aborted_goes_idle` - verifies `ToolAborted` from `CancellingTool` → `Idle`
- `prop_tool_complete_with_matching_id_succeeds` - verifies normal tool completion
- `test_cancel_during_tool_execution` - integration test for cancellation flow

But all tests assumed the executor would **only send valid events**. The bug was in the executor's event generation logic, not the state machine's transition logic. The state machine correctly rejected the invalid transition - it just had no way to recover.

## Regression Test Added

`test_cancelled_output_without_token_sends_tool_complete` verifies that:
- Tool returns output containing `"[command cancelled]"`
- Cancellation token is NOT signaled
- Executor sends `ToolComplete` (not `ToolAborted`)
- Conversation progresses normally

## Bonus Bug Found

While testing the fix, discovered a UTF-8 boundary panic in `patch/planner.rs`:

```rust
// Panics if byte 2000 is mid-character
let header = &content[..content.len().min(2000)];
```

Fixed by walking backward to find a valid char boundary.

## Commits

- `8f5bee3` - fix(executor): check cancellation token, not output string, for ToolAborted
- `1a6b032` - fix(patch): handle UTF-8 char boundaries when truncating content

## Lessons

1. **Don't infer state from side effects** - if you need to know whether X happened, check X directly, not its downstream consequences
2. **The executor is part of the trusted computing base** - state machine correctness assumes the executor sends valid events
3. **Property tests on the state machine don't test event generation** - need integration tests that verify the full loop
4. **Edge cases compound** - the trigger was some timing quirk that produced the magic string; the consequence was a state machine violation
