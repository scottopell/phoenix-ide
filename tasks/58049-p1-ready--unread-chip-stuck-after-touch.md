"New messages" chip never clears on iOS Safari: the bottom-confirmation event is edge-triggered and the edge is consumed while blocked during a moved touch.

Mechanism (all in `ui/`):

- `VirtualTranscript` reports pinned-ness via `onPinnedChange`, which fires only when its internal `store.pinned` boolean flips (layout effect keyed on `pinned`), using `PINNED_EPSILON = 1` px.
- When the user touch-drags back down to the bottom, `viewportPinnedChanged { atBottom: true }` arrives while `gesture.moved === true`, and `reduceScrollMachine` deliberately blocks the confirmation (spec rule `BottomDuringMovedTouchCannotReleaseOwnership` — correct mid-gesture).
- On `touchEnded`, `resolveTouch` keeps `follow: reading` and never re-examines `geometry.atBottom` (which is true). Since `store.pinned` is already true, no further pinned edge ever fires, so `confirmTailReturn` never runs.
- Result: machine stuck in `reading` + `unread: true` while the user sits pinned at the bottom. VirtualTranscript's own `wasPinned` auto-snap (in `applyPhysicalChange`) keeps following the tail independently of the policy machine, so tail-follow visibly works while the chip lies. Only clicking the chip (`jumpToNewestRequested`) clears it.

Repro (iOS, or any touch device): while a conversation streams at the bottom, drag up >1px and back down to the very bottom, release. Chip appears on the next message and never leaves.

Fix direction: on `touchEnded`/`touchCancelled` with no remaining touches, if `geometry.atBottom` is true, confirm tail return (clear unread, restore following) — i.e. add the missing "gesture ended at bottom" rule to `specs/messagelist-render-units/scroll_policy.allium` and `reduceScrollMachine`. Alternatively (or additionally) have the MessageList adapter re-dispatch the current pinned state after touch end so the machine is not starved of edges.
