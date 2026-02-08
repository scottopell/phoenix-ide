---
created: 2026-02-08
priority: p1
status: ready
---

# Show Subagent Task Descriptions

## Summary

Display actual task descriptions in SubAgentStatus instead of generic "Sub-agent 1" labels.

## Context

When spawn_agents is called, each subagent has a task description:
```json
{"tasks": [
  {"task": "Review security vulnerabilities"},
  {"task": "Check performance bottlenecks"}
]}
```

But the UI shows:
- ✓ Sub-agent 1 - completed
- ⏳ Sub-agent 2 - running...

This gives users no visibility into what's actually happening.

## Current Data Flow

1. `spawn_agents` tool receives task list
2. Backend creates `SubAgentSpec` with `task` field
3. State includes `pending_ids` (just IDs, not tasks)
4. `completed_results` has `SubAgentResult.task`

## Implementation

### Option A: Include tasks in state
Modify `AwaitingSubAgents` state to include task map:
```rust
pending_tasks: HashMap<String, String>,  // agent_id -> task
```

### Option B: Frontend tracks from spawn_agents message
Parse spawn_agents tool_use input to extract tasks, correlate with IDs.
More complex, may have timing issues.

### Option C: Include in completed_results SSE
Already have completed_results - ensure task is serialized.

Recommend Option A - cleanest, gives frontend everything it needs.

## Frontend Changes

```tsx
// Instead of:
<span className="subagent-label">Sub-agent {i + 1}</span>

// Show:
<span className="subagent-label">{stateData.pending_tasks?.[id] || `Sub-agent ${i + 1}`}</span>
```

## Acceptance Criteria

- [ ] SubAgentStatus shows task description for each subagent
- [ ] Task truncated with ellipsis if too long
- [ ] Fallback to "Sub-agent N" if task unavailable
- [ ] Completed items also show their task
