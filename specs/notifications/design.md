# Notifications — Design

This document describes the technical architecture for both
notification channels, implementing
`specs/notifications/requirements.md`.

## Two Channels, One Routing Rule

```
                                    ┌─ user-initiated action ─┐
                                    │  ("Send in Background", │
                                    │  MCP toggle, "Response  │
                                    │  sent", etc.)           │
                                    └────────────┬────────────┘
                                                 │
                                                 ▼
                                       ┌─────────────────┐
                                       │  useToast hook  │
                                       │  show*() calls  │
                                       └────────┬────────┘
                                                │
                                                ▼ always renders
                                       ┌─────────────────┐
                                       │  <Toast />      │  in-app
                                       │  container      │  channel ✅
                                       └─────────────────┘

  ┌─ SSE state_change events ─┐
  │  awaiting_task_approval   │
  │  awaiting_user_response   │
  │  error / context_exhausted│
  │  busy → idle              │       (REQ-NOTIF-005 gate)
  └─────────────┬─────────────┘            tab focused
                │                              and on this convo?
                ▼                                  │
      ┌──────────────────┐                  ┌─────┴─────┐
      │ event classifier │                  │  YES → suppress
      │  + per-event     │─────routing──→   │  NO  → fire
      │  toggle check    │                  └─────┬─────┘
      └──────────────────┘                        │
                                                  ▼
                                         ┌─────────────────┐
                                         │ new Notification│  desktop
                                         │ (title, body,   │  channel ❌
                                         │  click handler) │  (spec target)
                                         └─────────────────┘
```

The two channels are deliberately disjoint by trigger source — toasts
for user-initiated actions, desktop notifications for state-machine
events that need attention. The routing rule is "what triggered this?"
not "what does the user prefer?" — a user with desktop notifications
disabled still gets toast confirmations on their own actions.

## Channel 1: In-App Toasts (REQ-NOTIF-001, REQ-NOTIF-002)

### Components

- `ui/src/components/Toast.tsx` — render layer. `Toast` (container,
  maps `messages` to `ToastItem`s) + `ToastItem` (single toast with
  auto-dismiss, click-to-dismiss, leave animation).
- `ui/src/hooks/useToast.tsx` — hook providing `toasts`,
  `showToast(type, message, duration?)`, `showInfo` / `showWarning` /
  `showError` / `showSuccess` convenience wrappers, and `dismissToast`.

### Toast lifecycle

```
showToast(type, message, duration=4000)
  → adds {id, type, message, duration} to setToasts
  → ToastItem effects schedule setTimeout(duration)
  → on timeout: setIsLeaving(true), setTimeout(200) → onDismiss(id)
  → onDismiss removes the entry from the toast array

(or)
  → user clicks toast
  → handleDismiss(): setIsLeaving(true), setTimeout(200) → onDismiss(id)
```

`duration: 0` (rather than the default 4000ms) keeps the toast
indefinite until clicked — useful for errors that the user must
acknowledge. The hook accepts an optional `duration` parameter on
every `show*` method.

### Where toasts originate

The hook is consumed at multiple layers; each conversation page or
panel that triggers a user-visible action holds a `useToast()`
instance and passes `showToast` down to children that need it.

Today's call sites (verified by grep `showInfo|showWarning|showError|showSuccess|showToast` in `ui/src/`):

- `McpStatusPanel.tsx:77,107` — MCP server reload + toggle outcomes (success-coloured)
- `McpStatusPanel.tsx:81,109` — MCP failures (currently routed through the `showToast` prop, which is wired from `showSuccess` — see Phase 1 gap noted in REQ-NOTIF-002 status)
- `QuestionPanel.tsx:242,273` — "Response sent" / "Declined to answer"
- `QuestionPanel.tsx:467,533` — "Press Enter again to submit" (Enter-key hint)
- `ConversationListPage.tsx:98` — `showWarning` for storage-usage threshold
- `ConversationListPage.tsx:102,274,295,305,317` — `showError` for storage quota exceeded + chain operation failures (the only red-styled toasts in the UI today)
- `DesktopLayout.tsx:36,92` — `useToast()` instance hosted at the layout level; passes `showSuccess` down as the `showToast` prop into `FileExplorerPanel` → `McpStatusPanel`
- `FileExplorerPanel.tsx` — pass-through wiring only; doesn't trigger toasts itself

The container is mounted once per page (`DesktopLayout.tsx:111` for
the desktop layout; `ConversationListPage` mounts its own).

### What toasts do NOT do

- They do not survive across page reloads (in-memory React state).
- They do not show for events the user didn't trigger themselves
  (those are the desktop-notification channel's job).
- They do not stack indefinitely — a flood of error toasts is its
  own UX problem, but the spec doesn't cap them. The implementation
  could add a max-visible cap as a future hardening.

## Channel 2: Browser Desktop Notifications (REQ-NOTIF-003 → 009, all ❌)

### Trigger flow

The frontend SSE handler already processes `state_change` events for
conversation atom updates. The notification system would hook the
same handler:

1. **Classify the new state.** State-to-event mapping:
   - `awaiting_task_approval` → "Task approval needed"
   - `awaiting_user_response` → "Question asked"
   - `error | context_exhausted` → "Agent error"
   - `idle` (when previous state was busy long enough) → "Agent finished"
2. **Per-event toggle check** (REQ-NOTIF-006). If the event type is
   disabled, drop the trigger.
3. **Tab-focus gate** (REQ-NOTIF-005). If the tab is focused AND the
   conversation is the active route, suppress.
4. **Permission check.**
   - If `Notification.permission === 'granted'`, fire (step 5).
   - If `'default'` (not yet asked), drop this trigger and queue an
     in-app cue ("Enable desktop notifications in Settings to be
     pulled in for events like this one") to show on next focus.
     Do NOT call `Notification.requestPermission()` from this code
     path — `requestPermission()` is gated to user-gesture contexts
     (button clicks etc.) and an SSE handler is not one. A blocked
     / ignored prompt would be worse than no prompt. The settings
     panel's "Enable desktop notifications" button (REQ-NOTIF-003)
     is the only place that calls `requestPermission()`.
   - If `'denied'`, drop. The OS-level permission cannot be
     re-prompted programmatically; the settings UI shows guidance
     to change it in browser settings.
5. **Fire** the notification:
   ```ts
   const n = new Notification(title, { body, tag: conversationId });
   n.onclick = () => { window.focus(); navigate(`/c/${slug}`); };
   ```
   `onclick` is a property of the `Notification` instance, not a
   `NotificationOptions` field — assign it after construction.

The "previous state was busy" check on idle prevents spurious
"finished" notifications on page load when conversations were already
idle.

### Settings storage (REQ-NOTIF-006, REQ-NOTIF-009)

Server-side `notification_settings` table (single-row or kv):

```
notification_settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
)
```

Keys: `notifications_enabled`, `notify_task_approval`,
`notify_question`, `notify_error`, `notify_idle`. Boolean values
serialised as `"true"` / `"false"`.

API: `GET /api/settings/notifications` returns the current map; `PUT
/api/settings/notifications` replaces it. The frontend caches the
response in the conversation atom (or app-level state) and refreshes
on settings-page mount.

### Tab lifecycle and catch-up (REQ-NOTIF-008)

Background tabs degrade over time:

- **First ~5 minutes:** SSE alive, JS timers throttled to ~1/sec. Notifications fire normally.
- **After ~5 minutes:** Chrome may suspend the tab. The SSE TCP keep-alive may survive but JS does not process events.
- **Extended background:** the tab may be discarded; SSE dies.

The notification system handles this via the catch-up rule
(REQ-NOTIF-008): on SSE reconnect, scan the conversation list for any
non-sub-agent in a notification-worthy state and emit notifications
per the same gating rules. If the agent asked a question while the tab
was suspended, the question's notification fires when the tab wakes up
and the SSE reconnects. Not as instant as a service-worker push, but
functionally correct.

A service-worker push channel could be added later if instant
notifications during long-background sessions become a requirement.
The spec leaves it as a non-goal for v1.

### Click-to-navigate (REQ-NOTIF-007)

The `Notification` constructor's `onclick` handler:

```ts
notification.onclick = () => {
  window.focus();
  navigate(`/c/${slug}`);
};
```

`window.focus()` requests focus from the OS; modern browsers honor it
when the click came from a notification (user-gesture proxy). The
React Router `navigate` runs inside the still-alive React tree because
the tab was active when the notification was created.

### Browser permission (REQ-NOTIF-003)

`Notification.permission` is `'default'`, `'granted'`, or `'denied'`.
The settings UI surfaces the current state and a "Request permission"
button when `'default'`. When `'denied'`, no programmatic re-prompt is
possible — the UI shows guidance to change it in browser settings.

The first attempt to fire a notification when the permission is still
`'default'` should call `Notification.requestPermission()` rather than
silently dropping. The user has notifications enabled in Phoenix
settings; the browser-level prompt is the next gate.

## Why Two Channels, Not One

A naïve design might use only one mechanism (e.g. always show in-app
toasts, never desktop notifications). That would miss the canonical
"Phoenix is in another tab and the agent is blocked" case the
desktop-notification channel exists to serve.

The opposite naïve design (always desktop notifications, never
toasts) would interrupt the user with an OS-level popup every time
they clicked a button. The desktop notification permission would
quickly burn out and they'd disable it.

Two channels by trigger source is the right shape: each channel has
a clear "what fires me" rule that doesn't overlap with the other.

## Cross-Spec Dependencies

- `specs/sse_wire/`: every desktop-notification trigger reads from `state_change` events. The notification system is a parallel consumer of the same stream the conversation atom drives off. No new SSE event types needed.
- `specs/bedrock/`: phase transitions (`awaiting_task_approval`, `awaiting_user_response`, `error`, `context_exhausted`, busy → idle) define notification-worthiness. Same source of truth as the conversation atom and the StateBar.
- `specs/conversation-ui/`: the `<Toast />` container is mounted in `DesktopLayout.tsx:111`. The eventual notification settings panel would live in the same chrome.
- `specs/terminal-panel/` REQ-TPANEL-009: the assist-setup error path currently uses `console.error` and is documented as a gap. Once REQ-NOTIF-002 is the canonical visible-error mechanism, that gap closes by routing through `useToast.showError`.
