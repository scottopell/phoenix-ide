---
created: 2026-03-13
priority: p3
status: ready
artifact: pending
---

# Richer system messages with task context and file links

## Problem

System messages ("Task approved. You are on branch ... in ...") are plain text
pills with no structure. After approval, the user wants to see:
- The task title (what they're working on)
- The branch name
- A clickable link to the task file in the file explorer

After completion: the commit SHA, what branch it merged to.

## What to Do

System messages currently store plain text in `MessageContent::System { text }`.
Two approaches:

**Option A (backend)**: Emit structured system messages with a `display_data`
JSON payload containing `{ type: "task_approved", title, branch, task_file_path }`
or `{ type: "task_completed", sha, base_branch }`. The frontend renders these
with custom components instead of plain text.

**Option B (frontend only)**: Parse the text for known patterns and render
richer UI. Fragile but no backend changes.

Option A is cleaner. Add `display_data` to the system message persisted in
`execute_approve_task` and `confirm_complete`/`abandon_task` handlers. Create
a `SystemMessage` component that checks `display_data.type` and renders
accordingly.

## Acceptance Criteria

- [ ] Task approval system message shows title, branch, and task file link
- [ ] Task completion system message shows commit SHA and target branch
- [ ] Task abandon system message indicates worktree was deleted
- [ ] Clicking task file link opens it in the file explorer
- [ ] Plain text system messages still render as before (backward compat)
