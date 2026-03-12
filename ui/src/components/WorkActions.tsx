import { useState, useEffect, useCallback } from 'react';
import { api } from '../api';

interface WorkActionsProps {
  conversationId: string;
  convModeLabel?: string;
  displayState?: string;
  branchName?: string;
  baseBranch?: string | null;
}

type ModalState =
  | { type: 'closed' }
  | { type: 'loading' }
  | { type: 'confirm'; commitMessage: string; taskNotDone: boolean };

export function WorkActions({
  conversationId,
  convModeLabel,
  displayState,
  branchName,
  baseBranch,
}: WorkActionsProps) {
  const [modalState, setModalState] = useState<ModalState>({ type: 'closed' });
  const [editedMessage, setEditedMessage] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [abandoning, setAbandoning] = useState(false);

  // Only render for idle Work conversations
  if (convModeLabel !== 'Work') return null;
  if (displayState !== 'idle') return null;

  const isLoading = modalState.type === 'loading' || confirming || abandoning;

  return (
    <>
      <div className="work-actions">
        <button
          className="work-actions-btn work-actions-complete"
          disabled={isLoading}
          onClick={async () => {
            setError(null);
            setModalState({ type: 'loading' });
            try {
              const result = await api.completeTask(conversationId);
              setEditedMessage(result.commit_message);
              setModalState({
                type: 'confirm',
                commitMessage: result.commit_message,
                taskNotDone: !!result.task_not_done,
              });
            } catch (err) {
              setModalState({ type: 'closed' });
              setError(err instanceof Error ? err.message : 'Failed to prepare completion');
            }
          }}
        >
          {modalState.type === 'loading' ? 'Preparing...' : 'Complete'}
        </button>
        <button
          className="work-actions-btn work-actions-abandon"
          disabled={isLoading}
          onClick={async () => {
            const confirmed = window.confirm(
              'This permanently deletes all work in this worktree. The task will be marked wont-do. This cannot be undone.'
            );
            if (!confirmed) return;
            setError(null);
            setAbandoning(true);
            try {
              await api.abandonTask(conversationId);
              // Terminal state arrives via SSE
            } catch (err) {
              setError(err instanceof Error ? err.message : 'Failed to abandon task');
            } finally {
              setAbandoning(false);
            }
          }}
        >
          {abandoning ? 'Abandoning...' : 'Abandon'}
        </button>
        {error && (
          <span className="work-actions-error">{error}</span>
        )}
      </div>

      {modalState.type === 'confirm' && (
        <CommitModal
          commitMessage={editedMessage}
          onChangeMessage={setEditedMessage}
          taskNotDone={modalState.taskNotDone}
          baseBranch={baseBranch || branchName || 'main'}
          confirming={confirming}
          onConfirm={async () => {
            setConfirming(true);
            try {
              await api.confirmComplete(conversationId, editedMessage);
              setModalState({ type: 'closed' });
              // Terminal state arrives via SSE
            } catch (err) {
              setError(err instanceof Error ? err.message : 'Failed to confirm completion');
            } finally {
              setConfirming(false);
            }
          }}
          onCancel={() => {
            setModalState({ type: 'closed' });
            setEditedMessage('');
          }}
        />
      )}
    </>
  );
}

interface CommitModalProps {
  commitMessage: string;
  onChangeMessage: (msg: string) => void;
  taskNotDone: boolean;
  baseBranch: string;
  confirming: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

function CommitModal({
  commitMessage,
  onChangeMessage,
  taskNotDone,
  baseBranch,
  confirming,
  onConfirm,
  onCancel,
}: CommitModalProps) {
  const [nudgeDismissed, setNudgeDismissed] = useState(false);

  // beforeunload guard
  useEffect(() => {
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
    };
    window.addEventListener('beforeunload', handler);
    return () => window.removeEventListener('beforeunload', handler);
  }, []);

  // Escape key to dismiss
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !confirming) {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      }
    },
    [onCancel, confirming]
  );

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  return (
    <div
      className="commit-modal-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget && !confirming) {
          onCancel();
        }
      }}
    >
      <div className="commit-modal">
        <h3 className="commit-modal-title">Confirm Squash Merge</h3>
        <p className="commit-modal-subtitle">
          Merging into <code>{baseBranch}</code>
        </p>
        {taskNotDone && !nudgeDismissed && (
          <div className="commit-modal-nudge">
            <span>Task file is not marked done. Consider asking the agent to update it before completing.</span>
            <button
              className="commit-modal-nudge-dismiss"
              onClick={() => setNudgeDismissed(true)}
            >
              Dismiss
            </button>
          </div>
        )}
        <textarea
          className="commit-modal-textarea"
          value={commitMessage}
          onChange={(e) => onChangeMessage(e.target.value)}
          rows={8}
          disabled={confirming}
        />
        <div className="commit-modal-actions">
          <button
            className="commit-modal-btn commit-modal-cancel"
            onClick={onCancel}
            disabled={confirming}
          >
            Cancel
          </button>
          <button
            className="commit-modal-btn commit-modal-confirm"
            onClick={onConfirm}
            disabled={confirming || !commitMessage.trim()}
          >
            {confirming ? 'Merging...' : 'Merge'}
          </button>
        </div>
      </div>
    </div>
  );
}
