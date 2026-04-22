---
artifact: worktree-cleanup-on-context-exhausted
created: 2026-04-22
priority: p1
status: done
---

# Fix: Work/Branch worktree not cleaned up on ContextExhausted

## Background

When a Work- or Branch-mode conversation hits its context window limit and
transitions to `ConvState::ContextExhausted`, the state machine correctly
marks the conversation as terminal. Per `find_active_branch_conversation_slug`
(`src/git_ops.rs:247`) the DB no longer counts it as owning the branch.

The physical worktree, however, is never removed. Only two code paths
currently delete a Work/Branch worktree:

- `MarkAsMerged` (`src/api/lifecycle_handlers.rs` — squash-merge flow)
- `ConfirmAbandon` (`src/api/lifecycle_handlers.rs` — destructive abandon)

Both are user-triggered. The `ContextExhausted` transition goes through none
of them.

## Symptom

User hits "Context Window Full" on a Work- or Branch-mode conversation.
Clicks "Continue in new conversation" (new UI, `ConversationPage.tsx:730`).
The handler calls `api.createConversation(..., mode: 'branch', base_branch: parent.branch_name, ...)`.
The backend's `create_branch_worktree_blocking` (`src/api/handlers.rs:764`)
calls `check_branch_conflict` (`src/git_ops.rs:190`), which hits the
filesystem first via `find_branch_in_worktree_list`. The parent's worktree
is still there, so the check returns `BranchConflict::ExternalCheckout` —
not `PhoenixConversation`, because the parent is terminal and the DB lookup
returns `None`. The server responds 409 with:

  "Branch '<name>' is already checked out in a worktree at <path>.
   Git doesn't allow a branch to be checked out in two places at once.
   Switch to a different branch there first, or use Direct mode."

The continuation button can't recover — `conflict_slug` is unset (no active
owning conv), so the FE falls through to the toast.

## Spec

`specs/projects/projects.allium` now has a new rule
`WorkBranchWorktreeCleanupOnContextExhausted` (section 5b) that specifies the
intended behavior:

- Trigger: `bedrock/ConversationBecameTerminal` for a conversation with
  `mode in {work, branch}` and `parent_status = context_exhausted` that
  still has a `Worktree`.
- Effect: worktree removed. Branch is **preserved** (holds committed work;
  continuation needs it).
- The `MarkAsMerged`/`ConfirmAbandon` path already removes the worktree at
  action time, so by the time `ConversationBecameTerminal` fires with
  `parent_status = terminal`, `exists Worktree` is false and the rule is
  inert — no duplicate cleanup.

This matches the lifecycle model of the user-triggered actions (worktree is
the critical resource; branch is retained in Branch mode, deleted only in
Work mode).

## Code changes

### 1. Hook into the ContextExhausted transition

The state machine effect that announces context exhaustion is
`Effect::NotifyContextExhausted { summary }`
(`src/state_machine/effect.rs:142`), produced by the transition layer
(`src/state_machine/transition.rs:1511,1526,1541,1572`). Its only consumer
today is `Executor::handle_effect` at `src/runtime/executor.rs:1155`, which
broadcasts an SSE `StateChange` and nothing else.

Extend that handler: when the conversation's `ConvMode` is `Work` or
`Branch` and a worktree path is recorded, run the cleanup before (or
alongside) the SSE broadcast. The canonical cleanup sequence is in the
abandon handler at `src/api/lifecycle_handlers.rs:290-340` — mirror it,
minus the branch deletion:

- `git worktree remove <path> --force`
- On failure: `std::fs::remove_dir_all(<path>)` + `git worktree prune`
- Best-effort (log and continue — never block the terminal transition).
- **Do not delete the branch.**
- Broadcast a `ConversationUpdate` SSE clearing `worktree_path` so the
  UI's conversation metadata reflects reality post-cleanup (matches the
  pattern `execute_resolve_task` uses at
  `src/runtime/executor.rs:1936-1947`).
- Persist the mode/worktree change via
  `storage.update_conversation_cwd` and/or a mode update so the DB
  agrees with the SSE (and with `ServerRestartWorktreeReconciliation`
  which checks `worktree_path` for truthiness).

### 2. Verify no double-cleanup

Trace the `ConfirmAbandon` and `MarkAsMerged` flows to confirm that when
they run, by the time any `ContextExhausted` transition happens the
worktree is already gone. This should be naturally true (abandon/merge
both put the conv in `ConvState::Terminal`, not `ContextExhausted`), but
worth a sanity read of the state machine transitions.

### 3. Test coverage

- Integration test: Work-mode conversation transitions to
  `ContextExhausted` → worktree removed from disk, branch still exists
  (`git branch --list <name>` non-empty).
- Integration test: Branch-mode conversation same scenario → worktree
  removed, branch preserved.
- Integration test: Work-mode conversation completes via `MarkAsMerged` →
  worktree removed, branch deleted (existing behavior; regression guard).
- Unit/integration: creating a Branch-mode conversation on the same
  branch after cleanup succeeds (the continuation-button path).

### 4. Frontend follow-up

No FE changes required. After the cleanup lands, the
"Continue in new conversation" button (`ConversationPage.tsx:730`) will
succeed for Work/Branch parents without any behavior change. If the
cleanup fails (worktree remove errored), the FE falls through to the
existing `ExternalCheckout` error toast — no worse than today.

## Acceptance criteria

- [ ] Rule `WorkBranchWorktreeCleanupOnContextExhausted` in
      `specs/projects/projects.allium` has a corresponding code path that
      removes the worktree on the `ContextExhausted` transition for
      `ConvMode::Work` and `ConvMode::Branch`.
- [ ] Branch is **not** deleted.
- [ ] `MarkAsMerged` and `ConfirmAbandon` behavior unchanged (invariant:
      worktree removed before terminal, so the new rule is inert for
      their pathway).
- [ ] Integration tests above pass.
- [ ] Manual: hit context-full in a Work-mode conv, click "Continue in
      new conversation" → new Branch-mode conv opens on the same branch,
      no 409, input pre-populated with summary.

## Out of scope

- `/allium:propagate` test generation — separate step.
- UI change to warn user about uncommitted worktree changes before
  cleanup. The context-full banner + existing `commits_ahead` indicator
  already surface this. Revisit if it becomes a real footgun.
- Orphan-worktree adoption for the truly-orphaned case (external git
  ops or crash beyond what `ServerRestartWorktreeReconciliation`
  catches). Once the three cleanup rules cover normal operation, this
  tail case is rare enough to defer.

## References

- Spec: `specs/projects/projects.allium` §5b (new rule
  `WorkBranchWorktreeCleanupOnContextExhausted`)
- Related: `specs/bedrock/bedrock.allium` — `ConversationBecameTerminal`
  fires for any terminal transition, consumers discriminate on
  `parent_status`.
- Code loci:
  - `src/state_machine/effect.rs:142` — `Effect::NotifyContextExhausted`
  - `src/state_machine/transition.rs:1511,1526,1541,1572` — emitters
  - `src/runtime/executor.rs:1155` — consumer (hook point)
  - `src/api/lifecycle_handlers.rs:290-340` — reference cleanup sequence
  - `src/git_ops.rs:190` — `check_branch_conflict` (reads filesystem;
    becomes correct once cleanup runs on terminal)
  - `ui/src/pages/ConversationPage.tsx:730` — continuation button
