# Steering Messages — Executive Summary

## Requirements Summary

A steering message is a user-directed instruction sent to a conversation that is currently *busy* — running an LLM turn, waiting on tools, or in any non-idle state. Without this feature, the API would reject the send (`agent_busy`) and the user would have to wait for the turn to finish before redirecting it. The steering queue accepts the message anyway, persists it, and delivers it as a normal `UserMessage` the next time the conversation transitions back to idle. From the LLM's perspective, the message is indistinguishable from a regular send; from the user's perspective, the conversation never refuses input even when it's "thinking".

The queue is FIFO, one-entry-per-idle-transition (so the model gets to respond to each before the next arrives), survives Phoenix restarts (persisted in `conversations.steering_queue` JSON column), and supports cancellation by `message_id`. Cancellation is idempotent — cancelling an already-drained or never-existed entry is a successful 200 OK. Terminal conversations refuse steering entirely; the queue is bounded by absence rather than a numeric cap.

## Technical Summary

The queue is **executor-level, not state-machine-level**. `Event::SteerMessage` is intercepted by the executor before it reaches the bedrock transition function — bedrock has no awareness of the queue, and the queue exists alongside core_status rather than as another state. This keeps the bedrock state machine pure (`specs/bedrock/`) and lets steering compose orthogonally with every existing transition.

Three persistence-ordering rules are load-bearing:

- **P1 (enqueue):** `enqueue_steer_message` reads the current queue, appends the new entry, writes the updated queue to the DB, *then* sends `Event::SteerMessage` to the executor channel. The HTTP response returns only after the DB write succeeds. A crash between acceptance and executor processing does not lose the entry.
- **P2 (cancel):** the cancel handler updates the DB first, *then* sends `Event::CancelSteerMessage` to the live executor (if running) which removes the matching entry from in-memory state without a further DB write.
- **Drain:** when the executor enters idle and the queue is non-empty, `entries.remove(0)` produces a `UserMessage` event, persists the shortened queue, and dispatches the message. A crash between drain and dispatch re-drains the same entry on next startup; bedrock's `message_id` deduplication prevents double-delivery.

`Vec::remove(0)` is the FIFO mechanism. The persisted JSON array round-trips through `db.update_steering_queue` (`crates/phoenix-ide/src/db.rs:576-595`) and is loaded into the executor on startup via `with_steering_queue` (`runtime.rs:1007`). The queue is therefore live immediately after a Phoenix restart — no warm-up.

## Status Summary

The spec was distilled from a working implementation; all rules and invariants are anchored in code.

| Rule / Invariant | Status | Code anchor |
|---|---|---|
| **EnqueueSteeringMessage** (REQ-STEER-001) | ✅ Complete | `crates/phoenix-ide/src/runtime.rs:1140-1175` (`enqueue_steer_message`); persist-before-channel ordering at `:1157-1173` |
| **DrainOnIdleEntry** (REQ-STEER-002) | ✅ Complete | `crates/phoenix-ide/src/runtime/executor.rs:711-737` — entering-idle detector + `Vec::remove(0)` + persist after |
| **CancelSteeringMessage** (REQ-STEER-003) | ✅ Complete | `runtime/executor.rs:552` (in-memory removal); HTTP handler at `api/handlers.rs:96` (`cancel_steering_message` route) |
| **TerminalConversationRejectsSteer** (REQ-STEER-004) | ✅ Complete | Send path checks `is_terminal` before any queue logic runs |
| **DepthNonNegative** (entity invariant) | ✅ By construction | `entries: Vec<SteerEntry>` — `len()` is `usize`, structurally non-negative |
| **UniqueMessageIds** (entity invariant) | ✅ Enforced | `enqueue_steer_message` does not allow duplicate IDs; client-generated UUIDs |
| **OneQueuePerConversation** (invariant) | ✅ By construction | Queue is a column on `conversations`; one row per conversation |
| **IdempotentCancel** (surface guarantee) | ✅ Complete | Cancel handler returns 200 whether or not the entry was present |
| **SteerMessageQueuedAck** (surface guarantee) | ✅ Complete | `SseWireEvent::SteerMessageQueued` at `api/wire.rs:323`, `api/sse.rs:231`; client subscribes at `ui/src/hooks/useConnection.ts:437-441` |
| **PersistenceBeforeResponse** (surface guarantee) | ✅ Complete | Both enqueue (P1) and cancel (P2) write to DB before HTTP response |
| **DirectSendTransparency** (surface guarantee) | ✅ Complete | When `not is_busy`, message bypasses queue entirely; SSE `steer_message_queued` event is not emitted |
| **Crash recovery** | ✅ Complete | DB column `steering_queue TEXT` (`db.rs:176`); loaded via `with_steering_queue` on executor startup (`runtime.rs:1007`) |

**Progress:** All four rules and all invariants/guarantees implemented.

## Doc Debt

The `.allium` file's header references `REQ-STEER-001 through REQ-STEER-007`, but only four named rules (001..004) appear in the file. Requirements 005..007 likely correspond to invariants and surface guarantees but the mapping is implicit. A small follow-up: number the invariants and surface guarantees explicitly, or update the header to say "001..004" plus the named invariants. Not blocking — code is correct; documentation could be tighter.

## Cross-Spec Relationships

- **`specs/bedrock/`**: bedrock is the source of `core_status` transitions that trigger `DrainOnIdleEntry`. Steering is orthogonal to bedrock — the state machine has no awareness of the queue.
- **`specs/inline-references/`**: `@file` expansion runs as part of the same `expand()` call used by the normal send path, populating `llm_text` and `skill_invocation` on the `SteerEntry`.
- **`specs/sse_wire/`**: `steer_message_queued` is one of the SSE event types; persistence-before-broadcast applies (the queue is persisted before the event is emitted, mirroring `PersistBeforeBroadcast`).
