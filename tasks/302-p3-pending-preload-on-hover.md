---
created: 2026-02-04
priority: p3
status: pending
---

# Implement Preload on Hover

## Summary

Start loading conversation data when user hovers over a conversation item, reducing perceived load time.

## Context

Identified as an optimization opportunity in the navigation performance investigation. This would make navigation feel even more instant by preloading data before the user clicks.

## Acceptance Criteria

- [ ] Detect hover on conversation items
- [ ] Start preloading after 100ms hover delay
- [ ] Cache preloaded data
- [ ] Cancel preload if user moves away
- [ ] Don't preload if already cached
- [ ] Mobile: Consider preload on touch start

## Notes

- Use enhancedApi with forceFresh: false
- Be careful not to waste bandwidth
- Consider preloading only the first N messages
- Track metrics on how often preloaded data is used
