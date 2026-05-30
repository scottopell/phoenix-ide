# SSE delivery-order race vs client `applyIfNewer` high-water-mark

> Spun out of a perf-PR investigation. File:line citations verified against
> real bytes at filing time (runtime.rs ~2438 lines), but the file churns —
> re-confirm coordinates before editing.

## The dependency

The client conversation atom (`ui/src/conversation/atom.ts`, `applyIfNewer`)
keeps a SINGLE global `lastSequenceId` high-water mark per conversation and
DROPS any SSE event whose `sequence_id <= lastSequenceId` as a "replay".
This is intentional and spec-compliant — see `specs/sse_wire/` and
`specs/conversation_atom/conversation_atom.allium` (the two ordered guards:
`isStaleEpoch` then `applyIfNewer`). All event types share ONE monotonic
per-conversation counter (`SseBroadcaster::next_seq` = atomic `fetch_add`),
and every wire variant except per-subscriber `init` carries a `sequence_id`.

This dedup is correct IFF events are DELIVERED to each subscriber in
strictly-increasing seq order. That in-order-delivery assumption is
load-bearing and currently unenforced for the live broadcast channel.

## The gap

`SseBroadcaster::send_seq` (crates/phoenix-ide/src/runtime.rs:529) does:

    pub fn send_seq(&self, build: impl FnOnce(i64) -> SseEvent) -> Result<usize, ()> {
        let seq = self.next_seq();        // atomic alloc, NO lock held
        let event = build(seq);
        self.send_with_ring(event, seq, RingOp::Append)
    }

…and `send_with_ring` (runtime.rs:507) locks the ring ONLY to append, then
calls `self.tx.send(event)` OUTSIDE any lock. Allocate+broadcast is therefore
not atomic. Two tasks sending concurrently to the same broadcaster can
interleave: task B allocates the higher seq but reaches `tx.send()` first, so
the channel delivers high-then-low. The client drops the lower-seq event =
SILENT DATA LOSS.

The RING is protected (append drops entries <= anchor; `snapshot()` sorts on
read — runtime.rs:252-266 / 329-336, plus the
`replay_ring_snapshot_sorts_out_of_order_appends` test at ~2320). But the
LIVE broadcast-channel delivery order is NOT protected, and no invariant or
test asserts it. The spec acknowledges ring-append interleave; it is silent
on live channel delivery order.

## Why it mostly doesn't bite today (do NOT treat as "safe")

On the LLM streaming path the producers are effectively serial:
- During `complete_streaming(&request, &chunk_tx).await` (executor.rs:1934)
  the main turn task is PARKED. The token-forwarder task (spawned at
  executor.rs:1837) is the sole sender — and it sends `LlmFirstByte` + `Token`
  + `RateLimitSnapshot` serially from a single `recv` loop, so they are in
  order relative to each other.
- A drain barrier (`drop(chunk_tx)` + awaiting the forwarder handle —
  documented at executor.rs:1825-1831) happens-before the main task emits the
  terminal `state_change` / persisted `Message`, so tokens (lower seq) fully
  precede the finalize (higher seq). This is the explicit fix for the
  "repeated message" / phantom-streaming-buffer bug.

The real residual concurrent producer is the browser-session lifecycle bridge
(`start_browser_lifecycle_bridge`, runtime.rs:983; the `BrowserSessionState`
`send_seq` site at runtime.rs:1032-1037). It runs on its own spawned task,
fans out to each live conversation's shared `broadcast_tx`, and is NOT gated
by the LLM await or the drain barrier. So it CAN call `send_seq` on a
broadcaster concurrently with that conversation's turn task.

Why impact is low today: `BrowserSessionState { active: bool }` events are
rare (session create / kill / idle-cleanup), idempotent, and self-heal on the
next lifecycle edge or the next reconnect's `init` snapshot (which carries the
authoritative `browser_session_active`). A dropped toggle is corrected almost
immediately. But it is a real, unsynchronized concurrent producer on the same
broadcaster — the assumption "one serial sender per broadcaster" is already
false.

## Why it still matters

1. Latent correctness gap: the design relies on an invariant ("in-order live
   delivery") that nothing enforces. The moment ANY new concurrent producer
   is added to the broadcaster — or `BrowserSessionState` gains a
   non-idempotent payload, or toggles get frequent — this becomes a live drop
   bug.
2. Same shape as the "streaming-finalizes-then-disappears" / "messages vanish"
   family (cf. task 02679) if the serial assumptions ever break.

## Possible fixes (decide later)

- **A — per-broadcaster send Mutex** across allocate+build+`tx.send` so
  channel send order always matches seq order. Simplest; adds lock contention
  on the hot token path (every token takes the lock). Measure the
  token-throughput cost first.
- **B — single-writer funnel**: route ALL sends for a conversation through one
  mpsc consumed by a single task that allocates+sends serially. Removes the
  race by construction; larger refactor.
- **C — tolerate bounded reorder on the client**: small seq-keyed reorder
  buffer in `applyIfNewer`. Pushes complexity to the client and weakens the
  clean total-order model. Least preferred.

Whichever is chosen, also add:
1. An invariant in `specs/sse_wire/sse_wire.allium` asserting live-channel
   delivery order matches seq order (or explicitly documenting a tolerated
   reorder window and why the client survives it).
2. A test that exercises CONCURRENT `send_seq` and asserts CHANNEL delivery
   order — current tests only assert ring sort order, not channel order.

## Adjacent cleanup while here

The executor streaming block (crates/phoenix-ide/src/runtime/executor.rs,
~1825-1896) carries long rationale comments about the drain barrier and
ordering. Once the ordering guarantee is made structural, migrate that
rationale into the Allium spec (`@guidance` / invariant) and trim the prose
per the repo's "comments are local facts, not distributed specifications"
rule.

## Provenance / confidence

- HIGH confidence: the `applyIfNewer` single-high-water-mark design and its
  spec-compliance; the single shared monotonic counter; the streaming path
  being serialized by the drain barrier; the browser bridge being an
  unsynchronized concurrent producer on the shared broadcaster. All verified
  against source this session.
- The exact severity hinges on how often the browser bridge races a turn
  task in practice — low today, but unbounded as producers grow. Treat line
  numbers as accurate-at-filing; re-confirm before patching.
