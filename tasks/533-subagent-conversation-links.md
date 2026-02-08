---
created: 2026-02-08
priority: p3
status: ready
---

# Link to Subagent Conversations

## Summary

Allow users to view subagent conversations from the parent conversation.

## Context

Subagent conversations exist in the database with `parent_conversation_id` set. They have their own slug and full message history. But there's no way for users to navigate to them.

## Requirements

1. SubAgentStatus items should link to their conversation
2. spawn_agents summary (task 532) should include links
3. Links open in same tab (or new tab?)
4. Should work even if subagent is still running

## Implementation

### Backend
- SubAgentSpec includes conversation_id (already does?)
- State change events include conversation_id/slug for each subagent
- Or: API endpoint to get subagent conversations for a parent

### Frontend  
- Add click handler to subagent items
- Navigate to `/c/{subagent-slug}`
- Maybe show in sidebar or modal instead of navigating away?

## Data Needed

From `SubAgentSpec` / `SubAgentResult`:
```rust
agent_id: String,           // The conversation ID
task: String,
```

The `agent_id` IS the conversation_id. Just need to get the slug or use the ID directly.

## Design Options

1. **Navigate away**: Simple, but loses parent context
2. **Split view**: Show subagent in sidebar panel
3. **Modal**: Show subagent in overlay
4. **New tab**: `target="_blank"` link

Start with option 1 (navigate), consider split view later.

## Acceptance Criteria

- [ ] Subagent items are clickable
- [ ] Click navigates to subagent conversation
- [ ] Back button returns to parent
- [ ] Works for both running and completed subagents
