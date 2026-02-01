---
created: 2026-01-31
priority: p2
status: ready
---

# Offline and Reconnection Handling

## Summary

Improve handling of network disconnections with clear UI feedback and graceful recovery.

## Context

The UI has basic SSE reconnection (REQ-API-005) but no user feedback. Users don't know if they're disconnected or if messages will be lost.

## Acceptance Criteria

- [ ] Banner/toast shown when SSE connection is lost
- [ ] "Reconnecting..." state with retry indicator
- [ ] Automatic reconnection with exponential backoff
- [ ] Success feedback when reconnected
- [ ] Input disabled or queued while disconnected
- [ ] No duplicate messages after reconnection (use `after` param)

## Notes

- Current reconnect logic in `static/app.js` `handleSseEvent('disconnected')`
- SSE supports `after` query param to resume from last sequence_id
- Consider offline detection via `navigator.onLine` + SSE state
- May want to queue typed messages and send on reconnect
