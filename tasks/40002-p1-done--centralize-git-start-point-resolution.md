# Centralize git start-point resolution and tree reads

## Executive Summary

Phoenix currently has several places independently answering variants of the same question: “which git ref should this workflow read from, check out from, compare against, or persist as the logical base?” The Start from task polish fixed the immediate bugs by splitting logical `base_branch` from physical `checkout_ref`, but it left parallel policy in `api/handlers.rs`, `resolution_root.rs`, `git_ops.rs`, and runtime approval code.

Refactor toward a single correct-by-construction model:

- `git_ops.rs` remains the low-level git mechanics layer.
- A new domain module, likely `git_start.rs`, owns start-point policy and produces typed values for logical base, checkout ref, and tree-read ref.
- `resolution_root.rs` remains the read-view abstraction over either a working directory or committed git tree, but consumes already-resolved tree refs instead of owning start-point policy.
- Task listing/availability reuses the same tree-read abstraction as file and skill discovery, eliminating the current parallel `ls-tree` / `show` implementation in task handlers.
- Conversation creation, task listing, inline file/skill discovery, task approval, and compare/diff logic consume the same start-point model so `origin/main` can never accidentally become the persisted logical base branch, and stale/default-branch behavior is deliberate rather than patched per call site.

This should address the known review nit in the current Start from task PR by making the bug class structurally impossible rather than relying on discipline at each caller.

## Problem

The current code has overlapping ref-resolution paths:

1. `resolution_root.rs::ResolutionRoot::for_create`
   - Builds a `GitTree` for `/new` file and skill discovery.
   - Contains `resolve_tree_ref`, which mirrors `git_ops::materialize_branch` behavior.
   - Does not fetch per request, by design, because autocomplete may run frequently.

2. `/api/tasks` and `/api/tasks/availability` in `api/handlers.rs`
   - Fetch `origin` and refresh `origin/HEAD`.
   - Resolve a refreshed default task ref.
   - Independently scan git trees with `ls-tree` and read task content with `show`.
   - Reimplements committed-tree reads that are conceptually parallel to `ResolutionRoot`.

3. Managed conversation creation in `api/handlers.rs`
   - Accepts logical `base_branch` and optional physical `checkout_ref`.
   - Persists the logical base branch while using the checkout ref as the worktree start point.

4. Task approval in `runtime/executor.rs`
   - Resolves/materializes the approval base branch separately.

5. `git_ops.rs::effective_base_ref`
   - Chooses comparator refs for diff-like operations.

These paths are close enough that changes in one can silently desynchronize from the others. The recent Start from task work exposed this: task list, task preview, managed worktree start point, and persisted base branch all need to agree, but they currently agree by convention.

## Desired End State

Introduce a typed start-point model that separates semantic roles:

```rust
pub struct GitStartPoint {
    pub logical_base: LogicalBaseBranch,
    pub checkout_ref: CheckoutRef,
    pub tree_ref: TreeRef,
}
```

The exact names can vary, but the type must encode these distinctions:

- **Logical base**: user-facing/persisted branch identity, e.g. `main`.
- **Checkout ref**: actual git start point for worktree creation, e.g. `origin/main`, `main`, or an explicit `refs/...` value.
- **Tree ref**: committed tree used for pre-create reads such as file search, skill discovery, task listing, and task preview.
- **Comparator ref**: if needed, a derived ref for diff/compare operations, usually remote-preferred but not persisted as logical base.

Invalid states should be hard to represent:

- A remote tracking ref such as `origin/main` must not be accidentally persisted as the logical base branch for normal task-start flows.
- A normal branch name must not bypass branch materialization when worktree creation or approval requires it.
- Explicit refs must be validated as explicit refs, not materialized as local branches.
- The tree ref used for listing/preview should match the checkout ref used to create the worktree unless the constructor explicitly documents a different intent.

## Proposed Module Boundaries

### `git_ops.rs`

Keep as low-level mechanics:

- `run_git`, `run_git_bytes`, `run_git_capped`
- fetch helpers
- `materialize_branch`
- safe checked-out-branch handling
- `create_worktree`
- ref validation primitives
- diff capture mechanics

It should not own workflow-level decisions such as “for Start from task, persist `main` but check out `origin/main`.”

### New `git_start.rs` or `git_resolution.rs`

Own policy-level git start resolution.

Suggested API shape:

```rust
pub struct GitStartPoint {
    logical_base: LogicalBaseBranch,
    checkout_ref: CheckoutRef,
    tree_ref: TreeRef,
}

impl GitStartPoint {
    pub fn for_default_task_start(repo_root: &Path) -> Result<Self, GitStartError>;
    pub fn for_create_request(
        repo_root: &Path,
        base_branch: &str,
        checkout_ref: Option<&str>,
    ) -> Result<Self, GitStartError>;
    pub fn for_inline_discovery(
        repo_root: &Path,
        mode: CreationMode,
        base_branch: Option<&str>,
    ) -> Option<Self>;
    pub fn for_approval(
        cwd: &Path,
        repo_root: &Path,
        desired_base_branch: Option<&str>,
    ) -> Result<Self, GitStartError>;
}
```

Intent-specific constructors should make fetch behavior explicit:

- task default listing: refresh origin/default before resolving;
- autocomplete discovery: avoid per-keystroke fetch unless the caller already refreshed;
- creation: materialize or validate as needed before worktree creation;
- approval: materialize normal branches, validate explicit refs.

### `resolution_root.rs`

Remain the read-view abstraction:

```rust
pub enum ResolutionRoot {
    WorkingDir(PathBuf),
    GitTree { repo_root: PathBuf, reference: String },
}
```

But move policy out:

- Add constructors such as `ResolutionRoot::git_tree(repo_root, tree_ref)` or `ResolutionRoot::from_start_point(&GitStartPoint)`.
- Keep `working_dir`.
- Deprecate or slim `for_create` so it does not duplicate `materialize_branch` policy.
- Expose generic tree-read helpers that task listing can reuse:

```rust
impl ResolutionRoot {
    pub fn all_paths(&self) -> Vec<String>;
    pub fn read_text(&self, rel_path: &str) -> Option<String>;
    pub fn read_bytes(&self, rel_path: &str) -> Option<Vec<u8>>;
}
```

Reuse the existing cached `tree_paths` implementation for git-tree roots.

### Task listing module

Extract task-specific parsing out of `api/handlers.rs`, for example into `task_listing.rs` or a submodule under `api` if API types remain involved.

It should consume a read root, not raw git refs:

```rust
pub fn discover_task_dir(root: &ResolutionRoot, fallback_cwd: &Path) -> String;
pub fn list_task_entries(root: &ResolutionRoot, tasks_dir: &str, limit: Option<usize>) -> Vec<TaskEntryParts>;
```

Task-specific responsibilities:

- taskmd directory discovery policy (`tasks` preferred, otherwise lexical `_TEMPLATE.md` candidate, otherwise local fallback);
- taskmd filename parsing;
- task content preview loading;
- availability limiting.

Non-responsibilities:

- fetching origin;
- choosing default branch;
- deciding logical vs checkout refs;
- direct `git ls-tree` / `git show` calls.

## Migration Plan

### Phase 1: Introduce typed start-point primitives

- Add `git_start.rs` with typed wrappers for logical base, checkout ref, and tree ref.
- Move explicit-ref detection into one helper.
- Add unit tests for:
  - normal branch `main` resolves to logical `main`;
  - remote task start resolves to logical `main`, checkout/tree `origin/main`;
  - explicit `origin/main` or `refs/...` validates without materializing;
  - normal branch names still materialize where required;
  - local checked-out branches are not moved unsafely.

### Phase 2: Rewire managed creation

- Replace ad hoc `base_branch` + `checkout_ref` handling in `create_conversation` with `GitStartPoint::for_create_request`.
- Keep API wire shape unchanged unless intentionally changed:
  - `base_branch` remains logical/user-facing;
  - `checkout_ref` remains optional and physical.
- Pass `start.logical_base()` to persistence.
- Pass `start.checkout_ref()` to `create_managed_explore_worktree_blocking`.
- Assert via tests that Start from task sends/persists `main` while checking out `origin/main`.

### Phase 3: Rewire task listing/availability

- Replace `refreshed_default_task_ref` + `discover_task_dir_from_git_ref` + `task_entries_from_git_ref*` with:
  - `GitStartPoint::for_default_task_start(repo)`;
  - `ResolutionRoot::git_tree(repo, start.tree_ref())`;
  - shared task-listing functions over `ResolutionRoot`.
- Preserve current freshness semantics: project task listing and availability refresh origin/default before reading.
- Preserve fallback semantics for non-git or no-ref cases by using `ResolutionRoot::working_dir(cwd)`.
- Ensure returned task entries still carry:
  - `source_ref = Some(start.tree_ref())` for git-tree reads;
  - `content` from the same root/ref used for listing.

### Phase 4: Rewire inline discovery

- Replace `ResolutionRoot::for_create` policy with `GitStartPoint::for_inline_discovery` plus `ResolutionRoot::from_start_point`.
- Preserve no-fetch behavior for high-frequency autocomplete unless a caller explicitly requests refreshed discovery.
- Remove duplicated `resolve_tree_ref` logic from `resolution_root.rs` or make it a thin call into `git_start.rs`.
- Update file and skill discovery tests to assert the tree ref matches creation semantics.

### Phase 5: Rewire approval and compare/diff callers

- Replace `runtime/executor.rs::resolve_approval_base_branch` policy with `GitStartPoint::for_approval` or a narrower approval-specific resolver.
- Revisit `git_ops::effective_base_ref` and decide whether it becomes:
  - a method on `GitStartPoint`, or
  - a lower-level helper used by `git_start.rs`.
- Ensure approval base remains logical where persisted/user-facing, while comparator refs remain derived.

### Phase 6: Delete duplicate paths and comments

- Remove task-specific raw git tree read helpers from `api/handlers.rs`.
- Remove/refactor duplicated comments that describe start-point policy in multiple modules.
- Keep only local-fact comments; move durable behavior into tests/specs if needed.

## Acceptance Criteria

- [ ] There is one policy owner for logical base vs checkout ref vs tree ref.
- [ ] Project task listing, task availability, task preview, and managed worktree creation consume a shared `GitStartPoint` or equivalent typed model.
- [ ] `ResolutionRoot` no longer duplicates branch-materialization policy; it reads from a supplied root/ref.
- [ ] Task listing uses shared read-root/tree-read APIs instead of bespoke `ls-tree` / `show` code in handlers.
- [ ] `origin/main` cannot accidentally be stored as the logical base branch for normal Start from task flows.
- [ ] Normal branch names still materialize/refresh when creation or approval requires it.
- [ ] Explicit refs are validated but not materialized as branch names.
- [ ] Tests cover stale local default branch vs refreshed `origin/HEAD` task listing.
- [ ] Tests cover custom task directory discovery from the selected tree ref.
- [ ] Tests cover remote-only task start: list, preview, create worktree, persist logical base.
- [ ] Tests cover approval preserving/materializing the correct logical base.
- [ ] `./dev.py check` passes.

## Suggested Verification Commands

```bash
cargo test -p phoenix_ide project_task_ -- --nocapture
cargo test -p phoenix_ide resolution_root -- --nocapture
cargo test -p phoenix_ide git_ops -- --nocapture
cd ui && pnpm exec vitest run src/pages/NewConversationPage.workflow.test.tsx src/hooks/useCreateConversation.test.ts
./dev.py check
```

## Non-goals

- Do not redesign the public `/new` UI in this refactor.
- Do not change the `createConversation` TypeScript positional-argument cleanup unless intentionally scoped; if changed, prefer a separate options-object refactor.
- Do not persist physical checkout refs as conversation logical base branches.
- Do not make autocomplete fetch on every keystroke unless the product explicitly accepts that latency/network behavior.

## Notes

The Start from task PR intentionally introduced two separate concepts:

- `base_branch`: logical/user-facing/persisted branch, e.g. `main`.
- `checkout_ref`: physical worktree start point, e.g. `origin/main`.

This refactor should preserve that distinction and make it structural across the backend rather than relying on each caller to remember the rule.
