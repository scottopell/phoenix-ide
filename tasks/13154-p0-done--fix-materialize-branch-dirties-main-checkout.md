# Fix materialize_branch mutating checked-out branches in other worktrees

## Bug found

`git_ops::materialize_branch` can move a local branch ref with `git update-ref` while that branch is checked out in a different worktree.

The smoking gun is:

- `crates/phoenix-ide/src/git_ops.rs:259-274`
  - It checks only `git rev-parse --abbrev-ref HEAD` in the *cwd* worktree.
  - If `cwd` is not on `branch_name`, it runs `git update-ref refs/heads/{branch_name} <remote_sha>`.
- During Managed task approval:
  - `runtime/executor.rs:3352` calls `resolve_approval_base_branch(cwd, repo_root, ...)`.
  - `resolve_approval_base_branch` then calls `materialize_branch(cwd, &base_branch)` at `runtime/executor.rs:3247`.
  - In managed mode, `cwd` is the early Explore worktree on `task-pending-...`, while the selected base branch (often `main`) is commonly checked out in the user's main checkout.
  - Therefore `current_head != base_branch`, so `materialize_branch` may `update-ref refs/heads/main` to `origin/main` even though `main` is checked out in the user's main checkout.

That exactly matches the symptom: the main checkout's `HEAD` moves, but its working tree/index still contain the old checkout contents. Git then reports unstaged changes that look like a random revert or inverse of recent commits.

A secondary instance exists in Branch mode: `create_branch_worktree_blocking` calls `materialize_branch` before `check_branch_conflict`, so a branch checked out in another worktree could be moved before the conflict is reported.

## Fix plan

1. Make branch materialization worktree-aware, not cwd-HEAD-only.
   - Before moving a local branch ref, check `git worktree list --porcelain` via the existing `find_branch_in_worktree_list` helper.
   - If `branch_name` is checked out in *any* worktree, do not run `update-ref` for it.
   - Log/debug that the fast-forward was skipped because the branch is checked out.

2. Preserve remote freshness without mutating checked-out local branches.
   - Keep the single-branch fetch behavior.
   - Existing diff comparison already prefers `origin/<base>` via `effective_base_ref`, so skipping local ref movement is acceptable and safer.

3. Reorder Branch mode conflict handling if needed.
   - Check whether `branch_name` is checked out before materialization performs any local ref movement.
   - Or rely on the hardened `materialize_branch` if it checks all worktrees.

4. Add regression tests.
   - Create a repo with a main checkout on `main` and a Phoenix/managed-style worktree on `task-pending-*`.
   - Simulate `origin/main` advancing.
   - Call `materialize_branch` from the task-pending worktree for `main`.
   - Assert `refs/heads/main` does not move while `main` is checked out in the main checkout, and the main checkout remains clean.
   - Add a second test for a branch checked out in another worktree (Branch mode shape).

5. Run focused tests plus full check.
   - `cargo test git_ops materialize`
   - `./dev.py check`

## Acceptance criteria

- Phoenix never moves a local branch ref that is checked out in any worktree.
- Managed task approval cannot dirty the user's main checkout when upstream `main` advanced.
- Branch mode cannot mutate a branch checked out elsewhere before reporting a branch conflict.
- Regression tests fail against the current implementation and pass with the fix.
