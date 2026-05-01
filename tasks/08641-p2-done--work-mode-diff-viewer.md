---
created: 2026-04-07
priority: p2
status: done
artifact: ui/src/components/WorkActions.tsx
---

# Git diff viewer in Work mode actions

## Summary

Work mode has "Task complete? Merge to main / Abandon" actions but no way
to see what actually changed. The user has to open a terminal or leave
Phoenix to review the diff before deciding to merge or abandon.

## What to build

Add a "View diff" option alongside the existing Merge/Abandon actions in
WorkActions. Clicking it shows the git diff (worktree vs base branch) in
a panel or overlay with syntax-highlighted diff rendering.

## Backend

New endpoint: `GET /api/conversations/{id}/diff` that runs
`git diff {base_branch}..HEAD` in the worktree and returns the output.
Also include `git diff` (uncommitted changes) and `git log --oneline
{base_branch}..HEAD` for committed-but-unmerged work.

## Frontend

- Button in WorkActions next to Merge/Abandon
- Renders diff in a scrollable panel (could reuse the prose reader panel
  or a modal overlay)
- Syntax highlight the diff (green/red for add/remove lines)
- Show commit log summary at the top

## Done when

- [ ] "View diff" button in Work mode actions
- [ ] Diff fetched from worktree via API
- [ ] Diff rendered with syntax highlighting
- [ ] Includes both committed and uncommitted changes
