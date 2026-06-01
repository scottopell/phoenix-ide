# Fix Pierre diff gutter note button positioning

## Problem

The custom Phoenix `+` / add-note affordance in the new Pierre-based diff viewer overlays the line-number gutter. Pierre appends `[data-gutter-utility-slot]` inside the hovered line-number element and expects the slotted utility to use Pierre's default `[data-utility-button]` geometry. Phoenix's custom `.phoenix-diff-add-note` button does not opt into that geometry, so it is laid out at the wrong edge of the gutter.

## Plan

1. Update `PhoenixDiffCodeView`'s custom gutter utility button so it participates in Pierre's expected utility-button contract, likely by adding the `data-utility-button` attribute while keeping Phoenix-specific class/ARIA/title/click behavior.
2. Adjust `.phoenix-diff-add-note` CSS to complement rather than fight Pierre's slot positioning:
   - preserve Phoenix visual styling and hover/focus states,
   - avoid overriding Pierre's size/positioning in ways that reintroduce gutter overlap,
   - keep the hit target and icon visually aligned in unified and split diff modes.
3. Verify the button no longer overlaps line numbers in:
   - unified diff view,
   - split diff view,
   - added/deleted/context lines,
   - long line / horizontally scrollable diffs.
4. Run the relevant UI checks/tests for the diff viewer.

## Acceptance criteria

- The add-note control appears just outside/at the edge of the line-number gutter without covering the line number text.
- Clicking the control still opens the note dialog for the hovered line.
- The control remains keyboard/focus visible and accessible.
- Existing diff note tests continue to pass.
