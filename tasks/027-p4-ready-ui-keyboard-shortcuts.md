---
created: 2026-01-31
priority: p4
status: ready
---

# Keyboard Shortcuts (Desktop)

## Summary

Add keyboard shortcuts for common actions on desktop.

## Context

Power users expect keyboard shortcuts. Currently only Enter-to-send is implemented.

## Acceptance Criteria

- [x] Enter to send message (already done)
- [x] Shift+Enter for newline (already done)
- [ ] Escape to cancel in-progress operation (requires task 021)
- [ ] Cmd/Ctrl+K for new conversation
- [ ] Cmd/Ctrl+/ to show shortcut help
- [ ] Up arrow to edit last message (stretch)
- [ ] Shortcuts don't interfere with text input

## Notes

- Only applies to desktop (detect via hover capability or screen width)
- Add help modal or tooltip showing available shortcuts
- Consider vim-style navigation for power users (j/k to scroll messages)
