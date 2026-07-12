import { useCallback, useEffect, useState } from 'react';
import { AlertCircle, Loader2 } from 'lucide-react';
import { api } from '../../api';
import { ViewerShell } from './ViewerShell';
import { DiffView } from './DiffView';

type DiffPayload = Awaited<ReturnType<typeof api.getConversationDiff>>;

// The resolved variants carry the conversationId they were fetched for. The
// provider stays mounted across conversation switches (only the prop changes),
// so a stale `ready`/`error` from the previous conversation could otherwise
// render for one frame before the refetch effect runs. Gating on a matching
// conversationId makes that cross-conversation render structurally impossible.
type LoadState =
  | { status: 'loading' }
  | { status: 'error'; message: string; conversationId: string }
  | { status: 'ready'; payload: DiffPayload; conversationId: string };

interface ConversationDiffViewerProps {
  conversationId: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  inline?: boolean | undefined;
  takeover?: boolean | undefined;
  target?: 'workspace' | 'active_pr' | undefined;
}

/**
 * Diff loader for the `diff` viewer slot. The slot carries no payload — only
 * the conversation identity (the diff endpoint is conversation-keyed). This
 * fetches the diff on mount and renders DiffView, so the diff survives cold
 * reload via the URL (`?viewer=diff`) just like prose, instead of living in
 * client state that a refresh discards.
 */
export function ConversationDiffViewer({
  conversationId,
  onClose,
  onSendNotes,
  inline,
  takeover,
  target = 'workspace',
}: ConversationDiffViewerProps) {
  const [state, setState] = useState<LoadState>({ status: 'loading' });

  const loadDiff = useCallback(() => {
    setState({ status: 'loading' });
    return (target === 'active_pr'
      ? api.getActivePrDiff(conversationId)
      : api.getConversationDiff(conversationId))
      .then((payload) => ({ ok: true as const, payload }))
      .catch((err: unknown) => ({ ok: false as const, message: err instanceof Error ? err.message : 'Failed to load diff' }));
  }, [conversationId, target]);

  useEffect(() => {
    let cancelled = false;
    void loadDiff().then((result) => {
      if (cancelled) return;
      if (result.ok) setState({ status: 'ready', payload: result.payload, conversationId });
      else setState({ status: 'error', message: result.message, conversationId });
    });
    return () => { cancelled = true; };
  }, [conversationId, loadDiff]);

  // Treat a resolved state from a previous conversation as still-loading until
  // the effect refetches for the current conversationId.
  const resolved = state.status !== 'loading' && state.conversationId === conversationId;

  if (resolved && state.status === 'ready') {
    const p = state.payload;
    return (
      <DiffView
        open
        comparator={p.comparator}
        label={p.label ?? (target === 'active_pr' ? 'PR Diff' : 'Workspace Diff')}
        commitLog={p.commit_log}
        committedDiff={p.committed_diff}
        committedTruncatedKib={p.committed_truncated_kib}
        committedSaturated={p.committed_saturated}
        uncommittedDiff={p.uncommitted_diff}
        uncommittedTruncatedKib={p.uncommitted_truncated_kib}
        uncommittedSaturated={p.uncommitted_saturated}
        onClose={onClose}
        onSendNotes={onSendNotes}
        {...(inline !== undefined ? { inline } : {})}
        {...(takeover !== undefined ? { takeover } : {})}
      />
    );
  }

  return (
    <ViewerShell
      mode={inline ? 'inline' : takeover ? 'takeover' : 'overlay'}
      ariaLabel={target === 'active_pr' ? 'Pull request diff' : 'Worktree diff'}
      title={target === 'active_pr' ? 'PR Diff' : 'Workspace Diff'}
      noteCount={0}
      onToggleNotes={() => undefined}
      onSend={() => undefined}
      onClose={onClose}
    >
      <div className="diff-viewer-body">
        {resolved && state.status === 'error' ? (
          <div className="viewer-error">
            <AlertCircle size={32} />
            <span>{state.message}</span>
            {target === 'active_pr' && <span>Compare against the selected PR base branch when available.</span>}
            <div className="viewer-error-actions">
              <button onClick={() => { void loadDiff().then((result) => {
                if (result.ok) setState({ status: 'ready', payload: result.payload, conversationId });
                else setState({ status: 'error', message: result.message, conversationId });
              }); }}>Retry</button>
              <button onClick={onClose}>Close</button>
            </div>
          </div>
        ) : (
          <div className="viewer-loading">
            <Loader2 size={32} className="spinning" />
            <span>Loading diff...</span>
          </div>
        )}
      </div>
    </ViewerShell>
  );
}
