import { useState, useEffect } from 'react';
import { api } from '../api';

interface WorkActionsProps {
  conversationId: string;
  convModeLabel: string | undefined;
  /** Live phase type from atom (not stale conversation.display_state) */
  phaseType: string;
  branchName: string | undefined;
  baseBranch: string | null | undefined;
  /** When set, the parent has been continued into another conversation.
   *  REQ-BED-031 forbids abandon / mark-as-merged on a continued parent —
   *  the action belongs on the continuation. Server enforces with 409; UI
   *  disables the controls with a tooltip so the user never sees that
   *  error. */
  continuedInConvId: string | null | undefined;
  /** Send a user message to the conversation (for "ask agent to fix" flows) */
  onSendMessage?: (text: string) => void;
}

export function WorkActions({
  conversationId,
  convModeLabel,
  phaseType,
  continuedInConvId,
}: WorkActionsProps) {
  const [error, setError] = useState<string | null>(null);
  const [markingMerged, setMarkingMerged] = useState(false);
  const [abandoning, setAbandoning] = useState(false);

  // Clear stale errors when the agent runs (phaseType leaves idle then returns)
  useEffect(() => {
    setError(null);
  }, [phaseType]);

  const isBranch = convModeLabel === 'Branch';
  if (convModeLabel !== 'Work' && !isBranch) return null;
  if (phaseType !== 'idle') return null;

  const isLoading = markingMerged || abandoning;
  const hasContinuation = !!continuedInConvId;
  const continuationTooltip = hasContinuation
    ? 'This conversation has been continued. Abandon the continuation instead.'
    : undefined;

  return (
    <div className="work-actions-bar">
      <span className="work-actions-label">Done?</span>
      <button
        className="work-actions-btn work-actions-complete"
        disabled={isLoading || hasContinuation}
        title={continuationTooltip}
        data-testid="mark-merged-button"
        onClick={async () => {
          setError(null);
          setMarkingMerged(true);
          try {
            await api.markMerged(conversationId);
          } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to mark as merged');
          } finally {
            setMarkingMerged(false);
          }
        }}
      >
        {markingMerged ? 'Marking...' : 'Mark as Merged'}
      </button>
      <button
        className="work-actions-btn work-actions-abandon"
        disabled={isLoading || hasContinuation}
        title={continuationTooltip}
        data-testid="abandon-button"
        onClick={async () => {
          const confirmText = isBranch
            ? 'Abandon this conversation? The worktree will be deleted but your branch will be kept.'
            : 'Abandon this task? The worktree and task branch will be deleted.';
          const confirmed = window.confirm(confirmText);
          if (!confirmed) return;
          setError(null);
          setAbandoning(true);
          try {
            await api.abandonTask(conversationId);
          } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to abandon task');
          } finally {
            setAbandoning(false);
          }
        }}
      >
        {abandoning ? 'Abandoning...' : 'Abandon'}
      </button>
      {hasContinuation && (
        <span className="work-actions-continuation-note">
          Continued — actions belong on the continuation.
        </span>
      )}
      {error && (
        <div className="work-actions-error">{error}</div>
      )}
    </div>
  );
}
