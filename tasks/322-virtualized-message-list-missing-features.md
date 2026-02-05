---
created: 2026-02-05
completed: 2026-02-05
priority: p1
status: done
type: bug
---

# BUG: VirtualizedMessageList Missing Recent UI Improvements

## Resolution

Fixed by extracting shared components from MessageList.tsx into a new MessageComponents.tsx file, then having both MessageList.tsx and VirtualizedMessageList.tsx import from it.

### Changes Made

1. Created `ui/src/components/MessageComponents.tsx` with all shared message rendering components:
   - UserMessage (with timestamps)
   - QueuedUserMessage
   - AgentMessage (with timestamps)
   - ToolUseBlock (new design with copy buttons)
   - SubAgentStatus
   - formatMessageTime helper

2. Refactored `MessageList.tsx` to import from MessageComponents.tsx (reduced from 373 to 85 lines)

3. Refactored `VirtualizedMessageList.tsx` to import from MessageComponents.tsx (reduced from 421 to 242 lines)

### Features Now Available in Both Implementations

- ✅ New tool block design (task 313)
- ✅ Copy buttons (task 311)
- ✅ Timestamps (task 312)
- ✅ Proper "You"/"Phoenix" sender labels
- ✅ Status indicators (checkmarks for success/error)

### Fixed a Pre-existing Issue

- Fixed `Skeleton.tsx` type error (missing `style` prop in SkeletonProps)

## Original Summary

Conversations with >50 messages used `VirtualizedMessageList` which was missing all recent UI improvements.

## See Also

- Task 321 - investigation that led to this fix (Option B chosen)
