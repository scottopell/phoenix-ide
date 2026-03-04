---
created: 2025-07-13
priority: p3
status: ready
---

# Breadcrumb bar z-index bleed-through on mobile

## Summary

Tool result content renders visibly through/behind the breadcrumb bar at the bottom of the screen on mobile (iOS Safari).

## Context

`#breadcrumb-bar` in `ui/src/index.css` has no `position: relative` or `z-index`, so it doesn't establish its own stacking context. Content from siblings above it (message list, input area) that establish stacking contexts can paint on top of/through it.

Screenshot shows bash tool output text (e.g. `10:import { FileBrowserOverlay, useFileExplorer }...`) visible behind the breadcrumb bar.

## Fix

Add `position: relative; z-index: 1;` to `#breadcrumb-bar` (line ~200 in `ui/src/index.css`). Apply same treatment to `#state-bar` defensively.

## Acceptance Criteria

- [ ] `#breadcrumb-bar` has `position: relative` and `z-index: 1`
- [ ] `#state-bar` has `position: relative` and `z-index: 1`
- [ ] Tool result content no longer bleeds through bottom chrome on mobile
