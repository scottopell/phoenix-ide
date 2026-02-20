---
created: 2026-02-11
priority: p2
status: done
---

# Prose Reader - Trigger Requirements Analysis

## Summary

Analysis of missing requirements for prose reader discoverability on desktop.
The spec is vague about how to open the prose reader and has no desktop-specific
annotation trigger.

## Context

Two levels of missing requirements identified:

1. **Vague opening trigger** - "file browse button in the conversation" is not specific.
   Design doc suggests 3 approaches (toolbar button, keyboard shortcut, context menu)
   but doesn't mandate one.

2. **No desktop annotation trigger** - Spec assumes mobile long-press works everywhere.
   Desktop needs: hover state, icon affordance, or context menu. No keyboard shortcut
   documented for "Add Note".

Both bugs are spec compliance issues, not just implementation bugs.

## Acceptance Criteria

- [ ] Desktop prose reader opening mechanism is specified and implemented
- [ ] Desktop annotation trigger is specified and implemented
- [ ] Keyboard shortcuts documented

## Notes

- Related to task 512 (prose reader bugs)
- See requirements.md and design.md for current spec language

## Implementation (2026-02-19)

Resolved via AnnotatableBlock refactor (same change as task 512):
- Desktop annotation trigger: hover-reveal button (`.annotatable__btn`) on all annotatable elements
- Code view now uses custom `renderer` prop instead of `lineProps`, enabling child React elements (the button)
- Mobile: long-press gesture unchanged, button hidden via `@media (hover: none)`
