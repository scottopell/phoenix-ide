---
created: 2026-02-05
priority: p1
status: ready
type: question
---

# Investigate: MessageList vs VirtualizedMessageList Duplication

## Question

We have two implementations of the message list:
- `MessageList.tsx` - standard rendering
- `VirtualizedMessageList.tsx` - uses react-window for virtualization

Are these dual implementations necessary? Can/should they be consolidated?

## Current Usage

From `ConversationPage.tsx`:
```tsx
{messages.length > 50 ? (
  <VirtualizedMessageList ... />
) : (
  <MessageList ... />
)}
```

The switch happens at 50 messages.

## Concerns

1. **Maintenance burden** - Bug fixes and features must be applied to both components
2. **Inconsistency risk** - Easy for implementations to drift (e.g., skeleton support, timestamps, copy buttons)
3. **Arbitrary threshold** - Why 50? Is this the right number?
4. **Code duplication** - Both have similar UserMessage, AgentMessage, ToolUseBlock implementations

## Questions to Answer

1. What's the performance difference? Is virtualization actually needed?
2. Could we always use virtualization (even for small lists)?
3. Could we never use virtualization (is 50+ messages actually slow?)?
4. If we keep both, can we extract shared components to reduce duplication?
5. What's the UX difference between the two? Any visible behavior changes at the 50-message boundary?

## Recent Changes Affected

- Task 313 (tool block redesign) - applied to MessageList only?
- Task 311 (copy buttons) - applied to MessageList only?
- Task 312 (timestamps) - applied to MessageList only?
- Task 315 (skeletons) - MessageList only, task 319 created for VirtualizedMessageList

## Action Needed

Investigate and decide:
- **Option A**: Consolidate to single implementation
- **Option B**: Keep both, extract shared components
- **Option C**: Keep both, accept duplication (document why)

## See Also

- `ui/src/components/MessageList.tsx`
- `ui/src/components/VirtualizedMessageList.tsx`
- `ui/src/pages/ConversationPage.tsx` - switching logic
