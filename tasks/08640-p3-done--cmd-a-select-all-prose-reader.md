---
created: 2026-04-07
priority: p3
status: done
artifact: ui/src/components/FileExplorerPanel.tsx
---

# Cmd+A should select all text in code viewer / prose reader

## Problem

When viewing a file in the prose reader / code viewer panel, Cmd+A
selects all text on the page rather than just the file content. Users
expect Cmd+A to select the visible file content so they can copy it.

## Fix

When the code viewer / prose reader panel has focus, intercept Cmd+A
(or Ctrl+A on non-Mac) and scope the selection to the file content
container element.

## Done when

- [ ] Cmd+A in code viewer selects only the file content
- [ ] Cmd+A outside the code viewer still works normally (e.g., in textarea)
