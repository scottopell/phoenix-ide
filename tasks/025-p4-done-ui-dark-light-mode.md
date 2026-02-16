---
created: 2026-01-31
priority: p4
status: done
---

# Dark/Light Mode Toggle

## Summary

Add theme toggle to switch between dark and light modes.

## Context

The UI currently ships dark mode only. Some users prefer light mode, especially in bright environments or for accessibility reasons.

## Acceptance Criteria

- [x] Toggle button/switch in UI (header or settings)
- [x] Light mode color scheme defined in CSS
- [x] Preference persisted in localStorage
- [x] Respect system preference as default (`prefers-color-scheme`)
- [x] Smooth transition between modes (no flash)

## Notes

- Use CSS custom properties (already in place in `static/style.css`)
- Define `[data-theme="light"]` overrides for all `--` variables
- Consider adding to a settings panel rather than always-visible toggle
- Test readability of code blocks in both modes
