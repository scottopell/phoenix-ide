# Opportunistic Build Cache Pre-Warm for Phoenix Worktrees

## Context

macOS/APFS supports copy-on-write file clones (`clonefile(2)`, exposed by `cp -c`). Phoenix creates isolated git worktrees under `.phoenix/worktrees/{conversation-id}`. We can opportunistically seed those new worktrees with project-local build cache directories from an existing warm checkout/worktree, using copy-on-write semantics where available.

The goal is **best-effort pre-warm**, not a correctness mechanism and not direct cache sharing. Each worktree still owns independent paths; cloned files merely share blocks until rewritten by normal rebuilds.

## Semantic anchors found in the current codebase

- The durable domain concept for “a git-backed project eligible for worktree workflows” is `phoenix_core::domain::db_schema::Project`:
  - `canonical_path`: resolved git repo root path.
  - `main_ref`: canonical fork base/default branch.
- Project rows are created by `Database::find_or_create_project(canonical_path)` in `crates/phoenix-db/src/lib.rs`.
- New conversations detect projects in `crates/phoenix-ide/src/api/handlers.rs` using `detect_git_repo_root(&path)` and associate `project_id` with the conversation.
- Worktree placement is centralized by `crates/phoenix-ide/src/git_ops.rs::create_worktree`, which creates `.phoenix/worktrees/{conv_id}` relative to the repo root.
- Important worktree creation call sites:
  - Branch mode: `create_branch_worktree_blocking` in `api/handlers.rs`.
  - Managed Explore early worktree: `create_managed_explore_worktree_blocking` in `api/handlers.rs`.
  - Fork spawn/promote: `prepare_spawn_blocking` / `prepare_promote_blocking` in `runtime/fork_resolve.rs`.
- Existing project lifecycle specs live in `specs/projects/requirements.md` and `specs/projects/projects.allium`; REQ-PROJ-005 fixes worktree path uniqueness and REQ-PROJ-028 covers Managed early worktree creation.

## Proposed shape

Add a small, platform-aware pre-warm subsystem, tentatively named `project_opportunistic_build_warm`, that runs immediately after successful `git worktree add` and before the new worktree is handed to the agent/runtime.

### Responsibilities

1. Given a source project root and a destination worktree root, detect project-local build/cache directories worth pre-warming.
2. Filter to a conservative allowlist of directories only.
3. Copy/clone each candidate into the destination worktree only when:
   - the source path exists and is a directory,
   - the destination path does not already exist,
   - the candidate is repo-local and relative-path safe,
   - the clone/copy operation is supported for the current platform/filesystem.
4. Log all outcomes at debug/info level:
   - cloned,
   - skipped missing source,
   - skipped existing destination,
   - skipped unsupported filesystem/platform,
   - failed non-fatally.
5. Never block worktree creation on pre-warm failure.
6. Never silently fall back to a huge physical copy unless explicitly designed later.

## Initial candidate detector

Start conservative and deterministic:

- Rust: `target/`
- JavaScript/TypeScript local caches:
  - `node_modules/.cache/`
  - `.next/cache/`
  - `.turbo/`
  - `.vite/` if project-local
- Optional follow-up after measurement:
  - `node_modules/` as opt-in only, not first iteration.

Out of scope by construction: `.git/`, `.phoenix*`, runtime DB/log/env directories, sockets, pid files, lock files, and generic active dev-server output. This feature is not “copy everything ignored”; it is an allowlisted build-cache pre-warm.

## Source selection

First iteration should use the project canonical repo root (`Project.canonical_path` / `repo_root`) as the warm source because every current worktree creation path already has this value.

Potential follow-up: prefer the warmest existing Phoenix worktree for the same project when it has better candidates than the canonical checkout. That likely needs a small scoring function over existing conversation/worktree paths and should be separate from the first safe implementation.

## Platform semantics

Implement an abstraction such as:

```rust
enum WarmCopyOutcome { Cloned, Skipped(...), Failed(...) }
trait WarmCopier { fn clone_dir_best_effort(src: &Path, dst: &Path) -> WarmCopyOutcome; }
```

macOS first:

- Use a direct `Command` invocation of `/bin/cp -c -R` or a Rust `clonefile` binding.
- Prefer direct system call if dependency/unsafe footprint is acceptable; otherwise `cp -c -R` is fine for iteration.
- Guard by best-effort probe/attempt, not by assuming APFS.
- If unsupported, skip and log; do not physical-copy a large cache.

Linux follow-up:

- `cp --reflink=auto -a` on filesystems that support reflinks, or skip where unavailable.

## Integration points

The cleanest first integration is inside `git_ops::create_worktree` after `git worktree add` succeeds and before returning the path. Because that function only takes `cwd` (repo root), `conv_id`, branch info, and ignore strategy, it has exactly the source/destination paths needed for source=`cwd`, dest=`worktree_path`.

This automatically covers:

- Branch mode worktrees,
- Managed Explore early worktrees,
- fork spawn/promote worktrees that call `create_worktree`.

For adopt/retry paths that skip `create_worktree`, do nothing. If a worktree already exists, pre-warming it later risks overwriting agent/user state and is outside first iteration.

## Spec work

Update the project spec to capture the optimization as non-normative-for-correctness but normative-for-behavior once enabled:

- Add a requirement near REQ-PROJ-005/028: when Phoenix creates a worktree, it may best-effort pre-warm allowlisted project-local build cache directories, without affecting worktree isolation or failing creation.
- Add Allium guidance or a lightweight rule only if appropriate. This is a filesystem side effect attached to worktree creation, not a new lifecycle state; avoid over-modeling.

## Tests

Unit tests for the detector:

- finds only allowlisted candidates,
- ignores missing candidates,
- ignores unsafe relative paths,
- does not include `.git`, `.phoenix*`, lock files, sockets, or arbitrary ignored dirs.

Unit/integration tests for worktree creation behavior:

- successful worktree creation still succeeds when pre-warm fails,
- existing destination candidate is skipped, not overwritten,
- macOS clone command is abstracted/mocked so tests do not require APFS,
- `create_worktree` invokes pre-warm after successful `git worktree add`.

Manual validation:

- On macOS/APFS, create a warm `target/`, create a managed/branch worktree, confirm candidate appears quickly in the worktree.
- Confirm normal build can mutate/rebuild without affecting the source cache except shared CoW blocks diverging at filesystem level.

## Iteration plan

1. Add detector module and tests, no integration yet.
2. Add platform clone abstraction with a mockable executor and tests.
3. Wire `git_ops::create_worktree` to call pre-warm best-effort after successful worktree add.
4. Add logging and validate worktree creation flows.
5. Update `specs/projects` to document the behavior.
6. Measure locally on this repo with `target/` to decide whether any additional candidates are worth enabling.
