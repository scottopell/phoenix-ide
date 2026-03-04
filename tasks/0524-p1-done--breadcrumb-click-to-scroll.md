---
created: 2026-02-08
priority: p1
status: done
---

# Breadcrumb Click to Scroll

## Summary

Clicking a breadcrumb item should scroll the message list to the corresponding message or tool result.

## Context

Breadcrumbs show the LLM thought trail (User → bash → patch → LLM) but clicking them does nothing. Users should be able to click to navigate to that part of the conversation.

## Requirements

1. Each breadcrumb needs to store a reference to its message (sequence_id or message_id)
2. Backend: Include `message_id` or `sequence_id` in breadcrumb data
3. Frontend: Add click handler that scrolls to the message element
4. Smooth scroll animation
5. Brief highlight/flash on the target message after scrolling

## Implementation Notes

- Tool breadcrumbs should scroll to the agent message containing the tool_use block
- User breadcrumb scrolls to the user message
- LLM breadcrumb scrolls to the final agent response with text
- Use `element.scrollIntoView({ behavior: 'smooth', block: 'center' })`
- Consider adding a `data-message-id` attribute to message elements for targeting

## Acceptance Criteria

- [ ] Clicking User breadcrumb scrolls to user message
- [ ] Clicking tool breadcrumb scrolls to that tool's message
- [ ] Clicking LLM breadcrumb scrolls to final response
- [ ] Smooth scroll animation
- [ ] Visual feedback on target message
