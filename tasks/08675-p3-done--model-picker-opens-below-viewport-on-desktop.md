---
created: 2026-04-21
priority: p3
status: done
artifact: ui/src/index.css
---

## Resolution

Resolved in the same session it was filed. The mobile-only
`position: fixed; bottom: calc(var(--state-bar-height) + 8px)` rule
was promoted to the base `.model-picker` rule (no media query), so
the popover opens upward on every breakpoint. See `ui/src/index.css`
around line 2170.


# Model picker opens below viewport on desktop

## Summary

The StateBar sits at the bottom of the viewport on ALL breakpoints (not
just mobile). The model picker currently uses `top: calc(100% + 4px)` on
non-mobile viewports, which opens it downward off the bottom of the
screen. At 1280x800 the picker renders at y=740 with bottom=1019 -- 219px
below the viewport.

Discovered while verifying task 08673. The phase 3 fix for mobile
(switch to `position: fixed` + `bottom: calc(var(--state-bar-height) +
8px)` under `@media (max-width: 600px)`) does not apply above 600px.

## Context

- Mobile fix lives in `ui/src/index.css` around line 2186-2200
  (`@media (max-width: 600px)` block).
- Base rule is around line 2170-2185 (`.model-picker` with
  `top: calc(100% + 4px); left: 0`).
- StateBar position: the `#state-bar` element is laid out at the bottom
  of `.conversation-column` on both mobile and desktop.

## Why this wasn't caught in phase 2

Phase 2 testing focused on desktop layout with the picker trigger on an
idle conversation. On a tall enough viewport (e.g. 1440x1080+) the
picker might partially fit below the StateBar, masking the bug. At
common desktop heights (800, 900, 1024) the picker extends 200-300px
below the viewport.

## Acceptance Criteria

- [ ] On 1280x800 (and common desktop sizes), opening the model picker
      renders it entirely within the viewport.
- [ ] Picker is still anchored near the model trigger (not floating at
      a corner of the screen).
- [ ] Desktop click-outside and Escape behavior unchanged.

## Suggested fix

The simplest fix: extend the mobile rule to all viewports, since the
StateBar is always at the bottom. Remove the `@media (max-width: 600px)`
wrapper and make `position: fixed; bottom: calc(var(--state-bar-height,
42px) + 8px); right: <anchored to trigger>` the base behavior.

Alternative: compute upward vs downward at render time based on the
trigger's bounding rect + viewport height (JS-driven). More flexible
but adds complexity; the current always-upward approach is fine because
the StateBar is always at the bottom.

## Notes

- Discovered during phase 3 verification (task 08673).
- Not a regression -- this was broken in phase 2 as shipped.
- Low-ish priority because the picker is reachable on most real desktop
  viewports (the top portion is visible, user can still click items),
  but "Show all" at ~60vh height makes the lower rows + toggle
  unreachable.
