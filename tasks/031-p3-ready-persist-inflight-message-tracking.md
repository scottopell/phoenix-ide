---
created: 2026-02-02
priority: p3
status: ready
---

# Persist In-Flight Message Tracking

## Summary

The `sendingMessagesRef` Set in ConversationPage tracks which messages are currently being sent to prevent double-sends. However, this isn't persisted - if the page refreshes mid-send, a message could be re-sent.

## Current Behavior

1. User sends message → localId added to `sendingMessagesRef`
2. API call in progress
3. Page refreshes (or tab closes/reopens)
4. `sendingMessagesRef` is empty (it's a ref, not persisted)
5. useEffect sees queued message with status 'sending'
6. Message is sent again

Server-side deduplication (if any) would need to handle this. If the server doesn't deduplicate, user sees duplicate messages.

## Proposed Solution

Track in-flight state in the QueuedMessage itself:

```typescript
interface QueuedMessage {
  localId: string;
  text: string;
  images: ImageData[];
  timestamp: number;
  status: 'pending' | 'sending' | 'failed';  // Add 'pending' state
  sendAttemptedAt?: number;  // Timestamp of last send attempt
}
```

On page load:
- Messages with status 'sending' and `sendAttemptedAt` > 30s ago → treat as failed, let user retry
- Messages with status 'sending' and `sendAttemptedAt` < 30s ago → wait briefly, then retry
- Messages with status 'pending' → send immediately

## Acceptance Criteria

- [ ] In-flight sends survive page refresh without duplicates
- [ ] Stale in-flight messages (>30s) shown as failed with retry option
- [ ] Recent in-flight messages (<30s) handled gracefully

## Notes

This is low priority because:
1. Page refresh mid-send is rare
2. Server may already deduplicate
3. Worst case is a duplicate message (annoying but not catastrophic)
