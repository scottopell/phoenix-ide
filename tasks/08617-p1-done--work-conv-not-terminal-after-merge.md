---
created: 2026-04-03
priority: p1
status: done
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

## Implementation Guide

**Backend (`src/api/handlers.rs` -- `confirm_complete`):**
Mirror what `abandon_task` does after its git operations (around line 1800+):
- Set state to `ConvState::Terminal` via `db.update_conversation_state`
- Set mode to `ConvMode::Explore` via `db.update_conversation_mode`
- Broadcast SSE: `SseEvent::StateChange` with Terminal
- Broadcast SSE: `SseEvent::ConversationUpdate` with `conv_mode_label: Some("Explore")`
Currently `confirm_complete` returns success but never transitions state.

**Frontend (`ui/src/pages/ConversationPage.tsx`):**
The InputArea is hidden when `convStateForChildren.type === 'context_exhausted'`
or `=== 'awaiting_task_approval'` (line 566). Add `'terminal'` to that condition.
When terminal, show a banner with "Start new conversation" button that navigates
to `/new` (or opens NewConversationSheet) with the project's cwd pre-filled.

**State types:**
Check `ui/src/utils.ts` for `parseConversationState` -- the 'terminal' type may
already be defined there from the abandon flow. If so, no new types needed.

## Done When

- [ ] `confirm_complete` transitions conversation to `ConvState::Terminal`
- [ ] `conv_mode` set to `ConvMode::Explore` (same as abandon)
- [ ] SSE events broadcast so frontend updates in real-time
- [ ] InputArea hidden for Terminal conversations
- [ ] "Start new conversation" button shown in Terminal state
