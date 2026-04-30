---
created: 2026-03-13
priority: p2
status: done
artifact: pending
---

# Auto-scroll message list to bottom on new messages

## Problem

When new messages arrive (via SSE), the message list doesn't scroll to the bottom.
The user must manually scroll to see system messages, agent responses, and tool
results as they arrive. This is especially bad after task approval where the
"Task approved" system message is at the bottom of a long conversation.

## What to Do

Add auto-scroll behavior to the message list:
- On new `sse_message` events, scroll to bottom if the user was already near the
  bottom (within ~100px). Don't force-scroll if the user has scrolled up to read
  history.
- On initial page load / SSE init, always scroll to bottom.
- On streaming tokens, scroll to bottom (user is watching live output).

Look at `ui/src/components/MessageList.tsx` -- the `#messages` container is the
scroll target. Use a `useEffect` that watches the messages array length or a
`ref` to detect new content.

## Acceptance Criteria

- [ ] New messages cause auto-scroll to bottom when user is near bottom
- [ ] User scrolled up to read history is NOT force-scrolled
- [ ] Initial load scrolls to bottom
- [ ] Streaming tokens keep the view at the bottom
