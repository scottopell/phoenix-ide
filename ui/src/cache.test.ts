import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CacheDB } from './cache';

// Regression coverage for task 60006: when another tab holds the IDB
// open at a different version, indexedDB.open() never fires
// onsuccess/onerror/onblocked. Before the fix init() awaited that
// request forever, deadlocking useAppMachine.init() and cache
// hydration. The bug class is "init() can settle never" — these tests
// assert it always settles (reject within the timeout) and that a
// failed init does not poison future retries.

type FakeRequest = {
  onsuccess: (() => void) | null;
  onerror: (() => void) | null;
  onblocked: (() => void) | null;
  onupgradeneeded: ((e: unknown) => void) | null;
  error: unknown;
  result: unknown;
};

function makeRequest(): FakeRequest {
  return {
    onsuccess: null,
    onerror: null,
    onblocked: null,
    onupgradeneeded: null,
    error: null,
    result: null,
  };
}

describe('CacheDB.init() never deadlocks (task 60006)', () => {
  let openImpl: () => FakeRequest;
  let priorIndexedDB: unknown;
  let hadPriorIndexedDB: boolean;

  beforeEach(() => {
    vi.useFakeTimers();
    const g = globalThis as unknown as { indexedDB?: unknown };
    hadPriorIndexedDB = 'indexedDB' in g;
    priorIndexedDB = g.indexedDB;
    g.indexedDB = { open: () => openImpl() };
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    // Restore the shared global so the stub does not pollute later
    // tests (ui/src/test-setup.ts initializes global.indexedDB).
    const g = globalThis as unknown as { indexedDB?: unknown };
    if (hadPriorIndexedDB) {
      g.indexedDB = priorIndexedDB;
    } else {
      delete g.indexedDB;
    }
  });

  it('rejects within 5s when open hangs (no callback ever fires)', async () => {
    openImpl = () => makeRequest(); // never resolved by anything

    const db = new CacheDB();
    const init = db.init();
    // Attach the rejection handler before advancing timers so the
    // rejection is observed (no unhandled rejection).
    const assertion = expect(init).rejects.toThrow('timed out');

    await vi.advanceTimersByTimeAsync(5000);
    await assertion;
  });

  it('resets initPromise on failure so a later call can retry', async () => {
    // First open hangs and times out.
    openImpl = () => makeRequest();
    const db = new CacheDB();
    const first = db.init();
    const firstRejects = expect(first).rejects.toThrow('timed out');
    await vi.advanceTimersByTimeAsync(5000);
    await firstRejects;

    // Second open succeeds (blocking tab closed). A poisoned
    // initPromise would make this hang/reject again.
    let req: FakeRequest;
    openImpl = () => {
      req = makeRequest();
      return req;
    };
    const second = db.init();
    // Drive the success callback the way IDB would.
    req!.result = {
      objectStoreNames: { contains: () => true },
    };
    req!.onsuccess?.();
    await expect(second).resolves.toBeUndefined();
  });
});


describe('CacheDB replica metadata', () => {
  let dbCounter = 0;
  let priorIndexedDB: unknown;
  let hadPriorIndexedDB: boolean;

  type StoreMap = Map<string, Map<string, unknown>>;

  function makeKey(keyPath: string | string[], value: Record<string, unknown>): string {
    if (Array.isArray(keyPath)) {
      return JSON.stringify(keyPath.map((part) => value[part]));
    }
    return JSON.stringify(value[keyPath]);
  }

  function createMemoryIndexedDb() {
    const stores: StoreMap = new Map();

    return {
      open: () => {
        const request = makeRequest();

        queueMicrotask(() => {
          const db = {
            objectStoreNames: {
              contains: (name: string) => stores.has(name),
            },
            createObjectStore: (name: string, options?: { keyPath?: string | string[] }) => {
              stores.set(name, new Map());
              const keyPath = options?.keyPath ?? 'id';
              return {
                createIndex: () => undefined,
                put: (value: Record<string, unknown>) => {
                  stores.get(name)!.set(makeKey(keyPath, value), value);
                },
              };
            },
            transaction: () => ({
              objectStore: (name: string) => ({
                get: (key: unknown) => {
                  const req = makeRequest();
                  queueMicrotask(() => {
                    req.result = stores.get(name)?.get(JSON.stringify(key)) ?? null;
                    req.onsuccess?.();
                  });
                  return req;
                },
                put: (value: Record<string, unknown>) => {
                  const keyPath = name === 'replicaMeta' ? 'conversationId' : name === 'conversations' ? 'id' : ['conversation_id', 'sequence_id'];
                  stores.get(name)!.set(makeKey(keyPath, value), value);
                },
                getAll: (range?: IDBKeyRange | null, count?: number) => {
                  const req = makeRequest();
                  queueMicrotask(() => {
                    let values = Array.from(stores.get(name)?.values() ?? []) as Record<string, unknown>[];
                    if (name === 'messages' && range && 'lower' in range && 'upper' in range) {
                      const lower = (range as unknown as { lower: [string, number]; upper: [string, number] }).lower;
                      const upper = (range as unknown as { lower: [string, number]; upper: [string, number] }).upper;
                      values = values.filter((value) => {
                        const convId = value['conversation_id'];
                        const seq = value['sequence_id'];
                        return convId === lower[0] && typeof seq === 'number' && seq >= lower[1] && seq <= upper[1];
                      });
                    }
                    req.result = typeof count === 'number' ? values.slice(0, count) : values;
                    req.onsuccess?.();
                  });
                  return req;
                },
                delete: (key: unknown) => {
                  stores.get(name)?.delete(JSON.stringify(key));
                },
                index: () => ({
                  openCursor: () => {
                    const req = makeRequest();
                    queueMicrotask(() => {
                      req.result = null;
                      req.onsuccess?.();
                    });
                    return req;
                  },
                  getAll: (range?: IDBKeyRange | null) => {
                    const req = makeRequest();
                    queueMicrotask(() => {
                      let values = Array.from(stores.get(name)?.values() ?? []) as Record<string, unknown>[];
                      if (name === 'messages' && range && 'only' in range) {
                        const conversationId = (range as unknown as { only: string }).only;
                        values = values.filter((value) => value['conversation_id'] === conversationId);
                      }
                      req.result = values;
                      req.onsuccess?.();
                    });
                    return req;
                  },
                }),
              }),
            }),
          };

          request.result = db;
          request.onupgradeneeded?.({ target: { result: db } });
          request.onsuccess?.();
        });

        return request;
      },
    };
  }

  beforeEach(() => {
    vi.useRealTimers();
    const g = globalThis as unknown as { indexedDB?: unknown; IDBKeyRange?: typeof IDBKeyRange };
    hadPriorIndexedDB = 'indexedDB' in g;
    priorIndexedDB = g.indexedDB;
    g.indexedDB = createMemoryIndexedDb();
    g.IDBKeyRange = {
      only: (value: unknown) => ({ only: value } as unknown as IDBKeyRange),
      bound: (lower: [string, number], upper: [string, number]) => ({ lower, upper } as unknown as IDBKeyRange),
    } as typeof IDBKeyRange;
    dbCounter += 1;
  });

  afterEach(() => {
    const g = globalThis as unknown as { indexedDB?: unknown };
    if (hadPriorIndexedDB) {
      g.indexedDB = priorIndexedDB;
    } else {
      delete g.indexedDB;
    }
  });

  it('persists and reads replica metadata by conversation id', async () => {
    const db = new CacheDB();
    const meta = {
      conversationId: `conv-${dbCounter}`,
      latestMessageSequenceId: 12,
      latestEventSequenceId: 44,
      transcriptGeneration: 3,
      lastHydratedAt: '2025-01-02T03:04:05.000Z',
    };

    await db.putReplicaMeta(meta);

    await expect(db.getReplicaMeta(meta.conversationId)).resolves.toEqual(meta);
  });

  it('derives latest cached messages and max sequence id from the messages store', async () => {
    const db = new CacheDB();
    await db.putMessages([
      {
        message_id: 'm1',
        sequence_id: 1,
        conversation_id: 'conv-a',
        message_type: 'user',
        content: { text: 'one' },
        created_at: '2025-01-01T00:00:00Z',
      },
      {
        message_id: 'm3',
        sequence_id: 3,
        conversation_id: 'conv-a',
        message_type: 'agent',
        content: [],
        created_at: '2025-01-01T00:00:03Z',
      },
      {
        message_id: 'm2',
        sequence_id: 2,
        conversation_id: 'conv-a',
        message_type: 'tool',
        content: { tool_use_id: 't', result: 'two' },
        created_at: '2025-01-01T00:00:02Z',
      },
      {
        message_id: 'other',
        sequence_id: 9,
        conversation_id: 'conv-b',
        message_type: 'user',
        content: { text: 'other' },
        created_at: '2025-01-01T00:00:09Z',
      },
    ]);

    await expect(db.getLatestCachedMessages('conv-a', 2)).resolves.toMatchObject([
      { message_id: 'm3', sequence_id: 3 },
      { message_id: 'm2', sequence_id: 2 },
    ]);
    await expect(db.getMaxMessageSequenceId('conv-a')).resolves.toBe(3);
    await expect(db.getMaxMessageSequenceId('missing')).resolves.toBeNull();
  });
});
