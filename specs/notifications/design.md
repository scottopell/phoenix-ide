# Notifications -- Design

## Design Goals

Notifications are a frontend-heavy feature. The SSE stream already
delivers all state transitions; the notification system listens to
that stream and decides when to fire a browser notification. The only
backend component is settings persistence.

### Event Detection (REQ-NOTIF-001, REQ-NOTIF-007)

The frontend SSE handler already processes `state_change` events.
The notification system hooks into this same handler and checks:
1. Is the new state a notification-worthy event?
2. Is that event type enabled in settings?
3. Is the tab currently focused? (if focused, skip)
4. Fire `new Notification(...)` with title, body, and click handler.

State-to-event mapping:
- `awaiting_task_approval` -> "Task approval needed"
- `awaiting_user_response` -> "Question asked"
- `error` | `context_exhausted` -> "Agent error"
- `idle` (when previous state was busy) -> "Agent finished"

The "previous state was busy" check prevents spurious "finished"
notifications on page load when conversations are already idle.

### Settings Storage (REQ-NOTIF-003, REQ-NOTIF-004)

Server-side settings table:
```
notification_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
)
```

Simple key-value pairs:
- `notifications_enabled`: "true" | "false"
- `notify_task_approval`: "true" | "false"
- `notify_question`: "true" | "false"
- `notify_error`: "true" | "false"
- `notify_idle`: "true" | "false"

API: `GET /api/settings/notifications`, `PUT /api/settings/notifications`

### Browser Permission (REQ-NOTIF-002)

The `Notification.permission` API has three states: "default" (not asked),
"granted", "denied". The settings UI shows the current state and offers
a button to request permission when "default". When "denied", it shows
guidance to change it in browser settings (cannot re-prompt programmatically).

### Behavioral Specification

The complete behavioral contract is defined in
`specs/notifications/notifications.allium`.
