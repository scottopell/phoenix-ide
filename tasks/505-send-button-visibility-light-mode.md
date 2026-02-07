---
created: 2026-02-07
priority: p2
status: ready
---

# Send Button Nearly Invisible in Light Mode When Disabled

## Summary

The Send button on the New Conversation page has white text with `opacity: 0.35` when disabled. In dark mode this is visible (faint gray), but in light mode with a light background, white text at 35% opacity would be nearly invisible.

## Context

CSS analysis shows:
- `color: rgb(255, 255, 255)` (white text)
- `opacity: 0.35` when disabled
- `backgroundColor: rgba(0, 0, 0, 0)` (transparent)

In dark mode this renders as faint gray text on dark background - visible.
In light mode this would render as very faint white text on light background - nearly invisible.

## Reproduction

1. Go to `/new` page
2. Switch to light mode (if implemented)
3. Don't type anything (button disabled)
4. Observe Send button visibility

## Acceptance Criteria

- [ ] Send button is clearly visible in both light and dark modes
- [ ] Disabled state is visually distinct but still readable
- [ ] Consider using different text color or background in disabled state
- [ ] Test with actual light mode implementation

## Notes

Relevant CSS likely in `ui/src/pages/NewConversationPage.css` or similar.

The button styling should use theme-aware colors rather than hardcoded white.
