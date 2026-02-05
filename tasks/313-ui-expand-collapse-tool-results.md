---
created: 2026-02-05
priority: p2
status: ready
---

# Redesign Tool Block Display: Inline Calls, Collapsed Output

## Summary

Show tool calls (command/input) inline and visible by default, only collapse the output. Currently everything is hidden behind a collapsed header which makes it hard to follow what the agent is doing.

## Problem

Current behavior: Tool blocks show only the tool name (e.g., "bash") and require clicking to see anything:
```
▶ bash          <- have to click to see what command ran
```

This makes it tedious to review conversations - you have to expand every single tool block to understand what happened.

## Proposed Behavior

Show the tool call inline, only collapse the output:
```
┌─ bash ────────────────────────────────┐
│ $ ls -la src/                         │  <- always visible
│ ▶ output (2,341 chars)               │  <- collapsed, click to expand
└──────────────────────────────────────┘
```

### Smart Auto-Expand

Auto-expand output if it's short (configurable threshold):
- Output < 200 chars: show inline, no collapse
- Output 200-500 chars: collapsed by default, show preview
- Output > 500 chars: collapsed, show "output (N chars)"

## Acceptance Criteria

### Tool Call Display
- [ ] Tool input/command always visible (not collapsed)
- [ ] Format nicely per tool type:
  - `bash`: show `$ {command}`
  - `patch`: show filename and operation
  - `think`: show full thoughts (these are usually short)
  - `keyword_search`: show query terms
- [ ] Multi-line inputs shown with reasonable max-height + scroll

### Output Collapsing
- [ ] Short outputs (< 200 chars): show inline, no collapse
- [ ] Medium outputs (200-500 chars): collapsed with preview line
- [ ] Long outputs (> 500 chars): collapsed, show char count
- [ ] Click to expand/collapse output section only
- [ ] Expanded state persists during session (not across page loads)

### Visual Design
- [ ] Clear visual separation between input and output
- [ ] Success/error indicator on output (checkmark vs X)
- [ ] Subtle background difference for tool blocks vs prose

### Bulk Controls
- [ ] "Expand all outputs" / "Collapse all outputs" toggle
- [ ] Keyboard shortcut: `e` to toggle all (when not in text input)

## Technical Notes

Current implementation in `MessageList.tsx`:
```typescript
function ToolUseBlock({ block, result }) {
  const [expanded, setExpanded] = useState(false);
  // Currently: clicking header toggles EVERYTHING
  // New: header always shows input, only output section collapses
}
```

Need to restructure to:
1. Tool header (name + chevron for output)
2. Tool input section (always visible)
3. Tool output section (collapsible)

## Examples

### bash - short output (auto-expanded)
```
┌─ bash ────────────────────────────────┐
│ $ pwd                                  │
│ ✓ /home/user/project                  │
└──────────────────────────────────────┘
```

### bash - long output (collapsed)
```
┌─ bash ────────────────────────────────┐
│ $ find . -name "*.ts" | head -50       │
│ ▶ ✓ output (4,829 chars)              │
└──────────────────────────────────────┘
```

### patch - always show what's being modified
```
┌─ patch ───────────────────────────────┐
│ src/api.ts: replace                   │
│ - const timeout = 5000;               │
│ + const timeout = 10000;              │
│ ✓ applied                             │
└─────────────────────────────────────┘
```

## See Also

- `ui/src/components/MessageList.tsx` - `ToolUseBlock` component
- Task 311 (copy button) - add copy to both input and output sections
