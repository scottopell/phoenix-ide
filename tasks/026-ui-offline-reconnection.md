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
- [ ] Optimistic UI: message appears immediately with "sending" indicator
- [ ] "Sent" indicator (✓) shown when API returns `{queued: true}`
- [ ] "Failed" state shown on network error with tap-to-retry
- [ ] Pending messages queued in localStorage when offline
- [ ] Pending messages auto-sent when connection restored
- [ ] Key format: `phoenix:pending:{conversationId}`

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
