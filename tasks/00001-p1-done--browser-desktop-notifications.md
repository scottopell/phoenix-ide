# Implement browser desktop notifications

## Context

`specs/notifications/executive.md` says the in-app toast phase is complete, but the browser desktop notification phase is still not started:

- REQ-NOTIF-003: notify when the agent needs the user
- REQ-NOTIF-004: notify when a long-running task finishes
- REQ-NOTIF-005: suppress browser notifications when the user is already looking
- REQ-NOTIF-006: per-event + global notification settings
- REQ-NOTIF-007: click a notification to return to the right conversation
- REQ-NOTIF-008: catch-up notifications after reconnect/wake/list refresh (still relevant; audit note below)
- REQ-NOTIF-009: settings persisted server-side

There is a broader ready task (`tasks/27106-p1-ready--continue-spec-audit-bug-hunting.md`) that calls out `specs/notifications/` as a concrete starting point, but no focused task file for implementing the web/browser notification feature itself.

## Audit note: REQ-NOTIF-008 relevance

REQ-NOTIF-008 still appears relevant as a product requirement: if the tab sleeps or the network drops while a conversation transitions into `awaiting_task_approval`, `awaiting_user_response`, `error`, or `context_exhausted`, the user needs a catch-up notification when Phoenix becomes live again.

Current code does not appear to implement browser notifications or catch-up delivery yet. However, the spec's implementation guidance should not be followed literally without checking the current architecture:

- `ui/src/hooks/useConnection.ts` opens a **per-conversation** SSE stream and its `init` payload contains the current conversation snapshot, messages, breadcrumbs, and pending events — not the full conversation list.
- The global conversation list is refreshed separately by `ui/src/conversation/useConversationsRefresh.ts` via `api.listConversations()` on initial load, every 5s while visible, and on browser `online` events.
- Therefore, REQ-NOTIF-008's catch-up pass probably belongs either after a successful conversation-list refresh or in a notification coordinator that observes the `ConversationStore`, not blindly inside the per-conversation SSE `init` handler.
- The Allium guidance at `specs/notifications/notifications.allium:179-185` says "in the SSE init handler... iterate the conversation list"; that looks stale relative to the current per-conversation SSE design and should be updated if implementation confirms this.


## Plan

1. Read the normative notification specs before coding:
   - `specs/notifications/requirements.md`
   - `specs/notifications/design.md`
   - `specs/notifications/executive.md`
   - `specs/notifications/notifications.allium`
2. Inspect current frontend SSE/state handling and conversation routing to find the best integration point for notification triggers.
3. Implement browser Notification API permission/request flow and trigger mapping for notification-worthy states:
   - `awaiting_task_approval`
   - `awaiting_user_response`
   - `error`
   - `context_exhausted`
   - long-running busy → `idle` completion once the threshold/design decision is resolved
4. Add focus gating per REQ-NOTIF-005:
   - suppress when `document.visibilityState === 'visible'`
   - and `document.hasFocus()`
   - and the triggering conversation is the active route
5. Add click-to-navigate/focus behaviour for notifications per REQ-NOTIF-007.
6. Implement settings incrementally:
   - start with the spec-required master/per-event model
   - persist server-side if implementing REQ-NOTIF-009 in this slice; otherwise capture a follow-up if it proves too large
7. Implement reconnect/wake catch-up using the current architecture:
   - audit whether catch-up should be triggered from conversation-list refresh, visibility/online events, or per-conversation SSE reconnect
   - do **not** assume the per-conversation SSE `init` contains the full conversation list
   - if the implementation path differs from `notifications.allium` guidance, update the spec guidance/status in the same change
   - if the catch-up semantics need a design decision, capture a precise follow-up task rather than guessing
8. Update `specs/notifications/executive.md` statuses for completed requirements and keep notes precise.
9. Validate with `./dev.py check` and browser/manual testing.

## Acceptance

- Browser desktop notifications fire for notification-worthy conversation state changes when the Phoenix tab is not the active focused conversation.
- Notifications are suppressed when the user is already looking at the active triggering conversation.
- Clicking a notification returns the user to the correct conversation.
- User-configurable notification settings exist for implemented event types, with persistence matching the completed requirement scope.
- Reconnect catch-up behaviour is implemented or explicitly captured as a follow-up with a concrete design if it is not safe to complete in this task.
- `specs/notifications/executive.md` accurately reflects what is complete vs deferred.
- `./dev.py check` passes.
