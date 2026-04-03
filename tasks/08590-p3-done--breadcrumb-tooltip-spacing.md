---
created: 2026-02-28
priority: p3
status: done
artifact: completed
---

# Breadcrumb Tooltip Spacing

## Problem

The breadcrumb tooltip overlaps the breadcrumb bar content. The lower edge of the
tooltip should sit above the cursor/breadcrumb item so it doesn't cover what the user
is hovering.

## Fix

In `ui/src/index.css`, increase the `bottom` offset on `.breadcrumb-tooltip`:

```css
/* Current: */
bottom: calc(var(--breadcrumb-height) + 16px);

/* Increase gap so tooltip clears the bar: */
bottom: calc(var(--breadcrumb-height) + 24px);
```

Adjust the value until the tooltip arrow tip aligns near the cursor without the tooltip
body covering the breadcrumb text.

## Files

- `ui/src/index.css` — `.breadcrumb-tooltip` bottom offset
