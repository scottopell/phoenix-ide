import { useMemo } from 'react';
import { useKeyboardRouterShortcut } from '../../hooks/useFocusScope';

interface UseViewerFindKeyboardShortcutOptions {
  scopeId: string;
  onOpen: () => void;
  enabled?: boolean;
  allowWhenNoActiveScope?: boolean;
  dialogOpen?: boolean;
}

export function useViewerFindKeyboardShortcut({
  scopeId,
  onOpen,
  enabled = true,
  allowWhenNoActiveScope = false,
  dialogOpen = false,
}: UseViewerFindKeyboardShortcutOptions) {
  const registration = useMemo(() => ({
    id: `viewer-find:${scopeId}`,
    layer: 'viewer' as const,
    key: 'mod+f' as const,
    scopeId,
    enabled,
    allowWhenNoActiveScope,
    dialogOpen,
    handler: (event: KeyboardEvent) => {
      event.preventDefault();
      onOpen();
    },
  }), [allowWhenNoActiveScope, dialogOpen, enabled, onOpen, scopeId]);

  useKeyboardRouterShortcut(registration);
}
