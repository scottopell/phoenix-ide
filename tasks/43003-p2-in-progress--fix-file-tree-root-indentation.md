# Remodel FileTree row layout so indentation is depth-driven

## Problem

The File Explorer tree currently lets item type affect apparent nesting. Directories render a chevron slot before the label, while files render a fake chevron spacer plus a file dot before the label. As a result, root-level files are shifted right and visually resemble first-level children.

The underlying issue is not just a bad width: the row layout is implicit in conditional JSX. That makes indentation an emergent side effect of whether a row is a file or folder.

## Goal

Make FileTree layout tighter and more predictable by modeling row rendering explicitly:

- **Depth controls indentation.**
- **Item kind controls adornments only.**
- Rows at the same depth share the same indentation baseline.
- A depth-1 row is exactly one depth step deeper than a depth-0 row.

## Proposed model

Introduce a small row view model or equivalent helper logic in `ui/src/components/FileExplorer/FileTree.tsx`:

```ts
type TreeRowView = {
  path: string;
  depth: number;
  indentPx: number;        // derived only from depth
  disclosure: Disclosure;  // chevron, spinner, or empty slot
  icon: Icon;              // file glyph/dot or empty/folder slot
  label: string;
  disabled: boolean;
  active: boolean;
  dimmed: boolean;
};
```

Required invariant:

```ts
indentPx = BASE_INDENT + depth * DEPTH_STEP
```

No file/folder branch should add an additional depth-equivalent spacer.

Render each row with stable slots, e.g.:

```tsx
<Row style={{ paddingLeft: row.indentPx }}>
  <DisclosureSlot>{renderDisclosure(row.disclosure)}</DisclosureSlot>
  <IconSlot>{renderIcon(row.icon)}</IconSlot>
  <Label>{row.label}</Label>
</Row>
```

The exact implementation can be lighter than these names if it remains structurally clear, but the important property is that file/folder adornments cannot accidentally change tree indentation.

## Plan

1. Refactor `FileTreeItem` row rendering so row indentation is derived only from `depth`.
2. Replace the current ad-hoc `.ft-indent-spacer` behavior with stable disclosure/icon slots.
3. Adjust FileTree CSS in `ui/src/index.css` to make those slots explicit and predictable.
4. Preserve existing behavior:
   - click-to-expand directories
   - click-to-open viewable files
   - disabled opaque files
   - drag payloads
   - keyboard navigation
   - active row highlighting
   - gitignored dimming
   - loading/empty rows
5. Add regression tests for the layout model:
   - root-level files do not receive depth-1 visual indentation
   - rows at the same depth share the same indentation baseline regardless of file/folder kind
   - depth-1 rows are one depth step deeper than depth-0 rows
6. Run the relevant UI tests and visually verify the File Explorer panel if the dev server is available.

## Acceptance criteria

- Root-level files no longer appear nested under root-level folders.
- File/folder adornments render in stable slots and do not affect nesting.
- Same-depth rows align predictably.
- Nested rows remain visibly indented by a consistent depth step.
- Existing FileTree behavior tests continue to pass.
