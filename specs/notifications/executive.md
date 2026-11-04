# Notifications — Executive Summary

## Scope and Boundary

This spec governs **how Phoenix tells the user something happened** — through two channels, picked based on whether the user is currently looking:

- **In-app toasts** (`Toast.tsx`) — short ephemeral messages rendered in a corner of the UI. Used for action confirmations ("Response sent"), async results (MCP server toggled), and visible errors. Only useful when the user is looking at Phoenix.
- **Browser desktop notifications** — OS-level notifications via the Notification API. Used when Phoenix is not the focused tab and something needs the user's attention.

**In scope here (user-facing experiences):**
- Quiet confirmations after user actions (toasts) — implemented today
- Visible failure messages from background operations (toasts) — implemented today (red error styling in `ConversationListPage`, `McpStatusPanel`, `TerminalPanel` assist-setup; see REQ-NOTIF-002 status)
- Pull-me-back notifications when the agent needs me (browser notifications)
- Catch-up notifications after a missed event (browser notifications via SSE reconnect)
- Per-event configurability + global toggle
- Click-to-navigate and conversation-open acknowledgement for desktop notifications

**Owned by other specs:**
- `specs/conversation-ui/` — the parent layout that hosts the toast container and any settings panel
- `specs/sse_wire/` — the SSE event stream that drives browser-notification triggers (`state_change`, `agent_done`, etc.)
- `specs/bedrock/` — the conversation phase transitions (`awaiting_task_approval`, `awaiting_user_response`, `error`, `idle` after busy) that determine notification-worthiness

## Why It Exists

A user who delegates work to the agent will not stare at the tab waiting for it to finish. Without notifications they'll either (a) keep checking — wasting attention — or (b) walk away and come back hours later — wasting wall-clock time. The toast channel keeps the user grounded in the in-app flow ("what I just did succeeded / failed"); the browser-notification channel reaches the user when they've context-switched away.

These are deliberately two channels rather than one: the toast cannot help an unfocused tab, and a desktop notification for "you just clicked Submit and it worked" would be excessive. Routing is by trigger, not by user preference (see REQ-NOTIF-005).

## Status Summary

Browser desktop notifications are implemented with the same completed status as the in-app toast channel.

| Requirement | Status | Notes |
|---|---|---|
| **REQ-NOTIF-001:** Confirm My Action Quietly | ✅ Complete | `ui/src/components/Toast.tsx` (4 types, auto-dismiss, click-to-dismiss); `ui/src/hooks/useToast.tsx` (showSuccess / showInfo / showWarning / showError); 4s default duration. Confirmation call sites: `McpStatusPanel.tsx` (MCP reload + toggle outcomes), `QuestionPanel.tsx` ("Response sent" / dismissal confirmation), `ConversationListPage.tsx` (storage usage warning) |
| **REQ-NOTIF-002:** Tell Me When a Background Action Failed | ✅ Complete | Red `'error'` toast styling at `ConversationListPage.tsx` (storage quota exceeded, chain operation failures); `McpStatusPanel.tsx` routes "MCP reload failed" / "Failed to enable/disable" via the dedicated `showError` prop wired from `DesktopLayout.tsx` → `FileExplorerPanel.tsx`; `TerminalPanel.tsx` assist-setup failures also route via `showError` |
| **REQ-NOTIF-003:** Pull Me Back When the Agent Needs Me | ✅ Complete | Browser desktop notifications fire for live transitions to `awaiting_task_approval`, `awaiting_user_response`, `error`, and `context_exhausted` when permission/settings/focus gates allow. Permission prompts are only initiated from the settings panel; SSE/list-driven triggers cue the user to enable notifications later. |
| **REQ-NOTIF-004:** Pull Me Back When a Long-Running Task Finishes | ✅ Complete | Browser notification on busy → `idle` after a 30s threshold (`AGENT_FINISHED_THRESHOLD_MS`) for non-trivial agent work. The threshold filters quick turns and is controlled by the `notify_idle` setting. |
| **REQ-NOTIF-005:** Don't Notify When I'm Already Looking | ✅ Complete | Delivery gate suppresses only when `document.visibilityState === 'visible'`, `document.hasFocus()`, and the triggering conversation is the active `/c/:slug` route. Focused-tab/different-conversation events still notify. |
| **REQ-NOTIF-006:** Let Me Tune Which Events Notify Me | ✅ Complete | Sidebar settings panel exposes master enable, per-event toggles, browser permission state, request-permission button, and denied/unsupported guidance. |
| **REQ-NOTIF-007:** One Click Back to the Right Conversation | ✅ Complete | Notification click focuses the tab, acknowledges the triggering conversation's live notifications, and dispatches navigation to `/c/<slug>` through the mounted React Router tree. Opening the conversation through another navigation path also acknowledges and closes live notifications Phoenix still owns for that conversation. |
| **REQ-NOTIF-008:** Catch Me Up When I Reconnect After a Disconnect | ✅ Complete | Catch-up is implemented against the current conversation-list architecture: successful list refreshes populate `ConversationStore`, and `DesktopLayout` scans active top-level conversations for currently blocking states. |
| **REQ-NOTIF-009:** Settings That Survive Browser Clears + Server Restarts | ✅ Complete | Preferences are stored server-side in the durable `notification_settings` SQLite table via `GET/PUT /api/settings/notifications`; the frontend does not use localStorage for notification preferences. |
| **REQ-NOTIF-010:** Connected-page reminder alerts | Implemented; policy/feed tested | Dedicated reminder identity, quiet initial baseline, current target routing, server-persisted toggle and focus/permission gates; full frontend tests and reminder API/browser journey pass. Actual OS permission delivery was not exercised. |

**Progress:** Existing nine notification requirements are complete. REQ-NOTIF-010 reminder policy/feed tests and the reminder API/browser journey pass; actual OS permission delivery remains unqualified. Browser desktop notifications use server-persisted preferences, focus gating, live transitions, list-refresh catch-up, click navigation, and conversation-open acknowledgement of live notifications owned by the current page.

## Behavioural Specification

`specs/notifications/notifications.allium` models the configuration entity, conversation-state browser notifications and connected-page reminder alerts, the master+per-event-toggle gating, the tab-focus gate, the SSE-reconnect catch-up rule, notification click navigation, and conversation-open acknowledgement. In-app toasts are not modelled formally — they're stateless render-and-dismiss UI with no transition graph worth capturing.

## Cross-Spec Cross-References

- `specs/sse_wire/`: every browser-notification trigger reads from the same `state_change` events the conversation atom consumes. No new SSE event types are needed.
- `specs/bedrock/`: the phase transitions that map to notification events (`awaiting_task_approval`, `awaiting_user_response`, `error`, `context_exhausted`, busy → idle) are owned there.
- `specs/conversation-ui/`: the toast container is mounted in `DesktopLayout` and on the conversation list page; notification settings share that chrome.
- `specs/terminal-panel/` (REQ-TPANEL-006): the partial note for "Let Phoenix set this up" called out that assist-setup failures only hit `console.error`. Closed by routing through `useToast.showError` (`TerminalPanel.tsx`); REQ-NOTIF-002 is the canonical visible-error mechanism.


## Reminder notification integration

REQ-NOTIF-010 uses the global DesktopLayout reminder feed and a dedicated `reminder_due` event through the existing permission/settings/focus policy. Initial recent history establishes a quiet baseline; new occurrences dedupe by saved reminder ID separately from blocking attention. The server KV notification preferences add `notify_reminder`, enabled for an absent key and persisted through ordinary settings updates. Feed targets carry actual ordinary/Coordinator navigation identity, with no fabricated Conversation model. The one-day feed window follows observed occurrence time; durable cards remain queryable beyond that window and after input/model failure. Focused policy, routing, late-observation refresh and persistence tests are included; integrated browser acceptance is pending.
