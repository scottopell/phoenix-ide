---
created: 2026-02-08
priority: p1
status: ready
---

# Breadcrumb Hover Tooltip

## Summary

Hovering over a breadcrumb should show a tooltip with details about that step.

## Context

Breadcrumbs show tool names like "bash" or "patch" but don't reveal what was actually executed. A tooltip would provide quick context without scrolling.

## Requirements

1. Backend: Include preview/summary data in breadcrumb
   - For tools: first ~50 chars of command/input
   - For User: first ~50 chars of message
   - For LLM: "Agent response" or similar
2. Frontend: Show tooltip on hover with:
   - Tool name (bold)
   - Preview text (truncated)
   - Timestamp (optional)
3. Tooltip positioning: above breadcrumb, don't overflow viewport
4. Quick show/hide (150ms delay to show, immediate hide)

## Implementation Notes

- Use CSS tooltip or lightweight component (no heavy tooltip library)
- Truncate preview with ellipsis
- For bash: show command
- For patch: show filename
- For think: show "Internal reasoning"

## Acceptance Criteria

- [ ] Hover shows tooltip after brief delay
- [ ] Tooltip shows tool name and preview
- [ ] Tooltip doesn't overflow screen edges
- [ ] Tooltip hides immediately on mouseout
- [ ] Works on mobile (long-press or tap?)
