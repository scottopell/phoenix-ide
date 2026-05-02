---
created: 2026-05-02
priority: p3
status: done
artifact: ui/src/components/FileExplorer/FileExplorerPanel.tsx
---

When the user expands the Skills and/or Tasks panels inside
FileExplorerPanel.tsx, those panels render their list at natural
height with no max-height or internal scroll. The file tree above
(`<FileTree>` inside `.fe-tree-scroll`) has `flex: 1; overflow-y:
auto;` and shrinks to absorb the pressure, but the parent
`.fe-panel--expanded` is `overflow: hidden`, so a tall enough
expanded section can shove the file tree to zero height and push
the section header itself out of view (no `scroll-margin-top`
either).

Concrete repro: open a worktree with ~50 tasks (or many skills),
expand the Tasks panel, expand the Skills panel — the click target
moves under the cursor and the file tree disappears.

## Acceptance

- [ ] When Skills or Tasks expand, their content has a max-height
      and scrolls internally; the section header stays in place
      relative to the click.
- [ ] FileTree retains a usable minimum height (~3 rows) even with
      multiple sections expanded.
- [ ] Multiple expanded sections coexist without viewport scroll
      jumps in the panel.
- [ ] Visual: with all three (MCP / Skills / Tasks) expanded and
      lots of items in each, the panel is fully usable with each
      section contributing roughly its share of the available height.

## Implementation notes

Likely fix: each child panel (`McpStatusPanel`, `SkillsPanel`,
`TasksPanel`) gets `display: flex; flex-direction: column` with the
expanded list inside given `flex: 1; min-height: 0; overflow-y:
auto;`. Outer panel gives equal `flex: 1` to expanded children so
the file tree keeps its share. `scroll-margin-top` on the section
header keeps the click point in view across resize transitions.

CSS work, no JSX restructure needed.

## Lineage

Carved out of task 08653 (wont-do). The original conflated this
real UX bug with a misreading of the architecture; this task is the
narrower, correctly-scoped follow-up.
