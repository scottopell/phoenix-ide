import { useCallback, useEffect, useRef } from 'react';
import { useScopedState } from './useScopedState';

const DEBOUNCE_MS = 300;

function readDraft(storageKey: string | null): string {
  if (!storageKey) return '';
  try {
    return localStorage.getItem(storageKey) ?? '';
  } catch (error) {
    console.warn('Error reading draft from localStorage:', error);
    return '';
  }
}

/**
 * Hook for managing draft message text with debounced localStorage persistence.
 * Draft is automatically saved on every keystroke (debounced) and restored on mount.
 */
export function useDraft(conversationId: string | undefined): [
  string,
  (value: string) => void,
  () => void
] {
  const storageKey = conversationId ? `phoenix:draft:${conversationId}` : null;
  const initialDraft = readDraft(storageKey);
  const [draft, setDraftState] = useScopedState(conversationId, initialDraft);
  const debounceRef = useRef<number | null>(null);

  // Save to localStorage (debounced)
  const saveToStorage = useCallback((value: string) => {
    if (!storageKey) return;
    try {
      if (value === '') {
        localStorage.removeItem(storageKey);
      } else {
        localStorage.setItem(storageKey, value);
      }
    } catch (error) {
      console.warn('Error saving draft to localStorage:', error);
    }
  }, [storageKey]);

  // Set draft with debounced persistence
  const setDraft = useCallback((value: string) => {
    setDraftState(value);
    
    // Cancel pending save
    if (debounceRef.current !== null) {
      clearTimeout(debounceRef.current);
    }
    
    // Schedule new save
    debounceRef.current = window.setTimeout(() => {
      saveToStorage(value);
      debounceRef.current = null;
    }, DEBOUNCE_MS);
  }, [saveToStorage, setDraftState]);

  // Clear draft (immediate, no debounce)
  const clearDraft = useCallback(() => {
    // Cancel any pending save
    if (debounceRef.current !== null) {
      clearTimeout(debounceRef.current);
      debounceRef.current = null;
    }
    
    setDraftState('');
    if (storageKey) {
      try {
        localStorage.removeItem(storageKey);
      } catch (error) {
        console.warn('Error clearing draft from localStorage:', error);
      }
    }
  }, [storageKey, setDraftState]);

  // Track current draft value for flush on unmount
  const draftRef = useRef(draft);
  useEffect(() => {
    draftRef.current = draft;
  }, [draft]);

  // Cleanup on unmount - flush any pending draft save
  useEffect(() => {
    return () => {
      if (debounceRef.current !== null) {
        clearTimeout(debounceRef.current);
        // Flush the pending save immediately
        if (storageKey && draftRef.current) {
          try {
            localStorage.setItem(storageKey, draftRef.current);
          } catch (error) {
            console.warn('Error flushing draft on unmount:', error);
          }
        }
      }
    };
  }, [storageKey]);

  return [draft, setDraft, clearDraft];
}
