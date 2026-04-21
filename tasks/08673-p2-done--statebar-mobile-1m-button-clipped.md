---
created: 2026-04-21
priority: p2
status: done
artifact: ui/src/components/StateBar.tsx
---

# StateBar 1M button clipped in mobile portrait

## Summary

On iPhone 14 Pro (and similar ~393px-wide viewports) in portrait, the
"Switch to 1M?" button in the StateBar is pushed off-screen to the
right. User has to rotate to landscape to reach it.

## Context

Reported in-session. The StateBar renders left-aligned content (slug,
mode chip, model label, 1M badge, upgrade button) and right-aligned
status. On narrow portrait viewports the left group overflows its
container and the upgrade button is clipped before it becomes
scrollable or wraps.

Line 1 of the StateBar currently packs: back-arrow + slug + mode chip +
model abbreviation + (conditional) 1M badge + (conditional) upgrade
button. That is 4-6 inline items competing for width on the same row.

## Acceptance Criteria

- [x] On a 390px-wide viewport (iPhone 14 Pro portrait), the "Switch to
      1M?" button (or whatever replaces it if task 08672 ships first)
      is reachable without rotating.
- [x] Fix does not regress the desktop layout (same row stays single
      line at >=768px).
- [x] Verified by browser devtools responsive mode at 390x844 AND by
      running the existing StateBar component tests.

## Resolution

Phase 3 verification at 390x844 found TWO issues with the phase 2
picker implementation:

1. **StateBar line 1 overflowed its container**, clipping the model
   trigger behind the right-side state indicator. The model button's
   right edge rendered at x=326 while the left container clipped at
   x=248. Root cause: `.statebar-slug` had `min-width: auto` and
   `.statebar-line1` had no `max-width`, so with `align-items:
   flex-start` on the flex-column parent, line1 sized to its intrinsic
   content (314px) rather than the parent's 236px cross size. The slug
   then refused to shrink below its content width.

2. **Picker opened downward off the bottom of the viewport.** The
   StateBar sits at y=756 on a 844-tall viewport, leaving only 88px
   below it. Picker needs 280-506px. Also, ancestor `overflow: hidden`
   on `#state-bar-left` and `.statebar-line1` clipped any absolutely-
   positioned popover painting.

### CSS-only fixes applied to `ui/src/index.css`

- Added `min-width: 0` to `.statebar-slug` so flex-shrink can actually
  engage.
- Added `max-width: 100%` to `.statebar-line1` and `.statebar-line2`
  so they are constrained by the parent's cross size (since
  `align-items: flex-start` doesn't stretch them).
- Replaced the mobile `@media (max-width: 600px)` rule for
  `.model-picker`: now uses `position: fixed; top: auto; left: auto;
  right: 8px; bottom: calc(var(--state-bar-height, 42px) + 8px);` so
  the popover escapes ancestor `overflow: hidden` and anchors above
  the bottom StateBar.

### Verified at 390x844

- Model trigger fully visible inside viewport (slug truncates with
  ellipsis as "list-curren...").
- Caret visible to the right of the 1M badge.
- Picker opens upward, fully inside viewport (y=515, bottom=794).
- "Show all models" expands list to 16 items with scroll
  (scrollHeight=488, clientHeight=475, overflow-y: auto).
- Selecting a non-current model closes the picker and updates the
  StateBar model label.

### Desktop sanity check at 1280x800

- StateBar layout unchanged (slug hidden via existing
  `@media (min-width: 1025px)` rule, mode + model render as before).
- My mobile `@media (max-width: 600px)` block does not fire at
  1280px, so desktop picker behavior is unchanged from phase 2.

### Known issue out of scope (filed as follow-up)

Task 08675 tracks the pre-existing phase-2 bug where the desktop
picker also opens off the bottom of the viewport. The StateBar is at
the bottom on all breakpoints, but I limited my fix to mobile per the
task's "do not rework the picker structure" guard rail.

## Notes

- File: `ui/src/components/StateBar.tsx:254-310` for the markup.
- Likely options: wrap the model+upgrade group to its own line at narrow
  widths; convert the upgrade button to an icon-only affordance on
  mobile; or move the upgrade into an overflow menu.
- Cross-reference with task 08672 (mid-conversation model picker) --
  if that ships first, the layout rework should assume a general picker,
  not just the 1M button.
