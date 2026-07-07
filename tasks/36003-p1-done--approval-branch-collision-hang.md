# Fix task approval branch-collision hang

## Problem

On prod, approving conversation `propose-task-review-7f5e` appears to hang forever in the web UI.

Triage from local prod artifacts (no browser automation):

- Conversation: `9cfac9d9-2a9b-4d8e-a135-1ca7b361d0ff`
- Slug: `propose-task-review-7f5e`
- State in `~/.phoenix-ide/prod.db`: still `awaiting_task_approval`
- Task file: `tasks/07004-p1-ready--about-disk-drilldown-actions.md`
- Prod log sequence:
  - `POST /api/conversations/:id/approve-task` returned `200` immediately.
  - The async executor then failed:
    - `Task approval git operations failed`
    - `Failed to rename branch 'task-pending-9cfac9d9' to 'task-07004-about-disk-drilldown-actions': ... fatal: a branch named 'task-07004-about-disk-drilldown-actions' already exists`
  - Retrying produced the same failure.

Current code shape:

- HTTP handler `approve_task` only enqueues `Event::TaskApprovalDecided` and returns success before approval git work finishes.
- `execute_approve_task` catches async git failure, restores in-memory `AwaitingTaskApproval`, sends an SSE `Error`, and returns `Ok(())`.
- The DB remains awaiting approval, but the UI can still appear stuck because the initiating approve call succeeded and the failure is delivered only later over SSE.
- The branch collision is deterministic for taskmd approvals because `execute_approve_task_blocking` derives `branch_name = task-{task_id}-{slug}` and `open_early_worktree_and_rename_branch` runs `git branch -m temp target` without a preflight/typed collision outcome.

## Goal

Make task approval failure honest and recoverable when the target task branch already exists, and prevent the UI from showing an indefinite approving state.

## Proposed work

1. Add explicit approval error classification for branch-name collisions.
   - Detect target branch existence before attempting `git branch -m`, or classify the existing git failure into a typed approval error.
   - Return a user-facing message that names the conflicting branch and explains the recovery options.

2. Decide and implement the correct branch-collision behavior.
   - Preferred if safe: reuse/promote only when the existing branch is the same conversation's already-promoted branch and the operation is idempotent.
   - Otherwise: fail fast before mutating anything and keep the conversation awaiting approval.
   - Do not silently overwrite or force-move an existing branch.

3. Make the frontend approval action settle on asynchronous approval failure.
   - If the backend keeps the current async-ack API shape, ensure the SSE `Error` and/or reverted `awaiting_task_approval` state clears any local busy/approving affordance and reopens/keeps the approval reader actionable.
   - Surface the approval failure as an inline/toast error instead of leaving the user watching a spinner forever.
   - Consider adding an explicit state/metadata update after the executor reverts approval state if the current SSE stream does not cause the overlay to recover.

4. Add regression coverage.
   - Backend test: approving a task whose target branch already exists does not hang or partially promote; it returns/classifies a branch-collision failure and leaves the conversation/task approval retryable.
   - UI test: when task approval receives an async error after the POST returns `200`, the approval UI clears busy state and displays a recoverable error/retry path.
   - Include a retry/idempotency test if same-conversation already-promoted branch reuse is implemented.

## Acceptance criteria

- Reproducing the prod scenario with pre-existing `task-07004-about-disk-drilldown-actions` no longer leaves the UI in an indefinite approving/hanging state.
- The user sees a clear failure message naming the existing branch.
- The conversation remains in a retryable approval state unless approval actually succeeds.
- Approval never force-renames over or moves a branch that may belong to another worktree/conversation.
- Regression tests cover the async-failure UI recovery path and the backend branch-collision path.

## Validation notes

Useful prod artifacts for confirmation:

```bash
rg -n "propose-task-review-7f5e|Task approval git operations failed|task-07004-about-disk-drilldown-actions" ~/.phoenix-ide/prod.log
sqlite3 ~/.phoenix-ide/prod.db "SELECT id, slug, state, cm_kind, cm_worktree_path FROM conversations WHERE slug='propose-task-review-7f5e';"
```

Relevant code anchors:

- `crates/phoenix-ide/src/api/lifecycle_handlers.rs::approve_task`
- `crates/phoenix-ide/src/runtime/executor.rs::execute_approve_task`
- `crates/phoenix-ide/src/runtime/executor.rs::execute_approve_task_blocking`
- `crates/phoenix-ide/src/runtime/executor.rs::open_early_worktree_and_rename_branch`
- `ui/src/pages/ConversationPage.tsx::handleApproveTask`
- `ui/src/components/TaskApprovalReader.tsx`
