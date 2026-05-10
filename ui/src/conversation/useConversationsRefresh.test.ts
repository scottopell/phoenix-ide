import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ConversationStore } from './ConversationStore';

// Defer-resolving mocks so the test can interleave a second `refreshOnce`
// call while the first is still in flight.
let activePromises: Array<{
  resolve: (rows: unknown[]) => void;
  reject: (err: unknown) => void;
}> = [];

function makeDeferred() {
  let resolve!: (rows: unknown[]) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<unknown[]>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const listConversations = vi.fn((): Promise<unknown[]> => {
  const d = makeDeferred();
  activePromises.push({ resolve: d.resolve, reject: d.reject });
  return d.promise;
});
const listArchivedConversations = vi.fn(
  (): Promise<unknown[]> => Promise.resolve([]),
);

vi.mock('../api', () => ({
  api: {
    listConversations: () => listConversations(),
    listArchivedConversations: () => listArchivedConversations(),
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

async function flushMicrotasks() {
  // Two ticks: one for the cache promise, one for the api promise chain.
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

describe('refreshOnce coalescing (REQ-SIDEBAR-CREATE-TRAILING)', () => {
  beforeEach(() => {
    activePromises = [];
    listConversations.mockClear();
    listArchivedConversations.mockClear();
  });

  it('drops nothing: poke during in-flight schedules exactly one trailing re-fire', async () => {
    const store = new ConversationStore();

    // Kick off first refresh — leaves listConversations promise pending.
    const first = __testing.refreshOnce(store);
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(1);

    // Three pokes while the first is still in flight should collapse to
    // a single trailing re-fire — not three more, not zero.
    void __testing.refreshOnce(store);
    void __testing.refreshOnce(store);
    void __testing.refreshOnce(store);
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

  it('no poke during in-flight => no trailing re-fire', async () => {
    const store = new ConversationStore();

    const first = __testing.refreshOnce(store);
    await flushMicrotasks();
    expect(listConversations).toHaveBeenCalledTimes(1);

    activePromises[0]!.resolve([]);
    await first;
    await flushMicrotasks();
    // No second call — the pending flag was never set.
    expect(listConversations).toHaveBeenCalledTimes(1);
  });
});
