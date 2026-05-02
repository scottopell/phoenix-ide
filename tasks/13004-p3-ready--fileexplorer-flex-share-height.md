---
created: 2026-05-02
priority: p3
status: ready
artifact: ui/src/components/FileExplorer.tsx
---

FileExplorer middle-column sub-panels (Skills, MCP, Tasks) currently
each cap their expanded body height at `max-height: 300px` with internal
overflow (task 13002). That fixes the single-section-expanded case but
leaves a second-order issue: on a short viewport (e.g. 600-700px) with
all three sections expanded simultaneously, their stacked fixed maxima
can still exceed available height, squeezing the FileTree above to ~0px
because `.fe-panel--expanded` is `overflow: hidden` and the panels are
flex items in a column with no `min-height: 0` rule.

The robust containment is to make `.fe-panel--expanded` a proper flex
column where every section body is `flex: 1 1 auto` with
`min-height: 0` + internal `overflow-y: auto`, so the sections share
available height instead of stacking fixed maxima. Touches
`.fe-panel--expanded` (or wrapping container) plus the four section
bodies (FileTree, Skills, MCP, Tasks).

Surfaced by Copilot review on PR #13 against the 13002 fix.
