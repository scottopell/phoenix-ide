---
created: 2026-03-05
number: 602
priority: p1
status: ready
slug: projects-m2-task-approval
title: "Projects M2: create_task tool + task approval UI"
---

# Projects M2: Task Creation and Approval

## Summary

Add the `create_task` tool and the AwaitingTaskApproval state. This is the
human-in-the-loop gate between Explore (reading) and Work (writing).

## Context

Read first:
- `specs/projects/requirements.md` — REQ-PROJ-003, REQ-PROJ-004, REQ-PROJ-006, REQ-PROJ-012
- `specs/bedrock/design.md` — "Task Approval State" section
- `specs/bedrock/requirements.md` — REQ-BED-028

## Dependencies

- Task 0601 (M1: project entity + Explore mode)

## What to Do

### Backend

1. **`create_task` tool:** Accepts title, priority, and plan. Writes a task file
   to the project's `tasks/` directory using taskmd format (4-digit number,
   frontmatter). Commits to main. Returns the task ID.

2. **AwaitingTaskApproval state:** New ConvState variant. Entered when create_task
   completes. Holds task_id, task_path, and a oneshot reply channel. Valid incoming
   events: Approved, FeedbackProvided, Rejected.

3. **`update_task` tool:** For status and progress updates on the task file. Work
   mode and parent conversations only. Sub-agents rejected.

4. **Transition logic:**
   - create_task from Explore → AwaitingTaskApproval
   - Approved → Work mode (worktree creation deferred to M3 — for now, Work mode
     uses cwd directly)
   - FeedbackProvided → Explore (agent can revise and call create_task again)
   - Rejected → Explore (task file deleted)

5. **SSE events:** `task_approval_requested` event with task file content for the
   UI. On server restart in AwaitingTaskApproval, re-emit this event.

### Frontend

6. **Task approval UI:** When conversation enters AwaitingTaskApproval, show the
   task file in the prose reader. Provide Approve, Reject, and annotate-and-send-
   feedback actions.

7. **Prose reader integration:** The prose reader already supports line-level
   annotations. Wire the annotations as the feedback payload sent back to the agent.

## Acceptance Criteria

- [ ] `create_task` tool available in Explore mode, blocked in Work mode
- [ ] Task file written to `tasks/` in taskmd format with correct next number
- [ ] Conversation enters AwaitingTaskApproval and pauses
- [ ] UI shows task file in prose reader with approve/reject/feedback
- [ ] Approve transitions to Work mode (cwd-based for now, worktree in M3)
- [ ] Feedback returns annotations to agent, stays Explore
- [ ] Reject deletes task file, stays Explore
- [ ] Server restart restores AwaitingTaskApproval state
- [ ] `./dev.py check` passes

## Value Delivered

Human-in-the-loop. Agent proposes, user reviews. No writes happen without approval.
Even without worktree isolation (M3), this milestone delivers the core approval
workflow.
