---
title: Context exhausted state not restored after server restart
status: done
agent: claude
priority: p2
created: 2026-02-16
---

## Problem

When a conversation is in `context_exhausted` state and the server restarts, the state resets to `idle`. The continuation summary message IS persisted (message type `continuation`), but the recovery logic doesn't recognize it.

## Impact

1. **User confusion**: Exhausted conversation shows "ready" instead of "context full"
2. **Lost context**: The exhausted banner with summary disappears
3. **Wasted tokens**: If user sends a message, LLM request fires, hits threshold again, re-exhausts
4. **Backend still protects**: The message does eventually fail, but wastefully

## Root Cause

`runtime/recovery.rs::should_auto_continue()` only checks if the last message is a `tool` type for auto-continuation. It doesn't check for `continuation` message type.

## Fix

In `should_auto_continue()`, add a check at the top:

```rust
// Check for continuation message -> conversation is exhausted
if matches!(last_msg.message_type, MessageType::Continuation) {
    return RecoveryDecision {
        state: ConvState::ContextExhausted { 
            summary: extract_summary_from_continuation(last_msg) 
        },
        needs_auto_continue: false,
        reason: RecoveryReason::ContextExhausted,  // new variant
    };
}
```

Also need to add `ContextExhausted` to the `RecoveryReason` enum.

## Files

- `src/runtime/recovery.rs` - Add continuation message detection
- `src/db/messages.rs` - May need helper to extract summary from continuation content

## Test

1. Trigger context exhaustion
2. Restart server (`./dev.py restart`)
3. Verify conversation still shows "context full" state
4. Verify exhausted banner appears with summary
