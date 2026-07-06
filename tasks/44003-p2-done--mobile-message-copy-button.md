# Add mobile message copy button and fix copy icon glyph

## Problem

Message copying currently depends on the desktop message context menu. On touch/mobile, that context menu path is intentionally disabled or avoided so long-tap keeps native mobile expectations. As a result, mobile users do not have an obvious way to copy a whole message.

The shared `CopyButton` icon is also misleading: its current SVG reads more like a side-pane/open-panel glyph than the conventional overlapping-squares copy icon.

## Goal

Add an explicit, touch-friendly copy affordance for messages on mobile while preserving desktop context-menu behavior, and update the shared copy icon so every existing copy button uses a recognizable two-overlapping-boxes glyph.

## Proposed implementation

1. Update the shared `CopyButton` glyph in `ui/src/components/CopyButton.tsx`:
   - Replace the current copy SVG with a conventional overlapping-squares/two-documents icon.
   - Keep the existing copied/checkmark state, API, labels, disabled behavior, and styling contract.

2. Reuse the message-copy extraction semantics already present in `MessageContextMenu.tsx`:
   - User messages copy their raw text.
   - Agent messages copy text blocks joined as Markdown.
   - Avoid introducing a divergent second definition of “message markdown”; extract/share a helper if needed.

3. Add a mobile/touch message copy button in `ui/src/components/MessageComponents.tsx`:
   - Render a small `CopyButton` for finalized user and agent messages.
   - The button copies the same whole-message Markdown text as the existing context-menu `Copy as Markdown` action.
   - Use an accessible label such as `Copy message` or `Copy Phoenix message` / `Copy your message`.
   - Keep it unobtrusive on desktop; show it for touch/mobile contexts via CSS such as `@media (hover: none)` or the app’s existing mobile breakpoint conventions.
   - Do not interfere with message text selection, file-path taps/context menus, code-block copy buttons, tool output copy buttons, or long-tap native behavior.

4. Style the affordance:
   - Position it in/near the message header so it is discoverable without covering message content.
   - Ensure touch target size is reasonable for mobile.
   - Preserve desktop information density and avoid duplicate visible copy controls where desktop context menu already works.

5. Add/adjust tests:
   - Unit test that finalized user and agent messages expose the mobile copy button affordance and copy the expected text.
   - Regression test that the existing `MessageContextMenu` copy actions still copy the same values.
   - Test or snapshot/assertion for the updated shared copy icon if practical, without overfitting to SVG path internals.

## Acceptance criteria

- On mobile/touch layouts, each finalized user/agent message has an obvious copy button.
- Tapping the button copies the same whole-message Markdown content as the existing desktop context menu.
- Desktop context-menu behavior remains unchanged.
- Existing code-block, tool-input/tool-output, mermaid, and viewer copy buttons still work and now show the corrected copy glyph.
- The copy glyph clearly reads as “copy” rather than “open side pane”.
- Relevant UI tests pass.
