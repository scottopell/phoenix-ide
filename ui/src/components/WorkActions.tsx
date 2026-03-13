import { useState, useEffect, useCallback } from 'react';
import { api } from '../api';

interface WorkActionsProps {
  conversationId: string;
  convModeLabel: string | undefined;
  /** Live phase type from atom (not stale conversation.display_state) */
  phaseType: string;
  branchName: string | undefined;
  baseBranch: string | null | undefined;
}

type ModalState =
  | { type: 'closed' }
  | { type: 'loading' }
  | { type: 'confirm'; commitMessage: string; taskNotDone: boolean };

export function WorkActions({
  conversationId,
  convModeLabel,
  phaseType,
  branchName,
  baseBranch,
}: WorkActionsProps) {
  const [modalState, setModalState] = useState<ModalState>({ type: 'closed' });
  const [editedMessage, setEditedMessage] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [abandoning, setAbandoning] = useState(false);

  // Only render for idle Work conversations (phaseType is live from atom, not stale)
  if (convModeLabel !== 'Work') return null;
  if (phaseType !== 'idle') return null;

  const isLoading = modalState.type === 'loading' || confirming || abandoning;

  return (
    <>
      <div className="work-actions-bar">
        <span className="work-actions-label">Task complete?</span>
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
          {modalState.type === 'loading' ? 'Preparing...' : 'Merge to ' + (baseBranch || 'main')}
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
        <h3 className="commit-modal-title">Squash merge into <code>{baseBranch}</code></h3>
        {taskNotDone && !nudgeDismissed && (
          <div className="commit-modal-nudge">
            <span>Task file not marked done.</span>
            <button
              className="commit-modal-nudge-dismiss"
              onClick={() => setNudgeDismissed(true)}
            >
              Dismiss
            </button>
          </div>
        )}
        <label className="commit-modal-label">Commit message</label>
        <textarea
          className="commit-modal-textarea"
          value={commitMessage}
          onChange={(e) => onChangeMessage(e.target.value)}
          rows={6}
          disabled={confirming}
          autoFocus
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
