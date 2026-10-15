import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  activeProductCreationRequestId,
  activateProductCreationRequest,
  clearActiveProductCreationRequestId,
  clearProductCreationRequest,
  parsePendingCreationRequest,
  readProductCreationRequest,
  storeProductCreationRequest,
} from './productCreationStore';

const activeRequestKey = 'phoenix-active-product-create-request';
const records = new Map<IDBValidKey, unknown>();
let priorIndexedDb: IDBFactory;

function fakeRequest(): Record<string, unknown> {
  return { result: undefined, error: null, onsuccess: null, onerror: null };
}

function installIndexedDb(): void {
  const database = {
    objectStoreNames: { contains: () => true },
    close: vi.fn(),
    onversionchange: null,
    transaction: () => {
      const transaction = {
        error: null,
        oncomplete: null as (() => void) | null,
        onerror: null as (() => void) | null,
        onabort: null as (() => void) | null,
        objectStore: () => ({
          put: (value: unknown, key: IDBValidKey) => {
            records.set(key, value);
            queueMicrotask(() => transaction.oncomplete?.());
          },
          get: (key: IDBValidKey) => {
            const request = fakeRequest();
            queueMicrotask(() => {
              request['result'] = records.get(key);
              (request['onsuccess'] as (() => void) | null)?.();
              queueMicrotask(() => transaction.oncomplete?.());
            });
            return request;
          },
          delete: (key: IDBValidKey) => {
            records.delete(key);
            queueMicrotask(() => transaction.oncomplete?.());
          },
        }),
      };
      return transaction;
    },
  };
  const indexedDb = {
    open: () => {
      const request: Record<string, unknown> & {
        result: unknown;
        onsuccess: (() => void) | null;
      } = {
        ...fakeRequest(),
        result: undefined,
        onsuccess: null,
        onupgradeneeded: null,
      };
      queueMicrotask(() => {
        request.result = database;
        request.onsuccess?.();
      });
      return request;
    },
  };
  globalThis.indexedDB = indexedDb as unknown as IDBFactory;
}

describe('product creation recovery records', () => {
  beforeAll(() => {
    priorIndexedDb = globalThis.indexedDB;
    installIndexedDb();
  });

  afterAll(() => {
    globalThis.indexedDB = priorIndexedDb;
  });

  beforeEach(() => {
    records.clear();
    localStorage.clear();
    sessionStorage.clear();
  });

  afterEach(() => vi.restoreAllMocks());

  it('persists a complete image request outside localStorage', async () => {
    const image = { data: 'a'.repeat(5_500_000), media_type: 'image/png' };
    const stored = {
      version: 3 as const,
      kind: 'uncertain' as const,
      request: {
        request_id: 'request-1',
        cwd: '/repo',
        objective: 'Create with image',
        model: 'model-1',
        effort: 'high' as const,
        llm_language: null,
        images: [image],
      },
    };
    const localWrite = vi.spyOn(localStorage, 'setItem');

    await storeProductCreationRequest(stored);
    expect(activeProductCreationRequestId()).toBeNull();
    activateProductCreationRequest('request-1');

    expect(await readProductCreationRequest('request-1')).toEqual(stored);
    expect(localWrite).not.toHaveBeenCalled();
    expect(activeProductCreationRequestId()).toBe('request-1');
    await clearProductCreationRequest('request-1');
    expect(activeProductCreationRequestId()).toBeNull();
    localWrite.mockRestore();
  });

  it.each([
    null,
    { version: 2, kind: 'accepted', requestId: 'request-1', productConversationId: 'product-1' },
    { version: 3, kind: 'accepted', requestId: '', productConversationId: 'product-1' },
    { version: 3, kind: 'uncertain', request: { request_id: 'request-1' } },
    { version: 3, kind: 'uncertain', request: { request_id: 'request-1', cwd: '/repo', objective: 'x', model: 'm', images: [{}] } },
  ])('rejects an invalid recovery record: %j', (stored) => {
    expect(parsePendingCreationRequest(stored)).toBeNull();
  });

  it('does not clear a newer request pointer when an older request completes', () => {
    sessionStorage.setItem(activeRequestKey, 'request-2');

    clearActiveProductCreationRequestId('request-1');

    expect(activeProductCreationRequestId()).toBe('request-2');
    clearActiveProductCreationRequestId('request-2');
    expect(activeProductCreationRequestId()).toBeNull();
  });
});
