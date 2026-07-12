import { useEffect } from 'react';
import { useFocusScope } from '../../hooks/useFocusScope';

interface UseViewerFindKeyboardShortcutOptions {
  scopeId: string;
  onOpen: () => void;
  enabled?: boolean;
  allowWhenNoActiveScope?: boolean;
  dialogOpen?: boolean;
}

function isViewerFindInputTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  return Boolean(element instanceof HTMLElement && element.dataset['viewerFindInput'] === 'true');
}

function isEditableTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  if (!element || !(element instanceof HTMLElement)) return false;
  if (isViewerFindInputTarget(element)) return false;
  const tag = element.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || element.isContentEditable;
}

export function useViewerFindKeyboardShortcut({
  scopeId,
  onOpen,
  enabled = true,
  allowWhenNoActiveScope = false,
  dialogOpen = false,
}: UseViewerFindKeyboardShortcutOptions) {
  const { activeScope } = useFocusScope();

  useEffect(() => {
    if (!enabled) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== 'f') return;
      const ownsShortcut = activeScope === scopeId || (allowWhenNoActiveScope && activeScope === null);
      if (!ownsShortcut || dialogOpen) return;
      if (isEditableTarget(event.target)) return;
      event.preventDefault();
      if (isViewerFindInputTarget(event.target)) return;
      onOpen();
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [activeScope, allowWhenNoActiveScope, dialogOpen, enabled, onOpen, scopeId]);
}
