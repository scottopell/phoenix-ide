import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiResponseError, api } from '../api';
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

type ReleaseDiscoverySnapshot = Omit<ReleaseUpdateSnapshot, 'transaction'>;

function authorityText(authority: ReleaseUpdateAuthority): string | null {
  switch (authority.kind) {
    case 'allowed': return null;
    case 'remote_browser': return 'Approval is unavailable from this remote browser.';
    case 'not_production': return 'In-app updates are available only for production installations. Use dev.py for local HEAD builds.';
    case 'unsupported_host': return 'This host does not have a supported native deployment backend.';
    case 'unmanaged_installation': return `Updates are disabled because this Phoenix process has no supported runtime owner. ${authority.reason}`;
    case 'ambiguous_installation': return `Updates are disabled because runtime ownership is ambiguous. ${authority.reason}`;
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
      {transaction.release_tag && <div>Approved release: <strong>{transaction.release_tag}</strong></div>}
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

export function ReleaseUpdatePanel({
  onDeploymentChange,
}: {
  onDeploymentChange?: (snapshot: Pick<ReleaseUpdateSnapshot, 'current_version' | 'current_git_sha' | 'installation_ownership'>) => void;
}) {
  const [snapshot, setSnapshot] = useState<ReleaseDiscoverySnapshot | null>(null);
  const [transaction, setTransaction] = useState<ReleaseTransactionStatus | null>(null);
  const [discoveryError, setDiscoveryError] = useState<string | null>(null);
  const [transactionError, setTransactionError] = useState<string | null>(null);
  const [approvalError, setApprovalError] = useState<string | null>(null);
  const [handoffTransactionId, setHandoffTransactionId] = useState<string | null>(null);
  const [reconciliationTransactionId, setReconciliationTransactionId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [approving, setApproving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [confirmedIdentity, setConfirmedIdentity] = useState<string | null>(null);
  const [discoveryCheckedAt, setDiscoveryCheckedAt] = useState<string | null>(null);
  const loadInFlight = useRef(false);
  const mounted = useRef(false);
  const snapshotRef = useRef<ReleaseDiscoverySnapshot | null>(null);
  const transactionRef = useRef<ReleaseTransactionStatus | null>(null);
  const confirmedIdentityRef = useRef<string | null>(null);
  snapshotRef.current = snapshot;
  transactionRef.current = transaction;
  confirmedIdentityRef.current = confirmedIdentity;

  const markDiscoveryStale = useCallback((reason: string) => {
    setDiscoveryError(reason);
    setConfirming(false);
    setConfirmedIdentity(null);
  }, []);

  const mergeTransaction = useCallback((
    current: ReleaseTransactionStatus | null,
    next: ReleaseTransactionStatus,
  ): ReleaseTransactionStatus => {
    if (current?.kind === 'present' && !TERMINAL_STATES.has(current.state) && next.kind !== 'present') {
      setTransactionError(next.kind === 'unreadable'
        ? next.reason
        : 'Durable transaction status temporarily disappeared');
      return current;
    }
    if (next.kind !== 'unreadable') setTransactionError(null);
    return next;
  }, []);

  const load = useCallback(async (refresh = false): Promise<boolean> => {
    if (loadInFlight.current) return false;
    loadInFlight.current = true;
    try {
      const next = await api.releaseUpdateSnapshot(refresh);
      if (!mounted.current) return false;
      const { transaction: nextTransaction, ...nextDiscovery } = next;
      setTransaction((current) => mergeTransaction(current, nextTransaction));
      if (next.preview.kind === 'available') {
        const nextIdentity = `${next.preview.tag}:${next.preview.commit}:${next.preview.asset_sha256}`;
        if (confirmedIdentityRef.current !== null && confirmedIdentityRef.current !== nextIdentity) {
          setConfirming(false);
          setConfirmedIdentity(null);
        }
        setDiscoveryCheckedAt(next.sampled_at);
        setDiscoveryError(null);
        setSnapshot(nextDiscovery);
      } else {
        markDiscoveryStale(next.preview.reason);
        if (snapshotRef.current?.preview.kind !== 'available') {
          setDiscoveryCheckedAt(next.sampled_at);
        }
        setSnapshot((current) => current?.preview.kind === 'available'
          ? { ...nextDiscovery, preview: current.preview }
          : nextDiscovery);
      }
      onDeploymentChange?.(next);
      return true;
    } catch (cause) {
      if (mounted.current) markDiscoveryStale(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      if (mounted.current) setLoading(false);
      loadInFlight.current = false;
    }
  }, [markDiscoveryStale, mergeTransaction, onDeploymentChange]);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => { mounted.current = false; };
  }, [load]);

  const active = transaction?.kind === 'present'
    && !TERMINAL_STATES.has(transaction.state);
  const shouldPollTransaction = handoffTransactionId !== null
    || reconciliationTransactionId !== null
    || (!loading && snapshot === null && transaction === null)
    || transaction?.kind === 'none'
    || transaction?.kind === 'unreadable'
    || active;

  useEffect(() => {
    if (!shouldPollTransaction) return;
    let cancelled = false;
    let timer: number | null = null;
    const poll = async () => {
      try {
        const transaction = await api.releaseUpdateTransaction();
        if (!cancelled && mounted.current) {
          const current = transactionRef.current;
          if (transaction.kind === 'present') {
            setTransaction(transaction);
            setTransactionError(null);
            if (transaction.transaction_id === handoffTransactionId) {
              setHandoffTransactionId(null);
            }
            if (transaction.state === 'committed') {
              setReconciliationTransactionId(transaction.transaction_id);
              if (await load()) setReconciliationTransactionId(null);
            }
          } else if (current?.kind === 'present') {
            setTransactionError(transaction.kind === 'unreadable'
              ? transaction.reason
              : 'Durable transaction status temporarily disappeared');
          } else {
            setTransaction(transaction);
            setTransactionError(transaction.kind === 'unreadable' ? transaction.reason : null);
          }
        }
      } catch (cause) {
        if (!cancelled && mounted.current) {
          setTransactionError(cause instanceof Error ? cause.message : String(cause));
        }
      }
      if (!cancelled) timer = window.setTimeout(() => { void poll(); }, POLL_MS);
    };
    timer = window.setTimeout(() => { void poll(); }, POLL_MS);
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [handoffTransactionId, load, shouldPollTransaction]);

  const approve = useCallback(async () => {
    if (!snapshot || snapshot.preview.kind !== 'available' || discoveryError !== null) return;
    const currentIdentity = `${snapshot.preview.tag}:${snapshot.preview.commit}:${snapshot.preview.asset_sha256}`;
    if (confirmedIdentity !== currentIdentity) {
      setConfirming(false);
      setConfirmedIdentity(null);
      setDiscoveryError('The release preview changed. Review the new identity before approving.');
      return;
    }
    setApproving(true);
    setApprovalError(null);
    let approvedTransactionId: string;
    try {
      const approval = await api.approveReleaseUpdate(
        snapshot.preview.tag,
        snapshot.preview.commit,
        snapshot.preview.asset_name,
        snapshot.preview.asset_sha256,
      );
      approvedTransactionId = approval.transaction_id;
    } catch (cause) {
      if (mounted.current) {
        if (cause instanceof ApiResponseError
          && (cause.status === 409 || cause.code === 'release_discovery_failed')) {
          markDiscoveryStale(cause.message);
          setApprovalError(null);
        } else {
          setApprovalError(cause instanceof Error ? cause.message : String(cause));
        }
        setApproving(false);
      }
      return;
    }
    if (mounted.current) {
      setConfirming(false);
      setHandoffTransactionId(approvedTransactionId);
      setApproving(false);
    }
  }, [confirmedIdentity, discoveryError, markDiscoveryStale, snapshot]);

  const handoffPending = handoffTransactionId !== null;
  const approvalStatusSafe = !handoffPending && transactionError === null && (transaction?.kind === 'none'
    || (transaction?.kind === 'present'
      && TERMINAL_STATES.has(transaction.state)
      && transaction.state !== 'activation_failed_rollback_failed'));
  const availablePreview = snapshot?.preview.kind === 'available' ? snapshot.preview : null;
  const committedReleaseIsPreview = transaction?.kind === 'present'
    && transaction.state === 'committed'
    && availablePreview?.tag === transaction.release_tag
    && availablePreview.commit === transaction.source_commit;
  const discoveryFreshness = discoveryError && snapshot?.preview.kind === 'available'
    ? 'stale'
    : loading
      ? 'loading'
      : snapshot?.preview.kind === 'available'
        ? 'current'
        : 'unavailable';

  return (
    <section className="settings-section release-update" aria-label="Phoenix release updates">
      <div className="settings-section__title-row">
        <div>
          <h3 className="settings-section__title">Phoenix updates</h3>
          <div className="release-update__freshness-row">
            <span className={`release-update__freshness release-update__freshness--${discoveryFreshness}`}>
              {discoveryFreshness === 'loading' ? 'Loading' : discoveryFreshness === 'current' ? 'Current' : discoveryFreshness === 'stale' ? 'Stale' : 'Unavailable'}
              {discoveryCheckedAt && ` · ${new Date(discoveryCheckedAt).toLocaleString()}`}
            </span>
            <span className="release-update__hint">Release discovery changes only when you check.</span>
          </div>
        </div>
        <button type="button" className="settings-inline-btn" onClick={() => { setLoading(true); void load(true); }} disabled={loading}>
          {loading ? 'Checking…' : 'Check for updates'}
        </button>
      </div>

      {discoveryError && (
        <div className="settings-section__error">
          {snapshot?.preview.kind === 'available'
            ? `Release information is stale — ${discoveryError}`
            : `Release information unavailable — ${discoveryError}`}
        </div>
      )}
      {transactionError && (
        <div className="settings-section__error">Transaction status is stale — {transactionError}</div>
      )}
      {approvalError && (
        <div className="settings-section__error">Update approval failed — {approvalError}</div>
      )}
      {handoffPending && (
        <div className="release-update__hint">Approval handed off. Waiting for durable deployment status…</div>
      )}
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
              {discoveryFreshness === 'current' && snapshot.authority.kind === 'allowed' && snapshot.preview.newer_than_current && approvalStatusSafe && !committedReleaseIsPreview && (
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
        </>
      )}
      {active && <div className="release-update__hint">An approved update is in progress. Transaction status refreshes every 2 seconds; Phoenix may disconnect while the native backend activates, verifies, or rolls back.</div>}
      {transaction && <TransactionStatus transaction={transaction} />}
    </section>
  );
}
