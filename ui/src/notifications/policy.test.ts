import { describe, it, expect } from 'vitest';
import type { Conversation } from '../api';
import {
  AGENT_FINISHED_THRESHOLD_MS,
  DEFAULT_NOTIFICATION_SETTINGS,
  type NotificationEnv,
  type NotificationPolicyEvent,
  type NotificationPolicyState,
  initialPolicyState,
  notificationPolicyReducer,
} from './policy';

function conversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: 'conv-1',
    slug: 'conv-a',
    model: 'mock',
    cwd: '/tmp/project',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    message_count: 1,
    browser_session_active: false,
    terminal_uses_tmux: false,
    work_scope_key: 'conversation:conv-1',
    state: { type: 'idle' },
    ...overrides,
  };
}

function env(overrides: Partial<NotificationEnv> = {}): NotificationEnv {
  return {
    now: 0,
    permission: 'granted',
    visible: false,
    hasFocus: false,
    activeSlug: null,
    ...overrides,
  };
}

// Loaded settings + everything enabled — the common precondition for delivery.
function loadedState(): NotificationPolicyState {
  return { ...initialPolicyState(), settingsStatus: 'loaded', settings: DEFAULT_NOTIFICATION_SETTINGS };
}

function run(state: NotificationPolicyState, events: NotificationPolicyEvent[], e: NotificationEnv = env()) {
  let current = state;
  const effects = [];
  for (const event of events) {
    const result = notificationPolicyReducer(current, event, e);
    current = result.state;
    effects.push(...result.effects);
  }
  return { state: current, effects };
}

describe('notification policy reducer', () => {
  it('does not deliver until settings have loaded', () => {
    const { effects } = run(initialPolicyState(), [
      { type: 'conversation_state_changed', conversation: conversation(), previousState: { type: 'idle' }, nextState: { type: 'awaiting_user_response', questions: [] } },
    ]);
    expect(effects).toHaveLength(0);
  });

  it('emits a show effect once settings are loaded', () => {
    const { effects } = run(loadedState(), [
      { type: 'conversation_state_changed', conversation: conversation(), previousState: { type: 'idle' }, nextState: { type: 'awaiting_user_response', questions: [] } },
    ]);
    expect(effects).toHaveLength(1);
    expect(effects[0]).toMatchObject({ type: 'show_browser_notification', title: 'Question asked', tag: 'question_asked:conv-1' });
  });

  it('emits an agent error notification for async creation failure', () => {
    const creationFailed = { type: 'creation_failed' as const, job_id: 'job-1', error: 'boom', error_kind: 'server_error', can_retry: true };
    const { effects } = run(loadedState(), [
      {
        type: 'conversation_state_changed',
        conversation: conversation({ state: creationFailed }),
        previousState: { type: 'provisioning', job_id: 'job-1', message_id: 'msg-1' },
        nextState: creationFailed,
      },
    ]);
    expect(effects).toHaveLength(1);
    expect(effects[0]).toMatchObject({ type: 'show_browser_notification', title: 'Agent error', tag: 'agent_error:conv-1' });
  });

  it('suppresses (and dedupes) when focused on the triggering conversation', () => {
    const focused = env({ visible: true, hasFocus: true, activeSlug: 'conv-a' });
    const first = notificationPolicyReducer(loadedState(),
      { type: 'conversation_state_changed', conversation: conversation(), previousState: { type: 'idle' }, nextState: { type: 'awaiting_user_response', questions: [] } },
      focused);
    expect(first.effects).toHaveLength(0);
    // Focus-suppressed delivery still records attention so catch-up won't refire.
    expect(first.state.attentionSeenByConversationId.get('conv-1')).toBe('conv-1:question_asked');
  });

  it('suppresses when focused by stable conversation id', () => {
    const focused = env({ visible: true, hasFocus: true, activeSlug: 'conv-1' });
    const first = notificationPolicyReducer(loadedState(),
      { type: 'conversation_state_changed', conversation: conversation(), previousState: { type: 'idle' }, nextState: { type: 'awaiting_user_response', questions: [] } },
      focused);
    expect(first.effects).toHaveLength(0);
    expect(first.state.attentionSeenByConversationId.get('conv-1')).toBe('conv-1:question_asked');
  });

  it('queues a permission cue without consuming dedupe when permission is default', () => {
    const result = notificationPolicyReducer(loadedState(),
      { type: 'conversation_state_changed', conversation: conversation(), previousState: { type: 'idle' }, nextState: { type: 'awaiting_user_response', questions: [] } },
      env({ permission: 'default' }));
    expect(result.effects).toHaveLength(0);
    expect(result.state.permissionCuePending).toBe(true);
    expect(result.state.attentionSeenByConversationId.has('conv-1')).toBe(false);
  });

  it('gates agent_finished on the long-task threshold', () => {
    const conv = conversation();
    const below = run(loadedState(), [
      { type: 'conversation_state_changed', conversation: conv, previousState: { type: 'idle' }, nextState: { type: 'llm_requesting', attempt: 1 } },
      { type: 'conversation_state_changed', conversation: conv, previousState: { type: 'llm_requesting', attempt: 1 }, nextState: { type: 'idle' } },
    ], env({ now: AGENT_FINISHED_THRESHOLD_MS - 1 }));
    expect(below.effects).toHaveLength(0);

    // The busy-start time is captured from the env at the busy transition,
    // so we drive the two transitions with different `now` values.
    let state = loadedState();
    state = notificationPolicyReducer(state,
      { type: 'conversation_state_changed', conversation: conv, previousState: { type: 'idle' }, nextState: { type: 'llm_requesting', attempt: 1 } },
      env({ now: 0 })).state;
    const finished = notificationPolicyReducer(state,
      { type: 'conversation_state_changed', conversation: conv, previousState: { type: 'llm_requesting', attempt: 1 }, nextState: { type: 'idle' } },
      env({ now: AGENT_FINISHED_THRESHOLD_MS }));
    expect(finished.effects).toHaveLength(1);
    expect(finished.effects[0]).toMatchObject({ type: 'show_browser_notification', title: 'Agent finished' });
  });

  it('does not fire agent_finished when leaving awaiting_llm', () => {
    const conv = conversation();
    let state = loadedState();
    state = notificationPolicyReducer(state,
      { type: 'conversation_state_changed', conversation: conv, previousState: { type: 'idle' }, nextState: { type: 'awaiting_llm' } },
      env({ now: 0 })).state;
    const result = notificationPolicyReducer(state,
      { type: 'conversation_state_changed', conversation: conv, previousState: { type: 'awaiting_llm' }, nextState: { type: 'idle' } },
      env({ now: AGENT_FINISHED_THRESHOLD_MS * 2 }));
    expect(result.effects).toHaveLength(0);
  });

  it('catch-up dedupes against a prior live delivery for the same blocking state', () => {
    const blocked = conversation({ state: { type: 'awaiting_user_response', questions: [] } });
    const live = notificationPolicyReducer(loadedState(),
      { type: 'conversation_state_changed', conversation: blocked, previousState: { type: 'idle' }, nextState: { type: 'awaiting_user_response', questions: [] } },
      env());
    expect(live.effects).toHaveLength(1);
    const catchup = notificationPolicyReducer(live.state, { type: 'catchup_scan', conversations: [blocked] }, env());
    expect(catchup.effects).toHaveLength(0);
  });

  it('catch-up does not consume dedupe before settings load allows delivery', () => {
    const blocked = conversation({ state: { type: 'context_exhausted', summary: 'full' } });
    const beforeLoad = notificationPolicyReducer(initialPolicyState(), { type: 'catchup_scan', conversations: [blocked] }, env());
    expect(beforeLoad.effects).toHaveLength(0);
    expect(beforeLoad.state.attentionSeenByConversationId.has('conv-1')).toBe(false);

    const loaded = { ...beforeLoad.state, settingsStatus: 'loaded' as const, settings: DEFAULT_NOTIFICATION_SETTINGS };
    const afterLoad = notificationPolicyReducer(loaded, { type: 'catchup_scan', conversations: [blocked] }, env());
    expect(afterLoad.effects).toHaveLength(1);
  });

  it('re-notifies a blocking state after it resolves and recurs', () => {
    const blocked = conversation({ state: { type: 'context_exhausted', summary: 'full' } });
    const first = notificationPolicyReducer(loadedState(), { type: 'catchup_scan', conversations: [blocked] }, env());
    expect(first.effects).toHaveLength(1);
    // context_exhausted -> idle clears the dedupe key for this conversation.
    const resolved = notificationPolicyReducer(first.state,
      { type: 'conversation_state_changed', conversation: blocked, previousState: { type: 'context_exhausted', summary: 'full' }, nextState: { type: 'idle' } },
      env());
    expect(resolved.state.attentionSeenByConversationId.has('conv-1')).toBe(false);
    const recurs = notificationPolicyReducer(resolved.state, { type: 'catchup_scan', conversations: [blocked] }, env());
    expect(recurs.effects).toHaveLength(1);
  });

  it('skips sub-agent conversations on catch-up', () => {
    const result = notificationPolicyReducer(loadedState(), {
      type: 'catchup_scan',
      conversations: [conversation({ parent_conversation_id: 'parent-1', state: { type: 'context_exhausted', summary: 'full' } })],
    }, env());
    expect(result.effects).toHaveLength(0);
  });

  it('does not notify for a continued context_exhausted predecessor', () => {
    const result = notificationPolicyReducer(loadedState(),
      { type: 'conversation_state_changed', conversation: conversation({ continued_in_conv_id: 'next-1' }), previousState: { type: 'idle' }, nextState: { type: 'context_exhausted', summary: 'continued' } },
      env());
    expect(result.effects).toHaveLength(0);
  });

  it('seeding a snapshot remembers busy state without delivering', () => {
    const result = notificationPolicyReducer(loadedState(),
      { type: 'conversation_snapshot_seeded', conversation: conversation(), state: { type: 'awaiting_user_response', questions: [] } },
      env());
    expect(result.effects).toHaveLength(0);
    expect(result.state.attentionSeenByConversationId.has('conv-1')).toBe(false);
  });

  it('rolls back the dedupe mark when browser construction fails', () => {
    const delivered = notificationPolicyReducer(loadedState(),
      { type: 'conversation_state_changed', conversation: conversation(), previousState: { type: 'idle' }, nextState: { type: 'awaiting_user_response', questions: [] } },
      env());
    expect(delivered.state.attentionSeenByConversationId.get('conv-1')).toBe('conv-1:question_asked');
    const rolledBack = notificationPolicyReducer(delivered.state,
      { type: 'notification_delivery_failed', conversationId: 'conv-1', attentionKey: 'conv-1:question_asked' },
      env());
    expect(rolledBack.state.attentionSeenByConversationId.has('conv-1')).toBe(false);
  });

  describe('settings save ordering', () => {
    it('a later save wins; an earlier response cannot overwrite it', () => {
      let state = loadedState();
      const first = notificationPolicyReducer(state, { type: 'settings_save_requested', settings: { ...DEFAULT_NOTIFICATION_SETTINGS, notify_error: false } }, env());
      state = first.state;
      const firstId = (first.effects[0] as { requestId: number }).requestId;
      const second = notificationPolicyReducer(state, { type: 'settings_save_requested', settings: { ...DEFAULT_NOTIFICATION_SETTINGS, notify_idle: false } }, env());
      state = second.state;
      const secondId = (second.effects[0] as { requestId: number }).requestId;
      expect(secondId).toBeGreaterThan(firstId);

      // Stale first response arrives last — it must be ignored.
      state = notificationPolicyReducer(state, { type: 'settings_save_succeeded', requestId: firstId, settings: { ...DEFAULT_NOTIFICATION_SETTINGS, notify_error: false } }, env()).state;
      expect(state.settings.notify_idle).toBe(false);
      expect(state.settings.notify_error).toBe(true);

      state = notificationPolicyReducer(state, { type: 'settings_save_succeeded', requestId: secondId, settings: { ...DEFAULT_NOTIFICATION_SETTINGS, notify_idle: false } }, env()).state;
      expect(state.saving).toBe(false);
      expect(state.settings.notify_idle).toBe(false);
    });

    it('records a save error only for the latest request', () => {
      const result = run(loadedState(), [
        { type: 'settings_save_requested', settings: DEFAULT_NOTIFICATION_SETTINGS },
      ]);
      const requestId = (result.effects[0] as { requestId: number }).requestId;
      const failed = notificationPolicyReducer(result.state, { type: 'settings_save_failed', requestId, error: 'boom' }, env());
      expect(failed.state.saveError).toBe('boom');
      expect(failed.state.saving).toBe(false);
    });
  });

  describe('settings load transitions', () => {
    it('records a load failure error and clears it on retry', () => {
      let state = notificationPolicyReducer(initialPolicyState(), { type: 'settings_load_started' }, env()).state;
      state = notificationPolicyReducer(state, { type: 'settings_load_failed', error: 'offline' }, env()).state;
      expect(state.settingsStatus).toBe('failed');
      expect(state.loadError).toBe('offline');

      // A retry clears the surfaced error while the new fetch is in flight.
      state = notificationPolicyReducer(state, { type: 'settings_load_started' }, env()).state;
      expect(state.settingsStatus).toBe('loading');
      expect(state.loadError).toBeNull();
    });

    it('clears a prior load error once a user edit takes authority', () => {
      let state = notificationPolicyReducer(initialPolicyState(), { type: 'settings_load_started' }, env()).state;
      state = notificationPolicyReducer(state, { type: 'settings_load_failed', error: 'offline' }, env()).state;
      expect(state.loadError).toBe('offline');
      state = notificationPolicyReducer(state, { type: 'settings_save_requested', settings: DEFAULT_NOTIFICATION_SETTINGS }, env()).state;
      expect(state.loadError).toBeNull();
      expect(state.settingsStatus).toBe('loaded');
    });

    it('ignores a stale load result once a save has taken authority', () => {
      let state = notificationPolicyReducer(initialPolicyState(), { type: 'settings_load_started' }, env()).state;
      expect(state.settingsStatus).toBe('loading');
      state = notificationPolicyReducer(state, { type: 'settings_save_requested', settings: { ...DEFAULT_NOTIFICATION_SETTINGS, enabled: false } }, env()).state;
      expect(state.settingsStatus).toBe('loaded');
      // Server load that was already in flight resolves afterward — discard it.
      state = notificationPolicyReducer(state, { type: 'settings_loaded', settings: DEFAULT_NOTIFICATION_SETTINGS }, env()).state;
      expect(state.settings.enabled).toBe(false);
    });
  });
});
