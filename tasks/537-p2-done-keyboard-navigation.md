---
created: 2025-05-30
priority: p2
status: done
---

# Keyboard navigation for Phoenix UI

## Summary

Add comprehensive keyboard navigation to make Phoenix usable without a mouse. Benefits both human users and LLM agents using browser automation.

## Context

Currently navigating Phoenix requires clicking elements, which is:
- Slower for power users
- Awkward for LLM agents using browser tools (have to write JS to find and click elements)
- Not accessible

## Proposed Keybindings

### Conversation List
- `j` / `k` or `↓` / `↑` - Navigate between conversations
- `Enter` - Open selected conversation
- `n` - New conversation
- `/` - Focus search (if we add search)

### Conversation View
- `Escape` - Back to conversation list
- `i` or `/` - Focus message input
- `j` / `k` - Scroll through messages (or navigate between them)
- `Enter` (in input) - Send message
- `Shift+Enter` (in input) - Newline

### Global
- `?` - Show keyboard shortcuts help
- `Cmd/Ctrl+k` - Command palette (future)

## Acceptance Criteria

- [ ] Can navigate conversation list with keyboard
- [ ] Can open conversation with Enter
- [ ] Can return to list with Escape
- [ ] Focus states are visually clear
- [ ] Input field keyboard behavior preserved
- [ ] Works in both light and dark mode

## Notes

- Should not interfere when typing in input fields
- Consider vim-style (`j`/`k`) vs arrow keys vs both
- Visual focus indicator needed for selected items
