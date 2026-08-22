`MessageList` drives the scroll policy's gesture lifecycle from touch events
(`touchStarted` / `touchMoved` / `touchEnded` with `remainingTouches`), and a
touch target is fixed at touchstart. When virtualization unmounts the touched
row mid-gesture, the browser dispatches `touchend` at the detached node, where
it reaches no listener at all — verified against Chromium: scroller, document
and window all miss it (`window.log` recorded only `scroller:touchstart`).

Consequence: the gesture cannot be resolved at the moment the finger lifts.
Remaining-touch counts intersect the scroller-owned identifier set with each
event's live `touches` list, so the vanished touch is pruned by the next touch
event and the session self-heals rather than wedging — but between the lost
lift and the next gesture, the machine still believes a touch is down, which
suppresses tail-follow.

The fix is to drive the gesture lifecycle from pointer events, which retarget
when their element is removed and still deliver `pointerup` (same Chromium
probe: `scroller:pointerup` and `window:pointerup` both fire after the row is
detached). `VirtualTranscript` already tracks its scroll-activity gesture this
way. Doing the same in `MessageList` means either mapping pointer ids onto the
machine's touch-shaped events or renaming those events in
`specs/messagelist-render-units/scroll_policy.allium`, which is a spec change
rather than a local edit — hence deferred rather than folded into the scroll
fix PR.
