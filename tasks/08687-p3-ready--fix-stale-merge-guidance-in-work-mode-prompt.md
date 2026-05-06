---
created: 2026-05-08
priority: p3
status: ready
artifact: src/system_prompt.rs
---

# fix-stale-merge-guidance-in-work-mode-prompt

## Summary

`src/system_prompt.rs:463-465` tells Work-mode agents:

> When the work is complete, let the user know. They will initiate
> the merge to {base_branch} when ready. Task-file status renames
> are handled automatically during merge.

The second sentence is leftover from an older workflow where Phoenix
performed merges itself and could rename `in-progress` task files to
`done` as part of that. Current merges happen via GitHub PRs or git
directly — nothing touches the task files. The result is that
Work-mode agents leave tasks in `in-progress` indefinitely, and the
user has to either remember to flip them or ask the agent to do it
before merge.

## Plan

Update `src/system_prompt.rs:463-465` to instead direct the agent to:

- Mark the task file `done` before letting the user know the work is
  complete (rename `*-in-progress--*.md` → `*-done--*.md` and update
  the `status:` frontmatter).
- Tell the user the work is complete and ready for them to merge.

The corresponding test at line 958 (`assert!(prompt.contains("Task-file
status renames"))`) needs to update to assert the new wording.

## Acceptance

- The `Work` mode prompt no longer claims merge tooling renames task
  files.
- The agent is explicitly directed to flip status before signalling
  completion.
- `cargo test` passes.

## Why captured separately

The smooth-sidebar PR is large enough; touching the system prompt is
an unrelated concern. Keeping it as a small focused follow-up.
