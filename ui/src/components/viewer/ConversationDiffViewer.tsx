import { useEffect, useState } from 'react';
import { AlertCircle, Loader2 } from 'lucide-react';
import { api } from '../../api';
import { ViewerShell } from './ViewerShell';
import { DiffView } from './DiffView';

type DiffPayload = Awaited<ReturnType<typeof api.getConversationDiff>>;

type LoadState =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; payload: DiffPayload };

interface ConversationDiffViewerProps {
  conversationId: string;
  onClose: () => void;
  onSendNotes: (notes: string) => void;
  inline?: boolean | undefined;
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
}: ConversationDiffViewerProps) {
  const [state, setState] = useState<LoadState>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    api
      .getConversationDiff(conversationId)
      .then((payload) => { if (!cancelled) setState({ status: 'ready', payload }); })
      .catch((err: unknown) => {
        if (!cancelled) {
          setState({ status: 'error', message: err instanceof Error ? err.message : 'Failed to load diff' });
        }
      });
    return () => { cancelled = true; };
  }, [conversationId]);

  if (state.status === 'ready') {
    const p = state.payload;
    return (
      <DiffView
        open
        comparator={p.comparator}
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
      />
    );
  }

  return (
    <ViewerShell
      mode={inline ? 'inline' : 'overlay'}
      ariaLabel="Worktree diff"
      title="Diff"
      noteCount={0}
      onToggleNotes={() => undefined}
      onSend={() => undefined}
      onClose={onClose}
    >
      <div className="diff-viewer-body">
        {state.status === 'loading' ? (
          <div className="prose-reader-loading">
            <Loader2 size={32} className="spinning" />
            <span>Loading diff...</span>
          </div>
        ) : (
          <div className="prose-reader-error">
            <AlertCircle size={32} />
            <span>{state.message}</span>
            <button onClick={onClose}>Close</button>
          </div>
        )}
      </div>
    </ViewerShell>
  );
}
