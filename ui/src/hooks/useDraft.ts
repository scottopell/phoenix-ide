import { useCallback, useEffect, useLayoutEffect, useRef } from 'react';
import type { ConversationAtom } from '../conversation/atom';
import {
  useConversationDispatch,
  useConversationSlice,
} from '../conversation/useConversationAtom';

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

// Hoisted selectors — module-scoped so they're referentially stable and
// `useConversationSlice`'s `Object.is` comparison short-circuits cleanly.
const selectDraft = (atom: ConversationAtom): string => atom.draft;
const selectConversationId = (atom: ConversationAtom): string | null =>
  atom.conversationId;

/**
 * Subscribe to the draft text only. The calling component re-renders on
 * draft changes — and nothing else. Use in `<InputArea>` (the only
 * component that needs to *display* the draft).
 *
 * Other surfaces (page-level handlers, side-effect drivers) should reach
 * for {@link useDraftActions} or {@link useDraftLifecycle} instead so
 * keystrokes don't broadcast through the rest of the page.
 */
export function useDraftValue(slug: string): string {
  return useConversationSlice(slug, selectDraft);
}

export interface DraftActions {
  setDraft: (text: string) => void;
  appendDraft: (text: string) => void;
  clearDraft: () => void;
}

/**
 * Stable dispatchers for the conversation's draft. Subscribes to the atom's
 * `conversationId` only (used for `expectedConversationId` guarding), so the
 * caller re-renders at most when the conversation changes — never on
 * keystrokes.
 */
export function useDraftActions(slug: string): DraftActions {
  const conversationId = useConversationSlice(slug, selectConversationId);
  const dispatch = useConversationDispatch(slug);

  const setDraft = useCallback(
    (text: string) => {
      if (!conversationId) return;
      dispatch({ type: 'set_draft', text, expectedConversationId: conversationId });
    },
    [conversationId, dispatch],
  );
  const appendDraft = useCallback(
    (text: string) => {
      if (!conversationId) return;
      dispatch({ type: 'append_draft', text, expectedConversationId: conversationId });
    },
    [conversationId, dispatch],
  );
  const clearDraft = useCallback(() => {
    if (!conversationId) return;
    dispatch({ type: 'clear_draft', expectedConversationId: conversationId });
  }, [conversationId, dispatch]);

  return { setDraft, appendDraft, clearDraft };
}

/**
 * Owns the draft's persistence side-effects: hydrate from localStorage on
 * first observation of a conversationId, then debounced write-through on
 * every atom change.
 *
 * This hook DOES subscribe to the draft value (it has to — the persistence
 * effect reads it), so the calling component re-renders on every keystroke.
 * Mount it inside a dedicated wrapper component (`<DraftLifecycle>`) that
 * returns null, so those re-renders never produce DOM work and never
 * touch sibling subtrees.
 *
 * localStorage is a write-through cache; the atom is canonical at all
 * times. See the file header for the parallel-representations rationale.
 */
export function useDraftLifecycle(slug: string): void {
  const draft = useConversationSlice(slug, selectDraft);
  const conversationId = useConversationSlice(slug, selectConversationId);
  const dispatch = useConversationDispatch(slug);

  // Hydrate once per conversationId. `useLayoutEffect` runs after render
  // but before browser paint — so the first frame already reflects the
  // stored draft, matching the pre-refactor `useDraft`'s synchronous
  // localStorage read.
  const hydratedForRef = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (!conversationId) return;
    if (hydratedForRef.current === conversationId) return;
    hydratedForRef.current = conversationId;
    if (draft) return;
    const stored = readDraft(conversationId);
    if (stored) {
      dispatch({
        type: 'set_draft',
        text: stored,
        expectedConversationId: conversationId,
      });
    }
  }, [conversationId, draft, dispatch]);

  // Debounced write-through. On a conversationId change with a pending
  // write for the prior conversation, flush it synchronously before
  // scheduling for the new one — otherwise the last ~300ms of typing
  // in the previous conversation would never reach localStorage.
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

/**
 * Wrapper component that hosts {@link useDraftLifecycle} in isolation.
 * Renders `null` — its only purpose is to be the re-render target for the
 * draft-value subscription, so the page-level component doesn't churn on
 * keystrokes.
 *
 * Mount exactly once per conversation page:
 *   ```
 *   <DraftLifecycle slug={slug!} />
 *   ```
 */
export function DraftLifecycle({ slug }: { slug: string }): null {
  useDraftLifecycle(slug);
  return null;
}
