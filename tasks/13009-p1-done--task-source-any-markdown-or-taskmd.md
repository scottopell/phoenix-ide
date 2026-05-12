# Work-mode task file can be any markdown file (taskmd-named files additionally get taskmd handling)

## Why this blocks 1.0

Today a Work conversation's task file MUST be named `NNNNN-pX-status--slug.md` —
`propose_task`'s schema and `execute_approve_task_blocking` both hard-require the
taskmd filename pattern. That couples every Phoenix user to the taskmd workflow.
For 1.0 we want Phoenix to work for a user who hasn't adopted taskmd: point
`propose_task` at *any* markdown file and the Explore -> Work cycle still works.
taskmd stays the *default* (it's nice and simple) — this only removes the hard
dependency. Not a headline feature; just decoupling.

## Scope — KISS

Exactly two "task source" kinds, no more:

1. **taskmd** — a file whose name parses as `NNNNN-pX-status--slug.md`. Today's
   behavior, unchanged: id/priority/status/slug come from the filename; on approve
   the worktree's temp branch is renamed to `task-{id}-{slug}` and the file's
   status segment is promoted to `in-progress`.
2. **plain-markdown** — any other `.md` file. Treated as a task brief with no
   structured metadata: title from the body's `# H1`, `priority` defaults to `p2`
   (it's display-only), no status segment, **no status-rename on approve**, branch
   name derived from the file stem plus a short uniquifier (see below).

Do NOT build a third backend, a config UI, or a plugin registry. The point is that
the *seam* exists and is small — beads or another issue tracker could be added
later behind the same seam, but that's explicitly out of scope now.

## Design seam

A narrow `TaskSource` (trait or enum) with the two impls above. It answers, for a
given task-file path: does this path belong to me (taskmd: filename parses;
plain-markdown: it's a readable `.md` file) → and if so, the
`{ branch_name, task_id: Option, priority: Option, title, on_approve_action }`
the lifecycle needs. Pick taskmd first, fall back to plain-markdown. Keep it to
~one small module; don't add config/registry plumbing for hypothetical backends.

Configuration for v1 is *inferred*, not a knob: if `tasks/_TEMPLATE.md` exists the
project "has taskmd" and that's the default place the Explore prompt points the
agent; either way the agent may propose a plain-markdown file and it just works.
A future explicit "task source = plain | taskmd | beads | ..." setting can come
later — note it, don't build it.

## What changes (the coupling points)

- `crates/phoenix-ide/src/tools/propose_task.rs` — `input_schema` + `description`:
  `task_file` is "a markdown file under your working directory"; the taskmd naming
  convention is one accepted form, not the only one. Status-must-be-ready/in-progress/
  brainstorming only applies to taskmd-named files.
- `crates/phoenix-ide/src/state_machine/transition.rs`
  - `resolve_task_file`: today it requires the path under the configured tasks dir.
    Loosen to "any path inside the worktree" (so `docs/plan.md`, or even `README.md`,
    works). taskmd files still conventionally live in `tasks/`; plain-markdown task
    files should live *outside* `tasks/` so `./dev.py tasks validate` never sees a
    non-taskmd file there (no validator change needed — keep it that way).
  - `LlmResponse` interception that builds `AwaitingTaskApproval { task_file, title,
    priority, plan }`: `priority` from the filename when it parses as taskmd, else
    `p2`; `title` from the body `# H1` when the filename doesn't carry a slug.
- `crates/phoenix-ide/src/runtime/executor.rs` — `execute_approve_task_blocking`:
  the real work. When the filename parses as taskmd → unchanged. When it doesn't →
  `branch_name = task-{sanitized-stem}-{conv-id[..8]}` (the conv-id suffix is the
  uniquifier — two conversations proposing `feature.md` must not collide on the
  branch name; the existing `TASK_APPROVAL_MUTEX` only serializes, it doesn't
  uniquify), skip the `...-ready-- -> ...-in-progress--` rename entirely, and don't
  call `taskmd_core::filename::format_filename`.
- `crates/phoenix-ide/src/system_prompt.rs` — remove the "Your task ID prefix is {DD}.
  Task files in this worktree use IDs starting with {DD}..." line (and the
  `taskmd_core::ids::prefix_for(...)` call that feeds it) from the Work-mode block —
  it's dead once IDs aren't always taskmd IDs. Update the Explore-mode block: the
  agent may draft *any* markdown file as the task; if it follows the taskmd
  convention it additionally gets taskmd's metadata + the status-rename on approve.
- `specs/projects/requirements.md` + `specs/projects/design.md` + `specs/projects/projects.allium`
  — REQ-PROJ-003 / REQ-PROJ-006 / the task-approval rule: a task file is "any
  markdown file the agent points `propose_task` at; a taskmd-named file additionally
  yields id/priority/status/slug and the on-approve status promotion." Keep it
  factual and short — this isn't a new subsystem, it's a relaxation. Update the
  executive table / status rows accordingly.
- `crates/phoenix-ide/src/api/lifecycle_handlers.rs` (`abandon_task` / `mark_merged`)
  — verify nothing there assumes a parseable taskmd `task_id`. Branch deletion for
  Managed mode keys off `branch_name` (fine); no task-file edit on either path (fine).

## Out of scope

- beads or any other issue tracker — later, behind the same seam.
- An explicit per-project "task source" config knob / UI — later; v1 infers from
  `tasks/_TEMPLATE.md` presence + per-file fallback.
- Migrating existing taskmd-using projects — they keep working unchanged.

## Acceptance

- A project with no `tasks/_TEMPLATE.md` (taskmd not adopted): the full
  Explore -> `propose_task(<any>.md)` -> approve -> Work cycle works; the task
  branch is named sanely and is unique across two conversations that propose
  files with the same stem; no status-rename happens; the agent gets write access
  in the worktree.
- A project that uses taskmd: behavior is byte-for-byte unchanged (taskmd files
  still get id/priority/status/slug and the `ready -> in-progress` rename on approve;
  task files still committed on the task branch, never on main).
- `propose_task` accepts both a `tasks/01234-p2-ready--x.md` path and a `docs/plan.md`
  path; rejects a path outside the worktree.
- `./dev.py check` green (clippy, fmt, tests, allium, spec anchors, codegen-stale);
  `specs/projects/` audit clean.
- The "task ID prefix" line is gone from the generated Work-mode system prompt.
