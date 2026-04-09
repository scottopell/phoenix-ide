# Notifications -- Requirements

## User Stories

### Story 1: Don't Miss Agent Requests

As a developer who starts an agent task and switches to another window
or tab, I need to be pulled back to Phoenix when the agent needs my
input (task approval, question, error) so I don't leave the agent
blocked for minutes while I'm unaware.

### Story 2: Know When Work Completes

As a developer who kicked off a long-running agent task, I want to
know when it finishes so I can review the results and continue working
without polling the tab.

### Story 3: Tune Notification Noise

As a developer who uses Phoenix throughout the day, I need control
over which events trigger notifications so I can dial down the noise
without turning off notifications entirely.

---

### REQ-NOTIF-001: Browser Desktop Notifications

WHEN a notification-worthy event occurs on any conversation
AND the user has granted browser notification permission
AND the Phoenix tab is not focused
THE SYSTEM SHALL display a browser desktop notification with a title
identifying the event type and a body identifying the conversation

WHEN the Phoenix tab is focused
THE SYSTEM SHALL NOT display a desktop notification (the user is already looking)

**Rationale:** Desktop notifications are the only mechanism that reaches
the user when Phoenix is backgrounded. In-app indicators are invisible
when the tab is not active.

---

### REQ-NOTIF-002: Notification Permission Request

WHEN notifications are enabled in settings but browser permission has
not been granted
THE SYSTEM SHALL prompt the user to grant notification permission

WHEN the user denies permission
THE SYSTEM SHALL display a message explaining that notifications
require browser permission and offer a link to browser settings

**Rationale:** Browser notification permission is a one-time grant. The
system cannot send notifications without it, so the request must be
surfaced clearly.

---

### REQ-NOTIF-003: Configurable Event Types

THE SYSTEM SHALL support enabling or disabling notifications for each
of these event types independently:

1. **Task approval needed** -- conversation entered `awaiting_task_approval`
2. **Question asked** -- conversation entered `awaiting_user_response`
3. **Agent error** -- conversation entered `error` or `context_exhausted`
4. **Agent finished** -- conversation returned to `idle` after being busy

THE SYSTEM SHALL store these preferences server-side so they survive
browser clears and server restarts

**Rationale:** Different events have different urgency. Task approval and
questions block the agent; errors need investigation; idle is informational.
Users should control the noise level.

---

### REQ-NOTIF-004: Global Scope with Per-Event Toggles

THE SYSTEM SHALL apply notification preferences globally across all
conversations

THE SYSTEM SHALL NOT support per-conversation notification overrides
in the initial implementation

**Rationale:** A single set of global toggles covers the primary use case
without adding per-conversation UI complexity. Per-conversation overrides
can be added later if needed.

---

### REQ-NOTIF-005: Click-to-Navigate

WHEN the user clicks a desktop notification
THE SYSTEM SHALL focus the Phoenix browser tab or window
AND navigate to the conversation that triggered the notification

**Rationale:** The notification exists to pull the user back to a specific
conversation. One click should get them there.

---

### REQ-NOTIF-006: Notification Settings UI

THE SYSTEM SHALL provide a settings panel accessible from the sidebar
or StateBar where the user can:
- Enable or disable notifications globally (master toggle)
- Toggle each event type independently
- See current browser permission status
- Request browser permission if not yet granted

**Rationale:** Settings should be discoverable and editable without
leaving the app. The permission status display prevents confusion
when notifications don't appear despite being "enabled."

---

### REQ-NOTIF-007: SSE-Driven Notification Triggers

THE SYSTEM SHALL detect notification-worthy events from the existing
SSE stream (state_change events) rather than polling or adding new
API endpoints

WHEN a state_change SSE event arrives matching an enabled event type
THE SYSTEM SHALL trigger the notification pipeline

**Rationale:** The SSE stream already carries all state transitions in
real-time. Notifications are a frontend concern driven by data already
available. No backend changes needed for the trigger mechanism.
