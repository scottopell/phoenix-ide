# Fix MetaViewer first-open freezes and diff scrolling regression

## Problem

After the MetaViewer/viewer-shell refactor, two user-visible regressions need a focused repair:

1. Opening a file for the first time can freeze the React app for several seconds before the viewer becomes usable. The likely hot path is synchronous text/code/markdown rendering immediately after `/api/files/read` resolves:
   - `FileViewer` builds a full `MetaViewerPayload` and mounts `MetaViewer` immediately.
   - `CodeViewerBody` runs `react-syntax-highlighter` across the entire file and then wraps every rendered row in `AnnotatableBlock`.
   - `TextViewerBody` also creates one React element per line with no size guard.
   - `MarkdownViewerBody` parses/renders the whole document synchronously.
   - The first open also pays lazy chunk + syntax grammar registration cost, making the stall especially obvious.

2. The diff view cannot be scrolled. The likely regression is the new shared `ViewerShell` scroll/height hierarchy around `DiffView`/`PhoenixDiffCodeView`:
   - `.viewer-shell-body` currently has `overflow: auto`.
   - `.diff-viewer-body` hides overflow and expects Pierre `CodeView` to own scrolling.
   - The Pierre `CodeView` wrapper only receives `flex: 1; min-height: 0`, but may not get the explicit bounded scroller/height it needs after being moved under `ViewerShell`.

## Plan

1. Reproduce and instrument both issues in the UI.
   - Use a large source file to verify the first-open stall.
   - Open worktree diff and confirm which element should scroll vs which one currently receives wheel events.
   - Capture enough timing/DOM evidence to distinguish CPU-bound render from network latency.

2. Fix file first-open responsiveness.
   - Add a size/line-count based fallback path for large text-like files so opening a large file does not synchronously syntax-highlight or create thousands of annotatable nodes in one commit.
   - Prefer a structurally explicit render mode such as `plainLargeText`/`largeCodeFallback` over ad-hoc booleans, so large-file behavior is owned by the payload/render classification.
   - Keep small/normal files on the existing rich path with notes, jump-to-line, copy, and scroll restoration intact.
   - Ensure the first visible state remains responsive: the user should see a loading/large-file notice or plain text quickly instead of the whole app freezing.

3. Fix diff scrolling.
   - Make the shared shell support non-shell-owned scrollers without nested/competing overflow.
   - Ensure `DiffView` gives `PhoenixDiffCodeView`/Pierre a bounded height and the actual scrollable element receives wheel/trackpad input.
   - Preserve file viewer scrolling through `.viewer-content` and notes panel behavior.

4. Add regression coverage.
   - Unit/component tests for large-file routing/fallback behavior.
   - A DOM/layout-oriented test where feasible for diff viewer scroll container classes/structure.
   - Keep existing MetaViewer, FileViewer, and ConversationDiffViewer tests passing.

5. Validate.
   - Run targeted UI tests for viewer components.
   - Run the project check command appropriate for this repo before committing.
