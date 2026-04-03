---
created: 2026-04-03
priority: p1
status: ready
artifact: src/api/handlers.rs
---

# Work conversation not terminal after merge

## Summary

After completing a Work task (squash merge), the conversation shows the correct
system message ("Task completed. Squash merged to main as ...") but the user
can still send messages and continue working. The conversation should transition
to Terminal state, same as after abandon.

Additionally, once terminal, offer a button to start a new conversation off the
latest main branch.

## Reproduction

1. Approve a task (enters Work mode)
2. Complete the work, click "Merge to main"
3. Confirm the merge
4. System message appears, but InputArea is still active
5. User can type and send follow-up messages (should not be possible)

## Done When

- [ ] `confirm_complete` transitions conversation to `ConvState::Terminal`
- [ ] `conv_mode` set to `ConvMode::Explore` (same as abandon)
- [ ] InputArea hidden for Terminal conversations
- [ ] "Start new conversation" button shown in Terminal state
