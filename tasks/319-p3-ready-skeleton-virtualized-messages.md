---
created: 2026-02-05
priority: p3
status: ready
---

# Add Skeleton Support to VirtualizedMessageList

## Summary

Ensure the VirtualizedMessageList component shows skeleton loading states consistently with the regular MessageList.

## Context

The VirtualizedMessageList is used for conversations with >50 messages. It has its own rendering logic and may not benefit from the skeleton improvements made to MessageList. Need to ensure parity.

## Current State

- Regular `MessageList` - uses `MessageListSkeleton` (from task 315)
- `VirtualizedMessageList` - has its own implementation, may show spinner or nothing

## Acceptance Criteria

- [ ] VirtualizedMessageList shows skeleton during initial load
- [ ] Skeleton shown when scrolling to unloaded regions (if applicable)
- [ ] Consistent appearance with regular MessageList skeleton
- [ ] SubAgentStatus spinner replaced with skeleton if appropriate

## Technical Notes

- VirtualizedMessageList uses `react-window` for virtualization
- May need placeholder rows for unloaded items
- Check line 413: `<span className="spinner"></span>` in SubAgentStatus
- Initial render should show skeleton if messages not yet loaded

## Questions to Resolve

1. Does react-window support placeholder/skeleton rows?
2. Is there a loading state we can hook into?
3. Should skeleton show for scroll-to-load or just initial?

## See Also

- `ui/src/components/VirtualizedMessageList.tsx`
- `ui/src/components/MessageList.tsx` - reference implementation
- `ui/src/components/Skeleton.tsx`
