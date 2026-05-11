# Stop hard-coding `tasks/` — discover the project's taskmd directory instead

Phoenix-ide currently assumes the task directory is literally `tasks/` in
every code path that touches tasks: the `propose_task` flow, the executor's
worktree commit, the Explore-mode `PatchTool` allowlist, the
`resolve_task_file` validator, the system prompt, the task ID prefix
calculation, and so on. taskmd 1.0 allows any directory name — its CLI
auto-detects from `<dir>/_TEMPLATE.md`, and the Rust library takes the
tasks dir as an explicit `&Path` arg.

This blocks adoption for any phoenix-ide user whose project uses a
different convention (e.g. `taskmds/`, `worklog/`, `.tasks/`).

## What needs to change

1. **Discovery**: detect the tasks directory at worktree open time.
   Mirror the Python CLI's behaviour — walk the cwd / repo root and find
   the first dir containing `_TEMPLATE.md`; fall back to `tasks/` only if
   nothing is found. Cache the result per conversation (similar to
   `desired_base_branch`).

2. **Plumb it through**:
   - `state_machine/transition.rs::resolve_task_file`: the
     `first_component != "tasks"` check is the hard wall. Replace with
     "must be under <configured_tasks_dir>".
   - `runtime/executor.rs::execute_approve_task_blocking`: every
     `format!("tasks/...")`, `cwd.join("tasks")`, `worktree_path.join("tasks")`
     becomes the configured dir.
   - `tools/patch.rs`: the `restricted_to("tasks")` registration in
     `tools.rs` becomes `restricted_to(<configured_tasks_dir>)`. The
     PatchTool allowlist already takes a prefix arg, so this is a
     registration-time change.
   - `system_prompt.rs`: Explore-mode block currently says "draft a task
     file under `tasks/`" — should name the configured dir.
   - Task ID prefix calc in the Work-mode prompt
     (`taskmd_core::ids::prefix_for(&worktree.join("tasks"))`).

3. **Wire format**: `ProposeTaskInput.task_file` is already a free-form
   string path, so no schema change. But the LLM needs to learn the
   right prefix from the system prompt, not infer "tasks/".

4. **Allium/spEARS**: REQ-PROJ-002 / REQ-PROJ-013 reference Explore-mode
   write restrictions; update the design doc to say "scoped to the
   project's tasks dir" rather than literal `tasks/`.

## Acceptance criteria

- A repo with `taskmds/` instead of `tasks/` can complete the full
  Explore → propose_task → approve → Work cycle.
- The hard-coded literal "tasks" appears in tests only (where it's the
  fixture's chosen name), not in production code paths.
- `./dev.py up` on a phoenix-ide repo (which uses `tasks/`) still works
  end-to-end after the refactor.

## Out of scope

- Renaming the existing `phoenix-ide/tasks/` directory.
- Letting a single project have multiple task dirs.
