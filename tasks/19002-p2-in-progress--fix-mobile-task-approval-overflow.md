# Fix mobile task approval action overflow

The task approval reader overflows horizontally on narrow mobile screens. The bottom action toolbar currently renders four full-label buttons in one row (`Discard`, `Send Feedback`, `Continue here`, `Start fresh conversation`), so the rightmost controls extend beyond the viewport.

## Plan

1. Update the task approval reader mobile CSS in `ui/src/index.css`:
   - Keep desktop layout unchanged.
   - Add a narrow-viewport rule for `.task-approval-actions` so actions wrap or stack within the viewport instead of overflowing.
   - Ensure buttons can shrink/wrap text safely (`min-width: 0`, sensible flex basis, text centering) while retaining tappable height.
   - Include safe-area padding for mobile browser chrome where appropriate.
2. Check adjacent task approval elements for mobile width hazards:
   - Header/title row remains ellipsized with priority badge visible.
   - Markdown content/code spans do not force page-wide horizontal overflow beyond their scroll container.
3. Add or update a focused UI test if practical for the button labels/classes, and manually verify at an iPhone-sized viewport.

## Acceptance criteria

- At ~390px viewport width, the task approval screen has no horizontal page overflow.
- All four actions remain reachable and readable on mobile.
- Desktop task approval layout remains visually unchanged.
- Existing `TaskApprovalReader` tests continue to pass.
