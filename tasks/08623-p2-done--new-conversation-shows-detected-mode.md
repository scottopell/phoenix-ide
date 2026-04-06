---
created: 2026-04-03
priority: p2
status: done
artifact: ui/src/pages/NewConversationPage.tsx
---

# New conversation form shows detected mode

## Summary

When creating a conversation, the user picks a directory and model. The backend
silently determines the mode from git detection. The form gives no indication
of what mode will be created or what that means.

## What to change

After directory selection, show a one-line subtitle below the directory chip:

- Git repo detected: "Git project -- starts in Explore mode (read-only). The
  agent will propose a plan before making changes."
- No git: "Direct mode -- full tool access."

The subtitle should appear/update as the directory changes. Use the existing
`/api/validate-cwd` endpoint which already returns git detection info.

## Done when

- [ ] Mode preview shown after directory selection
- [ ] Text explains what the mode means (not just the mode name)
- [ ] Updates dynamically when directory changes
- [ ] Doesn't block or slow conversation creation
