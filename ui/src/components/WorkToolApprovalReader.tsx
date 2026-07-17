import { useState } from 'react';
import { api } from '../api';
import './TaskApprovalReader.css';

interface Props {
  conversationId: string;
  reason: string;
  onClose: () => void;
}

export function WorkToolApprovalReader({ conversationId, reason, onClose }: Props) {
  const [busy, setBusy] = useState<'approve' | 'reject' | null>(null);
  const [error, setError] = useState<string | null>(null);

  const decide = async (approved: boolean) => {
    setBusy(approved ? 'approve' : 'reject');
    setError(null);
    try {
      if (approved) await api.approveWorkTools(conversationId);
      else await api.rejectWorkTools(conversationId);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to resolve request');
      setBusy(null);
    }
  };

  return (
    <div className="task-reader-overlay" role="dialog" aria-modal="true" aria-labelledby="work-tools-title">
      <div className="task-reader">
        <div className="task-reader-header">
          <h2 id="work-tools-title" className="task-reader-title">Full Work toolset requested</h2>
        </div>
        <div className="task-reader-content">
          <p>{reason}</p>
          <p>Approval keeps this conversation in Explore mode, but enables unrestricted Work tools and sandbox permissions for the rest of the conversation.</p>
        </div>
        <div className="task-reader-actions">
          {error && <span className="task-reader-error" role="alert">{error}</span>}
          <button className="task-reader-btn task-reader-btn-secondary" disabled={busy !== null} onClick={() => void decide(false)}>
            {busy === 'reject' ? 'Rejecting…' : 'Reject'}
          </button>
          <button className="task-reader-btn task-reader-btn-primary" disabled={busy !== null} onClick={() => void decide(true)}>
            {busy === 'approve' ? 'Enabling…' : 'Enable full Work tools'}
          </button>
        </div>
      </div>
    </div>
  );
}
