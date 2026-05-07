---
created: 2026-05-07
priority: p1
status: in-progress
artifact: pending
---

# steering-messages

## Plan

# Steering Messages — Queue User Messages to an Ongoing Conversation

## Summary

Users can now type a follow-up message while the LLM is busy (generating, running tools, etc.). The message is queued and delivered when the conversation next reaches `Idle`. This is especially useful with background bash: "when that build finishes, run the full check command."

## Context

The current system returns `409 agent_busy` for any `UserMessage` sent when the conversation is not in `Idle`/`Error` state. With background bash, the conversation often idles quickly after the LLM acknowledges a running job — but there's a narrow window (while the LLM generates its acknowledgment) where the user wants to queue a follow-up. The steering queue closes that gap and makes the feature feel seamless.

The injection point already exists: `apply_transition_result()` in `executor.rs` is a single bottleneck where all state writes happen, already used to drain the sub-agent result buffer when entering `AwaitingSubAgents`. We mirror that pattern exactly.

## Design Decisions

**States that queue vs. still 409:**
| State | Behavior |
|-------|----------|
| `LlmRequesting`, `ToolExecuting`, `AwaitingSubAgents` | ✅ Queue — will return to Idle |
| `CancellingTool`, `CancellingSubAgents` | ✅ Queue — will return to Idle |
| `AwaitingTaskApproval`, `AwaitingUserResponse` | ❌ 409 — need specific response |
| `ContextExhausted`, `Terminal`, `Completed`, `Failed` | ❌ 409 — permanent |

**DB persistence:** Queue stored as JSON in a new `steering_queue` column on `conversations`. Crash-safe, survives server restart.

**Stop button:** Independent from the queue. Stop cancels the current LLM/tool operation only; queued messages survive and are delivered immediately when the resulting Idle is reached (natural "fast-forward" behavior). The user cancels a queued message via an X button on the message bubble itself.

**Queue depth:** FIFO, max 5 messages (hard limit — return 409 if full).

## What To Build

### 1. DB Migration

Add column to `conversations` via `src/db.rs` ALTER TABLE pattern:
```sql
ALTER TABLE conversations ADD COLUMN steering_queue TEXT NOT NULL DEFAULT '[]';
```

Add `db.get_steering_queue(id)` and `db.set_steering_queue(id, queue)` methods.

### 2. New Event Variant — `src/state_machine/events.rs`

```rust
Event::SteerMessage {
    text: String,
    llm_text: Option<String>,
    images: Vec<ImageData>,
    message_id: String,
    user_agent: Option<String>,
    skill_invocation: Option<SkillInvocation>,
}
```

### 3. Executor — `src/runtime/executor.rs`

- Add `steering_queue: Vec<SteerEntry>` field (loaded from DB on executor init)
- When receiving `Event::SteerMessage`: push to queue, persist to DB, emit `SteerMessageQueued` SSE
- In `apply_transition_result`, after the sub-agent buffer drain logic, add:

```rust
// Drain steering queue when entering Idle (mirrors sub-agent buffer pattern)
let entering_idle = !matches!(old_state, ConvState::Idle)
    && matches!(self.state, ConvState::Idle);

if entering_idle && !self.steering_queue.is_empty() {
    let entry = self.steering_queue.remove(0);
    tracing::debug!(message_id = %entry.message_id, "Delivering queued steering message");
    generated_events.push(Event::UserMessage {
        text: entry.text,
        llm_text: entry.llm_text,
        images: entry.images,
        message_id: entry.message_id,
        user_agent: entry.user_agent,
        skill_invocation: entry.skill_invocation,
    });
    // Persist updated queue (one removed) to DB
    persist_steering_queue_effect_or_inline(...);
}
```

The generated event feeds into the existing chained-events loop → normal `UserMessage` processing → `LlmRequesting`.

### 4. Runtime — `src/runtime.rs`

Add `enqueue_steer_message(id, event)` → looks up conversation handle, sends `Event::SteerMessage` to `event_tx`. Also loads `steering_queue` when initializing executor context (so crash recovery works).

### 5. API Handler — `src/api/handlers.rs`

In `send_chat`, replace the `AgentBusy | CancellationInProgress` 409 path:
- Check queue depth < 5, else return 409 with `error_type: "steering_queue_full"`
- Call `runtime.enqueue_steer_message()`
- Return `200 { queued: true, steering: true }`

Add `DELETE /api/conversations/:id/steering-queue/:message_id` to cancel a specific queued message (removes from executor queue + persists to DB).

### 6. SSE Wire Types — `src/api/wire.rs`

```rust
SseWireEvent::SteerMessageQueued {
    message_id: String,
    queue_position: usize,  // 0-based
}
```

(Codegen: add `#[derive(ts_rs::TS)]` and regenerate `ui/src/generated/`.)

### 7. Frontend — `ui/src/`

**`ui/src/sseSchemas.ts`**: Add `steer_message_queued` schema, add `satisfies` annotation.

**`ui/src/pages/ConversationPage.tsx`**:
- Handle `steering: true` in the send response — set message local phase to `'queued'` (new phase value) instead of `'awaiting_llm'`
- Handle `steer_message_queued` SSE → update queue position in local message state
- On SSE `user_message` echo that matches message_id → clear `queued` phase (normal flow takes over)

**Message display** (wherever message phases are rendered):
- `queued` phase: show message bubble with muted/italic style + ⏳ icon + "Queued" label
- Add an X button to cancel (calls `DELETE /api/conversations/:id/steering-queue/:message_id`, removes from local state on success)

**`ChatResponse` type**: add `steering?: boolean` field.

## Acceptance Criteria

1. While LLM is generating (LlmRequesting), user can type a message → it's accepted (200), appears in chat with "Queued" indicator
2. When LLM finishes its response and conversation reaches Idle → queued message is immediately delivered and LLM processes it (no user action needed)
3. Server restart while message is queued → message survives, is delivered when conversation resumes
4. Queuing in AwaitingTaskApproval/AwaitingUserResponse/ContextExhausted/Terminal still returns 409
5. Queue depth 5 → 6th message returns 409 `steering_queue_full`
6. X button on queued message → message is cancelled and removed from display
7. Stop while message is queued → queued message survives → delivered immediately when Idle is reached post-cancel
8. `./dev.py check` passes (codegen, clippy, tests)


## Progress

