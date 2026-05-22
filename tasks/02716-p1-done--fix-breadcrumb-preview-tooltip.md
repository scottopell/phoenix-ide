# Fix breadcrumb preview tooltip clipping and native title tooltip

## Problem

Hovering a breadcrumb item shows two tooltip systems:

1. the browser-native tooltip from the breadcrumb item's `title` attribute, e.g. `Running a tool`
2. Phoenix's richer breadcrumb preview tooltip, which includes the tool result/preview text

The native tooltip is redundant/noisy, and the richer preview is mostly hidden.

## Repro evidence

Started dev server in this worktree with:

```bash
tmux new-window -d -n phoenix-dev -c /Users/scott.opell/dev/phoenix-ide/.phoenix/worktrees/e508fab5-dcaa-458b-b667-a0f3a3b15dd2 \
  bash -lc './dev.py up; code=$?; echo EXIT:$code; sleep 3600'
```

Dev server came up at `http://localhost:8046` for this worktree (`port offset +5`).

Created a new conversation with prompt:

> Use a tool to run `pwd` and then briefly say done.

During execution, hovered a `.breadcrumb-item.tool` breadcrumb. Browser inspection showed:

- `.breadcrumb-tooltip` exists and has `position: fixed; z-index: 1000`
- its computed rect was approximately `top: 637px; bottom: 708px`
- `#breadcrumb-bar` rect was approximately `top: 654px; bottom: 690px`
- `#breadcrumb-bar` has `overflow: auto hidden`
- `document.elementsFromPoint()` inside the tooltip's upper body returned `#breadcrumb-trail` / `#breadcrumb-bar`, not `.breadcrumb-tooltip`

So the preview is not primarily losing a z-index fight; it is being clipped by the breadcrumb bar's overflow because the tooltip is rendered as a descendant of the scroll container.

Screenshot captured during repro:

`/tmp/phoenix-screenshot-138ef176-7cf5-4eb7-ab0a-361d27bd6ca8.png`

## Current code

`ui/src/components/BreadcrumbBar.tsx` renders the tooltip inline as a child of the hovered `.breadcrumb-item` and sets a native title:

```tsx
title={BREADCRUMB_TITLES[b.type] || b.label}
...
{showTooltip && <span className="breadcrumb-tooltip" ...>...</span>}
```

`ui/src/index.css` has:

```css
#breadcrumb-bar {
  overflow: auto hidden;
  position: relative;
  z-index: 1;
}

.breadcrumb-tooltip {
  position: fixed;
  bottom: calc(var(--breadcrumb-height) + 24px);
  z-index: 1000;
}
```

Because the fixed element is still inside an overflow-clipping ancestor, only the portion within the breadcrumb bar is visible.

## Fix plan

1. Remove the `title` attribute from `.breadcrumb-item` so the native browser tooltip no longer competes with the richer preview.
   - Preserve accessible naming if needed with `aria-label={BREADCRUMB_TITLES[b.type] || b.label}`.

2. Render the rich tooltip outside the `#breadcrumb-bar` scroll/clipping container.
   - Prefer a React portal to `document.body` from `BreadcrumbBar.tsx`.
   - Keep positioning data in component state.
   - Compute both horizontal and vertical viewport coordinates from the hovered item rect.
   - Position above the breadcrumb item using `top`, not the current global fixed `bottom`, because the breadcrumb bar sits above the state bar/terminal area rather than at the viewport bottom.

3. Keep the tooltip non-interactive unless there is a product reason to make it hoverable/copyable.
   - Current `pointer-events: none` is fine.

4. Add/adjust tests where practical.
   - Component-level test: breadcrumb item should not have `title`; should have accessible label if we add one.
   - Component-level/DOM test: hovering with preview/result renders tooltip through the portal container/body, not nested under `#breadcrumb-bar`.
   - Positioning helper tests if vertical positioning is factored into a pure function.

5. Validate manually in browser.
   - Start `./dev.py up`.
   - Create/run a tool conversation.
   - Hover a tool breadcrumb.
   - Confirm rich preview is fully visible above the breadcrumb bar.
   - Confirm no native browser tooltip appears.

## Notes

The existing `.breadcrumb-tooltip` `z-index: 1000` can stay high, but z-index alone will not solve the clipping while the tooltip remains a descendant of `#breadcrumb-bar`.
