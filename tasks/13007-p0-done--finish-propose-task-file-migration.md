# Finish the propose_task → task_file refactor

The taskmd 1.0 upgrade landed on the assumption that propose_task moves to a
file-based contract: the LLM calls `propose_task(task_file: "tasks/X.md")`,
the file IS the plan, and approval reuses the existing on-disk file
instead of allocating a fresh ID and re-serializing the plan from inline
args. Until that lands, the 1.0 upgrade is conceptually incomplete — the
inline `title`/`priority`/`plan` schema still exists in `propose_task`'s
public surface even though everything below it (taskmd-core, the executor's
notion of metadata, the spec narrative in AGENTS.md) treats the filename as
authoritative.

## What's already on `claude/task-file-support-XYYev`

Initial cut from before 1.0 landed (commit e800ca4):
- `tools/propose_task.rs`: schema is `{ task_file }` only.
- `state_machine/state.rs`: `ProposeTaskInput { task_file }`. `AwaitingTaskApproval`
  carries `{ task_file, title, priority, plan }` — the latter three are a
  cached snapshot for the UI; `task_file` is canonical.
- `state_machine/effect.rs`: `Effect::ApproveTask` carries `task_file`.
- `state_machine/transition.rs`: `resolve_task_file` helper reads the file at
  interception, validates filename pattern + status (must be in
  `{ready, in-progress, brainstorming}`), and populates the snapshot. Uses
  taskmd 1.0's `parse_filename` → `ParsedFilename` and the typed `Status` enum.
- `runtime/executor.rs`: signature of `Effect::ApproveTask` updated.
  Two `TODO(taskmd-1.0)` placeholders mark where the executor logic still
  needs the rewrite.

## What's still required

### 1. Rewrite `execute_approve_task_blocking` in `runtime/executor.rs`

Currently allocates a new ID, derives a slug from the title, formats a fresh
filename, and writes a templated markdown body (with frontmatter — pre-1.0
holdover). All of that is wrong for the file-based flow.

New shape:
- Receive `task_file` (relative to cwd) plus the snapshot fields.
- `parse_filename(filename)` → `ParsedFilename { id, priority, status, slug }`.
  No new ID allocation; ID and slug come from the on-disk filename.
- Re-read the file body from disk so the agent can edit pre-approval and the
  approval reflects the latest content.
- If `status != Status::InProgress`, use `taskmd_core::tasks::update_task`
  (1.0 / rc2+) to rename the file in place and capture the new filename.
- Branch name remains `task-{id}-{slug}`.
- Worktree creation logic stays the same (REQ-PROJ-028 early-worktree path
  vs. legacy path). The file already exists in the worktree (or the cwd that
  becomes the worktree); no fresh content needs to be written. Stage the
  (possibly renamed) task file and commit.
- Drop the frontmatter template entirely from this function.

### 2. Scope `PatchTool` in Explore mode to `tasks/`

The agent in Explore mode needs a way to put a task file on disk before
calling `propose_task`. Per the design decision: allow `patch` in Explore,
but reject paths outside `tasks/` at runtime. Add an
`allowed_path_prefix: Option<&'static str>` (or similar) on `PatchTool`
construction; in `tools.rs::ToolRegistry::explore_no_sandbox` and
`explore_with_sandbox`, register a patch tool scoped to `tasks/`.

### 3. Update `system_prompt.rs` Explore-mode block

Teach the LLM the new flow: "find an existing task file with `keyword_search`
or `taskmd list --slug-contains`; if no fitting task exists, draft one with
`patch overwrite` to `tasks/<slug>.md` (or use `taskmd new` via bash if
sandboxed); then call `propose_task` with the path."

### 4. Fix the proptests

`proptests.rs` and `project_proptests.rs` still construct
`AwaitingTaskApproval { title, priority, plan }` and `ProposeTaskInput { title,
priority, plan }` from the rc1 surface. Update generators to produce a valid
`task_file` path plus the snapshot fields.

### 5. Remove `TODO(taskmd-1.0)` placeholders in `runtime/executor.rs`

Two markers near `execute_approve_task` and the error-revert branch where
`task_file: String::new()` is currently used as a stand-in. Both go away
once step 1 lands.

### 6. UI sanity check

`AwaitingTaskApproval` still surfaces `{title, priority, plan}` to the UI in
the SSE notify payload, so `ui/src/components/TaskApprovalReader.tsx` should
work unchanged. Verify with `./dev.py up` end-to-end before claiming done.

## Acceptance criteria

- `cargo test -p phoenix_ide` is green (proptests fixed).
- A managed-mode Explore conversation can: find an existing task file, call
  `propose_task` with its path, get user approval, transition to Work mode
  on a `task-{id}-{slug}` branch, with the existing on-disk file
  re-used (status renamed to `in-progress` if it wasn't already).
- A managed-mode Explore conversation can: draft a new task file via the
  scoped `patch` tool, call `propose_task` with the path, and proceed
  through approval the same way.
- The legacy `{title, priority, plan}` inline path is fully gone — no
  reachable code path constructs `AwaitingTaskApproval` without a real
  `task_file`.
