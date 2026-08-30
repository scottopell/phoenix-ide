REQ-MLRU-014 says earlier history is acquired only if the reader moved the
viewport to the loaded-history boundary. `readerMovedViewport` implements that
as `reading`, or `navigating` in the `user-returning` phase.

The second disjunct is a proxy, and it is wrong. `user-returning` is entered by
`interactionStarted`, which fires on any pointerdown inside the transcript —
including clicking an expand or copy control on a row. That is interruption of a
positioning command, which is what the phase was designed to record, but it is
not viewport movement. So during a positioning navigation resting near the start
of loaded history, clicking a control can let a subsequent layout-driven range
publication acquire an older page the reader never scrolled toward.

Consequence is a spurious history page-load, not a viewport jump: bounded, and it
does not reproduce the unread-chip or scroll-position symptoms. That is why it is
filed rather than fixed inside PR #700.

The honest fix needs movement recorded separately from takeover. Options:

1. Split the navigating phase into `positioning | interrupted | moving`, with the
   transition to `moving` driven by an observed `upwardIntent` / `downwardMovement`.
   `readerMovedViewport` then tests `moving`. The existing `user-returning` checks
   (the downward-arrival confirmation, the pinned-callback guard) would accept both
   `interrupted` and `moving`. This puts the fact in the policy where the other
   ownership facts live, at the cost of touching a load-bearing enum.

2. Have the scroll handler arm a boundary check when it observes real upward
   movement, and have the range publication fulfil that arming rather than judge
   ownership for itself. This also removes the range publication`s dependence on
   `firstVisibleUnitIndexRef` being fresh, which is what PR #700 round 15 was
   working around.

Option 2 is probably better — it makes the range publication a completion
mechanism rather than an independent decision-maker — but it is a design change,
not a local edit.

Raised by automated review on PR #700 (round 21).
