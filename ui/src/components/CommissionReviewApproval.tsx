import { useState } from 'react';
import './TaskApprovalReader.css';

export interface CommissionReviewApprovalProps {
  brief: string;
  focus?: string | null;
  allowDirtyWorkingTree: boolean;
  onApprove: () => Promise<void> | void;
  onReject: () => Promise<void> | void;
}

export function CommissionReviewApproval({
  brief,
  focus,
  allowDirtyWorkingTree,
  onApprove,
  onReject,
}: CommissionReviewApprovalProps) {
  const [busy, setBusy] = useState<'approve' | 'reject' | null>(null);

  const run = async (kind: 'approve' | 'reject', fn: () => Promise<void> | void) => {
    if (busy) return;
    setBusy(kind);
    try {
      await fn();
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="task-approval-backdrop" role="dialog" aria-modal="true" aria-label="Commission review approval">
      <div className="task-approval-reader">
        <header className="task-approval-header">
          <div>
            <div className="task-approval-eyebrow">Capital spend request</div>
            <h2>Commission code review?</h2>
          </div>
        </header>

        <main className="task-approval-content">
          <section className="task-approval-section">
            <h3>Brief</h3>
            <p>{brief}</p>
          </section>

          {focus && (
            <section className="task-approval-section">
              <h3>Focus</h3>
              <p>{focus}</p>
            </section>
          )}

          <section className="task-approval-section">
            <h3>Scope</h3>
            <p>Phoenix will infer the review target from this conversation/worktree and use the configured default LLM.</p>
            <p>Dirty worktree review: {allowDirtyWorkingTree ? 'explicitly allowed' : 'not allowed'}</p>
          </section>
        </main>

        <footer className="task-approval-actions">
          <button
            type="button"
            className="task-approval-secondary"
            disabled={busy !== null}
            onClick={() => void run('reject', onReject)}
          >
            {busy === 'reject' ? 'Rejecting…' : 'Reject'}
          </button>
          <button
            type="button"
            className="task-approval-primary"
            disabled={busy !== null}
            onClick={() => void run('approve', onApprove)}
          >
            {busy === 'approve' ? 'Approving…' : 'Approve review'}
          </button>
        </footer>
      </div>
    </div>
  );
}
