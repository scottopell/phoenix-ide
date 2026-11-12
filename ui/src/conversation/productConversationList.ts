import { useSyncExternalStore } from 'react';
import { api } from '../api';
import type { CoordinatorListEntryView, ProductConversationListResponse, ProductConversationListRow } from '../api';
import { subscribeProductConversationListRevision } from '../notifications';

const CACHE_KEY = 'phoenix:product-conversation-list:v1';
const REQUEST_TIMEOUT_MS = 15_000;

export interface ProductConversationListSnapshot {
  rows: readonly ProductConversationListRow[];
  coordinator: CoordinatorListEntryView | null;
  /** Last refresh failure; rows keep the last good list (possibly from cache). */
  error: string | null;
  /** True once the server has answered at least once in this page session. */
  fresh: boolean;
  /** True when rows came from the local cache and no refresh has succeeded yet. */
  fromCache: boolean;
}

function readCache(): Pick<ProductConversationListSnapshot, 'rows' | 'coordinator'> | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<ProductConversationListResponse>;
    if (!Array.isArray(parsed.product_conversations)) return null;
    const rows = parsed.product_conversations.filter((row) => (
      typeof row?.product_conversation_id === 'string' && typeof row.read_state?.state === 'string'
    ));
    return { rows, coordinator: parsed.coordinator ?? null };
  } catch {
    return null;
  }
}

function writeCache(rows: readonly ProductConversationListRow[], coordinator: CoordinatorListEntryView | null): void {
  try {
    const value: ProductConversationListResponse = { product_conversations: [...rows], coordinator };
    localStorage.setItem(CACHE_KEY, JSON.stringify(value));
  } catch {
    // Storage may be unavailable or full; the list still works without it.
  }
}

function initialSnapshot(): ProductConversationListSnapshot {
  const cached = readCache();
  return {
    rows: cached?.rows ?? [],
    coordinator: cached?.coordinator ?? null,
    error: null,
    fresh: false,
    fromCache: cached !== null,
  };
}

let snapshot: ProductConversationListSnapshot = initialSnapshot();
const listeners = new Set<() => void>();
let inFlight: { controller: AbortController; promise: Promise<void> } | null = null;
let stopRevisionSubscription: (() => void) | null = null;

function publish(next: ProductConversationListSnapshot): void {
  snapshot = next;
  listeners.forEach((listener) => listener());
}

/**
 * Fetch the list. A newer refresh supersedes (and aborts) an older one still
 * in flight, so a slow response can never overwrite fresher rows.
 */
export function refreshProductConversationList(): Promise<void> {
  inFlight?.controller.abort();
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  const promise = api.listProductConversations(controller.signal)
    .then((response) => {
      if (inFlight?.controller !== controller) return;
      writeCache(response.product_conversations, response.coordinator);
      publish({
        rows: response.product_conversations,
        coordinator: response.coordinator,
        error: null,
        fresh: true,
        fromCache: false,
      });
    })
    .catch((error: unknown) => {
      if (inFlight?.controller !== controller) return;
      publish({ ...snapshot, error: error instanceof Error ? error.message : 'Failed to refresh conversations' });
    })
    .finally(() => {
      window.clearTimeout(timeout);
      if (inFlight?.controller === controller) inFlight = null;
    });
  inFlight = { controller, promise };
  return promise;
}

/** Apply an optimistic change (rename, close, delete) to the shared rows. */
export function updateProductConversationRows(
  updater: (rows: readonly ProductConversationListRow[]) => readonly ProductConversationListRow[],
): void {
  const rows = updater(snapshot.rows);
  if (rows === snapshot.rows) return;
  writeCache(rows, snapshot.coordinator);
  publish({ ...snapshot, rows });
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    stopRevisionSubscription = subscribeProductConversationListRevision(() => { void refreshProductConversationList(); });
    void refreshProductConversationList();
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      stopRevisionSubscription?.();
      stopRevisionSubscription = null;
    }
  };
}

function getSnapshot(): ProductConversationListSnapshot {
  return snapshot;
}

/** The one product conversation list every surface renders from. */
export function useProductConversationList(): ProductConversationListSnapshot {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

export const __testing = {
  reset: resetForTests,
  /** Re-read the snapshot from the current cache, as a fresh page load would. */
  resetCachedSnapshotForTests(): void {
    snapshot = initialSnapshot();
  },
  cacheKey: CACHE_KEY,
};

function resetForTests(): void {
  stopRevisionSubscription?.();
  stopRevisionSubscription = null;
  listeners.clear();
  inFlight?.controller.abort();
  inFlight = null;
  try { localStorage.removeItem(CACHE_KEY); } catch { /* ignore */ }
  snapshot = initialSnapshot();
}

if (import.meta.env.MODE === 'test') {
  const testGlobals = globalThis as {
    __phoenixResetProductConversationList?: () => void;
    __phoenixResetProductConversationListSnapshot?: () => void;
  };
  testGlobals.__phoenixResetProductConversationList = resetForTests;
  testGlobals.__phoenixResetProductConversationListSnapshot = () => { snapshot = initialSnapshot(); };
}
