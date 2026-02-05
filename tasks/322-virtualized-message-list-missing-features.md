---
created: 2026-02-05
priority: p1
status: ready
type: bug
---

# BUG: VirtualizedMessageList Missing Recent UI Improvements

## Summary

Conversations with >50 messages use `VirtualizedMessageList` which is missing all recent UI improvements.

## Impact

Users with long conversations don't see:
- New tool block design (task 313) - still shows old collapsed-by-default style
- Copy buttons (task 311) - no copy functionality
- Timestamps (task 312) - no timestamps on messages

## Root Cause

`ConversationPage.tsx` switches implementations at 50 messages:
```tsx
{messages.length > 50 ? (
  <VirtualizedMessageList ... />
) : (
  <MessageList ... />
)}
```

The two components have duplicate implementations that have drifted.

## Evidence

```bash
$ grep "tool-block\|tool-group" ui/src/components/VirtualizedMessageList.tsx
    <div className={`tool-group${expanded ? ' expanded' : ''}`}  # OLD style

$ grep "tool-block\|tool-group" ui/src/components/MessageList.tsx  
    <div className="tool-block"  # NEW style
```

## Fix Options

1. **Quick fix**: Port all changes to VirtualizedMessageList (duplicate work)
2. **Proper fix**: Resolve task 321 first (consolidate or extract shared components)

## Recommendation

Do the quick fix now to restore feature parity, then address consolidation (task 321) separately.

## See Also

- Task 321 - investigation of MessageList duplication
- Task 311, 312, 313 - features that were only applied to MessageList
