# Notifications — Executive Summary

## Scope and Boundary

This spec governs **how Phoenix tells the user something happened** — through two channels, picked based on whether the user is currently looking:

- **In-app toasts** (`Toast.tsx`) — short ephemeral messages rendered in a corner of the UI. Used for action confirmations ("Response sent"), async results (MCP server toggled), and visible errors. Only useful when the user is looking at Phoenix.
- **Browser desktop notifications** — OS-level notifications via the Notification API. Used when Phoenix is not the focused tab and something needs the user's attention. Not yet implemented.

**In scope here (user-facing experiences):**
- Quiet confirmations after user actions (toasts) — implemented today
- Visible failure messages from background operations (toasts) — implemented today
- Pull-me-back notifications when the agent needs me (browser notifications) — spec target
- Catch-up notifications after a missed event (browser notifications via SSE reconnect) — spec target
- Per-event configurability + global toggle — spec target
- Click-to-navigate from a desktop notification — spec target

**Owned by other specs:**
- `specs/conversation-ui/` — the parent layout that hosts the toast container and any settings panel
- `specs/sse_wire/` — the SSE event stream that drives browser-notification triggers (`state_change`, `agent_done`, etc.)
- `specs/bedrock/` — the conversation phase transitions (`awaiting_task_approval`, `awaiting_user_response`, `error`, `idle` after busy) that determine notification-worthiness

## Why It Exists

A user who delegates work to the agent will not stare at the tab waiting for it to finish. Without notifications they'll either (a) keep checking — wasting attention — or (b) walk away and come back hours later — wasting wall-clock time. The toast channel keeps the user grounded in the in-app flow ("what I just did succeeded / failed"); the browser-notification channel reaches the user when they've context-switched away.

These are deliberately two channels rather than one: the toast cannot help an unfocused tab, and a desktop notification for "you just clicked Submit and it worked" would be excessive. Routing is by trigger, not by user preference (see REQ-NOTIF-005).

## Status Summary

Phased delivery: in-app toasts shipped first; browser desktop notifications are the next phase.

| Requirement | Status | Notes |
|---|---|---|
| **REQ-NOTIF-001:** Confirm My Action Quietly | ✅ Complete | `ui/src/components/Toast.tsx` (4 types, auto-dismiss, click-to-dismiss); `ui/src/hooks/useToast.tsx` (showSuccess / showInfo / showWarning / showError); 4s default duration. Confirmation call sites: `McpStatusPanel.tsx:77,107` (MCP reload + toggle outcomes), `QuestionPanel.tsx:242,273` ("Response sent" / "Declined to answer"), `ConversationListPage.tsx:98` (storage usage warning) |
| **REQ-NOTIF-002:** Tell Me When a Background Action Failed | 🚧 Partial | Red `'error'` toast styling is used at `ConversationListPage.tsx:102,274,295,305,317` (storage quota exceeded, chain operation failures). Gap: `McpStatusPanel.tsx:81,109` shows error messages via the `showToast` prop, which `DesktopLayout.tsx:36,92` wires from `showSuccess` — so "MCP reload failed" / "Failed to enable/disable" render green even though their content describes a failure. Spec target: route those failures through `showError` so styling matches semantics |
| **REQ-NOTIF-003:** Pull Me Back When the Agent Needs Me | ❌ Not Started | Spec target: browser desktop notification on transitions to `awaiting_task_approval`, `awaiting_user_response`, `error`, `context_exhausted` when the Phoenix tab is not focused |
| **REQ-NOTIF-004:** Pull Me Back When a Long-Running Task Finishes | ❌ Not Started | Spec target: browser notification on `idle` after the conversation was busy long enough to be worth flagging (threshold TBD; the spec doesn't fix it) |
| **REQ-NOTIF-005:** Don't Notify When I'm Already Looking | ❌ Not Started | Spec target: tab-focus gate. When `document.visibilityState === 'visible'` and the conversation is the active route, suppress browser notifications. Toasts always render (REQ-NOTIF-001/002) regardless of focus |
| **REQ-NOTIF-006:** Let Me Tune Which Events Notify Me | ❌ Not Started | Spec target: per-event toggles + master toggle; surfaced in a settings panel reachable from sidebar/StateBar |
| **REQ-NOTIF-007:** One Click Back to the Right Conversation | ❌ Not Started | Spec target: clicking a desktop notification focuses the Phoenix tab and navigates to the triggering conversation |
| **REQ-NOTIF-008:** Catch Me Up When I Reconnect After a Disconnect | ❌ Not Started | Spec target: on SSE reconnect, scan the conversation list for any non-sub-agent in a notification-worthy state and emit a notification per match. Captures notifications the user "missed" while disconnected |
| **REQ-NOTIF-009:** Settings That Survive Browser Clears + Server Restarts | ❌ Not Started | Spec target: server-side notification_settings table; localStorage is too fragile for cross-device preferences |

**Progress:** 1 of 9 complete, 1 partial, 7 not started. Phase 1 (in-app toasts) is mostly shipped — the toast component, lifecycle, and confirmation paths are live; the remaining Phase 1 gap is routing McpStatusPanel's failure messages through `showError` rather than the green `showToast` prop so error styling matches content. Phase 2 (browser desktop notifications) is the next implementation block, with REQ-NOTIF-003 → 005 → 006 → 007 → 008 → 009 as the natural build order.

## Behavioural Specification

`specs/notifications/notifications.allium` models the configuration entity, the four browser-notification event types, the master+per-event-toggle gating, the tab-focus gate, and the SSE-reconnect catch-up rule. The Allium predates this restructuring; the current rules describe the browser-notification path. In-app toasts are not modelled formally — they're stateless render-and-dismiss UI with no transition graph worth capturing.

## Cross-Spec Cross-References

- `specs/sse_wire/`: every browser-notification trigger reads from the same `state_change` events the conversation atom consumes. No new SSE event types are needed.
- `specs/bedrock/`: the phase transitions that map to notification events (`awaiting_task_approval`, `awaiting_user_response`, `error`, `context_exhausted`, busy → idle) are owned there.
- `specs/conversation-ui/`: the toast container is mounted in the desktop layout (`DesktopLayout.tsx:111`) and on the conversation list page; the settings panel for notifications would live in the same chrome.
- `specs/terminal-panel/` (REQ-TPANEL-009): documents an open assist-setup error path that surfaces via `console.error` today; once REQ-NOTIF-002 is the canonical visible-error mechanism, terminal-panel's gap closes by routing through `useToast.showError`.
