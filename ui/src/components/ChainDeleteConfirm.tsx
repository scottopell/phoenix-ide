import { useEffect } from 'react';
import type { ChainView } from '../api';

interface ChainDeleteConfirmProps {
  visible: boolean;
  chain: ChainView | null;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Scope-explicit chain delete confirm. Names every member by chain index,
 * surfaces the worktree count when any member owns one, and is otherwise
 * styled identically to ConfirmDialog. Only mounts when `chain` is non-null
 * — callers gate on the same chain value they pass in.
 */
export function ChainDeleteConfirm({
  visible,
  chain,
  onConfirm,
  onCancel,
}: ChainDeleteConfirmProps) {
  useEffect(() => {
    if (visible) {
      const handleEscape = (e: KeyboardEvent) => {
        if (e.key === 'Escape') onCancel();
      };
      document.addEventListener('keydown', handleEscape);
      return () => document.removeEventListener('keydown', handleEscape);
    }
    return undefined;
  }, [visible, onCancel]);

  if (!visible || !chain) return null;

  const memberCount = chain.members.length;
  const worktreeCount = chain.members.reduce(
    (n, m) => n + (m.has_worktree ? 1 : 0),
    0,
  );
  const memberRefs = chain.members.map((_, idx) => `#${idx + 1}`).join(', ');

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div
        className="modal confirm-dialog chain-delete-confirm"
        onClick={(e) => e.stopPropagation()}
      >
        <h3>Delete chain &ldquo;{chain.display_name}&rdquo;?</h3>
        <div className="confirm-message">
          <p>This will permanently remove:</p>
          <ul className="chain-delete-bullets">
            <li>
              {memberCount} {memberCount === 1 ? 'conversation' : 'conversations'}
              {' '}({memberRefs})
            </li>
            {worktreeCount > 0 && (
              <li>
                {worktreeCount} git {worktreeCount === 1 ? 'worktree' : 'worktrees'}
              </li>
            )}
            <li>All messages and history</li>
          </ul>
          <p className="chain-delete-final">This cannot be undone.</p>
        </div>
        <div className="modal-actions">
          <button className="btn-secondary" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn-danger" onClick={onConfirm}>
            Delete chain
          </button>
        </div>
      </div>
    </div>
  );
}
