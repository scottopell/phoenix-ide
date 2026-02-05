---
created: 2026-02-05
priority: p3
status: ready
---

# Add Copy Button to Code Blocks

## Summary

Add a "copy to clipboard" button on code blocks in agent responses for easy copying of commands and code snippets.

## Context

The agent frequently returns code blocks containing commands, code snippets, or file contents. Currently, users must manually select and copy text from these blocks. A copy button would significantly improve the workflow, especially on mobile where text selection is cumbersome.

## Acceptance Criteria

- [ ] Copy button appears on hover (desktop) or always visible (mobile) for code blocks
- [ ] Button shows in top-right corner of each code block
- [ ] Click copies the code content to clipboard
- [ ] Visual feedback on copy: brief "Copied!" tooltip or icon change (✓)
- [ ] Works for both inline and multi-line code blocks
- [ ] Accessible: proper aria labels, keyboard focusable
- [ ] Does not interfere with code block scrolling

## Technical Notes

- Code blocks are rendered via `renderMarkdown()` in `ui/src/utils.ts`
- Will need to modify the markdown rendering to inject copy buttons
- Use `navigator.clipboard.writeText()` for modern clipboard API
- Consider using a React component instead of injecting raw HTML
- The tool output sections already use `.tool-result-content` class - consider adding copy there too

## Design

```
┌────────────────────────────────────────┐
│ npm install typescript          [📋] │
└────────────────────────────────────────┘
```

On click, icon briefly changes to ✓ then reverts.

## See Also

- `ui/src/utils.ts` - markdown rendering
- `ui/src/components/MessageList.tsx` - message display
