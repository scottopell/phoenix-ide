---
id: 016
title: Graceful shutdown and crash recovery for in-flight operations
status: done
priority: p1
created: 2025-02-15
---

# Graceful Shutdown and Crash Recovery

## Problem

When the server restarts (gracefully or crash), conversations can get stuck in an inconsistent state:

1. User sends message
2. Agent responds with tool_use blocks
3. Tools execute and complete
4. State transitions to `LlmRequesting`, LLM request spawned
5. **Server restarts before LLM response arrives**
6. On restart, runtime starts with `Idle` state (REQ-BED-007)
7. User sees "ready" but the agent never finished responding

The spec (REQ-BED-007) says "resume from idle" but this is wrong when the conversation was mid-turn.

## Root Cause

```rust
// src/runtime.rs get_or_create()
ConvState::Idle, // Always resume from idle (REQ-BED-007)
```

The runtime ignores persisted state and always starts Idle.

## Solution

### 1. Detect "orphaned tool results" on startup

When a conversation resumes, check if:
- State would be Idle
- Last message is a tool_result
- No subsequent agent message with text

If so, the conversation was interrupted mid-turn and should auto-continue.

### 2. Graceful shutdown (SIGTERM)

On SIGTERM:
1. Stop accepting new messages
2. Wait for in-flight LLM requests (with timeout, e.g., 30s)
3. If timeout, mark conversations as "needs_recovery"
4. Exit

### 3. State recovery on startup

On startup, for each active conversation:
1. If state is `LlmRequesting` or `AwaitingLlm` → fire `RequestLlm` effect
2. If state is `ToolExecuting` → transition to error, tools have unknown state
3. If messages suggest interrupted turn → auto-continue

## Implementation

### Phase 1: Message-based recovery (simpler, covers most cases)

```rust
// In get_or_create(), after loading conversation:
let state = if should_auto_continue(&conv, &messages) {
    ConvState::LlmRequesting { attempt: 1 }
} else {
    ConvState::Idle
};

fn should_auto_continue(conv: &Conversation, messages: &[Message]) -> bool {
    // Check if last message is tool_result with no following agent text
    let last_msg = messages.last()?;
    if last_msg.message_type != "tool" {
        return false;
    }
    
    // Find last agent message
    let last_agent = messages.iter().rev()
        .find(|m| m.message_type == "agent")?;
    
    // Check if agent message was tool_use only (no text)
    let content: Vec<ContentBlock> = serde_json::from_str(&last_agent.content).ok()?;
    content.iter().all(|b| matches!(b, ContentBlock::ToolUse { .. }))
}
```

Then fire `RequestLlm` effect immediately after runtime starts.

### Phase 2: Graceful shutdown

Add signal handler:
```rust
tokio::select! {
    _ = signal::ctrl_c() => { /* graceful shutdown */ }
    _ = server.serve() => {}
}
```

Wait for runtimes to reach safe states before exit.

## Test Cases

1. Kill server while LLM request in flight → restart → conversation auto-continues
2. Kill server while tool executing → restart → conversation shows error, user can retry
3. SIGTERM during LLM request → waits for response → clean shutdown
4. SIGTERM timeout → marks needs_recovery → restart auto-continues

## Acceptance Criteria

- [ ] Conversations interrupted mid-turn auto-continue on restart
- [ ] No manual "continue" message needed
- [ ] Graceful shutdown waits for in-flight operations (with timeout)
- [ ] State machine remains correct-by-construction
