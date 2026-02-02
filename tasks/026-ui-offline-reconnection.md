---
created: 2026-01-31
priority: p2
status: ready
---

# Offline and Reconnection Handling

## Summary

Implement robust offline handling with clear UI feedback, message persistence, and graceful recovery for users on unreliable networks (e.g., subway commute).

## Related Requirements

- REQ-UI-003: Message Composition (draft persistence)
- REQ-UI-004: Message Delivery States
- REQ-UI-005: Connection Status
- REQ-UI-006: Reconnection Data Integrity
- REQ-UI-011: Local Storage Schema
- REQ-API-005: SSE with `after` param for reconnection

## Acceptance Criteria

### Draft Persistence (REQ-UI-003)
- [ ] Draft text saved to localStorage on every keystroke (debounced)
- [ ] Draft restored on page load / navigation to conversation
- [ ] Draft cleared when message is sent
- [ ] Key format: `phoenix:draft:{conversationId}`

### Message Delivery States (REQ-UI-004)
- [ ] Optimistic UI: message appears immediately with ⏳ "sending" indicator
- [ ] ✓ "Sent" indicator shown when API returns `{queued: true}`
- [ ] ⚠️ "Failed" state shown on network error with tap-to-retry
- [ ] Messages queued in localStorage (same "sending" state whether online or offline)
- [ ] Queued messages auto-sent when connection restored
- [ ] Key format: `phoenix:queue:{conversationId}`

Three states only: **sending** (⏳) → **sent** (✓) or **failed** (⚠️)

### Connection Status UI (REQ-UI-005)
- [ ] Distinct "reconnecting" state with attempt count
- [ ] Exponential backoff: 1s, 2s, 4s, 8s, 16s, max 30s
- [ ] "Offline" banner after 3+ failed reconnect attempts
- [ ] Countdown timer to next retry in offline banner
- [ ] `navigator.onLine` integration for immediate offline detection
- [ ] Brief "reconnected" confirmation on recovery

### Data Integrity (REQ-UI-006)
- [ ] Track `last_sequence_id` from init and message events
- [ ] Store `last_sequence_id` in localStorage for page refresh recovery
- [ ] Reconnect with `?after={lastSequenceId}` parameter
- [ ] Deduplicate messages by `sequence_id` as safety net
- [ ] Key format: `phoenix:lastSeq:{conversationId}`

## Implementation Notes

See `specs/ui/design.md` for:
- Message delivery state machine
- Connection state machine with backoff logic
- localStorage schema
- MessageQueue class design
- useConnection and usePendingMessages hooks

## Test Scenarios

1. **Brief disconnect**: Lose connection for 5s, verify auto-reconnect, no duplicates
2. **Extended offline**: Lose connection, type messages, verify they queue and send on recovery
3. **Page refresh while offline**: Verify draft and pending messages survive refresh
4. **Rapid reconnect/disconnect**: Verify no duplicate messages, state machine stability
5. **Send failure**: Simulate 500 error, verify retry affordance works

---

## Agent Implementation Prompt

You are implementing offline and reconnection handling for the Phoenix web UI. This is critical UX for users on unreliable networks (subway commutes, spotty wifi).

### Core Principle

**Users must never lose work.** Typed text, unsent messages, and conversation state must survive network failures and page refreshes. When in doubt, persist to localStorage.

### Design Philosophy

1. **Optimistic UI**: Messages appear instantly when sent. Don't wait for server confirmation to show them.

2. **Invisible persistence**: Draft saving and message queuing should be invisible to users. They just type and send—the system handles the complexity.

3. **Simple mental model**: Three message states only (sending → sent/failed). Users don't need to understand the queue.

4. **Graceful degradation**: Offline is a normal state, not an error. The UI adapts smoothly.

5. **No duplicates ever**: The `sequence_id` mechanism exists specifically to prevent duplicates on reconnection. Use it.

### Key Technical Decisions

- **localStorage for persistence**: Draft text, message queue, and last sequence ID all persist. See REQ-UI-011 for schema.

- **Exponential backoff with ceiling**: 1s → 2s → 4s → ... → 30s max. Never stop retrying, but don't spam.

- **`?after=` parameter**: SSE reconnection uses this to resume from last known message. Critical for avoiding duplicates.

- **`navigator.onLine`**: Use it for immediate offline detection, but don't trust it completely—SSE errors are the source of truth.

### What to Read

1. `specs/ui/requirements.md` - REQ-UI-003 through REQ-UI-006, REQ-UI-011
2. `specs/ui/design.md` - State machines, MessageQueue class, localStorage schema
3. Current implementation in `ui/src/pages/ConversationPage.tsx` and `ui/src/components/`

### What to Build

New hooks:
- `useMessageQueue` - Manages sending/sent/failed states with localStorage persistence
- `useConnection` - Manages SSE lifecycle with backoff and sequence tracking
- `useDraft` - Manages draft persistence (simple, debounced localStorage)

Enhanced components:
- `InputArea` - Integrate draft persistence and message queue
- `StateBar` - Show reconnection attempts and offline banner
- `MessageList` - Render queued messages with correct states

### Success Criteria

A user on a subway can:
1. Type a message, go into a tunnel, and not lose their draft
2. Send messages while offline and have them delivered when back online
3. See clear feedback about connection state without anxiety
4. Never see duplicate messages after reconnection
5. Refresh the page and find everything intact
