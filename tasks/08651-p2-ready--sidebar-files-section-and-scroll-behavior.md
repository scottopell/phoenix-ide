---
created: 2026-04-08
priority: p2
status: ready
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
