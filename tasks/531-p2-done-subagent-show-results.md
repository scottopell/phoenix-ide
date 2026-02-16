---
created: 2026-02-08
priority: p2
status: done
---

# Show Subagent Results in UI

## Summary

Display actual outcomes from completed subagents instead of just a checkmark.

## Context

When a subagent completes, the backend has:
```rust
SubAgentResult {
    agent_id: String,
    task: String,
    outcome: SubAgentOutcome,  // Success(String) or Error(String)
}
```

But the UI just shows "✓ completed" with no details.

## Requirements

1. Show success results (truncated, expandable)
2. Show error results with error styling
3. Allow expanding to see full result

## UI Design

```
Sub-agents 2/3

✓ Review security
  Found 2 potential issues: SQL injection in...  [expand]
  
✓ Check performance  
  No major bottlenecks found. Average response...  [expand]

⏳ Analyze dependencies
  running...
```

## Implementation

1. **Backend**: Serialize `completed_results` with full data in StateChange:
   ```json
   {
     "type": "awaiting_sub_agents",
     "pending_ids": ["agent-3"],
     "completed_results": [
       {"agent_id": "agent-1", "task": "Review security", "outcome": {"success": "Found 2..."}},
       {"agent_id": "agent-2", "task": "Check perf", "outcome": {"success": "No major..."}}
     ]
   }
   ```

2. **Frontend**: Update `ConversationState` type to include outcome
3. **Frontend**: Render result preview with expand/collapse

## Acceptance Criteria

- [ ] Completed subagents show result preview (first ~100 chars)
- [ ] Click/tap expands to show full result
- [ ] Error outcomes styled differently (red)
- [ ] Works with existing pending/completed flow
