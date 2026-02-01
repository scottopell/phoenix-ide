---
created: 2026-01-31
priority: p3
status: ready
---

# Conversation Management UI

## Summary

Add UI for archiving, deleting, and renaming conversations.

## Context

The backend supports conversation lifecycle operations (REQ-API-006, REQ-API-007):
- Archive/unarchive
- Delete
- Rename (change slug)

Currently the UI has no way to access these features.

## Acceptance Criteria

- [ ] Archive button/action on conversation (moves to archived list)
- [ ] View archived conversations (separate list or toggle)
- [ ] Unarchive action on archived conversations
- [ ] Delete with confirmation dialog
- [ ] Rename/edit slug with validation (slug must be unique)
- [ ] Mobile: swipe actions or long-press menu
- [ ] Desktop: context menu or hover actions

## Notes

- APIs exist: `POST /api/conversations/:id/archive`, `/unarchive`, `/delete`, `/rename`
- Archived list: `GET /api/conversations/archived`
- Rename endpoint expects `{name: "new-slug"}` body
- Consider undo for archive (brief toast with undo button)
