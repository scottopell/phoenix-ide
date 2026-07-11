# Restore the FileTree single adornment-slot layout

## Problem

The depth-driven FileTree layout reached `main` through `9aa4aba5`, but the later refinement from `ee15f319` did not. Current rows still render separate disclosure and icon columns, consuming 28px before every label and locking the superseded layout into tests. The intended design uses one compact, fixed-width adornment slot so files and directories cannot alter apparent nesting.

## Goal

Bring the refined FileTree layout forward onto current `main` without blindly cherry-picking the historical commit:

- derive row indentation only from depth;
- render directory chevrons/spinners and file dots in one shared 14px adornment slot;
- remove the simultaneous disclosure-plus-icon columns;
- retain the compact 4px base indentation and 12px depth step from the approved refinement;
- retain subtle child guide lines aligned with the same depth model.

## Implementation plan

1. Update `ui/src/components/FileExplorer/FileTree.tsx`:
   - replace `TreeRowDisclosure` and `TreeRowIcon` with one `TreeRowAdornment` sum type;
   - replace the `disclosure` and `icon` fields in `TreeRowView` with `adornment`;
   - map directories to chevron/spinner adornments and files to the file-dot adornment;
   - combine the render helpers into one adornment renderer;
   - render exactly one fixed-width leading slot per row;
   - keep row, loading, and empty-state indentation derived from the shared depth function.
2. Update the owning FileTree styles. Prefer colocating component-specific CSS beside `FileTree.tsx` if this bounded section can be extracted safely; otherwise make the minimal existing `index.css` change:
   - replace disclosure/icon slot rules with one 14px leading-slot rule;
   - restore depth-aligned child guide lines;
   - ensure guide lines do not intercept input or affect layout.
3. Update layout regression tests to assert:
   - root file and root directory rows share the same indentation baseline;
   - same-depth file and directory labels align through the same single slot;
   - depth 1 differs from depth 0 by exactly one configured 12px step;
   - each row has one leading adornment slot;
   - legacy `.ft-disclosure-slot`, `.ft-icon-slot`, `.ft-indent-spacer`, and `.ft-expand-icon` elements are absent;
   - directory loading uses the same adornment slot;
   - child guide lines remain aligned with the depth model where practical to test structurally.
4. Preserve and verify all existing FileTree behavior:
   - click-to-expand directories;
   - click-to-open viewable files;
   - disabled opaque rows;
   - drag payloads, including opaque files and directories;
   - keyboard navigation;
   - active-file expansion and highlighting;
   - gitignored dimming;
   - loading and empty states;
   - expansion persistence and refresh behavior.
5. Use `ee15f319` only as patch/design evidence. Reconcile it manually with current `main` and any subsequent FileTree refactors rather than cherry-picking it wholesale.

## Acceptance criteria

- `TreeRowView` has one adornment field, not parallel disclosure/icon representations.
- Every file and directory row renders exactly one fixed-width leading adornment slot.
- Files and directories at the same depth have the same label baseline.
- Row indentation is `4px + depth * 12px`, with no item-kind-dependent spacer.
- Loading directory spinners occupy the same slot as directory chevrons.
- Child guide lines visually agree with the configured depth step and do not affect interaction.
- No legacy fake disclosure spacer or two-column adornment layout remains.
- Existing FileTree interaction tests continue to pass, and the refined layout invariants have explicit regression coverage.
- Repository validation passes with `./dev.py check`.

## Validation

Run at minimum:

```bash
cd ui && corepack pnpm exec vitest run \
  src/components/FileExplorer/FileTree.test.tsx \
  src/components/FileExplorer/FileTree.dnd.test.tsx
./dev.py check
```

Visually inspect the FileTree grounding fixture at root and nested depths, including expanded, loading, empty, active, dimmed, and disabled rows.
