# Stop message-list scroll jumps when queued messages become sent

## Problem

When a user sends a message, Phoenix renders it immediately as a pending tail unit. When the server echoes the message back over SSE, the pending tail entry is filtered out and the same message is inserted into the historical/virtualized message list. That promotion can change spacer/window geometry and the browser-visible scroll position jumps away right as the UI switches to the green sent checkmark.

This is especially painful because the user is already looking at the pending message; the acknowledgement should be visually stable.

## Likely cause

`MessageList` builds two different render regions:

- historical units from `atom.messages`, controlled by `useBottomAnchoredWindow`
- tail units from `pendingMessages`, rendered after the historical slice and outside the virtualized window

On acknowledgement, a message with the same logical key (`localId` / `message_id`) moves from `tailUnits` to `historicalUnits`. Current scroll compensation mostly handles prepend/window-boundary changes and ResizeObserver bottom pinning, but not this cross-region promotion. If the viewport is not bottom-pinned, or if virtualization collapses/expands around the transition, the top visible content is not anchored through the commit.

## Plan

1. Reproduce with a focused UI test around `MessageList`:
   - render a conversation with enough historical units to exercise the virtualized window/spacer
   - render a pending message at the tail
   - set a non-bottom scroll position / captured viewport
   - rerender with that pending message removed and the corresponding server `user` message appended to `messages`
   - assert the visible anchor/scrollTop remains stable rather than jumping
2. Implement scroll preservation for pending-to-sent promotion:
   - before render-unit shape changes, detect pending `localId`s that are about to appear in `messages`
   - capture the current visible unit anchor from the scroll root
   - after commit, restore by that anchor if possible, or apply equivalent scroll-height compensation
   - preserve normal bottom-pinned behavior: if the user is pinned to bottom, acknowledgement should remain pinned to bottom
3. Keep the fix structurally local to `MessageList` / render-unit handling; do not introduce parallel message status state. “Sent” remains derived from server echo presence.
4. Add/adjust regression tests for both:
   - not-bottom-pinned user keeps their visual position on ack
   - bottom-pinned user stays at bottom on ack
5. Run the relevant UI tests, then the project check if time permits.

## Acceptance criteria

- Sending-message acknowledgement no longer causes the message list viewport to jump.
- The pending bubble can become the green-check sent bubble without moving the user away from what they were reading.
- Existing auto-scroll-to-bottom behavior for bottom-pinned users still works.
- Regression coverage exists for the ack promotion path.
