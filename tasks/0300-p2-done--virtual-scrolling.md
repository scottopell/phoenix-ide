---
created: 2026-02-04
priority: p2
status: done
---

# Implement Virtual Scrolling for Large Conversations

## Summary

For conversations with 100+ messages, implement virtual scrolling to only render visible messages. This will improve performance and reduce memory usage.

## Context

During performance optimization, we noted that rendering many messages can be slow and memory-intensive. The current implementation renders all messages in the DOM, which becomes problematic for long conversations.

## Acceptance Criteria

- [x] Implement virtual scrolling using react-window or similar
- [x] Only render messages visible in viewport (+ buffer)
- [x] Maintain scroll position when navigating away and back
- [x] Smooth scrolling experience
- [x] Works with image messages
- [x] Performance: Handle 10,000+ messages smoothly

## Notes

- Consider react-window or react-virtualized
- Need to handle variable height messages (especially with images)
- Preserve scroll position in cache

## Implementation Notes (2026-02-04)

- Used react-window (v1.8.10) with VariableSizeList for variable heights
- Automatically switches to virtual scrolling for conversations >50 messages
- Dynamic height measurement after each row renders
- Scroll position saved to sessionStorage and restored on navigation
- Overscan count set to 3 for smooth scrolling
- All message types supported: user, agent, tools, queued, sub-agents
- Heights cache cleared when switching conversations
