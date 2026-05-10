import { useCallback, useContext, useEffect, useRef } from 'react';
import { ConversationContext } from './ConversationContext';
import type { ConversationStore } from './ConversationStore';
import { api } from '../api';
import { cacheDB } from '../cache';

const POLL_INTERVAL_MS = 5000;

/**
 * Pure refresh implementation. Reconciles the store with the cache and
 * the server's `listConversations` / `listArchivedConversations`
 * endpoints in this order:
 *
 *   1. Cache-first hydrate via `cacheDB.getAllConversations()`. The
 *      monotonic guard inside `upsertSnapshot` keeps stale cache rows
 *      from clobbering data SSE has already pushed into a live atom.
 *   2. Network refresh of both list endpoints when online; persist
 *      successful fetches back to the cache.
 *
 * In-flight coalescing: a `__refreshInFlight` flag on the store
 * prevents concurrent refreshes from stacking up. Pokes that arrive
 * while a refresh is already running set `__refreshPending`, which
 * triggers exactly one trailing re-fire after the in-flight call
 * settles. Any number of pokes during one in-flight collapse to that
 * single re-fire — without this, a sidebar mutation (rename / archive /
 * delete) that lands while the 5s driver tick is mid-flight would be
 * silently dropped and the sidebar would reflect pre-mutation state
 * until the next tick.
 *
 * Await semantics: callers who `await refresh()` get a promise that
 * resolves only after a refresh attempt that observed their poke has
 * settled. A poke during in-flight does not resolve when the in-flight
 * call ends — it resolves when the trailing re-fire ends. Without this,
 * `await refreshConversations()` followed by reading the store could
 * see pre-poke state because the awaited promise resolved before the
 * trailing fire ran. (Today no caller awaits, but the contract should
 * hold by default.)
 */
async function refreshOnce(store: ConversationStore): Promise<void> {
  // Flags live on the store so they're shared across every consumer
  // that might trigger a refresh (the driver, post-mutation pokes from
  // ConversationListPage handlers, the onConversationCreated callback
  // in DesktopLayout).
  const f = store as ConversationStore & {
    __refreshInFlight?: boolean;
    __refreshPending?: boolean;
    __refreshPendingPromise?: Promise<void>;
    __refreshPendingResolve?: () => void;
  };
  if (f.__refreshInFlight) {
    if (!f.__refreshPending) {
      f.__refreshPending = true;
      f.__refreshPendingPromise = new Promise<void>((resolve) => {
        f.__refreshPendingResolve = resolve;
      });
    }
    // Every concurrent poke awaits the same trailing-fire promise.
    return f.__refreshPendingPromise!;
  }
  f.__refreshInFlight = true;
  try {
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
    f.__refreshInFlight = false;
    if (f.__refreshPending) {
      f.__refreshPending = false;
      const pendingResolve = f.__refreshPendingResolve;
      f.__refreshPendingResolve = undefined;
      f.__refreshPendingPromise = undefined;
      // Settle the pending awaiters when the trailing fire completes —
      // not when it starts. A failed trailing fire still resolves (the
      // outer try/catch swallows network errors), so awaiters never
      // hang on a transient outage.
      void refreshOnce(store).then(() => pendingResolve?.());
    }
  }
}

/** Test-only handle on the private refresh implementation. Not part of
 *  the public surface — consumers should use `useConversationsRefresh`. */
export const __testing = { refreshOnce };

function useStoreFromContext(label: string): ConversationStore {
  const store = useContext(ConversationContext);
  if (!store) throw new Error(`${label} must be used within ConversationProvider`);
  return store;
}

/**
 * Side-effect-free accessor: returns `{ refresh }` for callers that
 * want to trigger a manual reconcile (e.g. after a mutation API call).
 * Does NOT mount any pollers or listeners — only the driver does that.
 *
 * Mount the driver exactly once per app — see
 * {@link useConversationsRefreshDriver}, which `ConversationProvider`
 * already calls. Multiple consumers calling this accessor share the
 * same in-flight + pending flags on the store, so concurrent pokes
 * collapse to a single trailing re-fire (see {@link refreshOnce}).
 */
export function useConversationsRefresh(): {
  refresh: () => Promise<void>;
} {
  const store = useStoreFromContext('useConversationsRefresh');
  const refresh = useCallback(() => refreshOnce(store), [store]);
  return { refresh };
}

/**
 * Owns the periodic refresh + online + hard-delete listeners. Mount
 * this exactly once per app — `ConversationProvider` already does so.
 * Other consumers should use {@link useConversationsRefresh}.
 *
 * Why split: pre-split, both the provider's driver and any consumer
 * that wanted `refresh` would mount duplicate intervals + listeners,
 * causing 2× polling and 2× reactions to every cascade event
 * (Codex review on PR #26). The accessor / driver split makes the
 * side-effect surface explicit at the call site.
 */
export function useConversationsRefreshDriver(): void {
  const store = useStoreFromContext('useConversationsRefreshDriver');
  // Stable refresh function for use inside effects.
  const refresh = useCallback(() => refreshOnce(store), [store]);
  // Ref so listeners don't re-bind every render.
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  // Initial load + periodic refresh.
  useEffect(() => {
    void refreshRef.current();
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible' && navigator.onLine) {
        void refreshRef.current();
      }
    }, POLL_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, []);

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
      void refreshRef.current();
    };
    window.addEventListener('phoenix:conversation-hard-deleted', handler);
    return () => {
      window.removeEventListener('phoenix:conversation-hard-deleted', handler);
    };
  }, [store]);

  // Online → immediately reconcile (catches up after a sleep / network
  // outage).
  useEffect(() => {
    const handler = () => {
      void refreshRef.current();
    };
    window.addEventListener('online', handler);
    return () => window.removeEventListener('online', handler);
  }, []);
}
