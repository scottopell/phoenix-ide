---
created: 2026-04-03
priority: p2
status: done
artifact: ui/src/components/TasksPanel.tsx
---

# Task to conversation navigation

## Summary

The Tasks panel shows tasks grouped by status, but clicking a task opens a
TaskViewer showing the file content. There's no way to navigate from a task
to the conversation that owns it. A user returning after a break thinks "where
was that auth refactor?" and has to scan the conversation list manually.

## What to change

Backend: include `conversation_id` (and optionally `conversation_slug`) in the
TaskEntry response. The task file doesn't store this, but ConvMode::Work has
task_id -- scan active Work conversations to match task IDs to conversations.

Frontend: TasksPanel items get a "Go to conversation" link/icon that navigates
to `/c/{slug}`. TaskViewer detail panel also gets the link.

For tasks without an active conversation (done/abandoned), show the link as
disabled or omit it.

## Done when

- [ ] TaskEntry API response includes conversation_slug (when available)
- [ ] Clicking a task's conversation link navigates to the conversation
- [ ] Tasks without active conversations don't show a broken link
