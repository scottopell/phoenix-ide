import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Conversation } from './api';
import {
  AGENT_FINISHED_THRESHOLD_MS,
  DEFAULT_NOTIFICATION_SETTINGS,
  closeNotificationsForConversation,
  notifyCatchUp,
  notifyConversationStateChange,
  notifyConversationSnapshotChange,
  resetNotificationRuntimeForTest,
} from './notifications';

const notifications: MockNotification[] = [];

class MockNotification {
  static permission: NotificationPermission = 'granted';
  onclick: (() => void) | null = null;
  onclose: (() => void) | null = null;
  closeCalls = 0;

  constructor(public title: string, public options?: NotificationOptions) {
    notifications.push(this);
  }

  close() {
    this.closeCalls++;
    this.onclose?.();
  }
}

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

function grantSettings() {
  resetNotificationRuntimeForTest(DEFAULT_NOTIFICATION_SETTINGS);
}

beforeEach(() => {
  notifications.length = 0;
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));
  resetNotificationRuntimeForTest();
  Object.defineProperty(window, 'Notification', {
    configurable: true,
    writable: true,
    value: MockNotification,
  });
  Object.defineProperty(document, 'visibilityState', {
    configurable: true,
    get: () => 'hidden',
  });
  vi.spyOn(document, 'hasFocus').mockReturnValue(false);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('browser desktop notifications', () => {
  it('does not deliver before server settings have loaded', () => {
    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );

    expect(notifications).toHaveLength(0);
  });

  it('suppresses when focused on the triggering conversation', () => {
    grantSettings();
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => 'visible',
    });
    vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    window.history.replaceState(null, '', '/c/conv-a');

    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );

    expect(notifications).toHaveLength(0);
  });

  it('notifies for a different conversation even when Phoenix is focused', () => {
    grantSettings();
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => 'visible',
    });
    vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    window.history.replaceState(null, '', '/c/conv-b');

    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );

    expect(notifications).toHaveLength(1);
    expect(notifications[0]?.title).toBe('Question asked');
  });

  it('only sends agent-finished after the long-task threshold', () => {
    grantSettings();
    const conv = conversation({ updated_at: '2026-01-01T00:00:31Z' });

    notifyConversationStateChange(conv, { type: 'idle' }, { type: 'llm_requesting', attempt: 1 });
    vi.advanceTimersByTime(AGENT_FINISHED_THRESHOLD_MS - 1);
    notifyConversationStateChange(conv, { type: 'llm_requesting', attempt: 1 }, { type: 'idle' });
    expect(notifications).toHaveLength(0);

    notifyConversationStateChange(conv, { type: 'idle' }, { type: 'llm_requesting', attempt: 1 });
    vi.advanceTimersByTime(AGENT_FINISHED_THRESHOLD_MS);
    notifyConversationStateChange(conv, { type: 'llm_requesting', attempt: 1 }, { type: 'idle' });
    expect(notifications).toHaveLength(1);
    expect(notifications[0]?.title).toBe('Agent finished');
    expect(notifications[0]?.options?.tag).toBe('agent_finished:conv-1:2026-01-01T00:00:31Z');
  });

  it('catch-up dedupes blocking events by unresolved conversation state', () => {
    grantSettings();
    const blocked = conversation({ state: { type: 'context_exhausted', summary: 'full' } });

    notifyCatchUp([blocked]);
    notifyCatchUp([blocked]);
    expect(notifications).toHaveLength(1);

    notifyCatchUp([conversation({
      state: { type: 'context_exhausted', summary: 'still full' },
      updated_at: '2026-01-01T00:01:00Z',
    })]);
    expect(notifications).toHaveLength(1);

    notifyConversationStateChange(blocked, { type: 'context_exhausted', summary: 'full' }, { type: 'idle' });
    notifyCatchUp([conversation({
      state: { type: 'context_exhausted', summary: 'full again' },
      updated_at: '2026-01-01T00:02:00Z',
    })]);
    expect(notifications).toHaveLength(2);
  });

  it('live notification dedupes the following catch-up pass for the same blocking state', () => {
    grantSettings();
    const blocked = conversation({ state: { type: 'awaiting_user_response', questions: [] } });

    notifyConversationStateChange(blocked, { type: 'idle' }, { type: 'awaiting_user_response', questions: [] });
    notifyCatchUp([blocked]);

    expect(notifications).toHaveLength(1);
  });

  it('catch-up does not consume dedupe before settings load allows delivery', () => {
    const blocked = conversation({ state: { type: 'context_exhausted', summary: 'full' } });

    notifyCatchUp([blocked]);
    expect(notifications).toHaveLength(0);

    grantSettings();
    notifyCatchUp([blocked]);
    expect(notifications).toHaveLength(1);
  });

  it('notification construction failures fail closed', () => {
    grantSettings();
    Object.defineProperty(window, 'Notification', {
      configurable: true,
      writable: true,
      value: class ThrowingNotification {
        static permission: NotificationPermission = 'granted';
        constructor() { throw new Error('blocked'); }
      },
    });

    expect(() => notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    )).not.toThrow();
  });

  it('suppresses continued context-exhausted predecessors', () => {
    grantSettings();

    notifyConversationStateChange(
      conversation({ continued_in_conv_id: 'next-1' }),
      { type: 'idle' },
      { type: 'context_exhausted', summary: 'continued' },
    );

    expect(notifications).toHaveLength(0);
  });

  it('live notification dedupes the following snapshot transition for the same blocking state', () => {
    grantSettings();
    const blocked = conversation({ state: { type: 'awaiting_user_response', questions: [] } });

    notifyConversationStateChange(blocked, { type: 'idle' }, { type: 'awaiting_user_response', questions: [] });
    notifyConversationSnapshotChange(blocked);
    notifyConversationSnapshotChange({ ...blocked, updated_at: '2026-01-01T00:01:00Z' });

    expect(notifications).toHaveLength(1);
  });

  it('catch-up skips sub-agent conversations', () => {
    grantSettings();
    notifyCatchUp([
      conversation({
        parent_conversation_id: 'parent-1',
        state: { type: 'context_exhausted', summary: 'full' },
      }),
    ]);

    expect(notifications).toHaveLength(0);
  });

  it('closes delivered notifications when their conversation is acknowledged without clicking', () => {
    grantSettings();

    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );
    closeNotificationsForConversation('conv-1');

    expect(notifications).toHaveLength(1);
    expect(notifications[0]?.closeCalls).toBe(1);
  });

  it('does not close notifications for other conversations when one conversation is acknowledged', () => {
    grantSettings();

    notifyConversationStateChange(
      conversation({ id: 'conv-1', slug: 'conv-a' }),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );
    notifyConversationStateChange(
      conversation({ id: 'conv-2', slug: 'conv-b' }),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );
    closeNotificationsForConversation('conv-1');

    expect(notifications).toHaveLength(2);
    expect(notifications[0]?.closeCalls).toBe(1);
    expect(notifications[1]?.closeCalls).toBe(0);
  });

  it('treats repeated conversation acknowledgement as a no-op', () => {
    grantSettings();

    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );
    closeNotificationsForConversation('conv-1');
    closeNotificationsForConversation('conv-1');

    expect(notifications).toHaveLength(1);
    expect(notifications[0]?.closeCalls).toBe(1);
  });

  it('closes a previous live notification before replacing the same tag', () => {
    grantSettings();

    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );
    notifyConversationStateChange(
      conversation(),
      { type: 'awaiting_user_response', questions: [] },
      { type: 'idle' },
    );
    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );

    expect(notifications).toHaveLength(2);
    expect(notifications[0]?.closeCalls).toBe(1);
    expect(notifications[1]?.closeCalls).toBe(0);

    closeNotificationsForConversation('conv-1');

    expect(notifications[0]?.closeCalls).toBe(1);
    expect(notifications[1]?.closeCalls).toBe(1);
  });

  it('notification clicks acknowledge the triggering conversation through the shared path', () => {
    grantSettings();
    const dispatch = vi.spyOn(window, 'dispatchEvent');

    notifyConversationStateChange(
      conversation(),
      { type: 'idle' },
      { type: 'awaiting_user_response', questions: [] },
    );
    notifications[0]?.onclick?.();
    closeNotificationsForConversation('conv-1');

    expect(notifications[0]?.closeCalls).toBe(1);
    expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({ type: 'phoenix:navigate-to-conversation' }));
  });
});
