import { useState, useCallback, useEffect, useRef } from 'react';

const DEBOUNCE_MS = 300;

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
  
  // Load initial value from localStorage
  const getInitialValue = (): string => {
    if (!storageKey) return '';
    try {
      return localStorage.getItem(storageKey) ?? '';
    } catch (error) {
      console.warn('Error reading draft from localStorage:', error);
      return '';
    }
  };

  const [draft, setDraftState] = useState<string>(getInitialValue);
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
  }, [saveToStorage]);

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
  }, [storageKey]);

  // Reload draft when conversationId changes
  useEffect(() => {
    if (!conversationId) {
      setDraftState('');
      return;
    }
    try {
      const stored = localStorage.getItem(`phoenix:draft:${conversationId}`);
      setDraftState(stored ?? '');
    } catch (error) {
      console.warn('Error reading draft from localStorage:', error);
      setDraftState('');
    }
  }, [conversationId]);

  // Cleanup on unmount - save any pending draft
  useEffect(() => {
    return () => {
      if (debounceRef.current !== null) {
        clearTimeout(debounceRef.current);
      }
    };
  }, []);

  return [draft, setDraft, clearDraft];
}
