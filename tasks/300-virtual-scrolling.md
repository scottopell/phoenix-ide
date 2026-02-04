---
created: 2026-02-04
priority: p2
status: pending
---

# Implement Virtual Scrolling for Large Conversations

## Summary

For conversations with 100+ messages, implement virtual scrolling to only render visible messages. This will improve performance and reduce memory usage.

## Context

During performance optimization, we noted that rendering many messages can be slow and memory-intensive. The current implementation renders all messages in the DOM, which becomes problematic for long conversations.

## Acceptance Criteria

- [ ] Implement virtual scrolling using react-window or similar
- [ ] Only render messages visible in viewport (+ buffer)
- [ ] Maintain scroll position when navigating away and back
- [ ] Smooth scrolling experience
- [ ] Works with image messages
- [ ] Performance: Handle 10,000+ messages smoothly

## Notes

- Consider react-window or react-virtualized
- Need to handle variable height messages (especially with images)
- Preserve scroll position in cache
