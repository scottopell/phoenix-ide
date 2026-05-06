import { describe, it, expect, vi } from 'vitest';
import { ConversationStore } from './ConversationStore';
import type { Conversation } from '../api';

function makeConv(
  slug: string,
  overrides: Partial<Conversation> = {},
): Conversation {
  return {
    id: `conv-${slug}`,
    slug,
    model: 'claude-3-5-sonnet',
    cwd: '/tmp',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:00Z',
    message_count: 0,
    ...overrides,
  } as Conversation;
}

describe('ConversationStore.upsertSnapshot (task 08684)', () => {
  it('creates a snapshot-only atom on first upsert', () => {
    const store = new ConversationStore();
    const conv = makeConv('alpha');
    const changed = store.upsertSnapshot('alpha', conv);
    expect(changed).toBe(true);
    const atom = store.getSnapshot('alpha');
    expect(atom.conversation).toBe(conv);
    // Snapshot-only: no SSE-driven fields populated yet.
    expect(atom.messages).toEqual([]);
    expect(atom.connectionEpoch).toBeNull();
    expect(atom.lastSequenceId).toBe(0);
  });

  it('is a no-op when the row is identical to the held one', () => {
    const store = new ConversationStore();
    const conv = makeConv('alpha', { updated_at: '2024-06-01T00:00:00Z' });
    expect(store.upsertSnapshot('alpha', conv)).toBe(true);
    expect(store.upsertSnapshot('alpha', conv)).toBe(false);
  });

  it('updates conversation when updated_at advances', () => {
    const store = new ConversationStore();
    const v1 = makeConv('alpha', { updated_at: '2024-06-01T00:00:00Z' });
    const v2 = makeConv('alpha', { updated_at: '2024-06-02T00:00:00Z', cwd: '/new' });
    store.upsertSnapshot('alpha', v1);
    const changed = store.upsertSnapshot('alpha', v2);
    expect(changed).toBe(true);
    expect(store.getSnapshot('alpha').conversation?.cwd).toBe('/new');
  });

  it('refuses to regress on a stale row (cache-clobber guard)', () => {
    // Panel concurrency finding #4: stale cache hydration must not
    // overwrite a fresher server response. The poller should call
    // upsertSnapshot after the cache hydration, but if the order is
    // reversed (or the cache is older than what SSE has already pushed
    // into the store) the upsert path must be safe.
    const store = new ConversationStore();
    const fresh = makeConv('alpha', { updated_at: '2024-06-02T00:00:00Z', cwd: '/fresh' });
    const stale = makeConv('alpha', { updated_at: '2024-06-01T00:00:00Z', cwd: '/stale' });
    store.upsertSnapshot('alpha', fresh);
    const changed = store.upsertSnapshot('alpha', stale);
    expect(changed).toBe(false);
    expect(store.getSnapshot('alpha').conversation?.cwd).toBe('/fresh');
  });

  it('preserves SSE-driven fields when upserting a newer snapshot', () => {
    // The cardinal invariant: a polling tick that arrives mid-stream
    // must not throw away `messages`, `breadcrumbs`, `lastSequenceId`,
    // `connectionEpoch`, etc. The upsert path mutates only
    // `atom.conversation`.
    const store = new ConversationStore();
    store.upsertSnapshot('alpha', makeConv('alpha', { updated_at: '2024-06-01T00:00:00Z' }));
    // Simulate live SSE state as if a ConversationPage had mounted.
    store.dispatch('alpha', { type: 'connection_opened', epoch: 1 });
    store.dispatch('alpha', {
      type: 'sse_message',
      epoch: 1,
      sequenceId: 5,
      message: {
        message_id: 'msg-1',
        sequence_id: 5,
        conversation_id: 'conv-alpha',
        message_type: 'agent',
        content: { text: 'hello' },
        created_at: '2024-06-01T00:00:00Z',
      } as never,
    });
    expect(store.getSnapshot('alpha').messages).toHaveLength(1);

    // Polling tick arrives with a slightly newer updated_at.
    store.upsertSnapshot('alpha', makeConv('alpha', { updated_at: '2024-06-02T00:00:00Z' }));
    const after = store.getSnapshot('alpha');
    // Conversation row updated.
    expect(after.conversation?.updated_at).toBe('2024-06-02T00:00:00Z');
    // SSE-derived fields untouched.
    expect(after.messages).toHaveLength(1);
    expect(after.connectionEpoch).toBe(1);
    expect(after.lastSequenceId).toBe(5);
  });

  it('upsertSnapshots returns the slugs that actually changed', () => {
    const store = new ConversationStore();
    const a = makeConv('alpha', { updated_at: '2024-06-01T00:00:00Z' });
    const b = makeConv('beta', { updated_at: '2024-06-01T00:00:00Z' });
    expect(store.upsertSnapshots([a, b])).toEqual(['alpha', 'beta']);
    // Same rows again — no-ops.
    expect(store.upsertSnapshots([a, b])).toEqual([]);
    // One advances.
    const aPrime = makeConv('alpha', { updated_at: '2024-06-02T00:00:00Z' });
    expect(store.upsertSnapshots([aPrime, b])).toEqual(['alpha']);
  });

  it('listSnapshots returns every conversation currently held', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('alpha', makeConv('alpha'));
    store.upsertSnapshot('beta', makeConv('beta'));
    // Atom for 'gamma' was observed via getSnapshot but never upserted
    // — no `conversation` row, must not appear in listSnapshots.
    store.getSnapshot('gamma');
    const list = store.listSnapshots();
    const slugs = list.map((c) => c.slug).sort();
    expect(slugs).toEqual(['alpha', 'beta']);
  });

  it('slugForId resolves conversation_id back to slug', () => {
    const store = new ConversationStore();
    const a = makeConv('alpha');
    store.upsertSnapshot('alpha', a);
    expect(store.slugForId(a.id)).toBe('alpha');
    expect(store.slugForId('unknown')).toBeUndefined();
  });

  it('remove drops the atom and clears the slugForId index', () => {
    const store = new ConversationStore();
    const a = makeConv('alpha');
    store.upsertSnapshot('alpha', a);
    store.remove('alpha');
    expect(store.slugForId(a.id)).toBeUndefined();
    // After remove, getSnapshot creates a fresh initial atom (no
    // conversation field).
    expect(store.getSnapshot('alpha').conversation).toBeNull();
  });

  it('subscribeAny fires for any atom mutation', () => {
    const store = new ConversationStore();
    const listener = vi.fn();
    store.subscribeAny(listener);
    store.upsertSnapshot('alpha', makeConv('alpha'));
    expect(listener).toHaveBeenCalledTimes(1);
    store.upsertSnapshot('beta', makeConv('beta'));
    expect(listener).toHaveBeenCalledTimes(2);
    // No-op upsert does not fire.
    store.upsertSnapshot('alpha', makeConv('alpha'));
    expect(listener).toHaveBeenCalledTimes(2);
  });
});
