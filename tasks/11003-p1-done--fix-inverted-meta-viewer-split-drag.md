# Fix inverted desktop meta viewer split-pane drag

## Problem

On wide desktop, the right-hand viewer pane for files/markdown/browser/diff/process inspector resizes in the wrong direction when dragging its divider. Dragging the divider left currently makes the right pane smaller; users expect it to make the right pane larger.

## Likely cause

`ConversationPage` wires the split viewer divider to `viewerPane.startDrag(e, 'x')`. Because the controlled pane is to the right of the divider, horizontal drag deltas need to be inverted, like the existing right-docked sub-agent viewer does with `pane.startDrag(e, 'x', true)`.

The keyboard resize behavior already appears to match right-docked semantics:

- `ArrowLeft` increases viewer pane width.
- `ArrowRight` decreases viewer pane width.

So the intended fix is to bring pointer-drag behavior into alignment with keyboard behavior and the other right-docked pane.

## Plan

1. Update the desktop split viewer divider in `ui/src/pages/ConversationPage.tsx` to pass the `invert` flag to `useResizablePane.startDrag` for the right-hand viewer pane.
2. Add or update a focused UI test covering the right-docked viewer divider behavior:
   - starting from a known pane width,
   - dragging left increases the viewer pane width,
   - dragging right decreases it,
   - behavior remains clamped/collapsible according to the existing hook semantics.
3. Run the relevant UI test suite and typecheck/check lane via `./dev.py` as appropriate.

## Acceptance criteria

- In the wide desktop split viewer, dragging the divider left makes the right-hand viewer pane wider.
- Dragging the divider right makes the right-hand viewer pane narrower.
- Keyboard resize behavior remains unchanged and consistent with the pointer behavior.
- No regression to left-side sidebar/file-explorer panes or the existing right-docked sub-agent viewer.
