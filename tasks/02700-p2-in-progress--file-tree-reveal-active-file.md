---
created: 2026-05-05
priority: p2
status: in-progress
artifact: pending
---

# file-tree-reveal-active-file

## Plan

## Summary

When a file is opened from outside the file tree (most notably via the Cmd+P command palette, but also from any other future entrypoint), have the desktop file explorer "seek" to that file: expand all ancestor directories and scroll the entry into view. This makes it trivial to keep clicking neighbouring files in the same directory once you've jumped to a file with Cmd+P.

## Context

- Cmd+P → `FileSource.onSelect` → `useFileExplorer().openFile(absPath, rootDir)` → updates `proseReaderState` → `activeFile` is derived in [`FileExplorerContext.tsx`](ui/src/components/FileExplorer/FileExplorerContext.tsx).
- `activeFile` is already passed through to [`FileTree.tsx`](ui/src/components/FileExplorer/FileTree.tsx) and used for the highlight class (`ft-item--active`), but the tree never auto-expands or scrolls — so when the active file lives in a collapsed subtree, opening it via Cmd+P leaves no visible indication in the tree.
- The tree already maintains `expandedPaths` (a `Set<string>` of absolute paths, persisted per conversation in localStorage) and lazy-loads `childItems` per directory. The needed primitive (`loadChildren`) already exists; we just need to drive it from `activeFile`.
- This satisfies REQ-FE-009 ("highlight it in the file tree") more meaningfully — a highlight inside a collapsed folder isn't really a highlight.

## What to do

### 1. Reveal-on-active-file effect in `FileTree.tsx`

Add a new effect keyed on `[activeFile, rootPath]` that:

1. Bails out if `activeFile` is null/undefined or doesn't start with `rootPath` (defensive — handles cwd mismatches).
2. Computes the list of ancestor directories between `rootPath` (exclusive) and `activeFile` (exclusive). Example:
   - `rootPath` = `/home/bits/dev/phoenix-ide`
   - `activeFile` = `/home/bits/dev/phoenix-ide/ui/src/components/FileTree.tsx`
   - ancestors = `[".../ui", ".../ui/src", ".../ui/src/components"]`
3. Merges those ancestors into `expansion.paths` (using the existing atomic `setExpansion` so the localStorage save effect picks them up).
4. For each ancestor not already in `childItems`, calls the existing `loadChildren(path)` so the rows actually materialize.

The expansion-loading effect at lines 325–331 already handles the "expanded but not yet loaded" case for switching conversations — so if we just merge the ancestors into `expandedPaths`, that effect will load them. We can rely on that and avoid a duplicated loop, *or* call `loadChildren` directly for clarity/immediacy. I'll go with the direct call so we don't depend on effect ordering.

### 2. Scroll-into-view after the row appears

The active row may not exist in the DOM at the moment we ask to reveal — its parent directories' children are still being fetched. Approach:

- Tag each `FileTreeItem`'s row `<div>` with `data-path={item.path}`.
- In a separate effect keyed on `[activeFile, childItems]`, after each `childItems` update, look for `document.querySelector('[data-path="..."]')` (scoped to the tree's root via a ref) and call `scrollIntoView({ block: 'nearest', behavior: 'smooth' })` once it's found. Use a one-shot guard (a ref like `lastRevealedFile`) so we don't re-scroll on every subsequent `childItems` change for the same file.

Using `block: 'nearest'` matches the convention already used in `useKeyboardNav.ts` and `CommandPaletteResults.tsx` — it avoids jarring repositioning when the row is already visible.

### 3. Don't fight user collapses

If the user manually collapses an ancestor of the active file, we should **not** immediately re-expand it. The reveal effect should only fire when `activeFile` itself changes, not when `expansion.paths` changes. Keying the effect on `[activeFile, rootPath]` (not on `expandedPaths`) gets this for free.

### 4. Mobile overlay

`FileBrowserOverlay` also renders `FileTree`; the reveal logic lives in `FileTree` so it works there too automatically. No additional work.

### 5. Tests

Add a Vitest test in `ui/src/components/FileExplorer/` that:

- Mounts `FileTree` with a mocked `/api/files/list` returning a nested structure.
- Initially renders with no expansion.
- Re-renders with `activeFile` set to a deeply-nested path.
- Asserts that the ancestor directories' chevrons flip to expanded, and that the active row gets `ft-item--active`.

Mock `Element.prototype.scrollIntoView` (jsdom doesn't implement it) and assert it was called with the active row.

## Acceptance criteria

- Opening a file via Cmd+P that lives in a collapsed subtree:
  - Expands every ancestor directory between `rootPath` and the file.
  - Loads each ancestor's children if not already cached.
  - Scrolls the file's row into view in the tree.
  - Highlights the file row (already worked, still works).
- The persisted per-conversation expansion state in localStorage now includes those auto-expanded ancestors (so it sticks across reloads — feels right; the user just navigated there).
- Manually collapsing an ancestor of the currently-active file does **not** immediately re-expand it.
- Clicking a file already visible in the tree still works exactly as before — the reveal effect is a no-op when ancestors are already expanded, and `scrollIntoView({ block: 'nearest' })` won't move anything that's already on-screen.
- Mobile `FileBrowserOverlay` gets the same behaviour.
- New test passes; `./dev.py check` is clean.

## Progress

