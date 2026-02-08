---
created: 2026-02-08
priority: p2
status: ready
---

# Persist Subagent Summary After Completion

## Summary

After subagents complete, show a persistent summary block in the conversation instead of nothing.

## Context

Currently:
1. spawn_agents tool_use shows "Spawning 3 sub-agents"
2. SubAgentStatus shows live progress during execution
3. When done... nothing. SubAgentStatus disappears.
4. User has no record of what happened

The spawn_agents tool_result just shows the initial task list, not the outcomes.

## Requirements

1. After all subagents complete, render a summary in the conversation
2. Summary should show each subagent's task and outcome
3. Should be expandable/collapsible like think blocks
4. Should link to subagent conversations if possible

## Design Options

### Option A: Modify spawn_agents tool_result
When subagents complete, update the spawn_agents tool_result message with outcomes.
- Pro: Natural place in message flow
- Con: Mutating existing messages is messy

### Option B: Create synthetic "subagent_summary" message
Insert a new message after spawn_agents completes.
- Pro: Clean, doesn't mutate
- Con: New message type

### Option C: Use display_data on tool_result
Add rich display_data to the tool_result when subagents complete.
- Pro: Uses existing mechanism
- Con: Need to update message after creation

Recommend Option C - display_data already exists for rich tool rendering.

## UI Design

```
┌─ spawn_agents ──────────────────────────────────────────────┐
│ Spawning 3 sub-agents:                                      │
│ 1. Review security                                          │
│ 2. Check performance                                        │
│ 3. Analyze dependencies                                     │
├─────────────────────────────────────────────────────────────┤
│ Results:                                                    │
│ ✓ Review security - Found 2 issues... [view conversation]   │
│ ✓ Check performance - No bottlenecks... [view conversation] │
│ ✗ Analyze dependencies - Timed out [view conversation]      │
└─────────────────────────────────────────────────────────────┘
```

## Acceptance Criteria

- [ ] spawn_agents shows summary after all subagents complete
- [ ] Each subagent shows task + outcome status
- [ ] Success/error visually distinguished
- [ ] Expandable to see full results
- [ ] Optional: Link to subagent conversation
