---
created: 2026-04-22
priority: p3
status: ready
artifact: src/runtime/executor.rs
---

# Runtime writes `artifact: pending` for auto-created task files on plan approval

## Summary

When a user approves a `propose_plan`, the Phoenix runtime writes a task file
with `artifact: pending` as a hardcoded placeholder (`src/runtime/executor.rs`
around line 2297, inside the frontmatter template). Since the `artifact` field
is now required and is supposed to name a concrete output, `pending` is a
consistency hole: CLI-created tasks name a real artifact; runtime-created ones
never do.

## Context

Discovered while updating `AGENTS.md`, `skills/phoenix-task-tracking/SKILL.md`,
and `specs/projects/{design,requirements}.md` (commit range around 2026-04-22)
to reflect `taskmd new` as the happy path. The spec updates describe
`artifact: pending` as an intentional runtime placeholder, but that description
is a concession, not a design intent.

Candidate real artifacts the runtime could name:
- The worktree path: `.phoenix/worktrees/{conv-id}/`
- The task branch: `task-{ID}-{slug}` (current convention)
- The concrete file list the plan promises to touch, if the LLM can supply it
  as part of `propose_plan`

Branch name is the cleanest of these: it's already derived from task ID + slug,
it's what the task is "about" operationally, and it's stable for the life of
the task. Plan-touched-file-list is richer but requires a schema change to
`propose_plan` and a synthesis step — bigger scope.

## Acceptance Criteria

- [ ] Runtime plan-approval path writes a non-`pending` `artifact` value. Pick
      one: (a) task branch name; (b) worktree path; (c) something richer
      threaded through `propose_plan`. Document the choice in
      `specs/projects/design.md` §REQ-PROJ-006 when making it.
- [ ] Existing tasks with `artifact: pending` are either left alone (grandfathered)
      or migrated by a one-shot script. Call this out explicitly; do NOT have
      `./dev.py tasks fix` auto-rewrite historical data.
- [ ] `./dev.py check` still passes.
- [ ] Update the paragraph in `specs/projects/design.md` that currently says
      "`pending` is an explicit placeholder for tasks Phoenix itself creates
      on plan approval" to reflect the new behavior.

## Notes

- Related updates that motivated this task: the 2026-04-22 rewrite of
  AGENTS.md §Task Tracking and SKILL.md to require `artifact` and recommend
  `taskmd new` as the happy path.
- Low priority (p3) — nothing breaks today because `taskmd` accepts any
  non-empty string for `artifact`. This is about removing an inconsistency
  between the two code paths that produce task files.
