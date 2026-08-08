# Steering Messages — Executive Summary

## Requirements Summary

A steering message is a user-directed instruction sent to a conversation that is currently *busy* — running an LLM turn, waiting on tools, or in any non-idle state. Without this feature, the API would reject the send (`agent_busy`) and the user would have to wait for the turn to finish before redirecting it. The steering queue accepts the message anyway, persists it, and delivers it as a normal `UserMessage` at the next drain point. From the LLM's perspective, drained messages are indistinguishable from regular sends; from the user's perspective, the conversation never refuses input even when it's "thinking".

The queue is FIFO, survives Phoenix restarts (persisted in the `steering_messages` table — one row per pending entry, ordered by `ordinal` — with `steering_message_files` / `steering_message_images` holding each entry's attachments), and supports cancellation by `message_id`. Cancellation is idempotent — cancelling an already-drained or never-existed entry is a successful 200 OK. Terminal conversations refuse steering entirely; the queue is bounded by absence rather than a numeric cap.

**Drain semantics (REQ-STEER-002, REQ-STEER-005):** all queued entries drain together as a batch at each hook point — the queue is never partially drained. There are two hook points:

- **Turn-end drain (REQ-STEER-002):** when the conversation transitions into `Idle` from any other state, every queued entry is delivered and the conversation re-enters `LlmRequesting` to respond. If executor-channel delivery races that transition, receiving the steer while already `Idle` runs the same drain immediately.
- **Mid-turn drain (REQ-STEER-005):** when the conversation transitions into `LlmRequesting` from `ToolExecuting` or `AwaitingSubAgents` (i.e., between tool rounds within a single user turn), every queued entry is persisted before the deferred `RequestLlm` starts, so that next call sees the newly drained messages.

Drain-all (rather than one-at-a-time) and the mid-turn hook are deliberate: the user's intent in queuing a steer mid-turn is "interject as soon as possible", not "let the model finish a full round-trip per steer." Coherence of multi-message bursts is a user concern — the queue makes no attempt to pace delivery.

## Technical Summary

The queue is **executor-owned, with a typed bedrock event as the delivery contract**. `Event::SteerMessage` and `Event::CancelSteerMessage` are intercepted by the executor and never reach the bedrock transition function — bedrock has no awareness of the queue's *existence*. But the executor delivers drained entries to bedrock via `Event::SteerDrainedUserMessages { entries: Vec<SteerEntry> }` (and its `CoreEvent` mirror), which bedrock handles with explicit transition arms. This keeps mid-turn user-message insertion a typed part of bedrock's contract rather than a side-channel write.

**Reducer authority:** the bedrock arm owns the exact ordered message batch, next state, and typed post-commit action. It emits one `CommitSteeringDrain` effect. The executor allocates sequential message IDs for that batch and supplies the reducer-owned state to one specialized storage transaction.

**Bedrock arms accept `SteerDrainedUserMessages`:**
- From `Idle` (turn-end drain): transitions to `LlmRequesting { attempt: 1 }` and emits one `CommitSteeringDrain` with the FIFO messages and `StartLlmAndNotifyState`.
- From `LlmRequesting` (mid-turn drain): stays in `LlmRequesting` and emits one `CommitSteeringDrain` with the FIFO messages and `ContinueExistingLlm`. **No new `RequestLlm`** — the executor runs the original transition's deferred request after the commit.
- All other source states: rejected as `InvalidTransition`. The executor emits the event only at the two transition hook points or when steer delivery arrives after the idle hook, so other source states are unreachable in normal flow.

The executor also closes the acceptance-to-idle race: if `Event::SteerMessage` arrives after the conversation has already reached `Idle`, it immediately takes the queue and routes `SteerDrainedUserMessages` through the normal idle arm. An accepted prompt therefore cannot remain parked until an unrelated later turn.

The send boundary serializes admission per conversation before observing state or performing slow payload validation. A busy send therefore reserves its place before a later idle send can overtake it. A non-empty durable steering queue remains an acceptance fence even if the live state is momentarily `Idle`, and the state is checked again before persistence so terminal or interaction-blocked conversations cannot accumulate undeliverable messages. Unrelated conversations use independent admission gates.

Three persistence-ordering rules are load-bearing:

- **P1 (enqueue):** `enqueue_steer_message` atomically appends one normalized queue row and obtains its committed FIFO position, broadcasts a live-only `steer_message_queued` projection, and then sends `Event::SteerMessage` to the executor channel. The HTTP response returns only after the DB write succeeds. A failed append emits no success projection, and a crash between acceptance and executor processing does not lose the entry.
- **P2 (cancel):** under the same per-conversation admission gate used by sends, the cancel handler deletes the exact queue row and learns whether a row was removed. Only a successful removal broadcasts the live-only `steer_message_cancelled` projection. Runtime startup, queue mutations, and queue consumption share a narrow projection fence. The executor reloads durable entries before each drain and holds the fence through persistence and removal, so cancellation and delivery have one durable winner: a committed cancellation is filtered before use, while a completed drain makes the later delete report no removal. Cancelling an absent row remains an idempotent 200 without publishing a false cancellation.
- **Drain (inline processing):** at a hook point with a non-empty queue, the executor calls `std::mem::take` to atomically swap the entire `Vec<SteerEntry>` out of in-memory state and synthesizes `Event::SteerDrainedUserMessages { entries }`. The event is processed **inline within the same `apply_transition_result` call** rather than queued for the outer event loop. The executor:
  1. Defers any `Effect::RequestLlm` from the original transition's effect list.
  2. Runs the remaining original effects.
  3. Calls `transition(state, context, SteerDrainedUserMessages)` directly; bedrock emits one `CommitSteeringDrain` containing the FIFO batch and post-commit action.
  4. Runs the deferred `RequestLlm` (if any). The spawned LLM task reads a DB that already contains the steered messages, so the in-flight call sees the steers deterministically — no race.

  `Database::commit_steering_drain` inserts every supplied message, persists the supplied state, and deletes only the supplied queue identities in one transaction. Failure rolls back all three, publishes nothing, starts no LLM, and exits the runtime so its successor reconstructs from database truth. Exact deletes preserve a concurrent enqueue. Startup routes durable pending entries through the same reducer and transaction. A matching pre-existing transcript row is reported as `LegacyAlreadyMaterialized` and consumed without duplication; mismatched data or a stale queue identity aborts once and leaves database truth intact for a later runtime reconstruction.

FIFO order is preserved because the database atomically assigns each append its committed `ordinal`; the runtime projection publishes that same zero-based position. The executor loads and drains entries in ordinal order, and the bedrock arm iterates the drained `entries` in that order when emitting persist effects.

New admissions use `Database::append_steering_entry`, which inserts one normalized entry and returns its committed position in one transaction; cancellation uses an exact-row delete. On startup the executor is seeded via `with_steering_queue`, sourced from a final `Database::get_steering_queue` read under the same narrow projection fence used by queue mutations. Before each drain, `StateStore::load_steering_entries` refreshes that projection under the fence, which remains held through `commit_steering_drain`. The queue is therefore live immediately after restart and cannot deliver a durably cancelled startup entry.

## Status Summary

The spec was distilled from a working implementation; all rules and invariants are anchored in code.

| Rule / Invariant | Status | Code anchor |
|---|---|---|
| **EnqueueSteeringMessage** (REQ-STEER-001) | Complete | `crates/phoenix-ide/src/runtime.rs:1140` (`enqueue_steer_message`); persist-before-channel ordering at `:1157-1175` |
| **DrainOnIdleEntry** (REQ-STEER-002) | Complete | `ConversationRuntime::maybe_drain_steering_queue`, `run_effects_with_inline_drain`, and the reducer's `(Idle, SteerDrainedUserMessages)` arm. |
| **DrainWhenSteerArrivesAfterIdle** | Complete | `ConversationRuntime::process_event` converts a late `SteerMessage` into `SteerDrainedUserMessages` when the executor is already idle, then uses the normal bedrock arm. |
| **CancelSteeringMessage** (REQ-STEER-003) | Complete | `runtime/executor.rs:552` (in-memory removal); HTTP handler at `api/handlers.rs:1844` (`cancel_steering_message`); route registered at `api/handlers.rs:98` |
| **TerminalConversationRejectsSteer** (REQ-STEER-004) | Complete | Send path checks `is_terminal` before any queue logic runs |
| **DrainOnEnteringLlmRound** (REQ-STEER-005) | Complete | `ConversationRuntime::maybe_drain_steering_queue`, `run_effects_with_inline_drain`, and the reducer's `(LlmRequesting, SteerDrainedUserMessages)` arm. |
| **DepthNonNegative** (entity invariant) | Complete by construction | `entries: Vec<SteerEntry>` — `len()` is `usize`, structurally non-negative |
| **UniqueMessageIds** (entity invariant) | Complete | `enqueue_steer_message` does not allow duplicate IDs; client-generated UUIDs |
| **OneQueuePerConversation** (invariant) | Complete by construction | `steering_messages` rows are keyed by `conversation_id` with a per-conversation `ordinal` (`UNIQUE(conversation_id, ordinal)`); the queue is the set of rows for a conversation, so it is one-to-one by construction |
| **IdempotentCancel** (surface guarantee) | Complete | Cancel handler returns 200 whether or not the entry was present |
| **SteerMessageQueuedAck** (surface guarantee) | Complete | `RuntimeManager::enqueue_steer_message` broadcasts the renderable durable entry; `useConnection` dispatches it into `ConversationAtom.steeringMessages` |
| **Steering reconnect/cancel projection** | Complete | `SseEvent::Init.steering_messages` restores the durable queue; `SteerMessageCancelled` removes cancelled entries; normal `Message` delivery removes the matching queue identity |
| **PersistenceBeforeResponse** (surface guarantee) | Complete | Both enqueue (P1) and cancel (P2) write to DB before HTTP response |
| **PendingQueueFencesDirectAcceptance** (surface guarantee) | Complete | `SendChatApplicationService::send` holds the conversation-scoped admission gate across state observation, validation, queue/active-owner inspection, and direct-or-steering persistence. |
| **MidTurnDrainSemantics** (surface guarantee) | Complete | The reducer selects `ContinueExistingLlm`; `run_effects_with_inline_drain` commits before running the original deferred `RequestLlm`. |
| **DirectSendTransparency** (surface guarantee) | Complete | When `not is_busy`, message bypasses queue entirely; SSE `steer_message_queued` event is not emitted |
| **EnqueueDuringDrainPreserved** (surface guarantee) | Complete | `Database::commit_steering_drain` deletes only the reducer-supplied identities; later or concurrent queue rows survive. |
| **Crash recovery** | Complete | The specialized transaction makes new partial drains impossible. Startup uses the same reducer-owned atomic drain. Exact matching legacy transcript rows are consumed once; stale or mismatched partials fail without mutation or an in-process retry loop. |

**Progress:** All five rules (REQ-STEER-001..005) and all invariants/guarantees implemented.

## Doc Debt

None outstanding. The previous doc debt — unmapped IDs 005..007 — is resolved by the explicit assignment in this revision:

- **REQ-STEER-005** is the named rule `DrainOnEnteringLlmRound` (mid-turn drain). It was added when drain-all + mid-turn delivery were implemented.
- The remaining slots (006, 007) previously suspected to map to invariants/guarantees are dropped. Invariants and surface guarantees in this spec are referenced by name (e.g., `DepthNonNegative`, `IdempotentCancel`) rather than numeric ID, matching the convention in `specs/bedrock/`. The `.allium` header now reads `REQ-STEER-001 through REQ-STEER-005`.

## Cross-Spec Relationships

- **`specs/bedrock/`**: bedrock is the source of `core_status` transitions that trigger both drain hooks. Steering queue storage remains orthogonal to bedrock, while the explicit `SteerDrainedUserMessages` arms own the exact transcript batch, next state, and post-commit LLM action.
- **`specs/inline-references/`**: `@file` expansion runs as part of the same `expand()` call used by the normal send path, populating `llm_text` and `skill_invocation` on the `SteerEntry`. Expansion happens once at enqueue time; drained entries are not re-expanded.
- **`specs/sse_wire/`**: `init` carries the authoritative durable steering queue, while `steer_message_queued` and `steer_message_cancelled` are live-only latency projections at the current stream watermark. They do not enter replay; reconnect reconstructs the queue from the database. Persistence-before-broadcast applies. Drained entries surface through the normal `message` path and remove the matching queued identity client-side.
