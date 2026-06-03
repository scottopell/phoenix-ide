# Close stale desktop notifications when their conversation is opened

## Problem

Phoenix desktop notifications are currently closed only from the notification's own `onclick` handler. If the user opens the relevant conversation another way — sidebar click, command palette, direct URL, browser history, etc. — the conversation is effectively "read"/acknowledged but the OS notification can remain visible.

## Proposed fix

1. Extend the notification runtime to retain live `Notification` instances by their semantic tag/conversation.
2. Add an exported acknowledgement API, e.g. `acknowledgeConversationNotifications(conversation)` or `closeNotificationsForConversation(conversationId)`.
3. Call that API from the route/layout path when the active `/c/:slug` conversation snapshot is known, so opening the conversation through any navigation path closes outstanding notifications for that conversation.
4. Keep existing notification-click behavior, but route it through the same acknowledgement mechanism so clicked notifications and non-notification navigation share one path.
5. Add unit tests in `ui/src/notifications.test.ts` covering:
   - a delivered notification is closed when its conversation is acknowledged without clicking the notification;
   - notifications for other conversations are not closed;
   - repeated acknowledgement is safe/no-op.
6. Update `specs/notifications/` to state that opening the triggering conversation acknowledges and dismisses live desktop notifications, not only notification clicks.

## Notes

Browser Notification instances are only closeable while the page that created them is alive and still holds the instance. This fixes the common in-session stale-notification case. Notifications surviving a full page reload or browser restart may be outside the web Notification API's control unless Phoenix adds service-worker notification ownership later.
