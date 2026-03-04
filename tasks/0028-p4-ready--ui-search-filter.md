---
created: 2026-01-31
priority: p4
status: ready
---

# Search and Filter Conversations

## Summary

Add ability to search and filter the conversation list.

## Context

As users accumulate conversations, finding a specific one becomes difficult. The current list only shows recent conversations ordered by update time (REQ-API-001).

## Acceptance Criteria

- [ ] Search box in conversation list view
- [ ] Filter by slug (partial match)
- [ ] Filter by working directory
- [ ] Client-side filtering for quick feedback
- [ ] Consider server-side search for content within messages (future)
- [ ] Clear/reset filter button

## Notes

- Start with client-side filtering of already-loaded conversations
- Backend would need new endpoint for full-text search of message content
- Could add filter chips: "Today", "This week", by directory
- Mobile: collapsible search bar to save space
