# Grounding Panel: File Tree Context Menu & Drag-and-Drop to Composer

## Problem

The grounding panel's file listing/browser UI is great for browsing and opening files, but the only interaction is single-click-to-open. There's no context menu and no drag-and-drop to the message composer — both are expected affordances in any file browser and would significantly reduce friction when referencing files in messages.

## Current State

- **FileTree** (`ui/src/components/FileExplorer/FileTree.tsx`): click file → opens in viewer; click directory → toggles expansion. That's it. No right-click, no drag, no keyboard arrow navigation.
- **InputArea** (`ui/src/components/InputArea.tsx`): already has OS file drag-and-drop (for attaching files from the desktop) and inline references (`@file` includes contents, `./path` references only, `/skill` loads skill). Draft is managed via `DraftStore` with `appendDraft` for programmatic insertion.
- **Existing context menu pattern** (`MessageContextMenu.tsx`, `FilePathContextMenu.tsx`): document-level `contextmenu` listener, viewport-clamped positioning, click-outside/Escape to close, mutual exclusion via custom events, shared `msg-context-menu` CSS.
- **External draft insertion**: `phoenix:insert-draft` custom event → `appendDraftCb` + `requestComposerFocus()` in `ConversationPage.tsx`. This is the established path for surfaces without direct draft access.

## Proposed Features

### Feature 1: File Tree Context Menu

New `FileTreeContextMenu.tsx` following the existing `FilePathContextMenu` pattern:

- Listens for `contextmenu` on the file tree container (`.ft-root` / `.fe-tree-scroll`)
- Extracts `data-path` from the `.ft-item` element under the cursor
- **File actions:**
  - Copy relative path (relative to `rootPath`)
  - Copy absolute path
  - Insert `@file` reference (include contents at send time)
  - Insert `./path` reference (point AI at the file, no expansion)
  - Open in viewer (same as click)
- **Directory actions:**
  - Copy relative path
  - Copy absolute path
  - Insert `./path` reference
- Insert actions dispatch `phoenix:insert-draft` (existing event) so the text lands in the composer and focus moves there
- Copy actions use the existing `copyToClipboard` utility
- "Open in viewer" uses `FileExplorerContext.openFile`
- Shift-right-click defers to native browser menu (existing escape-hatch convention)
- Mutual exclusion with existing context menus via the `contextMenuEvents` event system

### Feature 2: Drag-and-Drop File Tree → Composer

- `FileTreeItem` becomes `draggable={true}` with `onDragStart` setting a custom data transfer type (`application/x-phoenix-file-path`) carrying the file path and root directory
- `InputArea`'s drop handler detects the custom type *before* the existing OS `Files` check, so the two drop modes don't conflict
- On drop, inserts an `@path` reference into the draft (include-contents is the most common intent; the context menu covers `./path` for the less common case)
- Visual feedback: the composer's existing `isDragOver` highlight activates for custom-type drags too
- Directories are also draggable — dropping a directory inserts `@dir/` (the server's `@` expansion handles directories)

### Feature 3: Keyboard Navigation in File Tree

The file tree currently has `tabIndex` on items but no arrow-key handling and does not participate in the keyboard interaction model. The existing `specs/keyboard-interaction/` spec (REQ-KB-001 through REQ-KB-008, all complete) defines a focus scope stack — interactive panels register via `useRegisterFocusScope` and capture navigation keys. The file-explorer spec (`specs/file-explorer/`) has no keyboard requirements yet.

Adding keyboard nav to the file tree means:

- The file tree registers as a focus scope (via `useRegisterFocusScope`) so it captures navigation keys when focused, per REQ-KB-001 / REQ-KB-003 / REQ-KB-008
- Auto-focus on appearance per REQ-KB-004 (when the panel is expanded, focus the first tree item or the active file)
- Arrow Down/Up: move focus between visible tree items
- Enter/Space: open file or toggle directory expansion
- Arrow Left: collapse an expanded directory (or move to parent if collapsed)
- Arrow Right: expand a collapsed directory (or move to first child if expanded)
- Home/End: jump to first/last visible item
- Escape: blur / exit the tree's focus scope per REQ-KB-005
- The existing `useKeyboardNav` is flat-list-only (`{id, slug}[]`) and gated by `hasActiveScope` — the tree needs its own tree-specific handler that uses `stopPropagation` for consumed keys (REQ-KB-003)
- This should be spec'd: add a REQ-FE-011 to `specs/file-explorer/requirements.md` referencing `specs/keyboard-interaction/` per its dependency rule

## Implementation Notes

- The `FileTreeItem` memo equality function (`areFileTreeItemPropsEqual`) must remain correct — adding `draggable` and drag handlers shouldn't break the memoization unless the handlers change identity per render (use `useCallback`).
- The context menu must not interfere with the existing `FilePathContextMenu` (which handles `.file-path-link` elements inside the message list). Scope the listener to the file tree container, or use the mutual-exclusion event system.
- The `InputArea` drop handler currently checks `Array.from(e.dataTransfer.types).includes('Files')`. The custom-type check must come first, since a custom-type drag won't have `Files` in the types list — but the guard should be explicit to avoid ambiguity.
- The `phoenix:insert-draft` event is the cleanest insertion path — it already handles the "InputArea may be unmounted" case (narrow-desktop fullscreen) and focuses the composer.

## Testing

- Unit tests for `FileTreeContextMenu` (render, action dispatch, mutual exclusion, viewport clamping) following `MessageContextMenu.test.tsx` patterns
- Unit tests for drag-and-drop: `onDragStart` sets the correct data transfer type; `InputArea` drop handler inserts `@path` for custom-type drops and still handles OS file drops
- Keyboard navigation tests for `FileTree` (arrow keys, Enter, Escape, focus scope registration)
- Manual QA via the grounding panel fixture page (`/__qa/grounding-panel`)

## Scope

This task covers the three features above. Future possibilities (not in scope, but worth noting):
- File tree filter/search input
- Multi-select for batch operations
- "Copy file contents" to clipboard
- "Reveal in terminal" action
