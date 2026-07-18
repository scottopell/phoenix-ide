import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../api';
import type { ReleaseTransactionStatus } from '../generated/ReleaseTransactionStatus';
import type { ReleaseUpdateAuthority } from '../generated/ReleaseUpdateAuthority';
import type { ReleaseUpdateSnapshot } from '../generated/ReleaseUpdateSnapshot';
import './ReleaseUpdatePanel.css';

const POLL_MS = 2_000;
const TERMINAL_STATES = new Set([
  'committed',
  'precondition_failed',
  'activation_failed_rolled_back',
  'activation_failed_rollback_failed',
  'rejected_concurrent',
]);

function authorityText(authority: ReleaseUpdateAuthority): string | null {
  switch (authority.kind) {
    case 'allowed': return null;
    case 'remote_browser': return 'Updates can be reviewed remotely, but approval is available only from a browser on the Phoenix host.';
    case 'not_production': return 'In-app updates are available only for production installations. Use dev.py for local HEAD builds.';
    case 'unsupported_host': return 'This host does not have a supported native deployment backend.';
    case 'missing_prerequisite': return authority.reason;
  }
}

function stateText(state: string): string {
  return ({
    preparing: 'Preparing verified candidate',
    prepared: 'Candidate prepared',
    handed_off: 'Activation handed off',
    activating: 'Activating and verifying',
    committed: 'Update committed',
    precondition_failed: 'Preparation failed before disruption',
    activation_failed_rolled_back: 'Activation failed; previous release restored and verified',
    activation_failed_rollback_failed: 'Activation and rollback failed — offline recovery required',
    rejected_concurrent: 'Another deployment already owns the host claim',
  } as Record<string, string>)[state] ?? state.replaceAll('_', ' ');
}

function statusTone(transaction: ReleaseTransactionStatus): string {
  if (transaction.kind !== 'present') return 'muted';
  if (transaction.state === 'committed') return 'success';
  if (transaction.state === 'activation_failed_rolled_back') return 'warning';
  if (TERMINAL_STATES.has(transaction.state)) return 'danger';
  return 'active';
}

function TransactionStatus({ transaction }: { transaction: ReleaseTransactionStatus }) {
  if (transaction.kind === 'none') {
    return <div className="release-update__hint">No deployment transaction has been recorded on this host.</div>;
  }
  if (transaction.kind === 'unreadable') {
    return <div className="release-update__status release-update__status--danger">✗ {transaction.reason}</div>;
  }
  return (
    <div className={`release-update__status release-update__status--${statusTone(transaction)}`}>
      <div className="release-update__status-head">
        <strong>{TERMINAL_STATES.has(transaction.state) ? '●' : '…'} {stateText(transaction.state)}</strong>
        <code>{transaction.transaction_id}</code>
      </div>
      {(transaction.expected_version || transaction.expected_git_sha) && (
        <div>Runtime identity: {transaction.expected_version ?? 'unknown'} <code>{transaction.expected_git_sha ?? 'unknown'}</code></div>
      )}
      {transaction.source_commit && <div>Approved source commit: <code>{transaction.source_commit}</code></div>}
      {transaction.updated_at && <div>Updated: {new Date(transaction.updated_at).toLocaleString()}</div>}
      {transaction.failure && <div>Failure: {transaction.failure}</div>}
      {transaction.rollback_failure && <div>Rollback failure: {transaction.rollback_failure}</div>}
      {transaction.stale && <div className="release-update__recovery">Status is stale. Inspect the backend deployment log and use <code>./dev.py prod status</code> for offline recovery.</div>}
      {transaction.state === 'activation_failed_rollback_failed' && (
        <div className="release-update__recovery">The deployment claim remains retained. Do not clear it until the installed runtime and backend owner are inspected offline.</div>
      )}
    </div>
  );
}

export function ReleaseUpdatePanel() {
  const [snapshot, setSnapshot] = useState<ReleaseUpdateSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [approving, setApproving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [confirmedIdentity, setConfirmedIdentity] = useState<string | null>(null);
  const loadInFlight = useRef(false);

  const load = useCallback(async (refresh = false) => {
    if (loadInFlight.current) return;
    loadInFlight.current = true;
    try {
      const next = await api.releaseUpdateSnapshot(refresh);
      if (next.preview.kind === 'available') {
        const nextIdentity = `${next.preview.tag}:${next.preview.commit}:${next.preview.asset_sha256}`;
        if (confirmedIdentity !== null && confirmedIdentity !== nextIdentity) {
          setConfirming(false);
          setConfirmedIdentity(null);
        }
      }
      setSnapshot(next);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
      loadInFlight.current = false;
    }
  }, [confirmedIdentity]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => { void load(); }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const approve = useCallback(async () => {
    if (!snapshot || snapshot.preview.kind !== 'available') return;
    const currentIdentity = `${snapshot.preview.tag}:${snapshot.preview.commit}:${snapshot.preview.asset_sha256}`;
    if (confirmedIdentity !== currentIdentity) {
      setConfirming(false);
      setConfirmedIdentity(null);
      setError('The release preview changed. Review the new identity before approving.');
      return;
    }
    setApproving(true);
    setError(null);
    try {
      await api.approveReleaseUpdate(
        snapshot.preview.tag,
        snapshot.preview.commit,
        snapshot.preview.asset_name,
        snapshot.preview.asset_sha256,
      );
      setConfirming(false);
      await load();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setApproving(false);
    }
  }, [confirmedIdentity, load, snapshot]);

  const active = snapshot?.transaction.kind === 'present'
    && !TERMINAL_STATES.has(snapshot.transaction.state);
  const approvalStatusSafe = snapshot?.transaction.kind === 'none'
    || (snapshot?.transaction.kind === 'present'
      && TERMINAL_STATES.has(snapshot.transaction.state)
      && snapshot.transaction.state !== 'activation_failed_rollback_failed');
  const availablePreview = snapshot?.preview.kind === 'available' ? snapshot.preview : null;

  return (
    <section className="settings-section release-update" aria-label="Phoenix release updates">
      <div className="settings-section__title-row">
        <div>
          <h3 className="settings-section__title">Phoenix updates</h3>
          {snapshot && <div className="release-update__hint">{snapshot.backend.replaceAll('_', ' ')} · running {snapshot.current_version} <code>{snapshot.current_git_sha}</code></div>}
        </div>
        <button type="button" className="settings-inline-btn" onClick={() => { setLoading(true); void load(true); }} disabled={loading}>
          {loading ? 'Checking…' : 'Check for updates'}
        </button>
      </div>

      {error && <div className="settings-section__error">{error}</div>}
      {!snapshot && loading && <div className="settings-section__hint">Resolving the latest stable published release…</div>}

      {snapshot && (
        <>
          {snapshot.preview.kind === 'unavailable' ? (
            <div className="release-update__status release-update__status--danger">Release discovery unavailable: {snapshot.preview.reason}</div>
          ) : (
            <div className="release-update__candidate">
              <div className="release-update__candidate-head">
                <div>
                  <span className={snapshot.preview.newer_than_current ? 'release-update__badge release-update__badge--available' : 'release-update__badge'}>
                    {snapshot.preview.newer_than_current ? 'Update available' : 'Latest stable'}
                  </span>
                  <h4>{snapshot.preview.tag}</h4>
                </div>
                <a href={snapshot.preview.release_url} target="_blank" rel="noreferrer">View release ↗</a>
              </div>
              <div className="release-update__identity">
                <div><span>Commit</span><code title={snapshot.preview.commit}>{snapshot.preview.commit}</code></div>
                <div><span>Asset</span><code>{snapshot.preview.asset_name}</code></div>
                <div><span>SHA-256</span><code title={snapshot.preview.asset_sha256}>{snapshot.preview.asset_sha256}</code></div>
              </div>
              {snapshot.preview.notes && <details><summary>Release notes</summary><pre>{snapshot.preview.notes}</pre></details>}
              {authorityText(snapshot.authority) && <div className="release-update__hint">{authorityText(snapshot.authority)}</div>}
              {snapshot.authority.kind === 'allowed' && snapshot.preview.newer_than_current && approvalStatusSafe && (
                confirming ? (
                  <div className="release-update__confirm">
                    <span>Install {snapshot.preview.tag}? Phoenix will reconnect after backend-owned verification or rollback.</span>
                    <button type="button" className="settings-inline-btn release-update__approve" onClick={() => { void approve(); }} disabled={approving}>
                      {approving ? 'Preparing handoff…' : 'Approve and install'}
                    </button>
                    <button type="button" className="settings-inline-btn" onClick={() => { setConfirming(false); setConfirmedIdentity(null); }} disabled={approving}>Cancel</button>
                  </div>
                ) : (
                  <button type="button" className="settings-inline-btn release-update__approve" onClick={() => {
                    if (!availablePreview) return;
                    setConfirmedIdentity(`${availablePreview.tag}:${availablePreview.commit}:${availablePreview.asset_sha256}`);
                    setConfirming(true);
                  }}>Review and install {snapshot.preview.tag}</button>
                )
              )}
            </div>
          )}
          {active && <div className="release-update__hint">An approved update is in progress. Phoenix may disconnect while the native backend activates, verifies, or rolls back; this status restores after reconnect.</div>}
          <TransactionStatus transaction={snapshot.transaction} />
        </>
      )}
    </section>
  );
}
