# Stabilize conversation breadcrumb jumps

Clicking a chapter in the conversation navigation strip can fight the message-list scroll policy: the jump starts a smooth Virtuoso scroll, while fixed 120/320/600 ms callbacks subsequently mutate the DOM scroller position through `ensureTargetTopVisible`. Those callbacks race browser smooth-scroll timing, Virtuoso mounting/measurement, and newer navigation requests. The resulting competing writers explain both observed failure modes: a short up/down jitter loop and a delayed second displacement after the target initially appears.

## Outcome

A chapter click has one authoritative positioning operation. It lands on the selected render unit without later timer-driven displacement, takes durable reading ownership from the tail-follow policy, and highlights the target when Virtuoso reports it visible.

## Plan

1. Reproduce and instrument the chapter-jump path in the running UI, covering a nearby target, a long off-screen target, rapid selection of two chapters, and navigation while content is streaming or changing height.
2. Extend the normative conversation scroll behavior to define explicit chapter navigation as a single-owner operation: navigation cancels superseded navigation work, disables mount rescue/tail follow, and must not emit delayed position corrections after landing. Keep browser/Virtuoso physics outside the pure tail-follow policy.
3. Replace `MessageList.scrollToUnitIndex`'s smooth-scroll plus fixed retry ladder with a deterministic Virtuoso-owned jump lifecycle. Use the virtualizer's visible-range/render readiness signal to apply the highlight after the target mounts; do not poll row availability with wall-clock timers or directly adjust `scrollTop` during a Virtuoso jump.
4. Make a newer chapter selection structurally supersede the prior pending target so stale visibility/highlight work cannot affect the viewport or pulse the wrong row. Clear pending navigation state on conversation identity changes and unmount.
5. Add reducer coverage proving `navigationJumped` exits mount rescue, retains reading ownership through height changes and noisy movement, and cannot trigger a tail snap without an explicit return-to-newest action.
6. Add component regressions for long off-screen jumps, delayed target mounting, rapid consecutive clicks, and post-jump height changes. Assert one positioning command per click, no direct delayed `scrollTop` correction, and one highlight on only the latest selected target.
7. Run focused UI tests, TypeScript/lint checks, the Allium validator for the touched scroll specification, then `./dev.py check`. Verify the repaired interactions manually in the browser at desktop and narrow viewport sizes.

## Acceptance criteria

- Selecting any conversation chapter lands at that chapter and remains there until user input, another explicit navigation action, or an allowed virtualizer anchor adjustment.
- No visible flapping or delayed downward/upward correction occurs after a chapter jump.
- Rapid chapter selections leave the newest selection authoritative.
- Tail growth or layout measurement after a jump does not return the reader to the newest message.
- The target highlight appears only after the selected row is rendered and does not require fixed-delay retry timers.
- Mount rescue and normal live-follow behavior remain unchanged outside explicit chapter navigation.
