---
created: 2026-04-03
priority: p1
status: done
artifact: ui/src/components/StateBar.tsx
---

# Mode indicator in StateBar

## Summary

During a conversation, the StateBar shows model + cwd + branch but not the
conversation mode. The user has to go back to the conversation list and squint
at a 9px badge to confirm they're in Explore vs Work vs Standalone.

## What to change

Add a mode label to StateBar, between the back-arrow and the model name:

- Explore: "Explore" with a muted "read-only" suffix
- Work: "Work" with the task branch name
- Standalone/Direct: "Direct"

Use the existing badge styling from the conversation list but larger and always
visible (not hover-only).

## Done when

- [ ] StateBar shows mode label for all three modes
- [ ] Label is visible without hovering
- [ ] Work mode shows branch name inline (already partially there)
- [ ] Color-coded or icon-differentiated by mode
