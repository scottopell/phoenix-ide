---
created: 2026-05-01
priority: p2
status: done
artifact: src/git_ops.rs
---

Several diff-comparator code paths use bare local <base_branch> refs
(e.g. main...HEAD, task..base_branch) when computing what a worktree
has done relative to its base. The local <base> ref is only fast-
forwarded at lifecycle events (task approval, worktree creation, branch
checkout) via git_ops::materialize_branch. The periodic 1-minute fetch
loop in stream_conversation updates origin/<base> but does NOT
re-materialize the local ref. So for any task >1 day old where origin
has advanced, every diff comparison shows stale results.

Affected call sites (audit done 2026-05-01):

  - src/api/lifecycle_handlers.rs:269 (abandon_task snapshot, just
    shipped in 24662)
  - src/api/handlers.rs:215 commits_behind, format!("{task}..{base}")
  - src/api/handlers.rs:236 commits_ahead, format!("{base}..{task}")
  - Will also affect the future GET /api/conversations/{id}/diff
    endpoint planned for task 08641.

Fix: switch all diff-comparison call sites to compare against
origin/<base> instead of bare <base>. The codebase already keeps
origin/<base> fresh via the periodic fetch loop, so this is the actual
source of truth for "what is base right now."

Add a small helper in src/git_ops.rs:

  fn effective_base_ref(cwd: &Path, base: &str) -> String

Returns "origin/<base>" if `git rev-parse origin/<base>` succeeds,
falling back to bare "<base>" for local-only repos with no remote.
This matches the existing materialize_branch fallback policy.

Update call sites:

  - abandon_task snapshot: format!("{effective_base}...HEAD")
  - commits_behind/ahead: use effective_base in the rev-list ranges
  - 08641 diff endpoint, when implemented, uses the helper

Acceptance:
  - New helper has unit-test coverage for both branches (origin
    available, fallback to local-only).
  - All three identified call sites switched.
  - Existing snapshot tests in lifecycle_handlers still pass.
  - cargo test + dev.py check all green.

Discovered while triaging task 08641 (work-mode diff viewer); blocks
clean implementation of that task.
