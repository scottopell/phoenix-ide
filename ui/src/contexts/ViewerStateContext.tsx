import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

/**
 * Diff payload mounted by the active diff viewer (split-pane on wide
 * desktop, overlay otherwise). Same shape as the GET diff response.
 */
export interface DiffViewerPayload {
  comparator: string;
  commit_log: string;
  committed_diff: string;
  committed_truncated_kib?: number;
  uncommitted_diff: string;
  uncommitted_truncated_kib?: number;
}

interface DiffViewerStateValue {
  payload: DiffViewerPayload | null;
  open: (payload: DiffViewerPayload) => void;
  close: () => void;
}

const DiffViewerStateContext = createContext<DiffViewerStateValue | null>(null);

/**
 * Conversation-scoped diff-viewer slot. Lifted out of WorkActions so
 * the viewer can be rendered by ConversationPage at the appropriate
 * location (split pane on wide desktop, overlay on narrow / mobile)
 * instead of always being a centered modal.
 *
 * Single-slot model: the file viewer (FileExplorerContext) and the
 * diff viewer are mutually exclusive. When one opens, ConversationPage
 * closes the other so the user always sees a single viewer beside the
 * chat.
 */
export function DiffViewerStateProvider({ children }: { children: ReactNode }) {
  const [payload, setPayload] = useState<DiffViewerPayload | null>(null);

  const open = useCallback((p: DiffViewerPayload) => setPayload(p), []);
  const close = useCallback(() => setPayload(null), []);

  const value = useMemo<DiffViewerStateValue>(
    () => ({ payload, open, close }),
    [payload, open, close],
  );

  return (
    <DiffViewerStateContext.Provider value={value}>
      {children}
    </DiffViewerStateContext.Provider>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function useDiffViewerState(): DiffViewerStateValue {
  const ctx = useContext(DiffViewerStateContext);
  if (!ctx) {
    throw new Error(
      'useDiffViewerState must be used inside <DiffViewerStateProvider>. ' +
        'Wrap the conversation page in the provider.',
    );
  }
  return ctx;
}
