import { useId, useState } from 'react';
import './CommissionReviewApproval.css';

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

function scopeKindLabel(kind: string): string {
  return kind.replace(/[_-]+/g, ' ');
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
  const headingId = useId();
  const summaryId = useId();
  const statusId = useId();

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

  const disabled = busy !== null || settled !== null;
  const hasCleanScope = scope ? !scope.dirty : null;
  const outcomeTone = settled === 'approved' ? 'approved' : settled === 'rejected' ? 'rejected' : error ? 'error' : 'pending';

  return (
    <div className="commission-review-approval-backdrop">
      <section
        className={`commission-review-approval commission-review-approval--${outcomeTone}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={headingId}
        aria-describedby={summaryId}
        aria-busy={busy !== null}
      >
        <header className="commission-review-approval__header">
          <div className="commission-review-approval__title-block">
            <p className="commission-review-approval__eyebrow">Capital spend request</p>
            <h2 id={headingId} className="commission-review-approval__title">Commission code review?</h2>
            <p id={summaryId} className="commission-review-approval__summary">
              Review tokens will be spent only after explicit approval. Phoenix reviews committed changes only.
            </p>
          </div>
          <div className="commission-review-approval__state" aria-live="polite">
            <span className={`commission-review-approval__badge commission-review-approval__badge--${outcomeTone}`}>
              {busy === 'approve'
                ? 'Approving…'
                : busy === 'reject'
                  ? 'Rejecting…'
                  : settled === 'approved'
                    ? 'Approved'
                    : settled === 'rejected'
                      ? 'Rejected'
                      : error
                        ? 'Needs retry'
                        : 'Awaiting decision'}
            </span>
          </div>
        </header>

        <main className="commission-review-approval__content">
          <section className="commission-review-approval__panel">
            <h3 className="commission-review-approval__panel-title">Brief</h3>
            <p className="commission-review-approval__body">{brief}</p>
          </section>

          {focus && (
            <section className="commission-review-approval__panel">
              <h3 className="commission-review-approval__panel-title">Focus</h3>
              <p className="commission-review-approval__body">{focus}</p>
            </section>
          )}

          <section className="commission-review-approval__panel">
            <div className="commission-review-approval__panel-header">
              <h3 className="commission-review-approval__panel-title">Scope</h3>
              {scope && (
                <span
                  className={`commission-review-approval__scope-pill ${hasCleanScope ? 'commission-review-approval__scope-pill--clean' : 'commission-review-approval__scope-pill--dirty'}`}
                >
                  {hasCleanScope ? 'Committed-only scope' : 'Dirty tree detected'}
                </span>
              )}
            </div>
            <p className="commission-review-approval__body commission-review-approval__body--muted">
              Approval covers the requested comparison and branch pair only.
            </p>
            {scope ? (
              <dl className="commission-review-approval__facts">
                <div className="commission-review-approval__fact">
                  <dt>Target</dt>
                  <dd>{scopeKindLabel(scope.kind)}</dd>
                </div>
                <div className="commission-review-approval__fact commission-review-approval__fact--wide">
                  <dt>Repository</dt>
                  <dd className="commission-review-approval__code">{scope.repo_root}</dd>
                </div>
                <div className="commission-review-approval__fact">
                  <dt>Base branch</dt>
                  <dd className="commission-review-approval__code">{scope.base}</dd>
                </div>
                <div className="commission-review-approval__fact">
                  <dt>Head branch</dt>
                  <dd className="commission-review-approval__code">{scope.head}</dd>
                </div>
                <div className="commission-review-approval__fact">
                  <dt>Changed files</dt>
                  <dd>{scope.changed_files}</dd>
                </div>
                <div className="commission-review-approval__fact">
                  <dt>Diff stats</dt>
                  <dd>
                    +{scope.insertions} / -{scope.deletions}
                  </dd>
                </div>
              </dl>
            ) : (
              <p className="commission-review-approval__empty">Scope details are still loading.</p>
            )}
          </section>

          <section className="commission-review-approval__panel commission-review-approval__panel--callout">
            <h3 className="commission-review-approval__panel-title">Approval consequences</h3>
            <ul className="commission-review-approval__list">
              <li>Review runs against the proposed base → head comparison.</li>
              <li>Dirty working trees are rejected before review execution.</li>
              <li>Approval cannot be retried from this dialog once a decision succeeds.</li>
            </ul>
          </section>

          {settled === 'approved' && (
            <section id={statusId} className="commission-review-approval__notice commission-review-approval__notice--success" role="status" aria-live="polite">
              <h3 className="commission-review-approval__panel-title">Approved</h3>
              <p>Starting review… this dialog will close when the conversation state updates.</p>
            </section>
          )}
          {settled === 'rejected' && (
            <section id={statusId} className="commission-review-approval__notice commission-review-approval__notice--settled" role="status" aria-live="polite">
              <h3 className="commission-review-approval__panel-title">Rejected</h3>
              <p>Review request rejected. No review tokens will be spent.</p>
            </section>
          )}
          {error && (
            <section id={statusId} className="commission-review-approval__notice commission-review-approval__notice--error" role="alert">
              <h3 className="commission-review-approval__panel-title">Error</h3>
              <p>{error}</p>
            </section>
          )}
        </main>

        <footer className="commission-review-approval__actions">
          <button
            type="button"
            className="commission-review-approval__button commission-review-approval__button--secondary"
            disabled={disabled}
            aria-describedby={error ? statusId : undefined}
            onClick={() => void run('reject', onReject)}
          >
            {busy === 'reject' ? 'Rejecting…' : 'Reject'}
          </button>
          <button
            type="button"
            className="commission-review-approval__button commission-review-approval__button--primary"
            disabled={disabled}
            aria-describedby={error ? statusId : undefined}
            onClick={() => void run('approve', onApprove)}
          >
            {busy === 'approve' ? 'Approving…' : 'Approve review'}
          </button>
        </footer>
      </section>
    </div>
  );
}
