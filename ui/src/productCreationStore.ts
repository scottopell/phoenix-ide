import type { CreateProductConversationRequest, ModelEffort } from './api';

const DATABASE_NAME = 'phoenix-product-creation';
const DATABASE_VERSION = 1;
const REQUEST_STORE = 'requests';
const ACTIVE_REQUEST_KEY = 'phoenix-active-product-create-request';

export type PendingCreationRequest =
  | { readonly version: 3; readonly kind: 'uncertain'; readonly request: CreateProductConversationRequest }
  | { readonly version: 3; readonly kind: 'accepted'; readonly requestId: string; readonly productConversationId: string };

export function creationRequestId(pending: PendingCreationRequest): string {
  return pending.kind === 'uncertain' ? pending.request.request_id : pending.requestId;
}

function isCreateRequest(value: unknown): value is CreateProductConversationRequest {
  if (!value || typeof value !== 'object') return false;
  const request = value as Record<string, unknown>;
  const efforts: readonly ModelEffort[] = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
  return typeof request['request_id'] === 'string' && request['request_id'].length > 0
    && typeof request['cwd'] === 'string'
    && typeof request['objective'] === 'string'
    && typeof request['model'] === 'string' && request['model'].length > 0
    && (request['effort'] == null || efforts.some((effort) => effort === request['effort']))
    && (request['llm_language'] == null || typeof request['llm_language'] === 'string')
    && (request['images'] === undefined || (Array.isArray(request['images']) && request['images'].every((image: unknown) =>
      image !== null && typeof image === 'object'
      && 'data' in image && typeof image.data === 'string'
      && 'media_type' in image && typeof image.media_type === 'string')));
}

export function parsePendingCreationRequest(value: unknown): PendingCreationRequest | null {
  if (!value || typeof value !== 'object') return null;
  const stored = value as Record<string, unknown>;
  if (stored['version'] !== 3 || (stored['kind'] !== 'uncertain' && stored['kind'] !== 'accepted')) return null;
  if (stored['kind'] === 'uncertain' && isCreateRequest(stored['request'])) {
    return { version: 3, kind: 'uncertain', request: stored['request'] };
  }
  if (stored['kind'] === 'accepted'
    && typeof stored['requestId'] === 'string' && stored['requestId']
    && typeof stored['productConversationId'] === 'string' && stored['productConversationId']) {
    return {
      version: 3,
      kind: 'accepted',
      requestId: stored['requestId'],
      productConversationId: stored['productConversationId'],
    };
  }
  return null;
}

let databasePromise: Promise<IDBDatabase> | null = null;

function openDatabase(): Promise<IDBDatabase> {
  if (databasePromise) return databasePromise;
  databasePromise = new Promise((resolve, reject) => {
    let settled = false;
    let timeout = 0;
    function fail(error: Error): void {
      if (settled) return;
      settled = true;
      window.clearTimeout(timeout);
      reject(error);
    }
    timeout = window.setTimeout(() => {
      fail(new Error('Opening creation recovery storage timed out.'));
    }, 5000);
    let request: IDBOpenDBRequest;
    try {
      request = indexedDB.open(DATABASE_NAME, DATABASE_VERSION);
    } catch (error) {
      fail(error instanceof Error ? error : new Error('Unable to open creation recovery storage.'));
      return;
    }
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(REQUEST_STORE)) database.createObjectStore(REQUEST_STORE);
    };
    request.onsuccess = () => {
      const database = request.result;
      if (settled) {
        database.close();
        return;
      }
      settled = true;
      window.clearTimeout(timeout);
      database.onversionchange = () => {
        database.close();
        databasePromise = null;
      };
      resolve(database);
    };
    request.onerror = () => fail(request.error ?? new Error('Unable to open creation recovery storage.'));
    request.onblocked = () => fail(new Error('Creation recovery storage is blocked by another browser context.'));
  });
  void databasePromise.catch(() => { databasePromise = null; });
  return databasePromise;
}

function transactionComplete(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error ?? new Error('Creation recovery storage transaction failed.'));
    transaction.onabort = () => reject(transaction.error ?? new Error('Creation recovery storage transaction was aborted.'));
  });
}

export function activeProductCreationRequestId(): string | null {
  try {
    return sessionStorage.getItem(ACTIVE_REQUEST_KEY);
  } catch {
    return null;
  }
}

export function activateProductCreationRequest(requestId: string): void {
  sessionStorage.setItem(ACTIVE_REQUEST_KEY, requestId);
}

export function clearActiveProductCreationRequestId(requestId: string): void {
  if (activeProductCreationRequestId() === requestId) sessionStorage.removeItem(ACTIVE_REQUEST_KEY);
}

export async function readProductCreationRequest(requestId: string): Promise<PendingCreationRequest | null> {
  const database = await openDatabase();
  const transaction = database.transaction(REQUEST_STORE, 'readonly');
  const request = transaction.objectStore(REQUEST_STORE).get(requestId);
  const stored = await new Promise<unknown>((resolve, reject) => {
    request.onsuccess = () => resolve(request.result as unknown);
    request.onerror = () => reject(request.error ?? new Error('Unable to read creation recovery state.'));
  });
  await transactionComplete(transaction);
  const pending = parsePendingCreationRequest(stored);
  return pending && creationRequestId(pending) === requestId ? pending : null;
}

export async function storeProductCreationRequest(pending: PendingCreationRequest): Promise<void> {
  const requestId = creationRequestId(pending);
  const database = await openDatabase();
  const transaction = database.transaction(REQUEST_STORE, 'readwrite');
  transaction.objectStore(REQUEST_STORE).put(pending, requestId);
  await transactionComplete(transaction);
}

export async function clearProductCreationRequest(requestId: string): Promise<void> {
  const database = await openDatabase();
  const transaction = database.transaction(REQUEST_STORE, 'readwrite');
  transaction.objectStore(REQUEST_STORE).delete(requestId);
  await transactionComplete(transaction);
  clearActiveProductCreationRequestId(requestId);
}
