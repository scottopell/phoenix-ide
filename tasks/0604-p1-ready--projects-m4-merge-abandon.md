---
created: 2026-03-05
number: 604
priority: p1
status: ready
slug: projects-m4-merge-abandon
title: "Projects M4: Complete and abandon flows (task lifecycle)"
---

# Projects M4: Complete and Abandon

## Summary

Two user-initiated actions on idle Work conversations: **Complete** (squash merge
into base branch, cleanup, Terminal) and **Abandon** (destructive discard, cleanup,
Terminal). Plus a passive **commits-behind indicator** for awareness.

No new ConvState variant. No agent-initiated review. No diff review gate. No
return-to-Explore complexity.

## Context

Read first:
- `specs/projects/requirements.md` -- REQ-PROJ-009 (Complete), REQ-PROJ-010
  (Abandon), REQ-PROJ-011 (commits-behind), REQ-PROJ-017 (base_branch)
- `specs/bedrock/requirements.md` -- REQ-BED-029 (Terminal on resolution)
- `specs/bedrock/design.md` -- "Task Completion and Abandon" section
- `specs/projects/design.md` -- Executor git operations table
- Task 0603 (M3) -- worktree isolation (must be complete)

## Dependencies

- Task 0603 (M3: worktree isolation) -- DONE

## Implementation

### Batch 1: Backend -- base_branch + Complete flow

**1a. Add `base_branch` to `ConvMode::Work`** -- `src/db/schema.rs`

Add `base_branch: String` and `task_number: u32` fields with `#[serde(default)]`
rollout shim. Record the checked-out branch at approval time. Update
`execute_approve_task_blocking` in `src/runtime/executor.rs` to detect current
branch via `git rev-parse --abbrev-ref HEAD` and pass it through
`TaskApprovalResult`. Store in `ConvMode::Work` alongside `branch_name` and
`worktree_path`.

Spec: REQ-PROJ-017

**1b. Complete API endpoint** -- `src/api/handlers.rs`

New endpoint: `POST /api/conversations/:id/complete-task`

Validation:
- Conversation exists and is in Work mode (conv_mode is Work)
- Conversation state is Idle (agent not working)
- Project-scoped conversation

Response: `{ success: true, commit_message: "..." }` on pre-check pass, or
error with actionable message.

Flow:
1. Pre-checks (dispatched to executor via new event):
   - `git status --porcelain` in worktree -- block if dirty
   - `git merge-tree $(git merge-base base_branch HEAD) base_branch HEAD` or
     `git merge --no-commit --no-ff base_branch` dry run -- block if conflicts
2. Generate commit message: LLM call with `git diff base_branch...HEAD` +
   semantic commit instructions (REQ-PROJ-009). Use the conversation's configured
   model for the commit message generation.
3. Return commit message to frontend for confirmation

Spec: REQ-PROJ-009

**1c. Complete confirmation endpoint** -- `src/api/handlers.rs`

New endpoint: `POST /api/conversations/:id/confirm-complete`

Body: `{ commit_message: "..." }`

Executor sequence (blocking, on spawn_blocking):
1. Acquire per-project mutex before git operations on main checkout
2. Check main checkout is clean (`git status --porcelain` on repo root)
3. `git checkout base_branch` (in main checkout, not worktree)
4. `git merge --squash task_branch`
5. `git commit -m "{commit_message}"`
6. `git worktree remove {path} --force`
7. `git branch -D {branch}`
8. Release mutex after git sequence completes

After success:
- Transition conversation to Terminal state
- Inject system message: "Task completed. Squash merged to {base_branch} as {short_sha}."
- Broadcast SSE state_change + conversation_update

Spec: REQ-PROJ-009, REQ-BED-029

**1d. Task file done-status nudge** -- `src/api/handlers.rs`

In the complete-task endpoint, before returning the commit message, check if
the task file in the worktree has `status: done` in its frontmatter. If not,
include `task_not_done: true` in the response so the frontend can show a nudge.

Spec: REQ-PROJ-009

### Batch 2: Backend -- Abandon flow

**2a. Abandon API endpoint** -- `src/api/handlers.rs`

New endpoint: `POST /api/conversations/:id/abandon-task`

Validation: same as complete (Work mode, Idle state, project-scoped).

Executor sequence (blocking):
1. `git worktree remove {path} --force`
2. `git branch -D {branch}`
3. Acquire per-project mutex before git operations on main checkout
4. Check main checkout is clean (`git status --porcelain` on repo root)
5. `git checkout base_branch` (in main checkout)
6. Update task file status to `wont-do`:
   - Identify task file on base_branch by scanning `tasks/` for files whose
     4-digit prefix matches the task number stored in `ConvMode::Work`.
     Task IDs are immutable -- even if the agent renamed the file on the task
     branch, the base_branch copy retains the original name.
   - `git mv tasks/{old_filename} tasks/{new_filename_with_wontdo}` (taskmd
     convention: status in filename)
   - `git commit -m "task {NNNN}: mark wont-do"`
7. Release mutex after git sequence completes

After success:
- Transition conversation to Terminal state
- Inject system message: "Task abandoned. Worktree and branch deleted."
- Broadcast SSE state_change + conversation_update

Spec: REQ-PROJ-010, REQ-BED-029

### Batch 3: Backend -- Commits-behind indicator

**3a. Commits-behind calculation** -- `src/api/handlers.rs` or new module

New function: `commits_behind(repo_root, base_branch, task_branch) -> u32`

Implementation: `git rev-list --count task_branch..base_branch`

Called on SSE init and periodically.

**3b. SSE init integration** -- `src/api/handlers.rs`

Add `commits_behind: u32` to the SSE init payload for Work conversations.
Compute at SSE connect time. Zero for non-Work conversations.

**3c. Periodic polling** -- `src/runtime/executor.rs` or `src/api/sse.rs`

During SSE streaming for Work conversations, poll every ~60s. If the count
changes, emit a new SSE event (e.g., `commits_behind` event type with the
updated count).

Spec: REQ-PROJ-011

### Batch 4: Frontend -- Complete + Abandon UI

**4a. Complete button** -- `ui/src/components/StateBar.tsx` or new component

Show "Complete" button on idle Work conversations. Clicking triggers:
1. `POST /api/conversations/:id/complete-task`
2. If `task_not_done`, show nudge banner
3. Show editable commit message dialog (pre-filled from response)
4. While the commit message confirmation dialog is open, register a
   `beforeunload` handler to warn the user if they attempt to close or
   navigate away from the page
5. On confirm: `POST /api/conversations/:id/confirm-complete`
6. On success: conversation transitions to Terminal via SSE

**4b. Abandon button** -- same location as Complete

Show "Abandon" button on idle Work conversations. Clicking triggers:
1. Confirmation dialog: "This permanently deletes all work in this worktree.
   The task will be marked wont-do. This cannot be undone."
2. On confirm: `POST /api/conversations/:id/abandon-task`
3. On success: conversation transitions to Terminal via SSE

**4c. Commits-behind badge** -- `ui/src/components/StateBar.tsx`

Show "N behind" badge next to branch name when commits_behind > 0.
Update from SSE events.

**4d. Conversation update handling** -- `ui/src/api.ts`

Add `commits_behind` SSE event type. Handle in useConnection + atom reducer.

Spec: REQ-PROJ-009, REQ-PROJ-010, REQ-PROJ-011

## Files to Modify

| File | Change |
|------|--------|
| `src/db/schema.rs` | Add `base_branch` to ConvMode::Work |
| `src/runtime/executor.rs` | Record base_branch at approval, complete/abandon executor logic |
| `src/api/handlers.rs` | Three new endpoints, commits-behind in SSE init |
| `src/api/types.rs` | Request/response types for new endpoints |
| `src/api/sse.rs` | commits_behind SSE event |
| `src/runtime.rs` | SseEvent variant for commits_behind |
| `ui/src/api.ts` | New API methods, SSE event types |
| `ui/src/components/StateBar.tsx` | Complete/Abandon buttons, commits-behind badge |
| `ui/src/conversation/atom.ts` | commits_behind state, SSE handlers |
| `ui/src/hooks/useConnection.ts` | commits_behind SSE listener |

## Acceptance Criteria

- [ ] `base_branch` stored in ConvMode::Work at approval time (REQ-PROJ-017)
- [ ] Complete pre-checks block on dirty tree and merge conflicts (REQ-PROJ-009)
- [ ] LLM generates semantic commit message from diff (REQ-PROJ-009)
- [ ] User can edit commit message before confirming (REQ-PROJ-009)
- [ ] Squash merge into base_branch, worktree+branch deleted (REQ-PROJ-009)
- [ ] Conversation goes Terminal after Complete (REQ-BED-029)
- [ ] Abandon confirmation dialog warns of permanent deletion (REQ-PROJ-010)
- [ ] Abandon deletes worktree+branch, updates task to wont-do (REQ-PROJ-010)
- [ ] Conversation goes Terminal after Abandon (REQ-BED-029)
- [ ] Commits-behind badge shows on Work conversations (REQ-PROJ-011)
- [ ] Commits-behind updates on SSE connect + ~60s poll (REQ-PROJ-011)
- [ ] Complete/Abandon buttons disabled while agent is working
- [ ] Task-not-done nudge shown (non-blocking) at Complete time
- [ ] `./dev.py check` passes

## Value Delivered

Complete lifecycle: Explore -> propose -> approve -> work -> complete -> done.
Every step has a clean exit. Abandon preserves the attempt record without
carrying code forward. Users always know if their base branch has advanced.
