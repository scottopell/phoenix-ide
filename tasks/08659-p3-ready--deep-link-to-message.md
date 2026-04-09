---
created: 2026-04-08
priority: p3
status: ready
artifact: ui/src/components/MessageList.tsx
---

# Deep link to specific message via URL hash fragment

## Problem

No way to link someone to a specific message within a conversation.
Useful for share mode ("look at this tool output") and for the owner
linking coworkers to a specific point in a long conversation.

## What to build

1. **Anchor IDs on messages**: Each message DOM element gets
   `id="msg-{sequence_id}"` (messages already have
   `data-sequence-id` attributes).

2. **Hash fragment support**: URLs like `/c/{slug}#msg-42` or
   `/s/{token}#msg-42` scroll to and highlight that message on load.

3. **Scroll + highlight on load**: When the page loads with a hash
   fragment, scroll to the target message and apply a brief highlight
   animation (pulse or background flash) so it's visually obvious.

4. **Copy link in context menu**: Add "Copy link to message" in the
   right-click context menu (MessageContextMenu.tsx). Generates the
   full URL with hash fragment and copies to clipboard.

## Done when

- [ ] Messages have `id="msg-{N}"` on their DOM elements
- [ ] URL hash fragment scrolls to the target message on page load
- [ ] Target message gets a brief highlight animation
- [ ] Context menu offers "Copy link to message"
- [ ] Works on both `/c/` (owner) and `/s/` (share) URLs
