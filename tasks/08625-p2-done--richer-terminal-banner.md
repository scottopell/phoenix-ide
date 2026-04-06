---
created: 2026-04-03
priority: p2
status: done
artifact: ui/src/pages/ConversationPage.tsx
---

# Richer terminal banner

## Summary

After merge or abandon, the terminal banner shows only "Start new conversation"
with zero context. The user doesn't know from the banner whether this was a
successful merge or an abandon, what branch was involved, or what commit was
created.

## What to change

The system message above the banner already has this info ("Task completed.
Squash merged to main as abc123." or "Task abandoned."). Extract the terminal
reason from the last system message and show it in the banner:

- Merged: "Merged to main as abc123. [Start new conversation]"
- Abandoned: "Task abandoned. [Start new conversation]"
- Other terminal: "Conversation ended. [Start new conversation]"

## Done when

- [ ] Terminal banner shows context-appropriate text
- [ ] Merge shows branch/commit info
- [ ] Abandon shows that it was abandoned
- [ ] "Start new conversation" button still works
