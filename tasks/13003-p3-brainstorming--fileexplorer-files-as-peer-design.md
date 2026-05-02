---
created: 2026-05-02
priority: p3
status: brainstorming
artifact: pending
---

Currently the middle file-explorer panel (FileExplorerPanel.tsx)
has an asymmetric layout:

  - File tree: ALWAYS-VISIBLE primary content (the panel header is
    literally labeled "Files", and FileTree fills the panel body via
    `.fe-tree-scroll { flex: 1; overflow-y: auto }`).
  - MCP / Skills / Tasks: collapsible sections below the tree, each
    expandable with a chevron header.

The retired task 08653 (closed wont-do) suggested "Files should be a
collapsible section like the others," premised on a wrong
architectural reading. But the steel-manned form is real: should the
four sub-views (Files / MCP / Skills / Tasks) become equal-rank
collapsibles in a unified container? That has tradeoffs:

## In favor of equal-rank collapsibles

- Predictable mental model — "the file explorer panel has four
  things, all behave the same way."
- Mobile-friendlier: each section can be opened / closed
  individually without one of them dominating screen space.
- Composes with task 13002 (scroll containment) — once each
  section has internal scroll, making them all peers is a smaller
  delta.

## Against

- File tree is the dominant interaction surface (most clicks land
  there); demoting it to a collapsible adds one click for what is
  currently the default action.
- Existing UX has trained users to expect the file tree open. A
  collapsed-by-default Files section would surprise.
- The current asymmetry communicates priority: Files is primary,
  the others are auxiliary. Flattening loses that signal.

## Options

A. **Keep current shape; just fix scroll containment** (task 13002).
   No design change. Cheapest. Retains the priority signal.

B. **Four equal collapsibles, Files expanded by default.** Files
   gets a chevron header but starts open. Other sections behave the
   same way. Acceptable density when collapsed; breaks priority
   signal slightly.

C. **Two-tier UI**: Files stays primary (always visible), but
   MCP / Skills / Tasks move into a single grouped "Tools"
   collapsible to reduce visual weight at the bottom of the panel.

## Required before implementation

Decision from Scott on which option (or D = something else) to
pursue. This task is design-only until then; do not implement.

## Lineage

Steel-manned read of issue #1 from retired task 08653 (wont-do). The
original task is closed because its premise was wrong; this task
captures the actual design question that fell out of the triage.
