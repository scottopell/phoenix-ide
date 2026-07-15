# Reconcile stale accepted messages and awaiting-LLM UI state

Investigate and fix the production failure mode where server-accepted steering messages remain rendered as local hourglass bubbles while the conversation stays at `Preparing request…`, even though later conversation activity appears to show that the messages were handled. In the observed incident, both the queued bubbles and phase eventually reconciled after leaving and returning to the conversation, so treat this as a liveness/reconciliation defect rather than assuming durable queue corruption.

## Evidence and hypotheses

- `useMessageQueue` persists accepted entries in browser localStorage and only hides them when `atom.messages` contains the exact client-generated `localId` as an authoritative `message_id`.
- A non-steering POST optimistically sets `awaiting_llm`; only a later authoritative SSE/init state replaces it.
- Steering drain preserves each entry's original message ID, persists all user messages, persists the new state, and then clears matching durable queue rows.
- The normative `user_message_queue.allium` contract requires accepted messages not to remain silently pending forever and defines a surfaced `recoverable_inconsistency` state, but the TypeScript queue implementation has no such runtime state or reconciliation path.
- The incident self-cleared, which is consistent with a delayed/missed SSE projection or an init/history snapshot eventually catching up. It does not prove whether the delay originated in the executor, persistence/event publication, SSE reconnect/cursor handling, or browser state.

## Plan

1. Add correlation-quality diagnostics around one steering entry ID across:
   - POST acceptance and durable enqueue;
   - executor drain start/end;
   - user-message persistence and duplicate handling;
   - durable steering-row removal;
   - request/state transitions;
   - SSE message/state publication and reconnect init boundaries.
   Logs must make it possible to distinguish a server-side delayed drain from a client that missed authoritative events.
2. Build a deterministic regression scenario with multiple messages queued during a busy turn. Exercise drain, LLM reply completion, SSE disconnect/reconnect or page reload, and navigation away/back. Assert:
   - original client message IDs appear in authoritative history;
   - durable steering rows are removed only after persistence succeeds;
   - the final authoritative phase is idle;
   - no accepted local entry remains rendered once history contains its ID.
3. Audit the init/latest-history and SSE cursor merge path for a window where authoritative drained messages or the final idle event can be absent/rejected despite later events being visible. Fix the root cause if reproduced.
4. Implement the spec's liveness fallback: accepted `pending`/`steering_queued` entries that are causally proven stale must not spin forever. Trigger an authoritative reconciliation fetch and, if the exact ID is still absent, surface a typed recoverable inconsistency with explicit retry/dismiss behavior rather than silently resending an already accepted steering message.
5. Ensure localStorage is compacted after authoritative reconciliation so terminal entries do not accumulate indefinitely, while retaining message-ID-based rendering as the correctness join.
6. Add focused Rust and React tests, update the steering/user-message-queue executive coverage, validate any touched Allium specs, and run `./dev.py check`.

## Acceptance criteria

- A server-accepted queued message cannot remain an unbounded silent hourglass after later turn completion or authoritative idle evidence.
- Reload/reconnect/navigation converges to the same authoritative history and idle phase without duplicate sends.
- Recovery never automatically re-POSTs a message already accepted as steering.
- Diagnostics identify every lifecycle stage by conversation ID and message ID without logging message contents.
- Existing persistence-before-clear crash safety and concurrent-enqueue preservation remain intact.
