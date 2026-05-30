# SSE delivery-order race vs client `applyIfNewer` high-water-mark

> Filed from a perf-PR investigation. The file:line citations below were
> gathered through reads that were intermittently unreliable in that session —
> RE-VERIFY every coordinate against the real bytes before acting. The
> high-level mechanism is well-established; the exact line numbers may drift.

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
load-bearing and currently unenforced.

## The gap

`SseBroadcaster::send_seq` (crates/phoenix-ide/src/runtime.rs) does roughly:

    let seq = self.next_seq();   // atomic alloc, NO lock held
    let event = build(seq);
    self.send_with_ring(...)     // briefly locks the ring to append,
                                 // then tx.send() OUTSIDE any lock

Allocate+broadcast is NOT atomic. Two tasks sending concurrently to the same
broadcaster can interleave: task B allocates the higher seq but reaches
`tx.send()` first, so the channel delivers high-then-low. The client then
drops the lower-seq event = SILENT DATA LOSS.

The RING is protected (append drops entries <= anchor; `snapshot()` sorts on
read — runtime.rs ~252-266 / ~329-336, plus the
`replay_ring_snapshot_sorts_out_of_order_appends` test). But the LIVE
broadcast-channel delivery order is NOT protected, and no invariant or test
asserts it. The spec acknowledges ring-append interleave; it is silent on
live channel delivery order.

## Why it mostly doesn't bite today (do NOT treat as "safe")

On the LLM streaming path the producers are effectively serial:
- During `complete_streaming().await` the main turn task is PARKED, so the
  token-forwarder task is the sole sender — and it sends tokens serially in a
  single `recv` loop, so tokens are in order relative to each other.
- A drain join (`drop(chunk_tx); token_forwarder.await`) happens-before the
  main task emits the terminal `state_change` / persisted message, so tokens
  (lower seq) fully precede the finalize (higher seq).

The real residual concurrent producer is the browser-session lifecycle
bridge (runtime.rs — the `BrowserSessionState` `send_seq` site, ~line 1033).
It runs on its own task, shares the same per-conversation broadcaster
(`broadcaster_for`), and is NOT gated by the LLM await or the drain join.
Today its events are rare (create/destroy), idempotent booleans that
self-heal on the next lifecycle event or the next reconnect's init snapshot,
so concrete impact is low. An in-code comment already flags this and points
at "the SSE-ordering task" (this one).

## Why it still matters

1. Latent correctness gap: the design relies on an invariant ("in-order
   delivery") that nothing enforces. The moment ANY new concurrent producer
   is added to the broadcaster — or browser-state toggles become frequent or
   carry non-idempotent payload — this becomes a live drop bug.
2. It is the same shape as the "streaming-finalizes-then-disappears" /
   "messages vanish" family (cf. task 02679) if the serial assumptions break.

## Possible fixes (decide later)

- **A — per-broadcaster send Mutex** across allocate+build+`tx.send` so
  channel send order always matches seq order. Simplest; adds lock
  contention on the hot token path (every token takes the lock). Measure
  the token-throughput cost first.
- **B — single-writer funnel**: route ALL sends for a conversation through
  one mpsc consumed by a single task that allocates+sends serially. Removes
  the race by construction; larger refactor.
- **C — tolerate bounded reorder on the client**: small seq-keyed reorder
  buffer in `applyIfNewer`. Pushes complexity to the client and weakens the
  clean total-order model. Least preferred.

Whichever is chosen, also add:
1. An invariant in `specs/sse_wire/sse_wire.allium` asserting live-channel
   delivery order matches seq order (or documenting a tolerated reorder
   window).
2. A test that exercises CONCURRENT `send_seq` and asserts CHANNEL delivery
   order — current tests only assert ring sort order, not channel order.

## Adjacent cleanup while here

The executor streaming block (crates/phoenix-ide/src/runtime/executor.rs,
~1825-1896) carries stream-of-consciousness comments ("actually no — see
below", "NO: ... NOT guaranteed by construction here") that violate the
repo's "comments are local facts, not distributed specifications" rule. Once
the ordering guarantee is made structural, migrate that rationale into the
Allium spec and delete the hedging prose.

## Provenance / confidence

- HIGH confidence: the `applyIfNewer` single-high-water-mark design, its
  spec-compliance, the single shared monotonic counter, and that the
  streaming path is serial via the drain join.
- MEDIUM confidence (re-verify bytes): exact line numbers, and that the
  browser-bridge is the only currently-concurrent producer. A read-tool
  glitch during filing produced some inconsistent renderings (line-number
  jumps; an `unreachable!()` shown inside a function the running server
  clearly executes), so treat all coordinates as approximate until checked.
