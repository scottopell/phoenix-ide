---
created: 2026-03-05
number: 602
priority: p1
status: ready
slug: projects-m2-task-approval
title: "Projects M2: propose_plan tool + task approval workflow"
---

# Projects M2: Task Creation and Approval

## Summary

Add the `propose_plan` tool and the AwaitingTaskApproval state. This is the
human-in-the-loop gate between Explore (reading) and Work (writing). On approval,
create a dedicated branch and switch to Work mode in the existing checkout.

## Context

Read first:
- `specs/projects/requirements.md` -- REQ-PROJ-003, REQ-PROJ-004, REQ-PROJ-006, REQ-PROJ-012
- `specs/bedrock/requirements.md` -- REQ-BED-028

## Dependencies

- Task 0601 (M1: project entity + Explore mode) -- DONE

## What to Do

### Backend

1. **`propose_plan` tool:** Available in Explore mode only. Accepts title (string),
   priority (p0-p3), plan (string), and optional task_id (for revisions after
   feedback). On first call: assigns next sequential task ID, writes task file to
   `tasks/` in taskmd format, commits to main via `git commit --only <task-file>`.
   On revision: updates existing task file, commits to main the same way. Returns
   task_id. Rejected if called outside Explore mode or by a sub-agent.

2. **AwaitingTaskApproval state:** New ConvState variant holding task_id and
   task_file_path. Entered when propose_plan commits successfully. Valid incoming
   events:

   - **Approve:** Create branch `task-{NNNN}-{slug}` from main HEAD. Checkout
     branch. Set conv_mode = Work. Resume agent with "Task approved, you are on
     branch task-{NNNN}-{slug}."
   - **Feedback:** Close prose reader. Deliver annotations as user message.
     Return to Explore/Idle. Agent can revise and call propose_plan(task_id=...)
     again.
   - **Reject:** Update task file status to `abandoned` on main. Return to
     Explore/Idle. Agent gets rejection result.

   DB persistence: task_id and task_file_path serialized in ConvState. On server
   restart, reconstruct state from DB, read task file from disk, re-emit
   `task_approval_requested` SSE event on UI reconnect.

3. **`update_task` tool:** Available in Work mode only. Accepts status (optional
   enum) and progress (optional string). Presents update to user for approval
   before committing. On approval: updates task file on main via
   `git commit --only`, renames file if status changes. Rejected if called outside
   Work mode or by a sub-agent.

4. **Branch creation on approve:** `git branch task-{NNNN}-{slug}` from main HEAD,
   then `git checkout task-{NNNN}-{slug}`. One Work conversation per project at a
   time (shared checkout constraint until M3 adds worktrees). Error if a second
   task is approved while one is active.

5. **SSE events:** `task_approval_requested` with task file content. On server
   restart in AwaitingTaskApproval, re-emit on reconnect.

### Frontend

6. **Task approval UI:** When conversation enters AwaitingTaskApproval, open task
   file in prose reader. Show Approve, Reject, and annotation feedback actions.
   On feedback: close prose reader, send annotations as message. On next
   propose_plan: reopen prose reader with updated content.

7. **Work mode indicator:** When conv_mode transitions to Work, show branch name
   in conversation header. Update mode badge.

## Acceptance Criteria

- [ ] `propose_plan` tool available in Explore mode, blocked in Work/Standalone
- [ ] Task file written to `tasks/` in taskmd format with correct next number
- [ ] `git commit --only` used -- user's staging area untouched
- [ ] Conversation enters AwaitingTaskApproval and pauses
- [ ] UI shows task file in prose reader with approve/reject/feedback
- [ ] Approve creates branch and transitions to Work mode
- [ ] Feedback closes prose reader, delivers annotations, returns to Explore
- [ ] Agent can call propose_plan again after feedback (revision loop works)
- [ ] Reject marks task abandoned, stays Explore
- [ ] Server restart restores AwaitingTaskApproval state correctly
- [ ] update_task presents changes for user approval before committing to main
- [ ] Only one Work conversation per project (error on second approve)
- [ ] `./dev.py check` passes

## Value Delivered

Human-in-the-loop approval workflow. Agent proposes, user reviews with line-level
feedback, agent can revise. No writes happen without explicit approval. Branch
isolation proves the full logical flow (Explore -> propose -> approve -> Work)
even before M3 adds physical worktree isolation.

## Scope boundary (M2 vs M3)

M2 creates the branch and checks it out in the main checkout. The agent works on
the branch with full tool access, commits freely. One Work conversation at a time.

M3 adds `git worktree add` so each Work conversation gets a separate physical
directory. This enables parallel Work conversations and CWD-scoped write sandboxing.
