import { useState } from 'react';

export interface CommissionReviewApprovalProps {
  brief: string;
  focus: string | null | undefined;
  scope: {
    kind: string;
    repo_root: string;
    base: string;
    head: string;
    dirty: boolean;
    changed_files: number;
    insertions: number;
    deletions: number;
  } | undefined;
  onApprove: () => Promise<void> | void;
  onReject: () => Promise<void> | void;
}

export function CommissionReviewApproval({
  brief,
  focus,
  scope,
  onApprove,
  onReject,
}: CommissionReviewApprovalProps) {
  const [busy, setBusy] = useState<'approve' | 'reject' | null>(null);
  const [settled, setSettled] = useState<'approved' | 'rejected' | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async (kind: 'approve' | 'reject', fn: () => Promise<void> | void) => {
    if (busy || settled) return;
    setBusy(kind);
    setError(null);
    try {
      await fn();
      setSettled(kind === 'approve' ? 'approved' : 'rejected');
    } catch (err) {
      setError(err instanceof Error ? err.message : `Failed to ${kind} review`);
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
            <p>Phoenix reviews committed changes only and refuses dirty working trees before spending review tokens.</p>
            {scope && (
              <ul>
                <li>Target: {scope.kind}</li>
                <li>Repo: {scope.repo_root}</li>
                <li>
                  Proposed base → head: {scope.base} → {scope.head}{' '}
                  <span style={{ color: 'var(--text-muted)' }}>
                    (approved base resolves to its origin ref at review time)
                  </span>
                </li>
                <li>Stats: {scope.changed_files} files, +{scope.insertions}/-{scope.deletions}</li>
              </ul>
            )}
          </section>

          {settled === 'approved' && (
            <section className="task-approval-section" role="status">
              <h3>Approved</h3>
              <p>Starting review… this dialog will close when the conversation state updates.</p>
            </section>
          )}
          {settled === 'rejected' && (
            <section className="task-approval-section" role="status">
              <h3>Rejected</h3>
              <p>Review request rejected.</p>
            </section>
          )}
          {error && (
            <section className="task-approval-section" role="alert">
              <h3>Error</h3>
              <p>{error}</p>
            </section>
          )}
        </main>

        <footer className="task-approval-actions">
          <button
            type="button"
            className="task-approval-secondary"
            disabled={busy !== null || settled !== null}
            onClick={() => void run('reject', onReject)}
          >
            {busy === 'reject' ? 'Rejecting…' : 'Reject'}
          </button>
          <button
            type="button"
            className="task-approval-primary"
            disabled={busy !== null || settled !== null}
            onClick={() => void run('approve', onApprove)}
          >
            {busy === 'approve' ? 'Approving…' : 'Approve review'}
          </button>
        </footer>
      </div>
    </div>
  );
}
