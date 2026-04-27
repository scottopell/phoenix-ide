---
created: 2026-04-23
priority: p1
status: done
artifact: ui/src/pages/ConversationPage.tsx
---

User messages vanish from the UI between POST success and the server SSE echo — `markSent` removes the message from the queue the moment POST returns 200, and the optimistic `sse_message` dispatch with `sequence_id: -1` is silently dropped by the monotonic guard in `atom.ts:227` (`atom.lastSequenceId >= action.sequenceId`). Between those two events the message exists in no rendered state, so the UI goes purple (state_change dispatch works fine) without showing the message. Refreshing makes it appear because `sse_init` uses the full message list path. Fix: keep the queue entry with status `sending` until a message with the same `localId` appears in `atom.messages`, then mark sent.
