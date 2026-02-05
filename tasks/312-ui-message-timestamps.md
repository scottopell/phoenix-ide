---
created: 2026-02-05
priority: p4
status: ready
---

# Add Timestamps to Messages

## Summary

Display timestamps on messages to help users understand conversation timing and duration.

## Context

Long conversations can span hours or days. Users currently have no way to see when messages were sent or how long operations took. This is useful for:
- Understanding conversation timeline
- Debugging slow operations
- Reviewing past conversations

## Acceptance Criteria

- [ ] Timestamps shown on user messages (when sent)
- [ ] Timestamps shown on agent messages (when received)
- [ ] Compact format: "2:34 PM" for today, "Feb 4" for older
- [ ] Full datetime shown on hover/tap
- [ ] Optionally show duration for agent responses (time from user message to agent completion)
- [ ] Subtle styling that doesn't clutter the message display

## Technical Notes

- Message type already has `created_at` field (check API response)
- Use relative time for recent messages ("just now", "2 min ago")
- Consider grouping consecutive messages from same sender
- May need to persist timestamp display preference

## Design Options

### Option A: Inline with header
```
You · 2:34 PM
Create a new file called test.ts
```

### Option B: Right-aligned
```
You                           2:34 PM
Create a new file called test.ts
```

### Option C: On hover only (desktop)
Timestamp appears on hover to keep UI clean.

## See Also

- `ui/src/components/MessageList.tsx`
- `ui/src/utils.ts` - `formatRelativeTime()` already exists
- `ui/src/api.ts` - Message type definition
