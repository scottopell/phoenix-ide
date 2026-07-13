// Public notification surface.
//
// Wires the pure policy reducer (`./notifications/policy`) to its
// browser/API adapter (`./notifications/store`) behind a small singleton,
// and exposes the free functions and React bindings the rest of the app
// uses. The reducer holds all policy state; this layer only owns the
// snapshot-diff cache needed to translate conversation snapshots into
// reducer events.

import { useCallback, useEffect, useSyncExternalStore } from 'react';
import { useNavigate } from 'react-router-dom';
import type { Conversation, ConversationState, NotificationSettings } from './api';
import { parseConversationState } from './utils';
import {
  NotificationStore,
  getBrowserNotificationPermission,
  registerCoordinatorNotificationTarget,
} from './notifications/store';

import type { SettingsStatus } from './notifications/policy';

export {
  DEFAULT_NOTIFICATION_SETTINGS,
  AGENT_FINISHED_THRESHOLD_MS,
} from './notifications/policy';
export type { NotificationEventType } from './notifications/policy';
export { getBrowserNotificationPermission };

const store = new NotificationStore();

// Previous snapshot per conversation, used to diff snapshot updates into
// state-change events. Lives here rather than in the reducer because it is a
// translation cache, not policy state.
const previousSnapshotsById = new Map<string, Conversation>();

export function notifyConversationStateChange(
  conversation: Conversation | null | undefined,
  previousState: ConversationState | null | undefined,
  nextState: ConversationState,
): void {
  if (!conversation) return;
  store.dispatch({
    type: 'conversation_state_changed',
    conversation,
    previousState: previousState ?? undefined,
    nextState,
  });
}

export function notifyConversationSnapshotChange(next: Conversation): void {
  if (next.parent_conversation_id) return;
  const previous = previousSnapshotsById.get(next.id);
  const nextState = next.state ? parseConversationState(next.state) : { type: 'idle' as const };
  if (!previous) {
    store.dispatch({ type: 'conversation_snapshot_seeded', conversation: next, state: nextState });
    previousSnapshotsById.set(next.id, next);
    return;
  }
  const previousState = previous.state ? parseConversationState(previous.state) : undefined;
  store.dispatch({ type: 'conversation_state_changed', conversation: next, previousState, nextState });
  previousSnapshotsById.set(next.id, next);
}

export function registerCoordinatorForNotifications(conversationId: string): void {
  registerCoordinatorNotificationTarget(conversationId);
}

export function notifyCatchUp(conversations: readonly Conversation[]): void {
  store.dispatch({ type: 'catchup_scan', conversations });
}

export function closeNotificationsForConversation(conversationId: string): void {
  store.closeForConversation(conversationId);
}

export function loadNotificationSettings(): Promise<NotificationSettings> {
  return store.loadSettings();
}

export function loadNotificationSettingsAndCatchUp(conversations: readonly Conversation[]): Promise<NotificationSettings> {
  return store.loadSettings().then((settings) => {
    store.dispatch({ type: 'catchup_scan', conversations });
    return settings;
  });
}

export function consumeNotificationPermissionCue(): boolean {
  return store.consumePermissionCue();
}

export function resetNotificationRuntimeForTest(settings?: NotificationSettings): void {
  store.resetForTest(settings);
  previousSnapshotsById.clear();
}

// Reactive view of notification settings for the settings panel. Routes
// saves through the policy reducer so out-of-order responses cannot
// clobber a newer edit.
export function useNotificationSettings(): {
  settings: NotificationSettings;
  status: SettingsStatus;
  saving: boolean;
  error: string | null;
  save: (next: NotificationSettings) => void;
} {
  const state = useSyncExternalStore(store.subscribe, store.getState, store.getState);
  useEffect(() => {
    void store.loadSettings().catch(() => {});
  }, []);
  const save = useCallback((next: NotificationSettings) => {
    store.dispatch({ type: 'settings_save_requested', settings: next });
  }, []);
  return {
    settings: state.settings,
    status: state.settingsStatus,
    saving: state.saving,
    // Surface a save error if one is pending, otherwise a load failure, so a
    // failed initial fetch is visible rather than silently showing defaults.
    error: state.saveError ?? state.loadError,
    save,
  };
}

export function useNotificationClickNavigationBridge(): void {
  const navigate = useNavigate();
  useEffect(() => {
    const handler = (event: Event) => {
      const route = (event as CustomEvent<{ route?: string }>).detail?.route;
      if (route) navigate(route);
    };
    window.addEventListener('phoenix:navigate-to-conversation', handler);
    return () => window.removeEventListener('phoenix:navigate-to-conversation', handler);
  }, [navigate]);
}
