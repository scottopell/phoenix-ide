import { useCallback, useEffect, useRef } from 'react';
import { useConversationAtom } from '../conversation/useConversationAtom';

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

export interface DraftControl {
  draft: string;
  setDraft: (text: string) => void;
  appendDraft: (text: string) => void;
  clearDraft: () => void;
}

/**
 * Conversation-scoped draft. State lives in the conversation atom, NOT in
 * `<InputArea>` — so every surface that mutates the draft (typing, terminal
 * selection, prose-reader notes, retry-of-failed, seed hydration) talks to
 * one source of truth, regardless of whether `<InputArea>` is currently
 * mounted.
 *
 * localStorage (`phoenix:draft:<id>`) is a write-through cache. On first
 * observation of a conversationId the hook hydrates from localStorage if
 * the atom is empty, then a debounced effect mirrors every atom change
 * back to localStorage. The atom is canonical at all times in memory.
 */
export function useDraft(slug: string | undefined): DraftControl {
  const [atom, dispatch] = useConversationAtom(slug ?? '');
  const conversationId = atom.conversationId;

  // Hydrate from localStorage once per conversationId. Skipped if the atom
  // already has a draft — preserves in-memory edits when the user navigates
  // away and back within the same session.
  const hydratedForRef = useRef<string | null>(null);
  useEffect(() => {
    if (!conversationId) return;
    if (hydratedForRef.current === conversationId) return;
    hydratedForRef.current = conversationId;
    if (atom.draft) return;
    const stored = readDraft(conversationId);
    if (stored) {
      dispatch({
        type: 'set_draft',
        text: stored,
        expectedConversationId: conversationId,
      });
    }
  }, [conversationId, atom.draft, dispatch]);

  // Debounced write-through to localStorage. Flushes any pending write on
  // unmount (component teardown, page close) so 300ms of typing isn't lost.
  const pendingRef = useRef<{
    timer: number | null;
    conversationId: string | null;
    value: string | null;
  }>({ timer: null, conversationId: null, value: null });

  useEffect(() => {
    if (!conversationId) return;
    if (hydratedForRef.current !== conversationId) return;
    const value = atom.draft;
    const pending = pendingRef.current;
    if (pending.timer !== null) {
      window.clearTimeout(pending.timer);
    }
    pending.conversationId = conversationId;
    pending.value = value;
    pending.timer = window.setTimeout(() => {
      writeDraft(conversationId, value);
      pending.timer = null;
    }, DEBOUNCE_MS);
  }, [conversationId, atom.draft]);

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

  return { draft: atom.draft, setDraft, appendDraft, clearDraft };
}
