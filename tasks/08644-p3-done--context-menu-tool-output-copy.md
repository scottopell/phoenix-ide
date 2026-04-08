---
created: 2026-04-07
priority: p3
status: done
artifact: ui/src/components/MessageContextMenu.tsx
---

# Context-aware copy options in message context menu

## Problem

Right-clicking a message shows "Copy as Markdown" and "Copy as Plain Text"
for the entire message. When the message contains tool results (bash output,
patch results, etc.), there's no way to copy just the tool output or just
the command via the context menu. The inline CopyButtons exist but are small
and unclear about scope.

## What to build

Extend MessageContextMenu to detect which sub-element was right-clicked
and add context-specific copy options:

- Right-click on a bash command block: add "Copy command"
- Right-click on a tool result/output: add "Copy output"
- Right-click on agent text: add "Copy response text" (just text blocks,
  no tool calls)
- General options (Copy as Markdown, Copy Selection, Select All) always
  present

Walk up from the click target to find the nearest tool-result container
or command element, similar to how the menu already walks up to find
`.message`.

## Done when

- [ ] Right-click on tool output shows "Copy output" option
- [ ] Right-click on bash command shows "Copy command" option
- [ ] Right-click on plain text area shows general options only
- [ ] Existing whole-message copy options still work
