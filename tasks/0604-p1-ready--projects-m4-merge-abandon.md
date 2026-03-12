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

## Design Decisions (resolved)

These were resolved during discovery and stress-testing. Do not re-litigate.

1. **New `ConvState::Terminal` variant.** Existing terminal states (`Completed`,
   `Failed`) are sub-agent-specific and cause the executor loop to exit. A new
   `Terminal` variant is needed that: displays as terminal, rejects new messages,
   and keeps the executor alive long enough to broadcast final SSE events before
   exiting. Add it to `state.rs`, `DisplayState`, `reset_all_to_idle` (preserve
   on restart, same as `ContextExhausted`).

2. **Race window between complete-task and confirm-complete is accepted.** There
   is no cron-like mechanism that triggers events while Idle. The confirm endpoint
   re-validates Idle state as a safety check but no locking needed.

3. **Global mutex (reuse `TASK_APPROVAL_MUTEX`).** Complete/Abandon/Approve are
   all rare operations. A single global mutex serializing git-on-main-checkout
   operations is sufficient. No per-project map needed.

4. **Empty `base_branch` on existing Work rows: revert to Explore.** Same pattern
   as empty `worktree_path` in M3 reconciliation. Add to startup reconciliation.

5. **`task_number: u32` stored in `ConvMode::Work`.** Format to 4-digit zero-padded
   (`{:04}`) when scanning task files. The spec's `task_id: String` in REQ-PROJ-017
   is conceptually the same; use `u32` in code for consistency with existing
   `scan_highest_task_number`.

6. **Abandon ordering: worktree delete BEFORE mutex.** If worktree deletion
   succeeds but task-file rename fails (e.g., dirty main checkout), the conversation
   stays in Work mode with a dangling worktree_path. On next server restart,
   reconciliation detects the missing worktree and reverts to Explore. This is
   acceptable -- the user can retry Abandon after fixing the main checkout.

7. **Commit message LLM call: diff truncation.** If `git diff base_branch...HEAD`
   exceeds 50KB, fall back to `git diff --stat base_branch...HEAD` (summary only).
   Use a one-shot prompt with the diff + semantic commit instructions. No
   conversation history needed -- the diff is self-contained context.

8. **Task file identification: scan by 4-digit prefix.** Both Complete (nudge
   check) and Abandon (rename) locate the task file by scanning `tasks/` for
   files whose prefix matches `{task_number:04}`. If not found, skip silently
   (nudge) or skip rename with a warning log (abandon).

9. **Commits-behind SSE: use `ConversationUpdate`.** Add `commits_behind: Option<u32>`
   to `ConversationMetadataUpdate`. Reuse the existing SSE event type rather than
   adding a new variant.

10. **Commit message dialog: modal overlay.** Build a simple modal with a textarea
    for the commit message and Confirm/Cancel buttons. No existing modal component
    exists in the codebase -- create one. Also guard against React Router navigation
    (not just `beforeunload`) using a `useBlocker` or `Prompt` equivalent.

## Implementation

### Batch 1: Backend -- base_branch + ConvState::Terminal + Complete flow

**1a. Add `ConvState::Terminal`** -- `src/state_machine/state.rs`

New variant: `Terminal`. Map to `DisplayState::Terminal` (already exists). Add to
`reset_all_to_idle` exclusion list alongside `ContextExhausted` and
`AwaitingTaskApproval`. Add to `StepResult` as terminal. Update all exhaustive
matches (`proptests.rs`, `transition.rs` -- reject all events from Terminal).

**1b. Add `base_branch` and `task_number` to `ConvMode::Work`** -- `src/db/schema.rs`

Add `base_branch: String` and `task_number: u32` fields with `#[serde(default)]`
rollout shim. Add `base_branch()` and `task_number()` accessors. Update
`execute_approve_task_blocking` in `src/runtime/executor.rs` to detect current
branch via `git rev-parse --abbrev-ref HEAD` and pass it through
`TaskApprovalResult`. Store in `ConvMode::Work` alongside `branch_name` and
`worktree_path`.

Update startup reconciliation in `main.rs`: revert Work conversations with empty
`base_branch` to Explore (same pattern as empty `worktree_path`).

Spec: REQ-PROJ-017

**1c. Complete API endpoint** -- `src/api/handlers.rs`

New endpoint: `POST /api/conversations/:id/complete-task`

Validation:
- Conversation exists and is in Work mode (conv_mode is Work)
- Conversation state is Idle (agent not working)
- Project-scoped conversation

Pre-checks (blocking, on spawn_blocking):
1. `git status --porcelain` in worktree -- block if dirty
2. Conflict detection: `git merge-tree $(git merge-base base_branch HEAD)
   base_branch HEAD` in repo root -- block if output indicates conflicts

Task file nudge: scan worktree `tasks/` for file with matching task_number prefix.
If found and frontmatter status is not `done`, include `task_not_done: true` in
response.

Commit message generation:
1. `git diff base_branch...HEAD` in worktree. If output > 50KB, fall back to
   `git diff --stat base_branch...HEAD`.
2. LLM call with diff + semantic commit prompt (see
   `~/.config/home-dir-configs/claude/commands/semantic-commit.md` for style).
   Use conversation's configured model. One-shot, no conversation history.
3. Return `{ success: true, commit_message: "...", task_not_done: false }`.

On pre-check failure: return `{ error: "...", error_type: "dirty_worktree" |
"merge_conflicts" | "dirty_main_checkout" }` with HTTP 409.

Spec: REQ-PROJ-009

**1d. Complete confirmation endpoint** -- `src/api/handlers.rs`

New endpoint: `POST /api/conversations/:id/confirm-complete`

Body: `{ commit_message: "..." }`

Re-validate: conversation is Work mode AND Idle state (race guard).

Executor sequence (blocking, on spawn_blocking, under `TASK_APPROVAL_MUTEX`):
1. Acquire global mutex
2. `git status --porcelain` on repo root -- block if dirty
3. `git checkout base_branch` (in main checkout)
4. `git merge --squash task_branch`
5. `git commit -m "{commit_message}"`
6. Record short SHA: `git rev-parse --short HEAD`
7. `git worktree remove {worktree_path} --force`
8. `git branch -D {branch_name}`
9. Release mutex

After success:
- `self.state = ConvState::Terminal` (direct mutation, then persist)
- Update conv_mode to Explore (clear Work fields)
- Inject system message: "Task completed. Squash merged to {base_branch} as {sha}."
- Broadcast SSE state_change + conversation_update

Spec: REQ-PROJ-009, REQ-BED-029

### Batch 2: Backend -- Abandon flow

**2a. Abandon API endpoint** -- `src/api/handlers.rs`

New endpoint: `POST /api/conversations/:id/abandon-task`

Validation: same as complete (Work mode, Idle state, project-scoped).

Frontend sends this only after the user confirms the destructive action dialog.

Executor sequence (blocking, on spawn_blocking):
1. `git worktree remove {worktree_path} --force`
2. `git branch -D {branch_name}`
3. Acquire global mutex (`TASK_APPROVAL_MUTEX`)
4. `git status --porcelain` on repo root -- block if dirty (release mutex, return error)
5. `git checkout base_branch` (in main checkout)
6. Scan `tasks/` for file matching `{task_number:04}-*`. If found:
   - Parse filename: `NNNN-pX-status--slug.md`
   - Compute new filename with `wont-do` status
   - `git mv tasks/{old} tasks/{new}`
   - `git commit -m "task {NNNN:04}: mark wont-do"`
   - If not found: log warning, skip rename (task file may have been manually deleted)
7. Release mutex

After success:
- `self.state = ConvState::Terminal` (direct mutation, then persist)
- Update conv_mode to Explore
- Inject system message: "Task abandoned. Worktree and branch deleted."
- Broadcast SSE state_change + conversation_update

Spec: REQ-PROJ-010, REQ-BED-029

### Batch 3: Backend -- Commits-behind indicator

**3a. Commits-behind calculation** -- new function in `src/api/handlers.rs` or utility

`fn commits_behind(repo_root: &Path, base_branch: &str, task_branch: &str) -> u32`

Implementation: `git rev-list --count {task_branch}..{base_branch}` run in repo_root.
Returns 0 on any error (branch deleted, etc.).

**3b. SSE init integration** -- `src/api/handlers.rs`

For Work conversations, compute `commits_behind` at SSE connect time. Include in
init event via `ConversationMetadataUpdate` (add `commits_behind: Option<u32>` field).

**3c. Periodic polling** -- spawned task alongside SSE stream

When creating the SSE stream for a Work conversation, spawn a periodic task that:
1. Sleeps ~60s
2. Computes `commits_behind`
3. If value changed from last emission, broadcasts `SseEvent::ConversationUpdate`
   with `commits_behind` field
4. Repeats until broadcast channel closes (client disconnects)

Spec: REQ-PROJ-011

### Batch 4: Frontend -- Complete + Abandon UI

**4a. Complete button** -- `ui/src/components/` (new `WorkActions.tsx` component)

Show "Complete" and "Abandon" buttons on idle Work conversations. Render below
the StateBar or inline in the StateBar right section.

Complete flow:
1. Click "Complete" -> spinner -> `POST /api/conversations/:id/complete-task`
2. If error: show inline error message (distinguish dirty_worktree vs merge_conflicts)
3. If `task_not_done`: show dismissible nudge banner
4. Show modal dialog with editable commit message textarea + Confirm/Cancel
5. Register `beforeunload` handler + React Router navigation blocker while modal open
6. On confirm: `POST /api/conversations/:id/confirm-complete` -> Terminal via SSE

**4b. Abandon button** -- same component

1. Click "Abandon" -> confirmation dialog (browser `confirm()` or custom modal):
   "This permanently deletes all work in this worktree. The task will be marked
   wont-do. This cannot be undone."
2. On confirm: `POST /api/conversations/:id/abandon-task`
3. On success: conversation transitions to Terminal via SSE

**4c. Commits-behind badge** -- `ui/src/components/StateBar.tsx`

Show "N behind" badge next to branch name when `commits_behind > 0`.
Update from `sse_conversation_update` events (already handled in atom reducer).

**4d. SSE + atom integration** -- `ui/src/api.ts`, `ui/src/conversation/atom.ts`

- Add `commits_behind?: number` to `Conversation` interface
- `ConversationMetadataUpdate` already handles partial conversation updates
- New API methods: `completeTask()`, `confirmComplete()`, `abandonTask()`

Spec: REQ-PROJ-009, REQ-PROJ-010, REQ-PROJ-011

## Files to Modify

| File | Change |
|------|--------|
| `src/state_machine/state.rs` | Add `ConvState::Terminal` variant |
| `src/state_machine/transition.rs` | Reject events from Terminal state |
| `src/state_machine/proptests.rs` | Add Terminal to generators |
| `src/db/schema.rs` | Add `base_branch`, `task_number` to ConvMode::Work |
| `src/db.rs` | Preserve Terminal in `reset_all_to_idle` |
| `src/main.rs` | Reconcile empty base_branch Work rows |
| `src/runtime/executor.rs` | Record base_branch at approval, complete/abandon logic |
| `src/runtime.rs` | Add `commits_behind` to `ConversationMetadataUpdate` |
| `src/api/handlers.rs` | Three new endpoints, commits-behind in SSE init |
| `src/api/types.rs` | Request/response types for new endpoints |
| `src/api/sse.rs` | No new variants (reuses ConversationUpdate) |
| `ui/src/api.ts` | New API methods, `commits_behind` on Conversation |
| `ui/src/components/WorkActions.tsx` | New: Complete/Abandon buttons + commit msg modal |
| `ui/src/components/StateBar.tsx` | Commits-behind badge |
| `ui/src/conversation/atom.ts` | commits_behind in conversation state |
| `ui/src/hooks/useConnection.ts` | No changes (ConversationUpdate already wired) |

## Acceptance Criteria

- [ ] `ConvState::Terminal` variant added, preserved on restart (REQ-BED-029)
- [ ] `base_branch` + `task_number` stored in ConvMode::Work at approval time (REQ-PROJ-017)
- [ ] Empty base_branch Work rows reverted to Explore on startup
- [ ] Complete pre-checks block on dirty worktree, dirty main checkout, and merge conflicts (REQ-PROJ-009)
- [ ] LLM generates semantic commit message from diff, falls back to diff-stat if >50KB (REQ-PROJ-009)
- [ ] User can edit commit message before confirming (REQ-PROJ-009)
- [ ] Squash merge into base_branch, worktree+branch deleted (REQ-PROJ-009)
- [ ] Conversation goes Terminal after Complete (REQ-BED-029)
- [ ] Abandon confirmation dialog warns of permanent deletion (REQ-PROJ-010)
- [ ] Abandon deletes worktree+branch, updates task to wont-do (REQ-PROJ-010)
- [ ] Conversation goes Terminal after Abandon (REQ-BED-029)
- [ ] Commits-behind badge shows on Work conversations (REQ-PROJ-011)
- [ ] Commits-behind updates on SSE connect + ~60s poll (REQ-PROJ-011)
- [ ] Complete/Abandon buttons visible only on idle Work conversations
- [ ] Task-not-done nudge shown (non-blocking) at Complete time
- [ ] Navigation guard (beforeunload + router blocker) while commit dialog open
- [ ] `./dev.py check` passes

## Value Delivered

Complete lifecycle: Explore -> propose -> approve -> work -> complete -> done.
Every step has a clean exit. Abandon preserves the attempt record without
carrying code forward. Users always know if their base branch has advanced.
