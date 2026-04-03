import { useState, useEffect, useCallback } from 'react';
import { api, DirtyMainError } from '../api';

interface WorkActionsProps {
  conversationId: string;
  convModeLabel: string | undefined;
  /** Live phase type from atom (not stale conversation.display_state) */
  phaseType: string;
  branchName: string | undefined;
  baseBranch: string | null | undefined;
  /** Send a user message to the conversation (for "ask agent to fix" flows) */
  onSendMessage?: (text: string) => void;
}

type ModalState =
  | { type: 'closed' }
  | { type: 'loading' }
  | { type: 'confirm'; commitMessage: string; taskNotDone: boolean; autoStash?: boolean | undefined };

export function WorkActions({
  conversationId,
  convModeLabel,
  phaseType,
  branchName,
  baseBranch,
  onSendMessage,
}: WorkActionsProps) {
  const [modalState, setModalState] = useState<ModalState>({ type: 'closed' });
  const [editedMessage, setEditedMessage] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [dirtyMainInfo, setDirtyMainInfo] = useState<{ files: string[]; canAutoStash: boolean } | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [abandoning, setAbandoning] = useState(false);

  // Clear stale errors when the agent runs (phaseType leaves idle then returns)
  useEffect(() => {
    setError(null);
    setDirtyMainInfo(null);
  }, [phaseType]);

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
              if (err instanceof DirtyMainError) {
                setError(err.message);
                setDirtyMainInfo({ files: err.dirtyFiles, canAutoStash: err.canAutoStash });
              } else {
                setDirtyMainInfo(null);
                setError(err instanceof Error ? err.message : 'Failed to prepare completion');
              }
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
          <div className="work-actions-error">
            {error}
            {dirtyMainInfo && (
              <div className="work-actions-dirty-details">
                <div className="work-actions-dirty-files">
                  {dirtyMainInfo.files.map((f, i) => (
                    <div key={i} className="work-actions-dirty-file">{f}</div>
                  ))}
                </div>
                {dirtyMainInfo.canAutoStash && (
                  <button
                    className="work-actions-btn work-actions-stash"
                    disabled={isLoading}
                    onClick={async () => {
                      setError(null);
                      setDirtyMainInfo(null);
                      setModalState({ type: 'loading' });
                      try {
                        const result = await api.completeTask(conversationId);
                        setEditedMessage(result.commit_message);
                        setModalState({
                          type: 'confirm',
                          commitMessage: result.commit_message,
                          taskNotDone: !!result.task_not_done,
                          autoStash: true,
                        });
                      } catch (err2) {
                        setModalState({ type: 'closed' });
                        setError(err2 instanceof Error ? err2.message : 'Failed to prepare completion');
                      }
                    }}
                  >
                    Stash and merge
                  </button>
                )}
              </div>
            )}
          </div>
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
              await api.confirmComplete(conversationId, editedMessage, modalState.autoStash);
              setModalState({ type: 'closed' });
              setError(null);
            } catch (err) {
              setError(err instanceof Error ? err.message : 'Failed to confirm completion');
            } finally {
              setConfirming(false);
            }
          }}
          onCancel={() => {
            setModalState({ type: 'closed' });
            setEditedMessage('');
            setError(null);
          }}
          onAskAgentMarkDone={onSendMessage ? () => {
            setModalState({ type: 'closed' });
            setEditedMessage('');
            onSendMessage(
              'The task file is not marked as done yet. Please update the task file status to "done" so I can merge the work.'
            );
          } : undefined}
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
  onAskAgentMarkDone?: (() => void) | undefined;
}

function CommitModal({
  commitMessage,
  onChangeMessage,
  taskNotDone,
  baseBranch,
  confirming,
  onConfirm,
  onCancel,
  onAskAgentMarkDone,
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
            <div className="commit-modal-nudge-actions">
              {onAskAgentMarkDone && (
                <button
                  className="commit-modal-nudge-fix"
                  onClick={onAskAgentMarkDone}
                  disabled={confirming}
                >
                  Ask agent to fix
                </button>
              )}
              <button
                className="commit-modal-nudge-dismiss"
                onClick={() => setNudgeDismissed(true)}
              >
                Dismiss
              </button>
            </div>
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
