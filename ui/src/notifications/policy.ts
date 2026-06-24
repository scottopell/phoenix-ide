// Pure notification policy engine.
//
// This module is the single source of truth for *when* a desktop
// notification should fire, which one to suppress, and how settings load
// and save transitions resolve. It is a plain reducer — `(state, event,
// env) => { state, effects }` — with no DOM, network, or timer access, so
// every policy decision is exercisable without a browser. All side effects
// (constructing `Notification`, calling the settings API) are described as
// typed `NotificationEffect` values for the adapter to run.

import type { Conversation, ConversationState, NotificationSettings } from '../api';
import { isAgentWorking, parseConversationState } from '../utils';

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

export type BrowserPermission = NotificationPermission | 'unsupported';

export type SettingsStatus = 'unloaded' | 'loading' | 'loaded' | 'failed';

// A notification candidate derived from a conversation's state. Not every
// candidate is delivered — delivery is gated by `decideDelivery`.
export type PolicyNotificationEvent = {
  type: NotificationEventType;
  title: string;
  conversation: Conversation;
};

export type NotificationPolicyState = {
  settingsStatus: SettingsStatus;
  settings: NotificationSettings;
  // Monotonic id of the most recently *requested* save. A save response is
  // authoritative only while it is still the latest — older responses
  // cannot overwrite a newer user edit.
  latestSaveId: number;
  saving: boolean;
  saveError: string | null;
  // Error from the most recent failed settings load, surfaced until a retry
  // starts, a load succeeds, or a user edit takes authority. Kept separate
  // from saveError so neither clobbers the other.
  loadError: string | null;
  // True when delivery wanted to fire but browser permission was still
  // `default`. The adapter surfaces an in-app cue the next time the tab
  // gains focus, then dispatches `permission_cue_consumed`.
  permissionCuePending: boolean;
  // When a conversation entered a busy state, keyed by id. Used to gate
  // `agent_finished` on the long-task threshold.
  busyStartedAtByConversationId: Map<string, number>;
  // Last delivered blocking attention key per conversation, used to dedupe
  // live SSE delivery against list-refresh catch-up.
  attentionSeenByConversationId: Map<string, string>;
};

// Ambient browser facts the reducer needs to decide delivery. Supplied by
// the adapter at dispatch time; provided directly by tests.
export type NotificationEnv = {
  now: number;
  permission: BrowserPermission;
  visible: boolean;
  hasFocus: boolean;
  activeSlug: string | null;
};

export type NotificationPolicyEvent =
  | { type: 'settings_load_started' }
  | { type: 'settings_loaded'; settings: NotificationSettings }
  | { type: 'settings_load_failed'; error: string }
  | { type: 'settings_save_requested'; settings: NotificationSettings }
  | { type: 'settings_save_succeeded'; requestId: number; settings: NotificationSettings }
  | { type: 'settings_save_failed'; requestId: number; error: string }
  | { type: 'conversation_state_changed'; conversation: Conversation; previousState: ConversationState | undefined; nextState: ConversationState }
  | { type: 'conversation_snapshot_seeded'; conversation: Conversation; state: ConversationState }
  | { type: 'catchup_scan'; conversations: readonly Conversation[] }
  | { type: 'notification_delivery_failed'; conversationId: string; attentionKey: string | null }
  | { type: 'permission_cue_consumed' };

export type NotificationEffect =
  | { type: 'load_settings' }
  | { type: 'save_settings'; requestId: number; settings: NotificationSettings }
  | {
      type: 'show_browser_notification';
      title: string;
      body: string;
      tag: string;
      conversation: Conversation;
      // Carried through so a failed `new Notification(...)` can roll back the
      // dedupe mark the reducer set optimistically. Null for completions,
      // which are never deduped.
      attentionKey: string | null;
    };

export type ReduceResult = { state: NotificationPolicyState; effects: NotificationEffect[] };

export function initialPolicyState(): NotificationPolicyState {
  return {
    settingsStatus: 'unloaded',
    settings: DISABLED_NOTIFICATION_SETTINGS,
    latestSaveId: 0,
    saving: false,
    saveError: null,
    loadError: null,
    permissionCuePending: false,
    busyStartedAtByConversationId: new Map(),
    attentionSeenByConversationId: new Map(),
  };
}

function eventTypeEnabled(type: NotificationEventType, settings: NotificationSettings): boolean {
  if (!settings.enabled) return false;
  switch (type) {
    case 'task_approval_needed': return settings.notify_task_approval;
    case 'question_asked': return settings.notify_question;
    case 'agent_error': return settings.notify_error;
    case 'agent_finished': return settings.notify_idle;
  }
}

// A `context_exhausted` conversation that has already continued elsewhere is
// no longer actionable, so it is not notification-worthy.
export function eventForState(state: ConversationState, conversation: Conversation): PolicyNotificationEvent | null {
  switch (state.type) {
    case 'awaiting_task_approval':
    case 'awaiting_commission_review_approval':
      return { type: 'task_approval_needed', title: 'Task approval needed', conversation };
    case 'awaiting_user_response':
      return { type: 'question_asked', title: 'Question asked', conversation };
    case 'context_exhausted':
      if (conversation.continued_in_conv_id) return null;
      return { type: 'agent_error', title: 'Agent error', conversation };
    case 'error':
      return { type: 'agent_error', title: 'Agent error', conversation };
    default:
      return null;
  }
}

// Unresolved blocking states collapse by conversation + event so live and
// catch-up paths dedupe against each other. Completions get no key — each
// completion is independently notification-worthy.
export function attentionKeyFor(event: PolicyNotificationEvent): string | null {
  return event.type === 'agent_finished' ? null : `${event.conversation.id}:${event.type}`;
}

// Completions carry `updated_at` so a later completion for the same
// conversation is a distinct browser notification; blocking states collapse
// by conversation + event.
export function notificationTagFor(event: PolicyNotificationEvent): string {
  if (event.type === 'agent_finished') {
    return `${event.type}:${event.conversation.id}:${event.conversation.updated_at}`;
  }
  return `${event.type}:${event.conversation.id}`;
}

function shouldSuppressForFocus(env: NotificationEnv, conversation: Conversation): boolean {
  return env.visible && env.hasFocus && env.activeSlug === conversation.slug;
}

function markAttentionSeen(state: NotificationPolicyState, event: PolicyNotificationEvent): NotificationPolicyState {
  const key = attentionKeyFor(event);
  if (!key) return state;
  const attentionSeenByConversationId = new Map(state.attentionSeenByConversationId);
  attentionSeenByConversationId.set(event.conversation.id, key);
  return { ...state, attentionSeenByConversationId };
}

function clearAttentionSeen(state: NotificationPolicyState, conversationId: string): NotificationPolicyState {
  if (!state.attentionSeenByConversationId.has(conversationId)) return state;
  const attentionSeenByConversationId = new Map(state.attentionSeenByConversationId);
  attentionSeenByConversationId.delete(conversationId);
  return { ...state, attentionSeenByConversationId };
}

function deleteBusyStartedAt(state: NotificationPolicyState, conversationId: string): NotificationPolicyState {
  if (!state.busyStartedAtByConversationId.has(conversationId)) return state;
  const busyStartedAtByConversationId = new Map(state.busyStartedAtByConversationId);
  busyStartedAtByConversationId.delete(conversationId);
  return { ...state, busyStartedAtByConversationId };
}

// Remember when a conversation became busy (idempotent on the start time) so
// `agent_finished` can be gated on elapsed work; clear it otherwise.
function rememberBusyState(state: NotificationPolicyState, conversation: Conversation, convState: ConversationState, now: number): NotificationPolicyState {
  if (isAgentWorking(convState)) {
    if (state.busyStartedAtByConversationId.has(conversation.id)) return state;
    const busyStartedAtByConversationId = new Map(state.busyStartedAtByConversationId);
    busyStartedAtByConversationId.set(conversation.id, now);
    return { ...state, busyStartedAtByConversationId };
  }
  return deleteBusyStartedAt(state, conversation.id);
}

// The shared delivery gate: master toggle, per-event toggle, focus
// suppression, and browser permission. Marks the attention key seen only
// when delivery actually fired or was intentionally focus-suppressed, so a
// later catch-up pass can re-attempt anything that did not reach the user.
function decideDelivery(state: NotificationPolicyState, env: NotificationEnv, event: PolicyNotificationEvent): ReduceResult {
  if (state.settingsStatus !== 'loaded') return { state, effects: [] };
  if (!eventTypeEnabled(event.type, state.settings)) return { state, effects: [] };
  if (shouldSuppressForFocus(env, event.conversation)) {
    return { state: markAttentionSeen(state, event), effects: [] };
  }
  if (env.permission === 'default') {
    return { state: { ...state, permissionCuePending: true }, effects: [] };
  }
  if (env.permission !== 'granted') return { state, effects: [] };

  return {
    state: markAttentionSeen(state, event),
    effects: [{
      type: 'show_browser_notification',
      title: event.title,
      body: event.conversation.slug,
      tag: notificationTagFor(event),
      conversation: event.conversation,
      attentionKey: attentionKeyFor(event),
    }],
  };
}

function reduceConversationStateChanged(
  state: NotificationPolicyState,
  env: NotificationEnv,
  conversation: Conversation,
  previousState: ConversationState | undefined,
  nextState: ConversationState,
): ReduceResult {
  if (isAgentWorking(nextState)) {
    return { state: rememberBusyState(state, conversation, nextState, env.now), effects: [] };
  }

  const stateEvent = eventForState(nextState, conversation);
  if (stateEvent) {
    const cleared = deleteBusyStartedAt(state, conversation.id);
    const key = attentionKeyFor(stateEvent);
    if (key && cleared.attentionSeenByConversationId.get(conversation.id) === key) {
      return { state: cleared, effects: [] };
    }
    return decideDelivery(cleared, env, stateEvent);
  }

  if (
    nextState.type === 'idle' &&
    previousState &&
    isAgentWorking(previousState) &&
    previousState.type !== 'awaiting_llm'
  ) {
    const busyStartedAt = state.busyStartedAtByConversationId.get(conversation.id);
    const next = clearAttentionSeen(deleteBusyStartedAt(state, conversation.id), conversation.id);
    if (busyStartedAt !== undefined && env.now - busyStartedAt >= AGENT_FINISHED_THRESHOLD_MS) {
      return decideDelivery(next, env, { type: 'agent_finished', title: 'Agent finished', conversation });
    }
    return { state: next, effects: [] };
  }

  return { state: clearAttentionSeen(deleteBusyStartedAt(state, conversation.id), conversation.id), effects: [] };
}

function reduceCatchupScan(state: NotificationPolicyState, env: NotificationEnv, conversations: readonly Conversation[]): ReduceResult {
  let next = state;
  const effects: NotificationEffect[] = [];
  for (const conversation of conversations) {
    if (conversation.parent_conversation_id) continue;
    const convState = conversation.state ? parseConversationState(conversation.state) : { type: 'idle' as const };
    const event = eventForState(convState, conversation);
    if (!event) continue;
    const key = attentionKeyFor(event);
    if (key && next.attentionSeenByConversationId.get(conversation.id) === key) continue;
    const result = decideDelivery(next, env, event);
    next = result.state;
    effects.push(...result.effects);
  }
  return { state: next, effects };
}

export function notificationPolicyReducer(
  state: NotificationPolicyState,
  event: NotificationPolicyEvent,
  env: NotificationEnv,
): ReduceResult {
  switch (event.type) {
    case 'settings_load_started':
      return { state: { ...state, settingsStatus: 'loading', loadError: null }, effects: [{ type: 'load_settings' }] };

    case 'settings_loaded':
      // A save that landed mid-load already moved us to `loaded` with
      // user-authoritative settings; the stale load result is ignored.
      if (state.settingsStatus !== 'loading') return { state, effects: [] };
      return { state: { ...state, settingsStatus: 'loaded', settings: event.settings, loadError: null }, effects: [] };

    case 'settings_load_failed':
      if (state.settingsStatus !== 'loading') return { state, effects: [] };
      return { state: { ...state, settingsStatus: 'failed', loadError: event.error }, effects: [] };

    case 'settings_save_requested': {
      const latestSaveId = state.latestSaveId + 1;
      return {
        state: {
          ...state,
          latestSaveId,
          settings: event.settings,
          settingsStatus: 'loaded',
          saving: true,
          saveError: null,
          // The user's edit is now authoritative, so a prior load failure no
          // longer applies.
          loadError: null,
        },
        effects: [{ type: 'save_settings', requestId: latestSaveId, settings: event.settings }],
      };
    }

    case 'settings_save_succeeded':
      if (event.requestId !== state.latestSaveId) return { state, effects: [] };
      return { state: { ...state, settings: event.settings, saving: false, saveError: null }, effects: [] };

    case 'settings_save_failed':
      if (event.requestId !== state.latestSaveId) return { state, effects: [] };
      return { state: { ...state, saving: false, saveError: event.error }, effects: [] };

    case 'conversation_state_changed':
      return reduceConversationStateChanged(state, env, event.conversation, event.previousState, event.nextState);

    case 'conversation_snapshot_seeded':
      return { state: rememberBusyState(state, event.conversation, event.state, env.now), effects: [] };

    case 'catchup_scan':
      return reduceCatchupScan(state, env, event.conversations);

    case 'notification_delivery_failed': {
      if (event.attentionKey === null) return { state, effects: [] };
      if (state.attentionSeenByConversationId.get(event.conversationId) !== event.attentionKey) return { state, effects: [] };
      return { state: clearAttentionSeen(state, event.conversationId), effects: [] };
    }

    case 'permission_cue_consumed':
      if (!state.permissionCuePending) return { state, effects: [] };
      return { state: { ...state, permissionCuePending: false }, effects: [] };
  }
}
