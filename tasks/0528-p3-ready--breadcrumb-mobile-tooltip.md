---
created: 2026-02-08
priority: p3
status: ready
---

# Mobile Tooltip Support for Breadcrumbs

## Summary

Add touch support for breadcrumb tooltips on mobile devices.

## Context

Hover tooltips don't work on touch devices. Task 525 mentioned "Works on mobile (long-press or tap?)" but this wasn't implemented.

## Requirements

1. Long-press (~500ms) on a breadcrumb shows the tooltip
2. Tap anywhere else dismisses the tooltip
3. Single tap still triggers click-to-scroll behavior
4. Don't break existing hover behavior on desktop

## Implementation Notes

- Use `onTouchStart`/`onTouchEnd` to detect long press
- Need to distinguish between tap (scroll) and long-press (tooltip)
- Could use a small delay before scrolling to allow long-press detection
- Consider using `pointer-events` media query to detect touch device

## Acceptance Criteria

- [ ] Long-press shows tooltip on touch devices
- [ ] Tap scrolls to message (existing behavior)
- [ ] Tooltip dismisses on tap elsewhere
- [ ] Desktop hover still works unchanged
