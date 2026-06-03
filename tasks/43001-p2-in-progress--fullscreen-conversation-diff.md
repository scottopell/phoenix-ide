# Full-screen conversation diff takeover

## Summary

Add a full-screen presentation for the conversation “View Diff” flow, modeled after Task Approval’s takeover behavior but dismissible at any time.

Today the diff viewer is part of the generic viewer slot. On wide desktop it renders as a split pane beside the conversation, leaving the sidebar/file explorer/conversation switching affordances visible. For code-review-style diff inspection, add a deliberate full-screen mode that temporarily takes over the app surface so the user can focus on the diff and review notes without accidentally navigating away.

## Intended UX

- “View Diff” can open the diff as a full-screen takeover over the entire app chrome, including sidebar and file explorer.
- The takeover is dismissible with the existing back/close affordance and Escape; unlike Task Approval, the user is not forced to make a decision or send notes.
- The diff still supports all existing diff functionality:
  - unified/split style toggle,
  - committed/uncommitted sections,
  - line/file review notes,
  - send notes to the composer,
  - loading/error/empty states.
- The full-screen diff should be URL-restorable so reload/back/forward behavior remains predictable.
- Existing inline/split-pane diff behavior should remain structurally available where it is still intentionally used, rather than being coupled to the full-screen path by incidental viewport checks.

## Implementation plan

1. Extend the viewer-slot URL model with an explicit diff presentation mode.
   - Prefer a typed shape such as `ViewerSlot | { kind: 'diff'; presentation: 'fullscreen' | 'pane' }` rather than inferring full-screen from viewport.
   - Because deploys are atomic, update the `?viewer=diff` URL contract directly; no backward-compat shim for old diff URLs is needed.
   - Add a URL writer for opening full-screen diff, e.g. `openDiffFullscreen()` or `openDiff({ presentation: 'fullscreen' })`.

2. Update the “View Diff” action.
   - Make the Work control bar open the new full-screen diff presentation.
   - If keeping split-pane diff as a separate option, expose it as a clearly secondary action rather than overloading the primary “View Diff” button.

3. Render the full-screen diff above app chrome.
   - Mount the full-screen diff at the `DesktopLayout` level, or otherwise use a z-index/surface that covers sidebar, file explorer, command palette entry points, and the conversation content.
   - Reuse `ConversationDiffViewer`/`DiffView`/`ViewerShell` where practical, but ensure the full-screen variant is not constrained by `.app-split-pane` or `.desktop-main` layout.
   - Keep close behavior simple: close clears the diff viewer URL state and returns to the conversation.

4. Keep Task Approval semantics distinct.
   - Do not reuse Task Approval’s forced-decision controls or non-dismissible Escape behavior.
   - The shared principle is only the focused full-screen takeover surface.

5. Update tests.
   - `ViewerSlotContext` tests for diff presentation parsing, URL writes, malformed normalization, and storage/restore behavior under the new URL contract.
   - `WorkActions` tests that the primary “View Diff” action opens the full-screen presentation.
   - Component/layout tests that full-screen diff does not render as the wide-desktop split pane and is closable.
   - Existing `ConversationDiffViewer` behavior should remain covered.

## Acceptance criteria

- On wide desktop, clicking “View Diff” shows the diff full-screen over the entire Phoenix app rather than as a right-hand split pane.
- While full-screen diff is open, sidebar/file explorer/conversation switching controls are not visible or interactable.
- Closing the full-screen diff restores the conversation view without losing unsent review notes unless existing note-close confirmation behavior says otherwise.
- Refreshing a URL with the full-screen diff open restores the full-screen diff for the same conversation.
- Existing diff note creation and “send notes” behavior still works.
- `./dev.py check` passes.
