## Resolution

Fixed by commit `c504ad0` ("fix: REQ-PROJ-028 branch rename unreachable;
extract canonical repo_root helper (#23)"):

- `execute_approve_task_blocking` now takes an explicit `repo_root: &Path`
  alongside `cwd`. The caller derives it via the new
  `git_ops::repo_root_from_phoenix_worktree` helper, so `phoenix_dir` is
  anchored at the repo root rather than at the active worktree's cwd.
- The helper replaced four ad-hoc ancestor-walk implementations across
  `cleanup_worktree_if_present`, `execute_approve_task`,
  `reconcile_worktrees`, and `system_prompt`. Strict predicate
  (`Option<PathBuf>`); callers opt into the input-as-fallback behaviour
  via `.unwrap_or_else(|| cwd.clone())` at each call site so the contract
  is explicit.
- Regression test:
  `cwd_immutability_tests::approve_task_returns_same_path_as_explore_worktree`
  exercises Explore→Work promotion and asserts no nested worktree and no
  nested `.phoenix/` is created.

The remaining `handlers.rs` worktree-creation site (initial Explore
worktree) is only reached from a fresh conversation whose cwd the user
explicitly chose; a user pointing it inside an existing worktree is a
manual-misuse case and not the mainline-continuation nesting this task
was filed for. Tracking that as a separate p3 if it materialises.

The three nested worktrees observed when this task was filed should be
relocated or pruned manually — no auto-relocation, since the inner
worktrees may carry uncommitted work.

## Worktrees can be created nested inside other worktrees

Observed from `git worktree list` in the main checkout:

```
.phoenix/worktrees/1743ee05-3630-49a6-8b67-56c7b83afcfe                                                          [task-pending-1743ee05]
.phoenix/worktrees/1743ee05-3630-49a6-8b67-56c7b83afcfe/.phoenix/worktrees/1743ee05-3630-49a6-8b67-56c7b83afcfe  [task-09001-tmux-continuation-gap]

.phoenix/worktrees/d0c5da21-0274-489b-9340-56ed86cd352a                                                          [task-pending-d0c5da21]
.phoenix/worktrees/d0c5da21-0274-489b-9340-56ed86cd352a/.phoenix/worktrees/d0c5da21-0274-489b-9340-56ed86cd352a  [task-02703-smooth-conversation-sidebar]

.phoenix/worktrees/73823f97-fe5e-4072-aeab-98e6914035c5/.phoenix/worktrees/ebf67677-444c-4a74-9a93-66fbd2e78653  [seed-branch-demo] prunable
```

Three separate Phoenix worktrees ended up with a second worktree nested
inside their own `.phoenix/worktrees/` subdirectory. The nested path
segment matches the parent (same UUID, same branch name shape) in the
1743ee05 and d0c5da21 cases, suggesting the worktree-creation path was
run from inside an already-active worktree without resolving back to
the repo root.

This is an acute UX bug: an agent operating in such a worktree can be
confused about which copy of the source they're editing. In this very
incident, a continuation session arrived to find its system-prompt cwd
pointing at the outer (clean) worktree while the actual work-in-progress
lived in the inner one. Phase 2 work was nearly lost.

## Likely root cause

Worktree creation in `src/state_machine/projects.rs` (or its callers)
computes the new worktree path relative to the **current** working
directory rather than the **repo root**. When invoked from an existing
worktree, this nests.

Reference for the correct invariant: `crate::git_ops::repo_root_from_phoenix_worktree`
in `src/git_ops.rs` already knows how to resolve a Phoenix worktree path
back to its repo root — worktree creation should pre-resolve through it.

## Suggested fix

1. Find every call site that constructs a new worktree path under
   `.phoenix/worktrees/<uuid>` and ensure the parent directory is the
   repo root, not the current cwd.
2. Add a guard: refuse to create a worktree whose path would land
   inside another existing Phoenix worktree (`git rev-parse --git-common-dir`
   plus a path-prefix check).
3. Backfill: write a one-shot maintenance command to detect and report
   existing nested worktrees so users can manually relocate them. (Don't
   auto-relocate — the nested worktree may have uncommitted work.)

## Acceptance

- New worktrees never land inside another Phoenix worktree, regardless
  of what cwd the creating process happened to be in.
- The three currently-nested worktrees in this repo are surfaced via a
  detection command and given a documented manual-relocation procedure.
