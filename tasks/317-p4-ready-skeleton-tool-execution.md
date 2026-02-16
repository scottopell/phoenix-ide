---
created: 2026-02-05
priority: p4
status: ready
---

# Add Skeleton for Tool Execution Output

## Summary

Show a skeleton placeholder in tool blocks while a tool is executing, before the result arrives.

## Context

When Phoenix executes a tool (bash, patch, etc.), the tool block shows the input/command immediately but the output area is empty until the tool completes. For long-running commands, this creates uncertainty about whether something is happening.

## Current Behavior

```
┌─ bash ─────────────────────────┐
│ $ npm install                  │
│                                │  <- empty, no feedback
└────────────────────────────────┘
```

## Proposed Behavior

```
┌─ bash ─────────────────────────┐
│ $ npm install                  │
│ ░░░░░░░░░░░░░░ (running...)   │  <- skeleton with status
└────────────────────────────────┘
```

## Acceptance Criteria

- [ ] Show skeleton output area when tool is executing (no result yet)
- [ ] Include subtle "running..." or spinner indicator
- [ ] Skeleton replaced by actual output when tool completes
- [ ] Works for all tool types (bash, patch, think, etc.)
- [ ] Error state still shows properly if tool fails

## Technical Notes

- Tool execution state comes from SSE `state_change` events
- `convState === 'tool_executing'` indicates a tool is running
- `stateData.current_tool` has the tool being executed
- Need to match tool_use block ID with current executing tool
- May need to pass execution state down to `ToolUseBlock` component

## Design Consideration

Keep it subtle - a small shimmer bar with "running..." text is enough. Don't want to be too distracting for fast tools.

## See Also

- `ui/src/components/MessageList.tsx` - `ToolUseBlock` component
- `ui/src/pages/ConversationPage.tsx` - state management
- Task 313 - tool block redesign (completed)
