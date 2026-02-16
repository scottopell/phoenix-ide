---
created: 2026-02-08
priority: p3
status: ready
---

# Richer LLM Breadcrumb Preview

## Summary

Show excerpt of LLM response instead of generic "Agent response" in tooltip preview.

## Context

Currently all LLM breadcrumbs show "Agent response" as their preview. This isn't very informative when there are multiple LLM turns in a conversation.

## Requirements

1. Extract first ~50 chars of LLM text response for preview
2. Skip tool_use blocks when finding text
3. Strip markdown formatting for cleaner preview
4. Keep "Agent response" as fallback if no text found

## Implementation Notes

In `extract_breadcrumbs()`, when adding LLM breadcrumb:
- Find the ContentBlock::Text in the agent message
- Truncate to ~50 chars using existing `truncate_preview()`
- Maybe strip common markdown like `#`, `*`, `-` for cleaner display

## Example

Instead of:
- LLM: "Agent response"

Show:
- LLM: "I'll help you fix that. First, let me check…"

## Acceptance Criteria

- [ ] LLM tooltip shows excerpt of actual response
- [ ] Markdown stripped or handled gracefully
- [ ] Falls back to "Agent response" if no text content
