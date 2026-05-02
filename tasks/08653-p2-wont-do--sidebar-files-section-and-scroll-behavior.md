---
created: 2026-04-08
priority: p2
status: wont-do
artifact: ui/src/components/Sidebar.tsx
---

# Sidebar: add collapsible Files section + fix expand/scroll behavior

## Two issues

### 1. Missing Files section

The sidebar has collapsible sections for Skills, MCP, and Tasks, but no
equivalent for Files. The file explorer is a separate panel (FileExplorerPanel)
that lives outside the sidebar. For consistency, add a collapsible "Files"
section in the sidebar that either:
- Inlines a compact file tree (matching the other sections' density), or
- Acts as a toggle for the existing FileExplorerPanel

### 2. Expand/scroll behavior is inconsistent

When clicking a section header to expand it, the expected behavior is:
- The header I clicked stays under my cursor
- The section content opens below the header, pushing subsequent sections down
- If the expanded content is too tall, the section scrolls internally

Current issues:
- Expanding a section can push the header itself off-screen (the viewport
  scrolls to accommodate, moving the button away from the cursor)
- Multiple expanded sections can overflow the sidebar with no clear scroll
  containment
- No max-height or internal scroll on individual sections -- a section with
  many items pushes everything else off the bottom

### Fix for scroll behavior

Each collapsible section should:
- Keep the header fixed relative to the click (no viewport jump)
- Have a max-height with internal overflow-y scroll when expanded
- Leave enough room for at least 2-3 other section headers to remain visible
- Use `scroll-margin-top` or similar to prevent the header from being
  pushed out of view

## Done when

- [ ] Collapsible "Files" section in sidebar
- [ ] Expanding a section keeps the clicked header in place
- [ ] Expanded sections have max-height with internal scroll
- [ ] All sections (Files, Skills, MCP, Tasks) behave consistently
- [ ] Sidebar remains usable with multiple sections expanded

---

## Closed wont-do (2026-05-02)

This task is built on a wrong reading of the architecture and the
artifact path is incorrect. Replaced by two narrower tasks (filed
when this one was closed):

- **Scroll-containment fix** — FileExplorerPanel: contain expanded
  Skills / Tasks panels with internal scroll so multi-section
  expansion doesn't push the FileTree (and the click target) out of
  view. This is the real UX bug behind "issue #2" of this task.
- **Files-as-peer (design)** — should the file tree become a
  collapsible peer of Skills / MCP / Tasks instead of being the
  panel's primary content? That's the steel-manned read of "issue
  #1" but it's a real UX design decision that needs your call before
  any implementation.

### Why the premise was wrong

This task said: "The sidebar has collapsible sections for Skills, MCP,
and Tasks, but no equivalent for Files. The file explorer is a
separate panel (FileExplorerPanel) that lives outside the sidebar."

Reality:
- `Sidebar.tsx` is the LEFT column — conversation list, projects,
  theme toggle. It has no Skills/MCP/Tasks sections.
- `FileExplorerPanel.tsx` is the MIDDLE column. It contains the
  FileTree (the "Files" section, primary content with header
  literally labeled "Files") AND the Skills/MCP/Tasks panels below.
- So Skills/MCP/Tasks aren't "in the sidebar" — they're nested in
  the file explorer panel. Files isn't missing — it's the panel
  itself.

The artifact path on the original task (`ui/src/components/Sidebar.tsx`)
also points at the wrong file.
