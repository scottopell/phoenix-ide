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
