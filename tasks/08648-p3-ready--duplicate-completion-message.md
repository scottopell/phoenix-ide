---
created: 2026-04-07
priority: p3
status: ready
artifact: ui/src/components/MessageList.tsx
---

# Duplicate "Task completed" message after Work mode merge

## Problem

After completing a Work mode conversation (squash merge), the completion
message appears twice:
1. As a system message bubble in the chat: "Task completed. Squash merged
   to main as 0dd52cd."
2. As the terminal state banner below the messages with "Start new
   conversation" button: same text repeated

The backend sends one system message AND the frontend renders a terminal
state banner that echoes the same content.

## Fix

Either:
- Don't render the terminal banner text if the last system message already
  contains the completion info (deduplicate at render time)
- Or suppress the system message from the backend when the terminal banner
  will show it (deduplicate at source)

The simpler option is frontend: if the terminal banner is showing, skip
rendering the last system message if its text matches the banner content.

## Done when

- [ ] Completion message appears exactly once after merge
- [ ] Same fix for abandon (check if it has the same duplication)
