---
created: 2026-03-13
priority: p2
status: done
artifact: pending
---

# Show task title in StateBar for Work conversations

## Problem

After task approval, the StateBar shows branch name and worktree path but not
what the task actually is. The user has to remember or scroll back to find the
task title. The branch slug (e.g., `task-0001-systematic-benchmarking`) is a
poor substitute for the full title.

## What to Do

Expose the task title from `AwaitingTaskApproval` state data through to the
Work mode metadata. Options:

1. Store task title in `ConvMode::Work` (add `task_title: String` field)
2. Derive from the task file in the worktree at render time

Option 1 is simpler. Add `task_title` to `ConvMode::Work`, populate it during
`execute_approve_task`, expose in `EnrichedConversation` and
`ConversationMetadataUpdate`.

In the StateBar, show the task title as the primary label, with branch name
as secondary/tooltip. Example: "Add greeting module" with tooltip showing
the branch and worktree path.

## Acceptance Criteria

- [ ] Task title visible in StateBar for Work conversations
- [ ] Branch name available in tooltip
- [ ] Title persists across server restarts
- [ ] Non-Work conversations unaffected
