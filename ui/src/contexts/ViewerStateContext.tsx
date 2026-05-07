import { createContext, useCallback, useContext, useMemo } from 'react';
import type { ReactNode } from 'react';
import { useScopedState } from '../hooks/useScopedState';

/**
 * Diff payload mounted by the active diff viewer (split-pane on wide
 * desktop, overlay otherwise). Same shape as the GET diff response.
 */
export interface DiffViewerPayload {
  comparator: string;
  commit_log: string;
  committed_diff: string;
  committed_truncated_kib?: number;
  /** When true, committed_truncated_kib is a lower bound — UI renders
   *  with "≥" prefix. */
  committed_saturated?: boolean;
  uncommitted_diff: string;
  uncommitted_truncated_kib?: number;
  uncommitted_saturated?: boolean;
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
interface DiffViewerStateProviderProps {
  children: ReactNode;
  /**
   * Scope identifier (typically the active conversation slug). When this
   * changes, any open diff payload is dropped so the viewer never shows a
   * diff from the previous scope. Synchronous reset via the "adjust state
   * during render" pattern — the first render after a scope change already
   * has the cleared state.
   */
  scopeKey?: string | undefined;
}

export function DiffViewerStateProvider({ children, scopeKey }: DiffViewerStateProviderProps) {
  const [payload, setPayload] = useScopedState<DiffViewerPayload | null>(scopeKey, null);

  const open = useCallback((p: DiffViewerPayload) => setPayload(p), [setPayload]);
  const close = useCallback(() => setPayload(null), [setPayload]);

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

/* ── Browser view slot (REQ-BT-018) ────────────────────────────────────── */

interface BrowserViewStateValue {
  /** Whether the browser-view panel is currently mounted in the slot. */
  open: boolean;
  /** Server-authoritative: whether `BrowserSessionManager` currently holds a
   *  live session for this conversation. Threaded through from
   *  `atom.conversation.browser_session_active` — single source of truth.
   *  Gates the manual-open affordance and is watched by ConversationPage to
   *  decide auto-mount on the false→true edge. */
  browserSessionActive: boolean;
  openPanel: () => void;
  closePanel: () => void;
}

const BrowserViewStateContext = createContext<BrowserViewStateValue | null>(null);

/**
 * Conversation-scoped browser-view slot. Mutually exclusive with the
 * prose reader (FileExplorerContext) and the diff viewer (above) —
 * ConversationPage owns the resolution rules.
 *
 * `browserSessionActive` is a pass-through prop: the parent reads it from
 * the conversation atom and pushes it down. There is no provider-local
 * mirror; that would be a parallel representation of the same fact.
 */
interface BrowserViewStateProviderProps {
  children: ReactNode;
  /**
   * Scope identifier (typically the active conversation slug). When this
   * changes, the panel is closed so a new conversation never inherits the
   * previous one's panel-open state. Synchronous reset via the "adjust
   * state during render" pattern, matching `DiffViewerStateProvider` and
   * `ReviewNotesProvider`.
   */
  scopeKey?: string | undefined;
  /**
   * Server-authoritative live-session flag (see `BrowserViewStateValue`).
   * Required: when the parent has no atom yet, pass `false`.
   */
  browserSessionActive: boolean;
}

export function BrowserViewStateProvider({
  children,
  scopeKey,
  browserSessionActive,
}: BrowserViewStateProviderProps) {
  const [open, setOpen] = useScopedState(scopeKey, false);

  const openPanel = useCallback(() => setOpen(true), [setOpen]);
  const closePanel = useCallback(() => setOpen(false), [setOpen]);

  const value = useMemo<BrowserViewStateValue>(
    () => ({ open, browserSessionActive, openPanel, closePanel }),
    [open, browserSessionActive, openPanel, closePanel],
  );

  return (
    <BrowserViewStateContext.Provider value={value}>
      {children}
    </BrowserViewStateContext.Provider>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export function useBrowserViewState(): BrowserViewStateValue {
  const ctx = useContext(BrowserViewStateContext);
  if (!ctx) {
    throw new Error(
      'useBrowserViewState must be used inside <BrowserViewStateProvider>. ' +
        'Wrap the conversation page in the provider.',
    );
  }
  return ctx;
}
