import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type Conversation, type ConversationState, type NotificationSettings } from './api';
import { isAgentWorking, parseConversationState } from './utils';

export type NotificationEventType =
  | 'task_approval_needed'
  | 'question_asked'
  | 'agent_error'
  | 'agent_finished';

export const DEFAULT_NOTIFICATION_SETTINGS: NotificationSettings = {
  enabled: true,
  notify_task_approval: true,
  notify_question: true,
  notify_error: true,
  notify_idle: true,
};

const DISABLED_NOTIFICATION_SETTINGS: NotificationSettings = {
  ...DEFAULT_NOTIFICATION_SETTINGS,
  enabled: false,
};

export const AGENT_FINISHED_THRESHOLD_MS = 30_000;

type BrowserPermission = NotificationPermission | 'unsupported';

type NotificationEvent = {
  type: NotificationEventType;
  title: string;
  conversation: Conversation;
};

const notificationRuntime = {
  settings: DISABLED_NOTIFICATION_SETTINGS,
  settingsLoaded: false,
  settingsLoading: null as Promise<NotificationSettings> | null,
  permissionCuePending: false,
  lastCatchupKeyByConversationId: new Map<string, string>(),
};

export function getBrowserNotificationPermission(): BrowserPermission {
  if (typeof window === 'undefined' || !('Notification' in window)) return 'unsupported';
  return Notification.permission;
}

export function queueNotificationPermissionCue(): void {
  notificationRuntime.permissionCuePending = true;
}

export function consumeNotificationPermissionCue(): boolean {
  const pending = notificationRuntime.permissionCuePending;
  notificationRuntime.permissionCuePending = false;
  return pending;
}

export function updateNotificationRuntimeSettings(settings: NotificationSettings): void {
  notificationRuntime.settings = settings;
  notificationRuntime.settingsLoaded = true;
}

export function getNotificationRuntimeSettings(): NotificationSettings {
  return notificationRuntime.settings;
}

export function loadNotificationSettings(): Promise<NotificationSettings> {
  if (notificationRuntime.settingsLoaded) return Promise.resolve(notificationRuntime.settings);
  if (!notificationRuntime.settingsLoading) {
    notificationRuntime.settingsLoading = api.getNotificationSettings()
      .then((settings) => {
        updateNotificationRuntimeSettings(settings);
        return settings;
      })
      .catch((err: unknown) => {
        notificationRuntime.settingsLoading = null;
        throw err;
      });
  }
  return notificationRuntime.settingsLoading;
}

export function resetNotificationRuntimeForTest(settings?: NotificationSettings): void {
  notificationRuntime.settings = settings ?? DISABLED_NOTIFICATION_SETTINGS;
  notificationRuntime.settingsLoaded = settings !== undefined;
  notificationRuntime.settingsLoading = null;
  notificationRuntime.permissionCuePending = false;
  notificationRuntime.lastCatchupKeyByConversationId.clear();
  busyStartedAtByConversation.clear();
  previousSnapshotsById.clear();
}

function notificationEnabled(type: NotificationEventType, settings: NotificationSettings): boolean {
  if (!settings.enabled) return false;
  switch (type) {
    case 'task_approval_needed': return settings.notify_task_approval;
    case 'question_asked': return settings.notify_question;
    case 'agent_error': return settings.notify_error;
    case 'agent_finished': return settings.notify_idle;
  }
}

function eventForState(state: ConversationState, conversation: Conversation): NotificationEvent | null {
  switch (state.type) {
    case 'awaiting_task_approval':
      return { type: 'task_approval_needed', title: 'Task approval needed', conversation };
    case 'awaiting_user_response':
      return { type: 'question_asked', title: 'Question asked', conversation };
    case 'error':
    case 'context_exhausted':
      return { type: 'agent_error', title: 'Agent error', conversation };
    default:
      return null;
  }
}

function currentActiveSlug(): string | null {
  const match = window.location.pathname.match(/^\/c\/([^/?#]+)/);
  return match?.[1] ? decodeURIComponent(match[1]) : null;
}

function shouldSuppressForFocus(conversation: Conversation): boolean {
  return document.visibilityState === 'visible'
    && document.hasFocus()
    && currentActiveSlug() === conversation.slug;
}

function deliverNotification(event: NotificationEvent): void {
  if (!notificationRuntime.settingsLoaded) return;
  if (!notificationEnabled(event.type, notificationRuntime.settings)) return;
  if (shouldSuppressForFocus(event.conversation)) return;

  const permission = getBrowserNotificationPermission();
  if (permission === 'default') {
    queueNotificationPermissionCue();
    return;
  }
  if (permission !== 'granted') return;

  const notification = new Notification(event.title, {
    body: event.conversation.slug,
    tag: `${event.type}:${event.conversation.id}:${event.conversation.updated_at}`,
  });
  notification.onclick = () => {
    window.focus();
    window.dispatchEvent(
      new CustomEvent('phoenix:navigate-to-conversation', {
        detail: { slug: event.conversation.slug },
      }),
    );
    notification.close();
  };
}

export function notifyConversationStateChange(
  conversation: Conversation | null | undefined,
  previousState: ConversationState | null | undefined,
  nextState: ConversationState,
): void {
  if (!conversation) return;
  if (isAgentWorking(nextState)) {
    rememberBusyState(conversation, nextState);
    return;
  }

  const stateEvent = eventForState(nextState, conversation);
  if (stateEvent) {
    busyStartedAtByConversation.delete(conversation.id);
    deliverNotification(stateEvent);
    return;
  }

  if (
    nextState.type === 'idle' &&
    previousState &&
    isAgentWorking(previousState) &&
    previousState.type !== 'awaiting_llm'
  ) {
    const busyStartedAt = busyStartedAtByConversation.get(conversation.id);
    busyStartedAtByConversation.delete(conversation.id);
    if (busyStartedAt && Date.now() - busyStartedAt >= AGENT_FINISHED_THRESHOLD_MS) {
      deliverNotification({ type: 'agent_finished', title: 'Agent finished', conversation });
    }
    return;
  }

  busyStartedAtByConversation.delete(conversation.id);
}

const busyStartedAtByConversation = new Map<string, number>();

function rememberBusyState(conversation: Conversation, state: ConversationState): void {
  if (isAgentWorking(state)) {
    if (!busyStartedAtByConversation.has(conversation.id)) {
      busyStartedAtByConversation.set(conversation.id, Date.now());
    }
  } else {
    busyStartedAtByConversation.delete(conversation.id);
  }
}

const previousSnapshotsById = new Map<string, Conversation>();

export function notifyConversationSnapshotChange(next: Conversation): void {
  const previous = previousSnapshotsById.get(next.id);
  const nextState = next.state ? parseConversationState(next.state) : { type: 'idle' as const };
  if (!previous) {
    rememberBusyState(next, nextState);
    previousSnapshotsById.set(next.id, next);
    return;
  }
  const previousState = previous.state ? parseConversationState(previous.state) : undefined;
  notifyConversationStateChange(next, previousState, nextState);
  previousSnapshotsById.set(next.id, next);
}

export function notifyCatchUp(conversations: readonly Conversation[]): void {
  for (const conversation of conversations) {
    if (conversation.parent_conversation_id) continue;
    const state = conversation.state ? parseConversationState(conversation.state) : { type: 'idle' as const };
    const event = eventForState(state, conversation);
    if (!event) continue;
    const key = `${conversation.id}:${state.type}:${conversation.updated_at}`;
    const previousKey = notificationRuntime.lastCatchupKeyByConversationId.get(conversation.id);
    if (previousKey === key) continue;
    notificationRuntime.lastCatchupKeyByConversationId.set(conversation.id, key);
    deliverNotification(event);
  }
}

export function useNotificationClickNavigationBridge(): void {
  const navigate = useNavigate();
  useEffect(() => {
    const handler = (event: Event) => {
      const slug = (event as CustomEvent<{ slug?: string }>).detail?.slug;
      if (slug) navigate(`/c/${slug}`);
    };
    window.addEventListener('phoenix:navigate-to-conversation', handler);
    return () => window.removeEventListener('phoenix:navigate-to-conversation', handler);
  }, [navigate]);
}
