---
created: 2026-02-05
completed: 2026-02-05
priority: p1
status: done
type: question
---

# Investigate: MessageList vs VirtualizedMessageList Duplication

## Resolution

**Chosen approach: Option B - Keep both list implementations, extract shared components**

### Decision Rationale

1. **Why keep both list implementations?**
   - Virtualization via react-window requires different scrolling mechanics
   - Height calculation for variable-size items is complex
   - The list-level concerns (scrolling, height management) are fundamentally different
   - Virtualization provides real perf benefits for 100+ message conversations

2. **Why extract shared components?**
   - Message rendering (UserMessage, AgentMessage, ToolUseBlock) should be identical
   - Feature drift was the root cause of this bug
   - Single source of truth prevents future inconsistencies

### Implementation

Created `ui/src/components/MessageComponents.tsx` containing:
- `formatMessageTime()` - helper for timestamp formatting
- `UserMessage` - renders user messages with timestamps
- `QueuedUserMessage` - renders pending/failed user messages
- `AgentMessage` - renders agent responses with tool blocks
- `ToolUseBlock` - renders individual tool use/result pairs with copy buttons
- `SubAgentStatus` - renders sub-agent progress indicator

Both `MessageList.tsx` and `VirtualizedMessageList.tsx` now import from this shared module.

### Code Reduction

- **Before**: MessageList (373 lines) + VirtualizedMessageList (421 lines) = 794 lines
- **After**: MessageList (85 lines) + VirtualizedMessageList (242 lines) + MessageComponents (330 lines) = 657 lines
- **Net reduction**: 137 lines, and more importantly, zero duplication of UI rendering logic

### Future Guidance

Any UI changes to message rendering should be made in `MessageComponents.tsx`, NOT in the list implementations. The list implementations only handle:
- MessageList: Simple scrolling, scroll-to-bottom
- VirtualizedMessageList: react-window integration, height calculation, scroll position persistence

## Original Context

We had two implementations of the message list:
- `MessageList.tsx` - standard rendering
- `VirtualizedMessageList.tsx` - uses react-window for virtualization

The switch happens at 50 messages (in `ConversationPage.tsx`).

## Original Concerns (Addressed)

1. **Maintenance burden** ✅ - Now only need to update MessageComponents.tsx
2. **Inconsistency risk** ✅ - Single source of truth
3. **Arbitrary threshold** - 50 is reasonable; not addressed in this fix
4. **Code duplication** ✅ - Eliminated via shared components

## See Also

- `ui/src/components/MessageComponents.tsx` - shared message rendering
- `ui/src/components/MessageList.tsx` - simple list for <50 messages  
- `ui/src/components/VirtualizedMessageList.tsx` - virtualized list for >50 messages
- `ui/src/pages/ConversationPage.tsx` - switching logic
