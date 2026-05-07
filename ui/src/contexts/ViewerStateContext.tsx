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
  const [payload, setPayload] = useState<DiffViewerPayload | null>(null);
  const [trackedScope, setTrackedScope] = useState<string | undefined>(scopeKey);

  if (trackedScope !== scopeKey) {
    setTrackedScope(scopeKey);
    if (payload !== null) setPayload(null);
  }

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

/* ── Browser view slot (REQ-BT-018) ────────────────────────────────────── */

interface BrowserViewStateValue {
  /** Whether the browser-view panel is currently mounted in the slot. */
  open: boolean;
  /** Sticky: flips true the first time a `browser_*` tool is observed in
   *  this conversation. Drives the auto-mount-when-slot-empty rule and
   *  also gates the manual-open affordance (no point letting the user open
   *  an empty panel before there's any browser to show). */
  hasActivated: boolean;
  openPanel: () => void;
  closePanel: () => void;
  markActivated: () => void;
}

const BrowserViewStateContext = createContext<BrowserViewStateValue | null>(null);

/**
 * Conversation-scoped browser-view slot. Mutually exclusive with the
 * prose reader (FileExplorerContext) and the diff viewer (above) —
 * ConversationPage owns the resolution rules.
 *
 * `hasActivated` is sticky for the lifetime of the provider (= the lifetime
 * of the conversation page). Auto-mount on first activation is the
 * provider's responsibility-by-implication only: it just exposes the flag
 * and the open/close ops; the page wires them together.
 */
interface BrowserViewStateProviderProps {
  children: ReactNode;
  /**
   * Scope identifier (typically the active conversation slug). When this
   * changes, the panel is closed and `hasActivated` is cleared so a new
   * conversation never inherits the previous one's browser-view state.
   * Synchronous reset via the "adjust state during render" pattern,
   * matching `DiffViewerStateProvider` and `ReviewNotesProvider`.
   */
  scopeKey?: string | undefined;
}

export function BrowserViewStateProvider({ children, scopeKey }: BrowserViewStateProviderProps) {
  const [open, setOpen] = useState(false);
  const [hasActivated, setHasActivated] = useState(false);
  const [trackedScope, setTrackedScope] = useState<string | undefined>(scopeKey);

  if (trackedScope !== scopeKey) {
    setTrackedScope(scopeKey);
    if (open) setOpen(false);
    if (hasActivated) setHasActivated(false);
  }

  const openPanel = useCallback(() => setOpen(true), []);
  const closePanel = useCallback(() => setOpen(false), []);
  const markActivated = useCallback(() => setHasActivated(true), []);

  const value = useMemo<BrowserViewStateValue>(
    () => ({ open, hasActivated, openPanel, closePanel, markActivated }),
    [open, hasActivated, openPanel, closePanel, markActivated],
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
