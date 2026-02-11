---
created: 2026-02-11
priority: p1
status: ready
---

# P0: Prose Reader Spec - Critical UX Bugs

## Summary

Two critical UX bugs in the prose reader: cannot leave comments on table entries,
and "Add Note" feature is not discoverable on desktop.

## Context

### Bug #1: Cannot Leave Comments on Table Entries
**Severity:** P0 - Blocks core functionality

Users cannot add comments/annotations to entries within tables. The comment feature appears to work for regular text but fails for table cells or table rows.

### Bug #2: Missing "Add Note" Discoverability on Desktop
**Severity:** P0 - Core feature not discoverable

On desktop, there is no clear/visible way to discover how to trigger the 'add note' feature. The action may exist but users cannot find it without documentation.

## Acceptance Criteria

- [ ] Comments can be added to individual table cells
- [ ] Comments can be added to table rows
- [ ] Comments persist and display correctly
- [ ] "Add Note" action is discoverable on desktop (visible button/menu)
- [ ] Keyboard shortcut is displayed and documented

## Notes

- Bug #1: Comment UI is unavailable or fails silently on table elements
- Bug #2: Possible fixes include toolbar button, right-click context menu, or keyboard shortcut hint
