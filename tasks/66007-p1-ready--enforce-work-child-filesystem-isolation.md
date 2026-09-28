# Restore Git-backed Work-child execution with enforceable filesystem isolation

Split from PR #765 by explicit user decision on 2026-09-28. Part 1 repairs the owning conversation's approval capability and rejects Git-backed Work-child admission. Direct-mode Work delegation remains available. Do not make parent approval or the deflake workstream depend on this task.

## Evidence and reusable work

The attempted child implementation is preserved at commit af7d503e568ab0009caf8392cf4951356e5caf56 (local archive/pr765-child-isolation-af7d503). It includes scratch creation/cleanup fixes using worktree-rooted directory capabilities; those fixes do not solve inode aliasing. Do not simply restore that implementation.

Actual macOS sandbox reproduction: create an outside regular file, hard-link it to worktree/alias.txt, then run the Phoenix executable with --sandbox-exec -- 'echo escaped > alias.txt', PHOENIX_SANDBOX_WORKTREE_WRITE=1, PHOENIX_SANDBOX_WORKTREE_ROOT and PHOENIX_SANDBOX_REPO_ROOT set to the worktree, and scratch/temp under the worktree. The child exits successfully and the outside file changes. The local failing Rust regression is preserved in /tmp/pr765-hardlink-regression.patch; log /tmp/pr765-hardlink-probe.log. These local paths are supporting evidence, not required inputs to reproduce.

## Acceptance criteria

- Define the supported isolation contract and architecture before restoring admission. Shared writable paths do not imply isolated inodes.
- Preserve the parent worktree ownership contract and decide explicitly whether a private filesystem/edit transfer model is needed.
- Cover pre-existing external hard links, symlink replacement during writes, scratch setup and cleanup, inherited file descriptors, configured MCP/tool write bypasses, and background child processes. A preflight scan without a race model is insufficient.
- Exercise real macOS and Linux supported sandbox mechanisms; fail admission when the promised boundary cannot be enforced.
- Align requirements, Allium, prompts, admission, and persisted child mode. Restore only tools whose write authority is enforceable.
- Preserve ordinary Direct-mode delegation and the parent's approved write capability.
- Obtain independent adversarial review and exact-head CI before landing.

## Owning surfaces and scope

Primary surfaces: crates/phoenix-ide/src/runtime.rs (admission/materialization), crates/phoenix-tools/src/bash/sandbox.rs and operations.rs (process isolation and scratch lifecycle), crates/phoenix-core/src/domain/db_schema.rs (persisted child identity), and specs/subagents plus specs/bash (contract). Update their executive status when real-host isolation tests and independent review pass.

Out of scope: parent approval recovery, coordinator caching, tmux test deflaking, and production deployment.
