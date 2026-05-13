import { useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useSyncExternalStore } from 'react';
import { DraftContext } from '../conversation/DraftContext';
import { useConversationSnapshot } from '../conversation/useConversationAtom';

const DEBOUNCE_MS = 300;
const STORAGE_PREFIX = 'phoenix:draft:';

function storageKey(conversationId: string): string {
  return `${STORAGE_PREFIX}${conversationId}`;
}

function readDraft(conversationId: string): string {
  try {
    return localStorage.getItem(storageKey(conversationId)) ?? '';
  } catch (error) {
    console.warn('Error reading draft from localStorage:', error);
    return '';
  }
}

function writeDraft(conversationId: string, value: string): void {
  try {
    if (value === '') {
      localStorage.removeItem(storageKey(conversationId));
    } else {
      localStorage.setItem(storageKey(conversationId), value);
    }
  } catch (error) {
    console.warn('Error saving draft to localStorage:', error);
  }
}

/**
 * Remove a conversation's persisted draft from localStorage. Called from
 * the `phoenix:conversation-hard-deleted` cascade so a deleted conversation
 * doesn't leave stale draft text behind in the browser's storage.
 */
export function clearDraftStorage(conversationId: string): void {
  try {
    localStorage.removeItem(storageKey(conversationId));
  } catch (error) {
    console.warn('Error clearing draft from localStorage:', error);
  }
}

function useDraftStore() {
  const store = useContext(DraftContext);
  if (!store) {
    throw new Error('useDraft* hooks must be used within ConversationProvider');
  }
  return store;
}

/**
 * Subscribe to the draft text only. Re-renders the calling component on
 * draft changes and nothing else. Consumers of the conversation atom —
 * message list, terminal, breadcrumbs — never see keystroke mutations.
 */
export function useDraftValue(slug: string): string {
  const store = useDraftStore();
  const subscribe = useCallback(
    (listener: () => void) => store.subscribe(slug, listener),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => store.getSnapshot(slug).draft,
    [store, slug],
  );
  return useSyncExternalStore(subscribe, getSnapshot);
}

export interface DraftActions {
  setDraft: (text: string) => void;
  setDraftIfEmpty: (text: string) => void;
  appendDraft: (text: string) => void;
  clearDraft: () => void;
}

/**
 * Stable dispatchers for the slug's draft. No subscription — the returned
 * object is memoized on `(store, slug)`, so the caller never re-renders
 * on keystrokes.
 */
export function useDraftActions(slug: string): DraftActions {
  const store = useDraftStore();
  return useMemo(
    () => ({
      setDraft: (text: string) => store.dispatch(slug, { type: 'set_draft', text }),
      setDraftIfEmpty: (text: string) =>
        store.dispatch(slug, { type: 'set_draft_if_empty', text }),
      appendDraft: (text: string) =>
        store.dispatch(slug, { type: 'append_draft', text }),
      clearDraft: () => store.dispatch(slug, { type: 'clear_draft' }),
    }),
    [store, slug],
  );
}

/**
 * Owns the draft's persistence side-effects: hydrate from localStorage on
 * first observation of a conversationId, then debounced write-through on
 * every draft change.
 *
 * The conversation id (server-assigned, canonical for the localStorage key)
 * is read from the conversation snapshot — so hydration triggers on
 * cache-warm navigations (snapshot present, SSE init still in flight).
 * Re-renders are confined to the `<DraftLifecycle>` wrapper component,
 * which returns null — zero DOM work.
 */
export function useDraftLifecycle(slug: string): void {
  const draft = useDraftValue(slug);
  const conversationId = useConversationSnapshot(slug)?.id ?? null;
  const store = useDraftStore();

  // Hydrate once per conversationId. `useLayoutEffect` runs after render
  // but before browser paint, so the first frame already reflects the
  // stored draft.
  const hydratedForRef = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (!conversationId) return;
    if (hydratedForRef.current === conversationId) return;
    hydratedForRef.current = conversationId;
    if (draft) return;
    const stored = readDraft(conversationId);
    if (stored) {
      store.dispatch(slug, { type: 'set_draft', text: stored });
    }
  }, [conversationId, draft, slug, store]);

  // Debounced write-through. On a conversationId change with a pending
  // write for the prior conversation, flush it synchronously before
  // scheduling the new one — otherwise the last ~300ms of typing in the
  // previous conversation never reaches localStorage.
  const pendingRef = useRef<{
    timer: number | null;
    conversationId: string | null;
    value: string | null;
  }>({ timer: null, conversationId: null, value: null });

  useEffect(() => {
    if (!conversationId) return;
    if (hydratedForRef.current !== conversationId) return;
    const pending = pendingRef.current;
    if (pending.timer !== null) {
      window.clearTimeout(pending.timer);
      if (
        pending.conversationId !== null &&
        pending.conversationId !== conversationId &&
        pending.value !== null
      ) {
        writeDraft(pending.conversationId, pending.value);
      }
    }
    pending.conversationId = conversationId;
    pending.value = draft;
    pending.timer = window.setTimeout(() => {
      writeDraft(conversationId, draft);
      pending.timer = null;
    }, DEBOUNCE_MS);
  }, [conversationId, draft]);

  // Flush any pending write on unmount (page close, navigation away).
  useEffect(() => {
    return () => {
      const pending = pendingRef.current;
      if (pending.timer !== null) {
        window.clearTimeout(pending.timer);
        pending.timer = null;
        if (pending.conversationId !== null && pending.value !== null) {
          writeDraft(pending.conversationId, pending.value);
        }
      }
    };
  }, []);
}

/** Hosts {@link useDraftLifecycle} in isolation so the keystroke-frequency
 *  draft-value subscription doesn't re-render any sibling subtree. */
export function DraftLifecycle({ slug }: { slug: string }): null {
  useDraftLifecycle(slug);
  return null;
}
