import { useEffect } from 'react';
import { useFocusScope } from '../../hooks/useFocusScope';

interface UseViewerFindKeyboardShortcutOptions {
  scopeId: string;
  onOpen: () => void;
  enabled?: boolean;
}

function isEditableTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  if (!element || !(element instanceof HTMLElement)) return false;
  if (element.dataset.viewerFindInput === 'true') return false;
  const tag = element.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || element.isContentEditable;
}

export function useViewerFindKeyboardShortcut({ scopeId, onOpen, enabled = true }: UseViewerFindKeyboardShortcutOptions) {
  const { activeScope } = useFocusScope();

  useEffect(() => {
    if (!enabled) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== 'f') return;
      if (activeScope !== scopeId) return;
      if (isEditableTarget(event.target)) return;
      event.preventDefault();
      onOpen();
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [activeScope, enabled, onOpen, scopeId]);
}
