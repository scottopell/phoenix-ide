---
created: 2025-02-10
priority: p2
status: done
---

# Inline Patch Diff Preview

## Summary

Show a compact, inline preview of the diff directly in the patch tool block, rather than requiring users to expand or click through to see what changed.

## Context

Currently the patch tool shows:
1. File path and operation (e.g., `src/foo.rs: overwrite`)
2. Collapsed unified diff output (expandable)
3. `PatchFileSummary` with clickable file links

Users want to see at a glance what changed without expanding. A small inline diff preview would provide this.

## Design Options

### Option A: Side-by-side mini diff
Show 2-3 lines of context around changes in a compact side-by-side view.

### Option B: Unified diff summary
Show a collapsed view like:
```
-3 lines, +5 lines in 2 hunks
```
With hover to preview first hunk.

### Option C: Syntax-highlighted inline diff
Show the full diff but with proper syntax highlighting and line numbers, always visible (not collapsed).

## Acceptance Criteria

- [ ] Patch diffs visible inline without expanding
- [ ] Syntax highlighting for diff content
- [ ] Added lines highlighted green, removed red
- [ ] Compact enough to not overwhelm the message flow
- [ ] Still supports clicking through to prose reader for full file view
- [ ] Works well on mobile (consider horizontal scroll or wrap)

## Technical Notes

- Current diff rendering is in `ToolUseBlock` using `containsUnifiedDiff()`
- Could use `react-diff-viewer` or similar library
- Or roll custom with `react-syntax-highlighter` + diff parsing
- `PatchFileSummary` already parses unified diffs - could extend that

## Reference

- Rusty's recovered UI may have relevant components in `~/rustey-shelley-gui-recovery/`
- GitHub's inline diff preview on PR list pages is a good UX reference
