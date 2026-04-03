---
created: 2026-03-13
priority: p3
status: ready
artifact: pending
---

# Differentiated error messages for Complete pre-check failures

## Problem

The Complete flow returns structured errors (`error_type: "dirty_worktree"`,
`"dirty_main_checkout"`, `"merge_conflicts"`) but the frontend shows them as
generic error text. Each error type has a different recovery action:

- **dirty_worktree**: "Ask the agent to commit or stash your changes"
- **merge_conflicts**: "Ask the agent to rebase onto {base_branch}"
- **dirty_main_checkout**: "Commit or stash changes in the main checkout"

The UI should guide the user toward the right action, not just show the raw
error string.

## What to Do

In `WorkActions.tsx`, parse the error response from `completeTask()` to
extract `error_type`. Show tailored guidance:

- `dirty_worktree`: Show error + a "Ask agent to commit" button that
  pre-fills the input with "Please commit all changes" or similar.
- `merge_conflicts`: Show error + suggestion to ask for rebase.
- `dirty_main_checkout`: Show error explaining this is outside Phoenix's
  control (user needs to go to terminal).

## Acceptance Criteria

- [ ] Each error type shows specific, actionable guidance
- [ ] dirty_worktree error offers a one-click "ask agent to commit" action
- [ ] merge_conflicts error suggests rebase
- [ ] Generic fallback for unknown error types
