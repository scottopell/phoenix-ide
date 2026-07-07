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

  it('updates only cached PR when same-timestamp snapshot changes cached PR', () => {
    const store = new ConversationStore();
    const base = makeConv('alpha', {
      updated_at: '2024-06-01T00:00:00Z',
      browser_session_active: true,
    });
    const withPrAndStaleLiveFlag = makeConv('alpha', {
      updated_at: '2024-06-01T00:00:00Z',
      browser_session_active: false,
      cached_pr: {
        number: 12,
        title: 'Cached PR',
        url: 'https://example.test/pr/12',
        display_state: 'open',
        base: 'main',
        head: 'feature',
      },
    });

    expect(store.upsertSnapshot('alpha', base)).toBe(true);
    expect(store.upsertSnapshot('alpha', withPrAndStaleLiveFlag)).toBe(true);
    const row = store.getSnapshot('alpha').conversation;
    expect(row?.cached_pr?.number).toBe(12);
    expect(row?.browser_session_active).toBe(true);
  });

  it('clears only cached PR when same-timestamp snapshot drops cached PR', () => {
    const store = new ConversationStore();
    const withPr = makeConv('alpha', {
      updated_at: '2024-06-01T00:00:00Z',
      browser_session_active: true,
      cached_pr: {
        number: 12,
        title: 'Cached PR',
        url: 'https://example.test/pr/12',
        display_state: 'open',
        base: 'main',
        head: 'feature',
      },
    });
    const withoutPrAndStaleLiveFlag = makeConv('alpha', {
      updated_at: '2024-06-01T00:00:00Z',
      browser_session_active: false,
    });

    expect(store.upsertSnapshot('alpha', withPr)).toBe(true);
    expect(store.upsertSnapshot('alpha', withoutPrAndStaleLiveFlag)).toBe(true);
    const row = store.getSnapshot('alpha').conversation;
    expect(row?.cached_pr).toBeUndefined();
    expect(row?.browser_session_active).toBe(true);
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
    // must not throw away `messages`, `lastSequenceId`,
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

  it('moves a conversation to its current slug instead of keeping duplicate id snapshots', () => {
    const store = new ConversationStore();
    const original = makeConv('old-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    });
    const renamed = makeConv('new-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
      cwd: '/renamed',
    });

    expect(store.upsertSnapshot('old-slug', original)).toBe(true);
    expect(store.upsertSnapshot('new-slug', renamed)).toBe(true);

    expect(store.slugForId('conv-shared')).toBe('new-slug');
    expect(store.getSnapshot('old-slug').conversation).toBeNull();
    expect(store.getSnapshot('new-slug').conversation).toMatchObject({
      id: 'conv-shared',
      slug: 'new-slug',
      cwd: '/renamed',
    });
    expect(store.listSnapshots().map((c) => c.id)).toEqual(['conv-shared']);
  });

  it('preserves destination live atom fields when reconciling an old indexed alias', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('old-slug', makeConv('old-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }));
    store.dispatch('new-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('new-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [{
        message_id: 'msg-1',
        sequence_id: 1,
        conversation_id: 'conv-shared',
        message_type: 'user',
        content: { text: 'keep me' },
        created_at: '2024-06-01T00:00:00Z',
      } as never],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.upsertSnapshot('new-slug', makeConv('new-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
      cwd: '/fresh',
    }))).toBe(true);

    expect(store.getSnapshot('new-slug').messages).toHaveLength(1);
    expect(store.getSnapshot('new-slug').conversation?.cwd).toBe('/fresh');
    expect(store.getSnapshot('old-slug').conversation).toBeNull();
  });

  it('merges same-timestamp cached PR alias data into the canonical slug', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('canonical-slug', makeConv('canonical-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }));

    expect(store.upsertSnapshot('stale-alias', makeConv('stale-alias', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
      cached_pr: {
        number: 12,
        title: 'Cached PR',
        url: 'https://example.test/pr/12',
        display_state: 'open',
        base: 'main',
        head: 'feature',
      },
    }))).toBe(true);

    expect(store.slugForId('conv-shared')).toBe('canonical-slug');
    expect(store.getSnapshot('canonical-slug').conversation).toMatchObject({
      slug: 'canonical-slug',
      cached_pr: { number: 12 },
    });
    expect(store.getSnapshot('stale-alias').conversation).toBeNull();
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['canonical-slug']);
  });

  it('rejects same-timestamp stale aliases that do not carry fresher data', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('canonical-slug', makeConv('canonical-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }));

    expect(store.upsertSnapshot('stale-alias', makeConv('stale-alias', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }))).toBe(false);

    expect(store.slugForId('conv-shared')).toBe('canonical-slug');
    expect(store.getSnapshot('canonical-slug').conversation?.slug).toBe('canonical-slug');
    expect(store.getSnapshot('stale-alias').conversation).toBeNull();
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['canonical-slug']);
  });

  it('allows same-timestamp slug moves from authoritative snapshots', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('old-slug', makeConv('old-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }));

    expect(store.upsertSnapshot('new-slug', makeConv('new-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }), { allowEqualTimestampSlugMove: true })).toBe(true);

    expect(store.slugForId('conv-shared')).toBe('new-slug');
    expect(store.getSnapshot('old-slug').conversation).toBeNull();
    expect(store.getSnapshot('new-slug').conversation?.slug).toBe('new-slug');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['new-slug']);
  });

  it('does not use a stale indexed slug as the cached PR merge destination', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('shared-slug', makeConv('shared-slug', {
      id: 'conv-old',
      updated_at: '2024-06-01T00:00:00Z',
    }));
    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-old',
      conversation: makeConv('old-slug', {
        id: 'conv-old',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });
    store.dispatch('shared-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-new',
      conversation: makeConv('shared-slug', {
        id: 'conv-new',
        updated_at: '2024-06-02T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.upsertSnapshot('cache-alias', makeConv('cache-alias', {
      id: 'conv-old',
      updated_at: '2024-06-01T00:00:00Z',
      cached_pr: {
        number: 12,
        title: 'Cached PR',
        url: 'https://example.test/pr/12',
        display_state: 'open',
        base: 'main',
        head: 'feature',
      },
    }))).toBe(true);

    expect(store.getSnapshot('shared-slug').conversation?.id).toBe('conv-new');
    expect(store.getSnapshot('old-slug').conversation).toMatchObject({
      id: 'conv-old',
      cached_pr: { number: 12 },
    });
    expect(store.slugForId('conv-old')).toBe('old-slug');
    expect(store.slugForId('conv-new')).toBe('shared-slug');
  });

  it('rejects stale aliases without moving the canonical newer snapshot', () => {
    const store = new ConversationStore();
    const fresh = makeConv('current-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
      cwd: '/fresh',
    });
    const staleAlias = makeConv('old-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
      cwd: '/stale',
    });

    store.upsertSnapshot('current-slug', fresh);
    expect(store.upsertSnapshot('old-slug', staleAlias)).toBe(false);

    expect(store.slugForId('conv-shared')).toBe('current-slug');
    expect(store.getSnapshot('current-slug').conversation?.cwd).toBe('/fresh');
    expect(store.getSnapshot('old-slug').conversation).toBeNull();
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['current-slug']);
  });

  it('reconciles same-id atoms created by direct page hydration', () => {
    const store = new ConversationStore();
    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('old-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.slugForId('conv-shared')).toBe('old-slug');
    expect(store.upsertSnapshot('new-slug', makeConv('new-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('old-slug').conversation?.slug).toBe('old-slug');
    expect(store.getSnapshot('new-slug').conversation?.id).toBe('conv-shared');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['new-slug']);
  });

  it('does not drop a live old-slug atom while making the new slug canonical for the sidebar', () => {
    const store = new ConversationStore();
    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('old-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [{
        message_id: 'msg-1',
        sequence_id: 1,
        conversation_id: 'conv-shared',
        message_type: 'user',
        content: { text: 'active route' },
        created_at: '2024-06-01T00:00:00Z',
      } as never],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.upsertSnapshot('new-slug', makeConv('new-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('old-slug').messages).toHaveLength(1);
    expect(store.getSnapshot('old-slug').conversation?.slug).toBe('old-slug');
    expect(store.slugForId('conv-shared')).toBe('new-slug');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['new-slug']);
  });

  it('does not drop an empty hydrated old-slug route atom during alias cleanup', () => {
    const store = new ConversationStore();
    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('old-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.upsertSnapshot('new-slug', makeConv('new-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('old-slug').conversationId).toBe('conv-shared');
    expect(store.getSnapshot('old-slug').conversation?.slug).toBe('old-slug');
    expect(store.slugForId('conv-shared')).toBe('new-slug');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['new-slug']);
  });

  it('does not drop a subscribed snapshot row before page hydration completes', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('old-slug', makeConv('old-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }));
    const unsubscribe = store.subscribe('old-slug', () => {});

    expect(store.upsertSnapshot('new-slug', makeConv('new-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('old-slug').conversation?.slug).toBe('old-slug');
    expect(store.getSnapshot('old-slug').conversationId).toBeNull();
    expect(store.slugForId('conv-shared')).toBe('new-slug');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['new-slug']);
    unsubscribe();
  });

  it('notifies list subscribers when fresher direct hydration changes the alias index', () => {
    const store = new ConversationStore();
    const snapshots: string[][] = [];
    const unsubscribe = store.subscribeAny(() => {
      snapshots.push(store.listSnapshots().map((c) => c.slug));
    });

    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('old-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });
    store.dispatch('new-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('new-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-02T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    unsubscribe();
    expect(snapshots).toContainEqual(['new-slug']);
    expect(snapshots.at(-1)).toEqual(['new-slug']);
  });

  it('does not let equal-timestamp direct hydration take over the canonical slug', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('canonical-slug', makeConv('canonical-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T00:00:00Z',
    }));

    store.dispatch('stale-alias', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('stale-alias', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.slugForId('conv-shared')).toBe('canonical-slug');
    expect(store.getSnapshot('stale-alias').conversation?.slug).toBe('stale-alias');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['canonical-slug']);
  });

  it('removes all inactive aliases for a conversation id before inserting the canonical row', () => {
    const store = new ConversationStore();
    for (const slug of ['alias-a', 'alias-b', 'alias-c']) {
      store.upsertSnapshot(slug, makeConv(slug, {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }));
    }

    expect(store.upsertSnapshot('canonical', makeConv('canonical', {
      id: 'conv-shared',
      updated_at: '2024-06-02T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('alias-a').conversation).toBeNull();
    expect(store.getSnapshot('alias-b').conversation).toBeNull();
    expect(store.getSnapshot('alias-c').conversation).toBeNull();
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['canonical']);
  });

  it('listSnapshots dedupes same-id atoms as a final render-path guard', () => {
    const store = new ConversationStore();
    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('old-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });
    store.dispatch('new-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('new-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-02T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['new-slug']);
  });

  it('direct hydration updates the id index when it observes a fresher alias', () => {
    const store = new ConversationStore();
    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('old-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });
    store.dispatch('new-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-shared',
      conversation: makeConv('new-slug', {
        id: 'conv-shared',
        updated_at: '2024-06-02T00:00:00Z',
      }),
      messages: [],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.slugForId('conv-shared')).toBe('new-slug');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['new-slug']);
    expect(store.upsertSnapshot('cache-slug', makeConv('cache-slug', {
      id: 'conv-shared',
      updated_at: '2024-06-01T12:00:00Z',
    }))).toBe(false);
    expect(store.getSnapshot('cache-slug').conversation).toBeNull();
  });

  it('does not remove a slug that has been reused by a different conversation id', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('shared-slug', makeConv('shared-slug', {
      id: 'conv-old',
      updated_at: '2024-06-01T00:00:00Z',
    }));
    store.upsertSnapshot('shared-slug', makeConv('shared-slug', {
      id: 'conv-new',
      updated_at: '2024-06-02T00:00:00Z',
    }));

    expect(store.slugForId('conv-old')).toBeUndefined();
    expect(store.slugForId('conv-new')).toBe('shared-slug');
    expect(store.upsertSnapshot('fresh-slug', makeConv('fresh-slug', {
      id: 'conv-old',
      updated_at: '2024-06-03T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('shared-slug').conversation?.id).toBe('conv-new');
    expect(store.getSnapshot('fresh-slug').conversation?.id).toBe('conv-old');
    expect(store.listSnapshots().map((c) => c.id).sort()).toEqual(['conv-new', 'conv-old']);
  });

  it('rejects a stale different-id cache row for an occupied reused slug', () => {
    const store = new ConversationStore();
    store.dispatch('shared-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-new',
      conversation: makeConv('shared-slug', {
        id: 'conv-new',
        updated_at: '2024-06-02T00:00:00Z',
      }),
      messages: [{
        message_id: 'msg-1',
        sequence_id: 1,
        conversation_id: 'conv-new',
        message_type: 'user',
        content: { text: 'new conversation state' },
        created_at: '2024-06-02T00:00:00Z',
      } as never],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.upsertSnapshot('shared-slug', makeConv('shared-slug', {
      id: 'conv-old',
      updated_at: '2024-06-01T00:00:00Z',
    }))).toBe(false);

    expect(store.getSnapshot('shared-slug').conversation?.id).toBe('conv-new');
    expect(store.getSnapshot('shared-slug').messages).toHaveLength(1);
    expect(store.slugForId('conv-new')).toBe('shared-slug');
    expect(store.slugForId('conv-old')).toBeUndefined();
  });

  it('removeByConversationId drops every alias for the deleted conversation', () => {
    const store = new ConversationStore();
    store.dispatch('old-slug', {
      type: 'set_initial_data',
      conversationId: 'conv-doomed',
      conversation: makeConv('old-slug', {
        id: 'conv-doomed',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [{
        message_id: 'msg-1',
        sequence_id: 1,
        conversation_id: 'conv-doomed',
        message_type: 'user',
        content: { text: 'active route' },
        created_at: '2024-06-01T00:00:00Z',
      } as never],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });
    store.upsertSnapshot('new-slug', makeConv('new-slug', {
      id: 'conv-doomed',
      updated_at: '2024-06-02T00:00:00Z',
    }));
    store.upsertSnapshot('other-slug', makeConv('other-slug', {
      id: 'conv-other',
      updated_at: '2024-06-02T00:00:00Z',
    }));

    expect(store.removeByConversationId('conv-doomed').sort()).toEqual(['new-slug', 'old-slug']);

    expect(store.slugForId('conv-doomed')).toBeUndefined();
    expect(store.getSnapshot('old-slug').conversation).toBeNull();
    expect(store.getSnapshot('new-slug').conversation).toBeNull();
    expect(store.getSnapshot('other-slug').conversation?.id).toBe('conv-other');
    expect(store.listSnapshots().map((c) => c.id)).toEqual(['conv-other']);
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

  it('replaceSlugSnapshot moves a renamed conversation to its new slug', () => {
    const store = new ConversationStore();
    const original = makeConv('alpha', {
      id: 'conv-1',
      updated_at: '2024-06-01T00:00:00Z',
    });
    const renamed = makeConv('beta', {
      id: 'conv-1',
      updated_at: '2024-06-02T00:00:00Z',
    });
    store.upsertSnapshot('alpha', original);

    expect(store.replaceSlugSnapshot('alpha', renamed)).toBe(true);

    expect(store.getSnapshot('alpha').conversation).toBeNull();
    expect(store.getSnapshot('beta').conversation?.id).toBe('conv-1');
    expect(store.slugForId('conv-1')).toBe('beta');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['beta']);
  });

  it('replaceSlugSnapshot allows same-timestamp slug-only local renames', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('alpha', makeConv('alpha', {
      id: 'conv-1',
      updated_at: '2024-06-01T00:00:00Z',
    }));

    expect(store.replaceSlugSnapshot('alpha', makeConv('beta', {
      id: 'conv-1',
      updated_at: '2024-06-01T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('alpha').conversation).toBeNull();
    expect(store.getSnapshot('beta').conversation?.id).toBe('conv-1');
    expect(store.slugForId('conv-1')).toBe('beta');
    expect(store.listSnapshots().map((c) => c.slug)).toEqual(['beta']);
  });

  it('replaceSlugSnapshot drops old live state after a local rename navigation', () => {
    const store = new ConversationStore();
    store.dispatch('alpha', {
      type: 'set_initial_data',
      conversationId: 'conv-1',
      conversation: makeConv('alpha', {
        id: 'conv-1',
        updated_at: '2024-06-01T00:00:00Z',
      }),
      messages: [{
        message_id: 'msg-1',
        sequence_id: 1,
        conversation_id: 'conv-1',
        message_type: 'user',
        content: { text: 'old live state' },
        created_at: '2024-06-01T00:00:00Z',
      } as never],
      phase: { type: 'idle' },
      contextWindow: { used: 0 },
    });

    expect(store.replaceSlugSnapshot('alpha', makeConv('beta', {
      id: 'conv-1',
      updated_at: '2024-06-02T00:00:00Z',
    }))).toBe(true);

    expect(store.getSnapshot('alpha').conversation).toBeNull();
    expect(store.getSnapshot('alpha').conversationId).toBeNull();
    expect(store.getSnapshot('alpha').messages).toEqual([]);
    expect(store.getSnapshot('beta').conversation?.id).toBe('conv-1');
    expect(store.slugForId('conv-1')).toBe('beta');
  });

  it('replaceSlugSnapshot does not remove an old slug that belongs to a different conversation', () => {
    const store = new ConversationStore();
    store.upsertSnapshot('alpha', makeConv('alpha', { id: 'conv-alpha' }));
    const renamed = makeConv('beta', {
      id: 'conv-beta',
      updated_at: '2024-06-02T00:00:00Z',
    });

    store.replaceSlugSnapshot('alpha', renamed);

    expect(store.getSnapshot('alpha').conversation?.id).toBe('conv-alpha');
    expect(store.getSnapshot('beta').conversation?.id).toBe('conv-beta');
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
