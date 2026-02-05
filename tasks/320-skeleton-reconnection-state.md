---
created: 2026-02-05
priority: p4
status: ready
---

# Add Skeleton Overlay During SSE Reconnection

## Summary

Show a subtle skeleton overlay or indicator when SSE reconnects, signaling that displayed data might be refreshing.

## Context

When the SSE connection drops and reconnects, the UI continues showing cached/stale messages. The reconnection fetches new messages via `?after=lastSeqId`, but there's no visual indication that data is being refreshed. Users might not realize they're looking at potentially stale data.

## Current Behavior

1. Connection drops
2. "Reconnecting..." banner shows
3. Connection restored
4. Brief "Reconnected" message
5. New messages silently append

## Proposed Behavior

1. Connection drops
2. "Reconnecting..." banner shows
3. Messages area gets subtle skeleton overlay or opacity reduction
4. Connection restored, data refreshes
5. Overlay removed, new messages visible

## Acceptance Criteria

- [ ] Visual indication that message list may be stale during reconnection
- [ ] Subtle approach - don't hide content, just indicate staleness
- [ ] Overlay/indicator removed once reconnection completes
- [ ] Works with both MessageList and VirtualizedMessageList

## Design Options

### Option A: Opacity reduction
```css
.messages-stale {
  opacity: 0.6;
  pointer-events: none;
}
```

### Option B: Shimmer overlay
Subtle animated overlay on top of messages.

### Option C: Top banner skeleton
Show skeleton message at top indicating "Checking for new messages..."

## Priority

Low priority (p4) - edge case UX polish. Current reconnection handling is functional.

## See Also

- `ui/src/hooks/useConnection.ts` - connection state management
- `ui/src/pages/ConversationPage.tsx` - reconnection handling
- Task 026 - offline/reconnection handling (completed)
