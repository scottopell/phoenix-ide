import { useEffect, useCallback, useState } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';

/**
 * Global keyboard shortcuts that work on all pages.
 * Call this once in App or a layout component.
 */
export function useGlobalKeyboardShortcuts() {
  const navigate = useNavigate();
  const location = useLocation();

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Don't handle if user is typing in an input/textarea (except Escape)
      const target = e.target as HTMLElement;
      const isInput = target.tagName === 'INPUT' || 
                      target.tagName === 'TEXTAREA' || 
                      target.isContentEditable;

      if (e.key === 'Escape') {
        // Blur any focused input first
        if (isInput) {
          target.blur();
          e.preventDefault();
          return;
        }
        // If on conversation page, go back to list
        if (location.pathname.startsWith('/c/') || location.pathname.startsWith('/conversation/') || location.pathname === '/new') {
          e.preventDefault();
          navigate('/');
        }
      }

      // Don't handle other keys in inputs
      if (isInput) return;

      if (e.key === '/') {
        // Focus the message input if on conversation page
        if (location.pathname.startsWith('/c/') || location.pathname.startsWith('/conversation/')) {
          const input = document.querySelector('#message-input') as HTMLTextAreaElement;
          if (input) {
            e.preventDefault();
            input.focus();
          }
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [navigate, location.pathname]);
}

interface KeyboardNavOptions {
  /** Items to navigate through */
  items?: { id: string; slug?: string }[];
  /** Callback when item is selected (Enter pressed) */
  onSelect?: (item: { id: string; slug?: string }) => void;
  /** Callback for new item (n key) */
  onNew?: () => void;
  /** Whether navigation is enabled (disable when input focused) */
  enabled?: boolean;
}

export function useKeyboardNav(options: KeyboardNavOptions = {}) {
  const { items = [], onSelect, onNew, enabled = true } = options;
  const navigate = useNavigate();
  const [selectedIndex, setSelectedIndex] = useState(-1);

  // Reset selection when items change
  useEffect(() => {
    setSelectedIndex(-1);
  }, [items.length]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!enabled) return;

      // Don't handle if user is typing in an input/textarea
      const target = e.target as HTMLElement;
      const isInput = target.tagName === 'INPUT' || 
                      target.tagName === 'TEXTAREA' || 
                      target.isContentEditable;
      
      if (isInput) return;

      switch (e.key) {
        case 'j':
        case 'ArrowDown':
          e.preventDefault();
          if (items.length > 0) {
            setSelectedIndex((prev) => 
              prev < items.length - 1 ? prev + 1 : prev
            );
          }
          break;

        case 'k':
        case 'ArrowUp':
          e.preventDefault();
          if (items.length > 0) {
            setSelectedIndex((prev) => (prev > 0 ? prev - 1 : 0));
          }
          break;

        case 'Enter':
          if (selectedIndex >= 0 && selectedIndex < items.length) {
            e.preventDefault();
            const item = items[selectedIndex];
            if (onSelect) {
              onSelect(item);
            } else if (item.slug) {
              navigate(`/c/${item.slug}`);
            }
          }
          break;

        case 'n':
          if (onNew) {
            e.preventDefault();
            onNew();
          }
          break;

        case 'Escape':
          // Clear selection (global handler does navigation)
          setSelectedIndex(-1);
          break;

        case 'g':
          // gg = go to top
          if (selectedIndex !== -1) {
            setSelectedIndex(0);
          }
          break;
      }
    },
    [enabled, items, selectedIndex, onSelect, onNew, navigate]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  // Scroll selected item into view
  useEffect(() => {
    if (selectedIndex >= 0 && items[selectedIndex]) {
      const el = document.querySelector(`[data-id="${items[selectedIndex].id}"]`);
      el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
  }, [selectedIndex, items]);

  return {
    selectedIndex,
    selectedId: selectedIndex >= 0 ? items[selectedIndex]?.id : null,
    setSelectedIndex,
  };
}
