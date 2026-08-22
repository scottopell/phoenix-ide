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

## Update: pointer events are not the answer

A Chromium probe shows the browser fires `pointercancel` as soon as a native
pan takes over — while the finger is still down — and then reports no
`pointerup` at all when the finger lifts. Recorded log for a real pan:
`["scroller:pointerdown","window:pointerdown","scroller:pointercancel",
"window:pointercancel","scroll"]`, unchanged after the lift.

So no event source reliably reports a lift for a touch whose row is unmounted
mid-gesture: touch events go to a detached node nothing observes, and pointer
events are cancelled before the lift. Both `VirtualTranscript` and
`MessageList` therefore track touches and bound the damage instead — pruning
against each event's live `touches` list, and (in VirtualTranscript)
capping how long a touch alone defers reconciliation.

What remains open here is narrower than originally written: `MessageList`
still believes a finger is down between an unobservable lift and the next
touch event, which suppresses tail-follow for that window. A bound like
VirtualTranscript's would close it without a spec rename; moving the
lifecycle onto pointer events would not.
