# Fix: Conversation scroll "fighting" the user during streaming

## Problem

The user reports that scrolling up in the conversation view "goes too far"
and is especially bad while the agent is still streaming. The symptom is
that scrolling up during streaming feels like the view is fighting the
user — the viewport yanks back to the bottom.

## Root Cause: Double auto-scroll during streaming

Two independent auto-scroll mechanisms run simultaneously during streaming,
and they fight each other and the user's scroll-up intent.

### Key insight: Virtuoso's auto-scroll doesn't actually handle streaming

Traced through react-virtuoso 4.18.7's source. `followOutput="auto"` enables
two built-in mechanisms, **neither of which fires during streaming token
growth**:

1. **totalCount-based followOutput** — fires when `data.length` changes
   (new items appended). During streaming, `data.length` doesn't change
   (the streaming unit is already there, just growing). So this **never
   fires during streaming**.

2. **Size-increase handler** — fires when `atBottom` transitions from
   true→false AND `notAtBottomBecause === "SIZE_INCREASED"`. But when the
   user is at the bottom and a token grows the content by 50px,
   `scrollHeight` increases but `scrollTop` stays the same, so the user is
   now 50px from the bottom — still within the 100px `atBottomThreshold`.
   `atBottom` stays **true**. No transition. **The handler doesn't fire.**
   The user is left 50px above the bottom.

So Virtuoso has **no mechanism** that keeps the user pinned to the bottom
during streaming token growth. That's exactly the gap
`handleTotalListHeightChanged` was written to fill — the comment at
MessageList.tsx:458 says this explicitly:

> *"virtuoso's `followOutput="auto"` only fires when `data.length` grows;
> it doesn't re-snap when the LAST item's height changes async after mount.
> That leaves the user a few hundred pixels above true bottom"*

The manual re-snap is **necessary** — without it, streaming auto-follow
doesn't work at all. The bug is that it runs **alongside** Virtuoso's
size-increase handler, which fires in a different scenario and misfires.

### Mechanism A — Virtuoso's built-in size-increase handler (the buggy one)

`followOutput="auto"` enables a built-in handler that fires when `atBottom`
transitions from true→false AND `notAtBottomBecause === "SIZE_INCREASED"`.
It calls `scrollToIndex({ index: 'LAST', align: 'end', behavior: 'auto' })`.

During streaming, `scrollHeight` increases on every token. Virtuoso's
`notAtBottomBecause` priority order checks `scrollHeight > prevScrollHeight`
**first** (before `SCROLLING_UPWARDS`), so during active streaming the
reason is almost always classified as `"SIZE_INCREASED"` — even when the
user manually scrolled up. When the user scrolls up within the 100px
`atBottomThreshold`, `atBottom` flips to false, the reason is
`"SIZE_INCREASED"` (because scrollHeight grew), and the handler yanks the
user back to the bottom.

### Mechanism B — MessageList's `handleTotalListHeightChanged` (the necessary one)

`handleTotalListHeightChanged` (MessageList.tsx:480) fires
synchronously on every height delta. When `oldFromBottom <= 100` it calls
the same `scrollToIndex({ index: 'LAST', align: 'end', behavior: 'auto' })`.

This was added because `followOutput="auto"` only fires on `data.length`
changes (new items), not on height-only changes (streaming token growth).
It's the **only** mechanism that makes streaming auto-follow work. But it
runs alongside Mechanism A, creating a double-scroll that amplifies the
fighting.

### Why `followOutput` as a function doesn't help

Virtuoso's `FollowOutputCallback` form only controls the totalCount-based
followOutput. The size-increase handler uses the raw `followOutput` state
value (`it(w) !== false`), so a function (truthy) leaves it enabled. Only
`followOutput={false}` disables it.

## Fix

Set `followOutput={false}` to disable ALL of Virtuoso's built-in auto-scroll
mechanisms (both the totalCount-based followOutput and the size-increase
handler). Keep `handleTotalListHeightChanged` as the sole auto-scroll
mechanism — its `oldFromBottom` logic correctly distinguishes "user was
near the bottom" from "user scrolled up" using the pre-growth scroll
position vs. the pre-growth bottom.

This eliminates:
- The double-scroll (two mechanisms fighting)
- The `notAtBottomBecause` priority-order misclassification during streaming

`initialTopMostItemIndex`, `atBottomStateChange`, and the jump-to-newest
button are all independent of `followOutput` and continue to work.

### Trade-off analysis: is the manual scroll worth it?

The manual re-snap is **necessary** — Virtuoso's auto-scroll doesn't handle
streaming height growth at all (see Key Insight above). Without it, the
user drifts away from the bottom as tokens arrive. So the question isn't
"Virtuoso vs manual" but "manual alone" vs "manual + Virtuoso
(double-scroll)."

| | Virtuoso auto-scroll | Our manual re-snap |
|---|---|---|
| Handles streaming height growth | ❌ No | ✅ Yes |
| Handles new item append | ✅ Yes (totalCount) | ✅ Yes (height increased) |
| Battle-tested edge cases (mobile, resize) | ✅ Yes | ⚠️ We handle these |
| `notAtBottomBecause` misclassification | ❌ Buggy during streaming | ✅ `oldFromBottom` is more correct |
| Maintenance burden | ✅ Zero | ❌ We maintain custom code |

The one real downside of `followOutput={false}` is losing Virtuoso's
edge-case handling for new-item appends (mobile momentum, etc.). But the
manual re-snap already handles new-item appends (height increased →
re-snap), so the coverage is the same. The manual `oldFromBottom` logic is
also more correct than Virtuoso's `notAtBottomBecause` priority order,
which misclassifies user scroll-up as size-increase during streaming.

**Conclusion: worth it.** The manual scroll is the only thing that makes
streaming auto-follow work, and its pin/no-pin logic is more correct than
Virtuoso's built-in handler.

### Verification that `followOutput={false}` disables the size-increase handler

Traced through react-virtuoso 4.18.7 source (`Qo` system):
- The `a()` function that sets up the size-increase subscription is called
  with `y = (followOutput !== false)`. With `false`, `y` is `false`, and the
  handler's `y &&` guard fails.
- The totalCount-based `shouldFollow` computation: `Jo(false, ...)`
  evaluates to `false`, so `shouldFollow` is always `false`.
- `atBottomStateChange` is derived from `atBottomState` independently of
  `followOutput` — it still fires.

## Secondary improvements (optional, same task)

1. **Use `scrollTo` instead of `scrollToIndex` for the re-snap.** During
   streaming, `scrollToIndex` goes through Virtuoso's internal scroll-into-
   view machinery on every token. `virtuosoRef.current?.scrollTo({ top:
   scroller.scrollHeight, behavior: 'auto' })` is a direct scroll that
   avoids the overhead. Or use `autoscrollToBottom()`.

2. **Reduce `PIN_TO_BOTTOM_THRESHOLD` during streaming.** The 100px
   threshold means the user must scroll >100px up in a single gesture to
   escape auto-follow. During fast streaming this feels aggressive.
   Consider a smaller threshold (e.g., 30px) or scroll-direction detection
   (don't re-snap when the user is actively scrolling up).

3. **Debounce `DeferredSyntaxHighlighter` height changes.** Code blocks
   that mount their syntax highlighter after 1500ms (`requestIdleCallback`)
   fire `totalListHeightChanged`, potentially triggering re-snaps. Multiple
   code blocks firing at different times cause a series of small jumps.

## Files to modify

- `ui/src/components/MessageList.tsx` — change `followOutput="auto"` to
  `followOutput={false}`; optionally switch re-snap to `scrollTo`/`autoscrollToBottom`
- `specs/messagelist-render-units/requirements.md` — update REQ-MLRU-014/015
  to reflect `followOutput={false}` + manual re-snap as the sole auto-scroll
- `specs/messagelist-render-units/design.md` — update the Virtuoso
  configuration section

## Testing

- In-browser smoke: stream a response, scroll up during streaming — the
  viewport should NOT yank back to the bottom (within the threshold).
- Scroll up past the threshold during streaming — "↓ New messages" button
  should appear, viewport should stay put.
- New message appended while at bottom — viewport should follow to bottom.
- Switch conversations — should land pinned to bottom.
- Existing unit tests should pass (virtuoso is mocked in tests, so the
  `followOutput` change doesn't affect them).
