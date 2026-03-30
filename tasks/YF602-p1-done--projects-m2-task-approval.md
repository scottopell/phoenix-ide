---
created: 2026-03-05
priority: p1
status: done
artifact: pending
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
- `specs/prose-feedback/requirements.md` -- REQ-PF-015, REQ-PF-016 (task approval UI)

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

4. **Approval outcomes are system messages.** When the user approves, rejects, or
   sends feedback, the outcome is injected as a new system/user message into the
   conversation history. The original propose_plan tool result ("Plan submitted
   for review") is never retroactively modified.

5. **Prevent-close-without-action is core.** The prose reader in task approval
   mode must suppress back/Escape-to-close. The user MUST choose Approve,
   Discard, or Send Feedback. This is not polish -- the UX is broken without it.

## What to Do

### Backend -- State Machine

1. **New ToolInput variant:** `ToolInput::ProposePlan { title, priority, plan }`.
   Add parsing in `ToolInput::from_name_and_value`.

2. **LlmResponse interception:** In the `LlmRequesting + LlmResponse` transition
   arm, add a check before the existing sub-agent branch:
   - Detect `propose_plan` tool_use
   - Validate it's the only tool in the response (error if not)
   - Extract title, priority, plan from input
   - Build synthetic `ToolResult::success(tool_id, "Plan submitted for review")`
   - Persist `CheckpointData::ToolRound(assistant_msg, [tool_result])`
   - Transition to `AwaitingTaskApproval { title, priority, plan }`

3. **AwaitingTaskApproval state variant:** Carries title (String), priority
   (String), plan (String). All serializable, no channels, no file paths.

4. **New UserEvent variant:** `TaskApprovalResponse { outcome }` where outcome
   is `Approved | Rejected | FeedbackProvided { annotations: String }`.

5. **Transitions from AwaitingTaskApproval:**
   - Approved: emit effects for git operations, inject system message
     "Task approved. You are on branch task-{NNNN}-{slug}.", transition to
     Idle with conv_mode = Work
   - FeedbackProvided: inject annotations as user message, transition to
     Idle (Explore). Agent may call propose_plan again.
   - Rejected: inject system message "Task rejected.", transition to
     Idle (Explore). No effects.
   - UserCancel: treat as Rejected
   - UserMessage: reject ("Conversation is awaiting task approval")
   - TriggerContinuation: reject

6. **DisplayState:** Add `AwaitingApproval` variant (user must act).

### Backend -- Git Operations (Approve handler)

7. **On approve, the executor runs these git operations in sequence:**
   - Assign next task ID (scan `tasks/` for highest NNNN, serialized via
     project-level in-memory Mutex)
   - Derive slug from title
   - `mkdir -p tasks/` if directory does not exist
   - Write task file to `tasks/{NNNN}-{priority}-in-progress--{slug}.md`
   - `git add tasks/{file} && git commit --only tasks/{file}`
   - Check for branch name collision: if `task-{NNNN}-{slug}` exists and is
     fully merged, delete it. If not merged, error with actionable message.
   - `git branch task-{NNNN}-{slug}` from main HEAD
   - Check for dirty working tree before checkout. If dirty, error with
     "please commit or stash your changes before approving"
   - `git checkout task-{NNNN}-{slug}`
   - On any failure: log what succeeded, return error. Partial state
     (e.g., branch created but not checked out) is self-healing on retry
     via the collision check.

8. **One Work conversation per project:** Check DB before executing approve.
   If another conversation for the same project has conv_mode = Work, reject
   with actionable error. This is a runtime/API check, not state machine.

9. **First task welcome wizard:** When `tasks/` doesn't exist (first task on
   this project), after creating the directory, emit a frontend event to show
   a brief welcome modal explaining the task system and linking to taskmd
   tooling. Frame as: "Phoenix uses this directory to track work plans.
   You, other developers, and other tools can read and create task files too."

### Backend -- SSE

10. **`task_approval_requested` via state_change:** Emitted as part of the
    state_change SSE event. The `awaiting_task_approval` state payload carries
    title, priority, plan text. On server restart in AwaitingTaskApproval,
    re-emit on UI reconnect. On page reload, REST API returns same data in
    the conversation state field.

### Frontend

11. **`awaiting_task_approval` state variant:** Add to ConversationState union.
    Handle in all switches (StateBar, breadcrumb, utils). Map to
    DisplayState `AwaitingApproval`.

12. **ProseReader task approval mode (REQ-PF-015, REQ-PF-016):** Extend with
    `taskApproval` prop adding Approve, Discard, Send Feedback buttons.
    Render plan content from the state_change payload (not file on disk).
    Suppress back/Escape-to-close -- user MUST choose an action.
    Discard shows confirmation dialog.

13. **Auto-open on state transition:** ConversationPage watches for
    `awaiting_task_approval` phase and opens ProseReader in approval mode.
    On page reload / reconnect: same path via REST state or SSE init.

14. **Work mode indicator:** Show branch name in StateBar when in Work mode.
    Add `branch_name` field to Conversation API response.

15. **API endpoints:** `POST /api/conversations/:id/approve-task`,
    `POST /api/conversations/:id/reject-task`. Feedback uses existing
    message endpoint with annotations payload.

## Acceptance Criteria

- [ ] `propose_plan` intercepted at LlmResponse, never enters ToolExecuting
- [ ] propose_plan with other tools in same response produces error
- [ ] AwaitingTaskApproval state carries plan data, fully serializable
- [ ] Prose reader opens automatically, cannot be dismissed without action
- [ ] Approve writes task file, commits to main, creates branch, checks it out
- [ ] Approve injects system message with branch name
- [ ] Feedback closes prose reader, delivers annotations as user message
- [ ] Agent can call propose_plan again after feedback (revision loop works)
- [ ] Reject returns to Explore with system message, no git operations
- [ ] Server restart restores AwaitingTaskApproval from serialized state
- [ ] Page reload correctly reopens prose reader with plan content
- [ ] Only one Work conversation per project (runtime check)
- [ ] Branch name collision handled (check before create)
- [ ] Dirty working tree detected and reported (no silent stash)
- [ ] First-task welcome wizard shown when tasks/ doesn't exist
- [ ] Branch name shown in StateBar during Work mode
- [ ] `./dev.py check` passes

## Scope boundary (M2 vs M3)

M2 creates the branch and checks it out in the main checkout. The agent works on
the branch with full tool access, commits freely. One Work conversation at a time.

M3 adds `git worktree add` so each Work conversation gets a separate physical
directory. This enables parallel Work conversations and CWD-scoped write sandboxing.
