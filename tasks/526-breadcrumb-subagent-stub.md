---
created: 2026-02-08
priority: p2
status: ready
---

# Breadcrumb Subagent Support (Stub)

## Summary

Add minimal subagent representation in breadcrumbs when subagents are spawned.

## Context

The breadcrumb system supports a "subagents" type but it's not implemented. When an agent spawns subagents, the breadcrumb trail should reflect this. This is a stub implementation - full subagent UI is out of scope.

## Requirements

1. Backend: Detect subagent tool calls in message history
   - Look for `subagent` tool_use blocks
   - Create breadcrumb with type "subagents" and count label
2. Frontend: Display subagent breadcrumb with distinct styling
   - Label: "subagent" or "2 subagents" (with count)
   - Different color/icon to distinguish from regular tools
3. No click/hover interaction needed (stub only)

## Implementation Notes

- Subagent tool is named "subagent" in tool_use blocks
- Could show "subagent (slug)" as label if slug is in input
- Keep it simple - just show that subagents were used
- Existing CSS may have `.breadcrumb-item.subagent` or similar

## Acceptance Criteria

- [ ] Subagent tool calls appear in breadcrumb trail
- [ ] Visually distinct from other tools
- [ ] Shows count if multiple subagents
- [ ] No errors when subagents not present
