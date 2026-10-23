import { describe, expect, it, vi } from 'vitest';
import { ConversationStore } from '../../conversation/ConversationStore';
import type { Conversation } from '../../api';

const conversation = (id: string, slug: string): Conversation => ({
  id, slug, model: 'fixture', cwd: '/tmp',
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z', message_count: 0,
} as Conversation);

// Test adapter for the proposed observer's non-creating identity read, not
// a substitute for testing the eventual observer's phase/epoch receipt logic.
function readLive(store: ConversationStore, id: string) {
  const key = store.slugForId(id);
  if (key === undefined) return undefined;
  const atom = store.peekSnapshot(key);
  return atom?.conversation?.id === id ? atom : undefined;
}

describe('non-creating observer snapshot read', () => {
  it('does not initialize missing atoms or notify subscribers', () => {
    const store = new ConversationStore();
    const notify = vi.fn();
    const stop = store.subscribeAny(notify);
    expect(store.peekSnapshot('missing')).toBeUndefined();
    expect(store.peekSnapshot('missing')).toBeUndefined();
    expect(store.listSnapshots()).toEqual([]);
    expect(notify).not.toHaveBeenCalled();
    stop();
  });

  it('does not recreate a deleted atom during the stale-index callback', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('old', conversation('live', 'old'));
    const observations: Array<{ key: string | undefined; exists: boolean }> = [];
    const stop = store.subscribeAny(() => {
      const key = store.slugForId('live');
      observations.push({ key, exists: key !== undefined && store.peekSnapshot(key) !== undefined });
      expect(readLive(store, 'live')).toBeUndefined();
    });
    store.removeByConversationId('live');
    expect(observations).toContainEqual({ key: 'old', exists: false });
    expect(store.peekSnapshot('old')).toBeUndefined();
    expect(store.slugForId('live')).toBeUndefined();
    stop();
  });

  it('follows rekeys without recreating old keys or accepting another identity', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('old', conversation('live', 'old'));
    store.replaceSlugSnapshot('old', conversation('live', 'new'));
    expect(store.peekSnapshot('old')).toBeUndefined();
    expect(store.slugForId('live')).toBe('new');
    expect(readLive(store, 'live')?.conversation?.id).toBe('live');
    expect(readLive(store, 'other')).toBeUndefined();
    expect(store.peekSnapshot('other')).toBeUndefined();
  });

  it('unsubscribes the observation callback without mutating stored state', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('live-key', conversation('live', 'live-key'));
    const before = store.peekSnapshot('live-key');
    const observe = vi.fn(() => readLive(store, 'live'));
    const stop = store.subscribeAny(observe);
    stop();
    expect(store.peekSnapshot('live-key')).toBe(before);
    store.removeByConversationId('live');
    expect(observe).not.toHaveBeenCalled();
    expect(store.peekSnapshot('live-key')).toBeUndefined();
  });
});
