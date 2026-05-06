import { useCallback, useContext, useEffect, useRef } from 'react';
import { ConversationContext } from './ConversationContext';
import { api } from '../api';
import { cacheDB } from '../cache';

const POLL_INTERVAL_MS = 5000;

/**
 * Owns the periodic refresh that keeps the conversation store in sync
 * with the server's `listConversations` / `listArchivedConversations`
 * endpoints. Mounted exactly once near the app root (inside
 * `ConversationProvider`).
 *
 * Behaviour:
 *   1. Cache-first hydrate: read every conversation from `cacheDB` and
 *      `upsertSnapshot` each one into the store. The monotonic guard
 *      makes this safe even if the cache is stale relative to data SSE
 *      has already pushed into a live atom.
 *   2. Network refresh: when `navigator.onLine`, fetch both lists in
 *      parallel and `upsertSnapshots` each. Persist successful fetches
 *      back to the cache.
 *   3. Periodic poll: while the tab is visible and online, refresh
 *      every 5s. Identical rows produce no upserts and no notifications,
 *      so the price of polling when nothing has changed is one fetch.
 *   4. Hard-delete cascade hook: listen for the
 *      `phoenix:conversation-hard-deleted` window event (emitted by
 *      `useConnection` after a per-conv SSE stream announces deletion),
 *      and remove the atom directly so consumers see the row disappear
 *      without waiting for the next poll.
 *
 * The hook is driven by `ConversationContext` so it inherits the same
 * provider-singleton lifetime as the store itself.
 */
export function useConversationsRefresh(): {
  refresh: () => Promise<void>;
} {
  const store = useContext(ConversationContext);
  if (!store) {
    throw new Error(
      'useConversationsRefresh must be used within ConversationProvider',
    );
  }
  // Stable ref so callers can pass `refresh` to children without
  // worrying about reference identity churn.
  const inFlightRef = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    try {
      // Cache-first hydrate. The (id, updated_at) monotonic guard
      // inside `upsertSnapshot` keeps this safe even when the cache is
      // stale relative to live SSE state — stale cache rows are
      // dropped, fresher rows pass through.
      try {
        const cached = await cacheDB.getAllConversations();
        if (cached.length > 0) {
          store.upsertSnapshots(cached);
        }
      } catch {
        // Cache failures are non-fatal — we'll fall through to network.
      }

      if (!navigator.onLine) return;

      const [freshActive, freshArchived] = await Promise.all([
        api.listConversations(),
        api.listArchivedConversations(),
      ]);
      store.upsertSnapshots(freshActive);
      store.upsertSnapshots(freshArchived);
      try {
        await cacheDB.syncConversations([...freshActive, ...freshArchived]);
      } catch {
        // Cache write failures are non-fatal.
      }
    } catch {
      // Network failure leaves the store untouched. Live atoms still
      // reflect SSE state; the next successful poll reconciles.
    } finally {
      inFlightRef.current = false;
    }
  }, [store]);

  // Initial load + periodic refresh.
  useEffect(() => {
    void refresh();
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible' && navigator.onLine) {
        void refresh();
      }
    }, POLL_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, [refresh]);

  // REQ-BED-032: hard-delete cascade. The per-conversation SSE channel
  // emits this after the row is gone server-side. Remove the atom
  // directly so the sidebar updates immediately rather than waiting
  // for the next poll tick.
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent<{ conversationId?: string }>).detail;
      if (!detail?.conversationId) return;
      const slug = store.slugForId(detail.conversationId);
      if (slug) store.remove(slug);
      // Always re-poll — the deleted row may have been part of a chain
      // whose other members' counts are now stale.
      void refresh();
    };
    window.addEventListener('phoenix:conversation-hard-deleted', handler);
    return () => {
      window.removeEventListener('phoenix:conversation-hard-deleted', handler);
    };
  }, [store, refresh]);

  // Online → immediately reconcile (catches up after a sleep / network
  // outage).
  useEffect(() => {
    const handler = () => {
      void refresh();
    };
    window.addEventListener('online', handler);
    return () => window.removeEventListener('online', handler);
  }, [refresh]);

  return { refresh };
}
