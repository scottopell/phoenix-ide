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
human-in-the-loop gate between Explore (reading) and Work (writing). `propose_plan`
is a pure data carrier (like submit_result) -- no side effects until the user
approves. On approval, write the task file, commit to main, create a branch, and
switch to Work mode.

## Context

Read first:
- `specs/projects/requirements.md` -- REQ-PROJ-003, REQ-PROJ-004, REQ-PROJ-006, REQ-PROJ-012
- `specs/bedrock/requirements.md` -- REQ-BED-028
- `specs/bedrock/design.md` -- "Task Approval State" section, submit_result interception pattern

## Dependencies

- Task 0601 (M1: project entity + Explore mode) -- DONE

## Design Decisions

1. **propose_plan is a pure data carrier.** Intercepted at the LlmResponse handler
   (same pattern as submit_result for sub-agents). Never enters ToolExecuting or
   the tool executor. No file writes, no git operations. The plan exists only in
   the AwaitingTaskApproval state until approved.

2. **All git operations on approve only.** Writing the task file, committing to
   main, creating the branch, and checking it out all happen in the approval
   handler. Reject and feedback are free -- nothing to clean up.

3. **No update_task tool.** During Work mode, agents update task files via the
   patch tool like any other file. Task file changes live on the branch and merge
   with code in M4.

4. **submit_result interception pattern.** The LlmResponse handler already
   intercepts submit_result before entering ToolExecuting. propose_plan follows
   the same path: detect tool_use, validate it's the only tool, extract data,
   persist ToolRound, transition to AwaitingTaskApproval.

## What to Do

### Backend -- State Machine

1. **New ToolInput variant:** `ToolInput::ProposePlan { title, priority, plan }`.
   Add parsing in `ToolInput::from_name_and_value`.

2. **LlmResponse interception:** In the `LlmRequesting + LlmResponse` transition
   arm, add a check before the existing sub-agent branch:
   - Detect `propose_plan` tool_use
   - Validate it's the only tool (error if not)
   - Extract title, priority, plan from input
   - Build synthetic `ToolResult::success(tool_id, "Plan submitted for review")`
   - Persist `CheckpointData::ToolRound(assistant_msg, [tool_result])`
   - Transition to `AwaitingTaskApproval { title, priority, plan }`

3. **AwaitingTaskApproval state variant:** Carries title (String), priority
   (String), plan (String). All serializable, no channels, no file paths.

4. **New UserEvent variants:** `TaskApprovalResponse { outcome }` where outcome
   is `Approved | Rejected | FeedbackProvided { annotations: String }`.

5. **Transitions from AwaitingTaskApproval:**
   - Approved: emit effects for git operations (write task file, commit, branch,
     checkout), transition to Idle with conv_mode = Work
   - FeedbackProvided: deliver annotations as user message, transition to
     Idle (Explore)
   - Rejected: transition to Idle (Explore), no effects
   - UserCancel: treat as Rejected

6. **DisplayState:** Add `AwaitingApproval` variant (user must act).

### Backend -- Git Operations (Approve handler)

7. **On approve, the executor runs these git operations in sequence:**
   - Assign next task ID (scan `tasks/` for highest NNNN)
   - Derive slug from title
   - Write task file to `tasks/{NNNN}-{priority}-in-progress--{slug}.md`
   - `git add tasks/{file} && git commit --only tasks/{file} -m "task {NNNN}: {title}"`
   - `git branch task-{NNNN}-{slug}` from main HEAD
   - `git checkout task-{NNNN}-{slug}`
   - If any step fails, roll back prior steps (delete branch if created)

8. **One Work conversation per project:** Check DB before executing approve.
   If another conversation for the same project has conv_mode = Work, reject
   with actionable error. This is a runtime check, not a state machine concern.

### Backend -- SSE

9. **`task_approval_requested` event:** Emitted on entering AwaitingTaskApproval.
   Carries title, priority, plan text. On server restart in AwaitingTaskApproval,
   re-emit on UI reconnect (data is in the serialized ConvState).

### Frontend

10. **`awaiting_task_approval` state variant:** Add to ConversationState union.
    Handle in all switches (StateBar, breadcrumb, utils). DisplayState mapping.

11. **ProseReader task approval mode:** Extend with `taskApproval` prop adding
    Approve, Reject buttons alongside existing Send Notes. Render plan content
    from the SSE event payload (not from a file on disk).

12. **Auto-open on state transition:** ConversationPage watches for
    `awaiting_task_approval` phase and opens ProseReader in approval mode.

13. **Work mode indicator:** Show branch name in StateBar when in Work mode.
    Add `branch_name` field to Conversation API response.

14. **API endpoints:** `POST /api/conversations/:id/approve-task`,
    `POST /api/conversations/:id/reject-task`. Feedback uses existing
    message endpoint.

## Acceptance Criteria

- [ ] `propose_plan` intercepted at LlmResponse, never enters ToolExecuting
- [ ] propose_plan with other tools in same response produces error
- [ ] AwaitingTaskApproval state carries plan data, fully serializable
- [ ] Approve writes task file, commits to main, creates branch, checks it out
- [ ] Approve transitions to Work mode with branch name in response
- [ ] Feedback closes prose reader, delivers annotations, returns to Explore
- [ ] Agent can call propose_plan again after feedback (revision loop works)
- [ ] Reject returns to Explore with no git operations
- [ ] Server restart restores AwaitingTaskApproval from serialized state
- [ ] Only one Work conversation per project (runtime check)
- [ ] Branch name collision handled (check before create)
- [ ] `./dev.py check` passes

## Scope boundary (M2 vs M3)

M2 creates the branch and checks it out in the main checkout. The agent works on
the branch with full tool access, commits freely. One Work conversation at a time.

M3 adds `git worktree add` so each Work conversation gets a separate physical
directory. This enables parallel Work conversations and CWD-scoped write sandboxing.
