# Notifications

## Scope

Phoenix has two notification channels:
1. **In-app toasts** — short messages rendered in the conversation UI; useful only when the user is looking
2. **Browser desktop notifications** — OS-level pings that reach the user even when the tab is not focused

This spec covers both, plus the rules for picking which channel applies to a given trigger. The toast UI is shipped (REQ-NOTIF-001 ✅); the error-styled half of the toast story is partial (REQ-NOTIF-002 🚧 — red `showError` is wired in `ConversationListPage`, but `McpStatusPanel` failures still render green, see executive's status table). The desktop-notification half is spec'd here as the target (REQ-NOTIF-003 onwards). See `specs/notifications/executive.md` for the full boundary.

## User Story

As a Phoenix user, I delegate work to the agent and rarely watch it actively. I need Phoenix to:

- Confirm my own actions worked (or failed), so I know whether to retry
- Tell me when the agent finishes a long task or hits something it needs me for
- Reach me even when I've context-switched away (other tab, other window, other app)
- Stay quiet when I'm already looking — no tab badges or popups that just say "you're here, FYI"
- Let me dial down the noise on the events I don't care about

## Transparency Contract

The notifications system must let me confidently answer:

1. Did my last action succeed or fail?
2. Did Phoenix run a background operation while I was looking? What was its result?
3. Is the agent currently waiting on me to do something?
4. Did I miss a notification while I was disconnected from Phoenix?
5. Why does (or doesn't) Phoenix interrupt me for event X? Can I change it?
6. If a notification fires for a different conversation, can I get there in one click?

Each numbered question maps to one or more requirements below.

---

## Requirements

### REQ-NOTIF-001: Confirm My Action Quietly

WHEN I take an action whose result is not immediately obvious from the page (e.g., "Send in Background", MCP server toggle, "Response sent" on a question)
THE SYSTEM SHALL display an in-app toast confirming the outcome

WHEN the toast is shown
THE SYSTEM SHALL auto-dismiss it after a brief interval (default ~4 seconds)
AND SHALL accept a click as an explicit dismiss

WHEN I take multiple actions in quick succession
THE SYSTEM SHALL stack toasts vertically rather than replacing them, so I can see what I confirmed

**Rationale:** In-app actions need closure but rarely deserve a modal interruption. The toast is the right disclosure shape: visible enough to register, transient enough to ignore. Stacking matters because the user often performs sequences ("toggle five MCP servers") and would otherwise see only the last result.

---

### REQ-NOTIF-002: Tell Me When a Background Action Failed

WHEN a user-initiated action fails (network error, server-side rejection, validation issue)
THE SYSTEM SHALL display an in-app toast with the failure reason, styled distinctly from success toasts (`error` type, red accent)

WHEN the failure happens because of an external service Phoenix doesn't control (e.g., MCP server not responding)
THE SYSTEM SHALL include enough context in the toast for the user to know which thing failed (server name, operation name)

**Rationale:** A silently-failing button is the worst outcome — the user doesn't know whether to wait, retry, or work around. The error toast is the floor: it doesn't have to explain how to fix the problem, just confirm that one happened. The `console.error`-only path that PR #56 documented (REQ-TPANEL-009) is the anti-pattern: invisible to the user, only visible to people inspecting devtools.

---

### REQ-NOTIF-003: Pull Me Back When the Agent Needs Me

WHEN a conversation transitions into a state that blocks the agent on me (`awaiting_task_approval`, `awaiting_user_response`, `error`, `context_exhausted`)
AND the Phoenix tab is not focused (REQ-NOTIF-005)
AND notifications for that event type are enabled (REQ-NOTIF-006)
AND I have granted browser notification permission
THE SYSTEM SHALL display a browser desktop notification with a title naming the event ("Question asked", "Task approval needed", "Agent error") and a body identifying the conversation slug

WHEN browser notification permission has not yet been granted
THE SYSTEM SHALL silently drop SSE-driven triggers — `Notification.requestPermission()` requires a user-gesture context that an SSE handler does not have, and a blocked / ignored prompt would be a worse experience than no prompt at all
AND SHALL surface an in-app cue (toast or banner) the next time I focus Phoenix, telling me how to enable desktop notifications via the settings panel

WHEN I click the settings-panel "Enable desktop notifications" button (a user gesture)
THE SYSTEM SHALL call `Notification.requestPermission()` and reflect the resulting `granted` / `denied` state immediately in the panel

**Rationale:** When Phoenix is in another tab and the agent is blocked on me, that's the canonical "interrupt me" moment. Without a desktop notification I'd come back hours later to find work parked. But: browsers gate `requestPermission()` to user-gesture contexts. Calling it from inside an SSE handler is unreliable — most browsers either block the prompt or quietly ignore the call. Routing all permission requests through an explicit settings-panel click makes the prompt land where the browser will allow it, and the in-app cue on next focus is the discoverability nudge for users who haven't visited the settings panel yet.

---

### REQ-NOTIF-004: Pull Me Back When a Long-Running Task Finishes

WHEN a conversation transitions from busy to `idle` (the agent finished a turn cleanly)
AND the conversation was busy long enough to be worth flagging
AND the Phoenix tab is not focused
AND the "agent finished" event type is enabled
THE SYSTEM SHALL display a browser desktop notification: "Agent finished" + conversation slug

**Rationale:** Long agent runs are the second canonical "interrupt me" moment — the agent isn't blocked but I asked for the work and want to know it's done. The "long enough" threshold is deliberately not pinned in this spec; the implementation picks a value (e.g., 30 seconds) that filters out routine quick turns. Lowering the threshold over time is reversible; users complaining about noise can disable the event type via REQ-NOTIF-006.

---

### REQ-NOTIF-005: Don't Notify When I'm Already Looking

WHEN the Phoenix tab is focused (`document.visibilityState === 'visible'` AND the document has focus) AND the triggering conversation is the active route
THE SYSTEM SHALL NOT fire a browser desktop notification for that event — I'm already looking at the thing it would point me to

WHEN the tab is focused but the triggering conversation is NOT the active route
THE SYSTEM SHALL fire a notification for the other conversation — being on Phoenix doesn't mean I'm watching every conversation

WHEN the tab is not focused
THE SYSTEM SHALL fire notifications per the normal gating (REQ-NOTIF-006 toggles + permission state)

THE SYSTEM SHALL continue to render in-app toasts (REQ-NOTIF-001/002) regardless of tab focus — toasts are the in-app channel and don't compete with desktop notifications

**Rationale:** Desktop notifications exist to reach me when in-app indicators can't. Firing one while I'm already on the specific conversation in question is noise. The "tab focused but different conversation" case matters: a multi-conversation user wants to know when conversation B needs them, even while looking at conversation A.

---

### REQ-NOTIF-006: Let Me Tune Which Events Notify Me

THE SYSTEM SHALL provide a settings panel reachable from the sidebar or StateBar where I can:

- Enable or disable browser notifications globally (master toggle)
- Toggle each event type independently (task approval, question, agent error, agent finished)
- See current browser notification permission status (granted / denied / not yet asked)
- Request browser permission when status is `default` (not yet asked) — the OS-level permission cannot be re-prompted programmatically once denied; the UI shows guidance to change it in browser settings instead

WHEN the master toggle is off
THE SYSTEM SHALL NOT fire any browser notifications regardless of per-event toggles

WHEN browser permission is denied
THE SYSTEM SHALL display guidance for re-enabling in browser settings, since the OS-level permission cannot be re-prompted programmatically

**Rationale:** Different event types have different urgency to different people. A user who wants to be pulled in for questions but not for completions needs that switch. The master toggle is the "leave me alone for now" panic button. Showing permission status separately from the toggles prevents the confusion of "I enabled it, why no notifications?" — the permission is upstream of the toggles.

---

### REQ-NOTIF-007: One Click Back to the Right Conversation

WHEN I click a desktop notification
THE SYSTEM SHALL focus the Phoenix browser tab/window
AND SHALL navigate to the conversation that triggered the notification (`/c/<slug>`)

**Rationale:** The notification exists to interrupt me _into_ a specific conversation. If the click landed me on the conversation list and I had to find the right one, the value drops in half. This is one-click recovery, not "Phoenix is open in some tab somewhere."

---

### REQ-NOTIF-008: Catch Me Up When I Reconnect After a Disconnect

WHEN the SSE connection re-establishes after a disconnect (network blip, laptop wake, server restart)
THE SYSTEM SHALL scan the conversation list for any non-sub-agent conversation in a notification-worthy state (`awaiting_task_approval`, `awaiting_user_response`, `error`, `context_exhausted`)
AND for each match SHALL emit a desktop notification per the same gating rules (REQ-NOTIF-005, REQ-NOTIF-006)

THE SYSTEM SHALL NOT emit catch-up notifications for `idle` (which is ambiguous — may have been idle for hours) or for sub-agent conversations (which the user doesn't manage directly)

**Rationale:** Live SSE events drive the normal notification flow. Without a catch-up pass, anything that transitioned during the disconnect window is invisible: I close my laptop, agent asks a question, I reopen and the question is silently waiting. The catch-up pass is conservative on idle because "agent went idle three hours ago while you were at lunch" is not actionable; only currently-blocking states warrant the interrupt.

---

### REQ-NOTIF-009: Settings That Survive Browser Clears + Server Restarts

THE SYSTEM SHALL persist notification preferences server-side (not in localStorage)

WHEN a user signs in from a new browser or clears local data
THE SYSTEM SHALL restore the same preferences

WHEN the Phoenix server restarts
THE SYSTEM SHALL restore preferences from durable storage on startup

**Rationale:** Notification preferences are a tuning decision the user makes once and expects to persist. Storing them in localStorage means the user re-tunes after every browser clear and the preferences don't follow them across devices. Server-side storage is the correct durability tier for "I configured this; remember it."
