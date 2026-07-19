# Make full-screen viewers true app-level takeovers

## Observed journey

- On a wide desktop conversation, open a Markdown document in the viewer pane and choose **Fullscreen**.
- The document expands, but the conversation/sidebar/file-explorer pane dividers still paint over the full-screen surface. The supplied live-browser screenshot shows vertical app-layout rules at approximately the old pane boundaries.
- In some full-screen views, the Markdown filename/header label is clipped or laid out as though width is still being constrained by the underlying application panes.
- Returning to the pane must continue to restore the same document and existing review state.

## Verified findings

- `ConversationPage` mounts focused prose/message/review viewers inside the conversation page’s `#app`, which itself remains inside `DesktopLayout` beside the sidebar, file explorer, their `PaneDivider`s, and optional sub-agent dock (`ui/src/pages/ConversationPage.tsx`, `ui/src/components/DesktopLayout.tsx`).
- `ViewerShell` implements `mode="takeover"` only as a fixed `.viewer-shell--overlay` with a larger z-index. It does not move the takeover out of the pane/layout subtree (`ui/src/components/viewer/ViewerShell.tsx`, `ui/src/index.css`).
- The desktop pane dividers are independently stacked flex siblings (`.pane-divider { z-index: 10; }`). Raising the nested viewer z-index is therefore not a reliable app-level ownership boundary.
- The full-screen diff prior-art task explicitly required mounting at `DesktopLayout` level or otherwise escaping `.app-split-pane` / `.desktop-main`, and required sidebar/file-explorer controls to be neither visible nor interactive (`tasks/43001-p2-done--fullscreen-conversation-diff.md`). The implementation added takeover styling/z-index but left the viewer mounted in `ConversationPage`.
- `MetaViewer` already demonstrates the robust primitive for image takeover: it portals the shell to `document.body`. Text/Markdown focused takeover is not portaled; only image takeover is (`MetaViewer` return path).
- All relevant focused surfaces already converge on shared `ViewerShell mode="takeover"`: Markdown/file (`MetaViewer`), finalized message (`MessageViewer`), commission review (`CommissionReviewViewer`), and conversation diff (`ConversationDiffViewer`). This makes the shell the smallest shared correction point.
- Existing component tests prove roles, URL presentation, exit handling, notes, and scroll restoration, but there is no browser regression proving an app-level takeover covers desktop layout siblings or that a long title retains usable header space.
- The existing viewer-slot and prose-feedback requirements already describe a focused takeover and responsive presentation. This is implementation drift, not a new product behavior.

## Failure model and owning invariant

A “takeover” is currently a visual style applied inside the conversation layout, rather than a structurally app-level surface. Nested z-index cannot reliably defeat sibling stacking/layout boundaries, and the mounted subtree can retain pane-related sizing or clipping. A full-screen viewer must be rendered against the viewport/body boundary so underlying app chrome, pane dividers, and pane width constraints cannot participate in its paint or geometry.

## Proposed scope

### 1. Centralize true takeover rendering

- Make shared `ViewerShell` render every `mode="takeover"` surface through one body-level portal (or an equally small dedicated app-level overlay root).
- Keep `inline` and ordinary `overlay` behavior unchanged.
- Remove the image-only portal special case from `MetaViewer` once the shared shell owns this behavior; there must be one representation/path for takeover mounting, not parallel per-viewer fixes.
- Apply the correction automatically to Markdown/file, message, commission-review, image, and diff takeovers through the shared shell.
- Do **not** solve this with another z-index escalation, pane-width subtraction, or viewer-specific hiding of sidebar/divider elements.

### 2. Keep shared header layout robust

- Ensure the takeover header measures against the viewport, not an underlying pane.
- Give the title and action group explicit flex shrink/growth behavior (`min-width: 0` where required) so long filenames use a deliberate ellipsis without pushing, clipping, or hiding **Find**, copy, notes, and **Return to pane** controls.
- Preserve the full filename/path via the existing tooltip/accessibility surface. “No clipped labels” means controls remain complete and usable; a filename too long for the available viewport may intentionally ellipsize rather than overflow.
- Check the same shared header behavior in every takeover consumer rather than adding Markdown-only CSS.

### 3. Preserve existing interaction contracts

- Preserve typed URL presentation and pane/fullscreen restoration.
- Preserve the distinction between close and **Return to pane**, focused-review pending-note guards, find/Escape precedence, scroll position, annotations, and notes.
- Keep narrow layouts’ existing overlay behavior and omission of a meaningless pane/fullscreen toggle.
- Do not broaden this into a modal framework, browser-native fullscreen, or viewer redesign.

## Regression coverage

Add focused automated coverage at the shared boundary:

- A `ViewerShell` takeover is portaled outside its layout host to the body/app overlay root, while inline/overlay modes keep their existing mount semantics.
- Existing image takeover does not create a second/nested portal path.
- Markdown and diff focused viewers still expose the expected modal role and return/close controls after the shared change.
- Long-title fixture/test coverage keeps action controls present and the title constrained rather than widening the surface.
- Existing `MetaViewer`, `MessageViewer`, `CommissionReviewViewer`, `ConversationDiffViewer`, and viewer-slot tests continue to pass.

## Required live-browser validation

This fix is not complete based on unit tests alone. Run the actual Phoenix app and drive the full conversation journey in a live browser:

1. On a wide desktop viewport with expanded conversation sidebar and file explorer, open a Markdown task/document in the right viewer pane and choose **Fullscreen**.
2. Capture/inspect the focused state and verify the viewer bounds equal the viewport; sidebar, file explorer, all desktop pane dividers, conversation content, and sub-agent divider (if mounted) are neither visible nor hit-testable.
3. Verify the filename starts after the viewer’s own close button, the title area has non-negative usable width, and **Find**, copy, notes (when present), and **Return to pane** are fully visible and clickable.
4. Repeat with a deliberately long filename and at least one constrained desktop width; confirm intentional title ellipsis and no control-label clipping or horizontal page overflow.
5. Return to pane and verify the same document, scroll location, find state, and pending review state remain correct.
6. Open a conversation diff full-screen and verify it uses the identical app-level takeover boundary with no underlying pane dividers.
7. Check a narrow/mobile viewport to confirm the existing overlay remains usable and does not gain an invalid pane toggle.
8. Check browser console output for unexpected errors throughout the journey.

Use screenshots and DOM geometry/hit-testing as acceptance evidence; a component test that only sees `class="viewer-shell--takeover"` is insufficient.

## Acceptance criteria

- Every `ViewerShell` takeover occupies the full viewport independently of sidebar, file-explorer, conversation-viewer, terminal, and sub-agent pane widths.
- No underlying app divider/chrome is visible or interactive while a takeover is open.
- Long Markdown filenames cannot clip or displace full-screen header action labels; overflow is deliberate and local to the title.
- Markdown and diff full-screen views use one shared takeover mechanism.
- Return/close, URL restoration, notes, find, Escape precedence, and scroll behavior do not regress.
- Relevant UI tests/typecheck and `./dev.py check` pass.

## Non-goals

- Browser Fullscreen API or new-window rendering.
- General viewer chrome redesign.
- Changing pane widths, sidebar behavior, viewer-slot URL shape, review-note semantics, or diff functionality.
- Hiding underlying panes as a substitute for establishing the correct portal/mount boundary.
