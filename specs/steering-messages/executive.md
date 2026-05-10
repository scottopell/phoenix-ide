# Steering Messages — Executive Summary

## Requirements Summary

A steering message is a user-directed instruction sent to a conversation that is currently *busy* — running an LLM turn, waiting on tools, or in any non-idle state. Without this feature, the API would reject the send (`agent_busy`) and the user would have to wait for the turn to finish before redirecting it. The steering queue accepts the message anyway, persists it, and delivers it as a normal `UserMessage` at the next drain point. From the LLM's perspective, drained messages are indistinguishable from regular sends; from the user's perspective, the conversation never refuses input even when it's "thinking".

The queue is FIFO, survives Phoenix restarts (persisted in `conversations.steering_queue` JSON column), and supports cancellation by `message_id`. Cancellation is idempotent — cancelling an already-drained or never-existed entry is a successful 200 OK. Terminal conversations refuse steering entirely; the queue is bounded by absence rather than a numeric cap.

**Drain semantics (REQ-STEER-002, REQ-STEER-005):** all queued entries drain together as a batch at each hook point — the queue is never partially drained. There are two hook points:

- **Turn-end drain (REQ-STEER-002):** when the conversation transitions into `Idle` from any other state, every queued entry is delivered and the conversation re-enters `LlmRequesting` to respond.
- **Mid-turn drain (REQ-STEER-005):** when the conversation transitions into `LlmRequesting` from `ToolExecuting` or `AwaitingSubAgents` (i.e., between tool rounds within a single user turn), every queued entry is persisted into the transcript. The `RequestLlm` for the next round was already dispatched by the prior transition, so the in-flight LLM call does not see these freshly-drained messages — they land in the LLM call that follows it (the round after the upcoming `LlmResponse`).

Drain-all (rather than one-at-a-time) and the mid-turn hook are deliberate: the user's intent in queuing a steer mid-turn is "interject as soon as possible", not "let the model finish a full round-trip per steer." Coherence of multi-message bursts is a user concern — the queue makes no attempt to pace delivery.

## Technical Summary

The queue is **executor-owned, with a typed bedrock event as the delivery contract**. `Event::SteerMessage` and `Event::CancelSteerMessage` are intercepted by the executor and never reach the bedrock transition function — bedrock has no awareness of the queue's *existence*. But the executor delivers drained entries to bedrock via `Event::SteerDrainedUserMessages { entries: Vec<SteerEntry> }` (and its `CoreEvent` mirror), which bedrock handles with explicit transition arms. This keeps mid-turn user-message insertion a typed part of bedrock's contract rather than a side-channel write.

**Single-writer property:** all transcript persistence still flows through `Effect::persist_user_message → execute_effect → storage.add_message_with_seq`. The bedrock arms emit one `Effect::persist_user_message` per drained entry; the executor's effect loop runs them serially, each allocating a sequential `sequence_id`. There is no parallel/sidecar persistence path for steering — the queue feeds the same single writer the normal send path uses.

**Bedrock arms accept `SteerDrainedUserMessages`:**
- From `Idle` (turn-end drain): transitions to `LlmRequesting { attempt: 1 }`, emits one `persist_user_message` per entry in FIFO order, then `PersistState`, then `ClearSteeringQueue`, then `NotifyLlmRequesting`, and finally `RequestLlm`.
- From `LlmRequesting` (mid-turn drain): stays in `LlmRequesting`, emits one `persist_user_message` per entry in FIFO order, then `PersistState`, then `ClearSteeringQueue`. **No new `RequestLlm`** — the in-flight one races with persistence and does not see the freshly-persisted messages; the next LLM call (after the upcoming tool round) will.
- All other source states: rejected as `InvalidTransition`. The executor only emits the event at the two hook points above, so other source states are unreachable in normal flow.

Three persistence-ordering rules are load-bearing:

- **P1 (enqueue):** `enqueue_steer_message` reads the current queue, appends the new entry, writes the updated queue to the DB, *then* sends `Event::SteerMessage` to the executor channel. The HTTP response returns only after the DB write succeeds. A crash between acceptance and executor processing does not lose the entry.
- **P2 (cancel):** the cancel handler updates the DB first, *then* sends `Event::CancelSteerMessage` to the live executor (if running) which removes the matching entry from in-memory state without a further DB write.
- **Drain:** at a hook point with a non-empty queue, the executor calls `std::mem::take` to atomically swap the entire `Vec<SteerEntry>` out of in-memory state and emits `Event::SteerDrainedUserMessages { entries }`. Bedrock's arm produces N `Effect::persist_user_message` effects in FIFO order, then `PersistState`, then `Effect::ClearSteeringQueue` (which writes the empty queue to the DB). The effect loop runs them serially: each persist allocates a sequential `sequence_id`. **The DB queue is cleared only AFTER all persist effects succeed.** A crash anywhere in this window leaves the DB queue populated; on restart the queue reloads with all entries, the next drain hook re-fires `SteerDrainedUserMessages`, and `Effect::PersistMessage` skips already-persisted entries via a `storage.message_exists(message_id)` precheck. The combination of "clear-after-persist" + idempotent persist provides at-least-once delivery without double-insertion.

FIFO order is preserved because the queue is a `Vec` with append-at-tail enqueue and whole-vec take at drain; the bedrock arm iterates the drained `entries` in order when emitting persist effects.

The persisted JSON array round-trips through `db.update_steering_queue` (`crates/phoenix-ide/src/db.rs:576`) and is loaded into the executor on startup via `with_steering_queue` (`runtime.rs:1007`). The queue is therefore live immediately after a Phoenix restart — no warm-up.

## Status Summary

The spec was distilled from a working implementation; all rules and invariants are anchored in code.

| Rule / Invariant | Status | Code anchor |
|---|---|---|
| **EnqueueSteeringMessage** (REQ-STEER-001) | Complete | `crates/phoenix-ide/src/runtime.rs:1140` (`enqueue_steer_message`); persist-before-channel ordering at `:1157-1175` |
| **DrainOnIdleEntry** (REQ-STEER-002) | Complete | Detector: `crates/phoenix-ide/src/runtime/executor.rs:734-764` (`maybe_drain_steering_queue`, `entering_idle` branch); drain-all uses `std::mem::take` at `:749`. Bedrock arm: `crates/phoenix-ide/src/state_machine/transition.rs:427-439` (`(Idle, SteerDrainedUserMessages)` — emits N `persist_user_message` then `RequestLlm`). |
| **CancelSteeringMessage** (REQ-STEER-003) | Complete | `runtime/executor.rs:552` (in-memory removal); HTTP handler at `api/handlers.rs:1844` (`cancel_steering_message`); route registered at `api/handlers.rs:98` |
| **TerminalConversationRejectsSteer** (REQ-STEER-004) | Complete | Send path checks `is_terminal` before any queue logic runs |
| **DrainOnEnteringLlmRound** (REQ-STEER-005) | Complete | Detector: `runtime/executor.rs:737-741` (`entering_llm_requesting_from_tool_round`). Bedrock arm: `state_machine/transition.rs:441-455` (`(LlmRequesting, SteerDrainedUserMessages)` — emits N `persist_user_message` then `PersistState`; no `RequestLlm`). |
| **DepthNonNegative** (entity invariant) | Complete by construction | `entries: Vec<SteerEntry>` — `len()` is `usize`, structurally non-negative |
| **UniqueMessageIds** (entity invariant) | Complete | `enqueue_steer_message` does not allow duplicate IDs; client-generated UUIDs |
| **OneQueuePerConversation** (invariant) | Complete by construction | Queue is a column on `conversations`; one row per conversation |
| **IdempotentCancel** (surface guarantee) | Complete | Cancel handler returns 200 whether or not the entry was present |
| **SteerMessageQueuedAck** (surface guarantee) | Complete | `SseWireEvent::SteerMessageQueued` emitted from `runtime/executor.rs:542`; client subscribes at `ui/src/hooks/useConnection.ts` |
| **PersistenceBeforeResponse** (surface guarantee) | Complete | Both enqueue (P1) and cancel (P2) write to DB before HTTP response |
| **MidTurnDrainSemantics** (surface guarantee) | Complete | Bedrock arm at `state_machine/transition.rs:443` omits `RequestLlm` — in-flight call cannot see new messages by construction |
| **DirectSendTransparency** (surface guarantee) | Complete | When `not is_busy`, message bypasses queue entirely; SSE `steer_message_queued` event is not emitted |
| **Crash recovery** | Complete | DB queue cleared AFTER persist effects via `Effect::ClearSteeringQueue` (`runtime/executor.rs:1343-1352`). `Effect::PersistMessage` is idempotent via `storage.message_exists` precheck (`runtime/executor.rs:1128-1142`). Queue loaded on startup via `with_steering_queue` (`runtime.rs:1007`). On crash mid-drain: queue still has entries, restart loads them, next drain re-fires the event, already-persisted entries are skipped. At-least-once delivery without double-insertion. |

**Progress:** All five rules (REQ-STEER-001..005) and all invariants/guarantees implemented.

## Doc Debt

None outstanding. The previous doc debt — unmapped IDs 005..007 — is resolved by the explicit assignment in this revision:

- **REQ-STEER-005** is the named rule `DrainOnEnteringLlmRound` (mid-turn drain). It was added when drain-all + mid-turn delivery were implemented.
- The remaining slots (006, 007) previously suspected to map to invariants/guarantees are dropped. Invariants and surface guarantees in this spec are referenced by name (e.g., `DepthNonNegative`, `IdempotentCancel`) rather than numeric ID, matching the convention in `specs/bedrock/`. The `.allium` header now reads `REQ-STEER-001 through REQ-STEER-005`.

## Cross-Spec Relationships

- **`specs/bedrock/`**: bedrock is the source of `core_status` transitions that trigger both drain hooks (entering `Idle`, and entering `LlmRequesting` from `ToolExecuting` / `AwaitingSubAgents`). Steering remains orthogonal to bedrock in the sense that bedrock does not read queue state — but bedrock has explicit transition arms for `(Idle, SteerDrainedUserMessages)` and `(LlmRequesting, SteerDrainedUserMessages)`, so the *delivery mechanism* is now a typed part of bedrock's contract rather than a side-channel. This trade-off was made to preserve the single-writer transcript-persistence property (CLAUDE.md "no parallel representations of the same semantic value"): instead of the executor calling into storage directly to insert user messages mid-turn, it routes through bedrock so that all `UserMessage` transcript writes flow through one `Effect::persist_user_message` path.
- **`specs/inline-references/`**: `@file` expansion runs as part of the same `expand()` call used by the normal send path, populating `llm_text` and `skill_invocation` on the `SteerEntry`. Expansion happens once at enqueue time; drained entries are not re-expanded.
- **`specs/sse_wire/`**: `steer_message_queued` is one of the SSE event types; persistence-before-broadcast applies (the queue is persisted before the event is emitted, mirroring `PersistBeforeBroadcast`). Drained entries surface to clients through the normal `user_message` SSE path emitted by `persist_user_message`, not via a steering-specific event.
