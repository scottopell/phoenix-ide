---
created: 2026-04-03
priority: p3
status: ready
artifact: ui/src/pages/ConversationPage.tsx
---

# Onboarding tooltip for first Explore conversation

## Summary

There's no explanation of the mode system anywhere. The FirstTaskWelcome modal
only fires after task approval and only explains the tasks/ directory. A user's
first Explore conversation gives zero context about what Explore means, why
it's read-only, or how to get to Work mode.

## What to change

On the first Explore conversation (check localStorage flag), show a dismissible
inline banner above the input area:

"This is an Explore conversation -- the agent can read and analyze the codebase
but won't make changes. When you're ready to modify code, describe what you want
and the agent will propose a plan for your review."

Dismiss on click or after first message sent. Store dismissal in localStorage
so it only shows once.

## Done when

- [ ] First Explore conversation shows the banner
- [ ] Banner dismisses on click or after first send
- [ ] Banner does not appear again after dismissal
- [ ] Banner does not appear in Work or Standalone conversations
