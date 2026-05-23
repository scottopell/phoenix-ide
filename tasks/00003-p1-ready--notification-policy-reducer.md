# Refactor desktop notification policy into a pure reducer with typed effects

## Problem

Browser desktop notifications now work, but the policy logic lives in `ui/src/notifications.ts` as mutable module state plus imperative helpers. That made review edge cases easy to miss:

- settings can be loading while catch-up scans run
- catch-up dedupe must only be recorded after delivery can actually run
- live SSE delivery and list-refresh catch-up need to share dedupe semantics
- notification settings saves must preserve ordering across overlapping edits
- `context_exhausted` is notification-worthy only while it is still actionable
- browser `Notification` construction can fail and must be an isolated effect

The current implementation has tests for these cases, but the design would be easier to reason about if the notification subsystem were a small policy engine: pure state transitions that emit typed effects, with browser/API side effects handled by a thin adapter.

## Proposal

Extract the desktop-notification policy into a pure reducer with typed effects.

Recommended shape:

```ts
type NotificationPolicyState = {
  settingsStatus: 'unloaded' | 'loading' | 'loaded' | 'failed';
  settings: NotificationSettings;
  latestSaveId: number;
  saving: boolean;
  saveError: string | null;
  permissionCuePending: boolean;
  busyStartedAtByConversationId: Map<string, number>;
  attentionSeenByConversationId: Map<string, string>;
};

type NotificationPolicyEvent =
  | { type: 'settings_load_started' }
  | { type: 'settings_loaded'; settings: NotificationSettings }
  | { type: 'settings_load_failed'; error: string }
  | { type: 'settings_save_requested'; settings: NotificationSettings }
  | { type: 'settings_save_succeeded'; requestId: number; settings: NotificationSettings }
  | { type: 'settings_save_failed'; requestId: number; error: string }
  | { type: 'conversation_state_changed'; conversation: Conversation; previousState: ConversationState | undefined; nextState: ConversationState }
  | { type: 'catchup_scan'; conversations: Conversation[] }
  | { type: 'permission_cue_consumed' };

type NotificationEffect =
  | { type: 'load_settings' }
  | { type: 'save_settings'; requestId: number; settings: NotificationSettings }
  | { type: 'show_browser_notification'; title: string; body: string; tag: string; slug: string }
  | { type: 'queue_permission_cue' };
```

Keep the browser/API integration as a thin effect runner:

- `load_settings` -> `api.getNotificationSettings()`
- `save_settings` -> serialized `api.updateNotificationSettings()`
- `show_browser_notification` -> guarded `new Notification(...)` + click navigation
- `queue_permission_cue` -> in-app cue on next focus

This should remain a plain TypeScript reducer/effect system, not XState, unless the UI flow grows much more complex. The subsystem is a policy engine with side effects, not a user-visible multi-state workflow.

## Allium/spec updates

Extend `specs/notifications/notifications.allium` to cover the implementation edge cases explicitly:

1. Notifications cannot deliver until server settings have loaded.
2. Catch-up must not mark an event as seen unless delivery succeeded or was intentionally focus-suppressed.
3. Live SSE delivery and list-refresh catch-up dedupe the same unresolved blocking state.
4. Leaving a blocking state clears the attention dedupe key so a future blocking state can notify again.
5. `context_exhausted` with `continued_in_conv_id` is not notification-worthy.
6. Settings saves are persisted in user-edit order; older saves cannot overwrite newer saves.
7. `agent_finished` completions for the same conversation use a completion-specific tag, while unresolved blocking states collapse by conversation/event.

## Acceptance criteria

- Notification policy decisions are implemented by a pure reducer returning `{ state, effects }`.
- Browser/API side effects are isolated in an adapter and are not required for reducer tests.
- Existing behavior from `ui/src/notifications.test.ts` is preserved and moved/expanded to reducer-first tests.
- Tests cover settings-load gating, save ordering, focus suppression, long-task thresholding, live/catch-up dedupe, catch-up no-dedupe-before-delivery, sub-agent skipping, continued-context suppression, and notification construction failure handling.
- `specs/notifications/notifications.allium` includes the edge-case rules above.
- `./dev.py check` passes.
