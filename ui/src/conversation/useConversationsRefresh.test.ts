import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ConversationStore } from './ConversationStore';
import { DraftStore } from './DraftStore';
import type { Conversation } from '../api';
import { cacheDB } from '../cache';
import { getLastViewer, setLastViewer } from '../storage/lastViewerStorage';

function makeConv(slug: string, overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: `conv-${slug}`,
    slug,
    model: 'claude-3-5-sonnet',
    cwd: '/tmp',
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:00Z',
    message_count: 0,
    archived: false,
    ...overrides,
  } as Conversation;
}

// Defer-resolving mocks so the test can interleave a second `refreshOnce`
// call while the first is still in flight.
let activePromises: Array<{
  resolve: (rows: Conversation[]) => void;
  reject: (err: unknown) => void;
}> = [];

function makeDeferred() {
  let resolve!: (rows: Conversation[]) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<Conversation[]>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const listConversations = vi.fn((): Promise<Conversation[]> => {
  const d = makeDeferred();
  activePromises.push({ resolve: d.resolve, reject: d.reject });
  return d.promise;
});
const listArchivedConversations = vi.fn(
  (): Promise<Conversation[]> => Promise.resolve([]),
);
const getConversationSlug = vi.fn((id: string): Promise<string | null> => Promise.resolve(id));

vi.mock('../api', () => ({
  api: {
    listConversations: () => listConversations(),
    listArchivedConversations: () => listArchivedConversations(),
    getConversationSlug: (id: string) => getConversationSlug(id),
  },
}));

vi.mock('../cache', () => ({
  cacheDB: {
    getAllConversations: vi.fn(() => Promise.resolve([])),
    syncConversations: vi.fn(() => Promise.resolve()),
    putConversation: vi.fn(() => Promise.resolve()),
  },
}));

// Imported after mocks are registered.
import { __testing } from './useConversationsRefresh';

function refreshOnce(store: ConversationStore, draftStore = new DraftStore()): Promise<void> {
  return __testing.refreshOnce(store, draftStore);
}

async function flushMicrotasks() {
  // Two ticks: one for the cache promise, one for the api promise chain.
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

describe('refreshOnce coalescing (REQ-SIDEBAR-CREATE-TRAILING)', () => {
  beforeEach(() => {
    activePromises = [];
    localStorage.clear();
    listConversations.mockClear();
    listArchivedConversations.mockClear();
    getConversationSlug.mockClear();
    getConversationSlug.mockImplementation((id: string) => Promise.resolve(id));
    vi.mocked(cacheDB.getAllConversations).mockResolvedValue([]);
    vi.mocked(cacheDB.syncConversations).mockClear();
  });

  it('drops nothing: poke during in-flight schedules exactly one trailing re-fire', async () => {
    const store = new ConversationStore();

    // Kick off first refresh — leaves listConversations promise pending.
    const first = refreshOnce(store);
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(1);

    // Three pokes while the first is still in flight should collapse to
    // a single trailing re-fire — not three more, not zero.
    void refreshOnce(store);
    void refreshOnce(store);
    void refreshOnce(store);
    await flushMicrotasks();
    // No new network calls yet — still gated by __refreshInFlight.
    expect(listConversations).toHaveBeenCalledTimes(1);

    // Resolve the first call. The trailing re-fire should now run and
    // make exactly one additional listConversations call.
    activePromises[0]!.resolve([]);
    await first;
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(2);

    // Resolve the trailing call's promise so we don't leak a pending
    // promise into the next test.
    activePromises[1]!.resolve([]);
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(2);
  });

  it('cache hydration is upsert-only, then successful network refresh prunes missing rows', async () => {
    const store = new ConversationStore();
    const cachedOnly = makeConv('cached-only');
    const stillPresent = makeConv('still-present');
    store.upsertSnapshot('cached-only', cachedOnly);
    store.upsertSnapshot('still-present', stillPresent);

    vi.mocked(cacheDB.getAllConversations).mockResolvedValueOnce([stillPresent]);
    getConversationSlug.mockResolvedValueOnce(null);
    const refresh = refreshOnce(store);
    await flushMicrotasks();

    expect(store.getSnapshot('cached-only').conversation).toBe(cachedOnly);
    expect(listConversations).toHaveBeenCalledTimes(1);

    activePromises[0]!.resolve([stillPresent]);
    await refresh;

    expect(store.getSnapshot('cached-only').conversation).toBeNull();
    expect(store.getSnapshot('still-present').conversation).toBe(stillPresent);
    expect(cacheDB.syncConversations).toHaveBeenCalledWith([stillPresent]);
  });

  it('cleans up deleted rows pruned by successful network refresh', async () => {
    const store = new ConversationStore();
    const draftStore = new DraftStore();
    const deleted = makeConv('deleted', { id: 'conv-deleted' });
    store.upsertSnapshot('deleted', deleted);
    draftStore.dispatch('deleted', { type: 'set_draft', text: 'stale draft' });
    setLastViewer('deleted', 'file=%2Frepo%2Ffoo&root=%2Frepo');
    localStorage.setItem('phoenix:draft:conv-deleted', 'stale draft');
    getConversationSlug.mockResolvedValueOnce(null);

    const refresh = refreshOnce(store, draftStore);
    await flushMicrotasks();
    activePromises[0]!.resolve([]);
    await refresh;

    expect(store.getSnapshot('deleted').conversation).toBeNull();
    expect(draftStore.getSnapshot('deleted').draft).toBe('');
    expect(getLastViewer('deleted')).toBeNull();
    expect(localStorage.getItem('phoenix:draft:conv-deleted')).toBeNull();
  });

  it('does not prune omitted rows when existence confirmation still finds them', async () => {
    const store = new ConversationStore();
    const stillExists = makeConv('still-exists', { id: 'conv-still-exists' });
    store.upsertSnapshot('still-exists', stillExists);
    getConversationSlug.mockResolvedValueOnce('still-exists');

    const refresh = refreshOnce(store);
    await flushMicrotasks();
    activePromises[0]!.resolve([]);
    await refresh;

    expect(store.getSnapshot('still-exists').conversation).toBe(stillExists);
    expect(getConversationSlug).toHaveBeenCalledWith('conv-still-exists');
  });

  it('does not prune child conversations during sidebar refresh', async () => {
    const store = new ConversationStore();
    const child = makeConv('child', { parent_conversation_id: 'conv-parent' });
    store.upsertSnapshot('child', child);

    const refresh = refreshOnce(store);
    await flushMicrotasks();
    activePromises[0]!.resolve([]);
    await refresh;

    expect(store.getSnapshot('child').conversation).toBe(child);
  });

  it('no poke during in-flight => no trailing re-fire', async () => {
    const store = new ConversationStore();

    const first = refreshOnce(store);
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(1);

    activePromises[0]!.resolve([]);
    await first;
    await flushMicrotasks();
    // No second call — the pending flag was never set.
    expect(listConversations).toHaveBeenCalledTimes(1);
  });

  it('await refresh() waits for the trailing re-fire that observed the poke', async () => {
    // Regression for PR #68 review (Copilot bot): callers awaiting a
    // poked refresh must not see their await resolve before the
    // reconcile that included their poke has actually run. Without
    // this, `await refreshConversations(); /* read store */` could
    // observe pre-mutation state.
    const store = new ConversationStore();

    const first = refreshOnce(store);
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(1);

    // Poke during in-flight. Await the returned promise.
    let pokeResolved = false;
    const pokeAwait = refreshOnce(store).then(() => {
      pokeResolved = true;
    });

    // Resolve the in-flight call. The trailing re-fire kicks off but
    // its listConversations promise is itself pending — the poke's
    // await must NOT resolve yet.
    activePromises[0]!.resolve([]);
    await first;
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(2);
    expect(pokeResolved).toBe(false);

    // Resolve the trailing call. Now the poke's await resolves.
    activePromises[1]!.resolve([]);
    await pokeAwait;
    expect(pokeResolved).toBe(true);
  });

  it('all concurrent pokes share one trailing re-fire promise', async () => {
    const store = new ConversationStore();

    const first = refreshOnce(store);
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(1);

    const pokeA = refreshOnce(store);
    const pokeB = refreshOnce(store);
    const pokeC = refreshOnce(store);

    activePromises[0]!.resolve([]);
    await first;
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(2);

    // All three pokes resolve when the single trailing re-fire ends.
    activePromises[1]!.resolve([]);
    await Promise.all([pokeA, pokeB, pokeC]);
    expect(listConversations).toHaveBeenCalledTimes(2);
  });
});
