---
created: 2026-02-08
priority: p2
status: ready
---

# Breadcrumb Tooltip Position Relative to Item

## Summary

Tooltip should appear above the specific breadcrumb being hovered, not centered in viewport.

## Context

Current implementation uses `position: fixed` with `left: 50%` to escape the overflow clipping of the breadcrumb bar. This works but means the tooltip always appears in the center of the screen regardless of which breadcrumb you hover.

## Requirements

1. Tooltip should appear directly above the hovered breadcrumb
2. Must still escape the overflow clipping of #breadcrumb-bar
3. Should handle edge cases (leftmost/rightmost breadcrumbs near viewport edge)

## Implementation Options

1. **Calculate position in JS**: On hover, get breadcrumb's `getBoundingClientRect()` and set tooltip position dynamically
2. **Portal approach**: Render tooltip in a React portal at document.body level
3. **CSS anchor positioning**: Use CSS anchor positioning (newer spec, limited support)

Option 1 is probably simplest - add position calculation in BreadcrumbBar component.

## Acceptance Criteria

- [ ] Tooltip appears centered above the hovered breadcrumb
- [ ] Tooltip doesn't overflow viewport edges (shifts left/right as needed)
- [ ] Arrow pointer still points at the breadcrumb
