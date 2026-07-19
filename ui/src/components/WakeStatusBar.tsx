import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, type WakeStatus } from '../api';
import './WakeStatusBar.css';

type WakeAnnouncement = {
  kind: 'success' | 'error';
  text: string;
};

function formatExpiry(epochSeconds: number | null): string | null {
  if (epochSeconds == null) return null;
  const deltaMs = epochSeconds * 1000 - Date.now();
  if (deltaMs <= 0) return 'due now';
  const minutes = Math.round(deltaMs / 60_000);
  if (minutes < 1) return 'in under 1 minute';
  if (minutes < 60) return `in ${minutes} minute${minutes === 1 ? '' : 's'}`;
  const hours = Math.round(minutes / 60);
  return `in ${hours} hour${hours === 1 ? '' : 's'}`;
}

function contractActionLabel(contractId: string, cancelling: boolean): string {
  return cancelling ? `Cancelling wake ${contractId}` : `Cancel wake ${contractId}`;
}

export function WakeStatusBar({ conversationId }: { conversationId: string }) {
  const [status, setStatus] = useState<WakeStatus | null>(null);
  const [fetchFailed, setFetchFailed] = useState(false);
  const [cancellingContractId, setCancellingContractId] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState<WakeAnnouncement | null>(null);
  const requestGeneration = useRef(0);

  const refresh = useCallback(async (expectedGeneration = requestGeneration.current): Promise<WakeStatus | undefined> => {
    const generation = expectedGeneration;
    try {
      const next = await api.getWakeStatus(conversationId);
      if (generation === requestGeneration.current) {
        setStatus(next);
        setFetchFailed(false);
      }
      return next;
    } catch {
      if (generation === requestGeneration.current) setFetchFailed(true);
      return undefined;
    }
  }, [conversationId]);

  useEffect(() => {
    requestGeneration.current += 1;
    setStatus(null);
    setFetchFailed(false);
    setAnnouncement(null);
    setCancellingContractId(null);
    void refresh();
    const timer = window.setInterval(() => {
      void refresh();
    }, 5000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const visibleStatus = status;
  const soonestExpiry = useMemo(() => {
    if (!visibleStatus) return null;
    return formatExpiry(visibleStatus.soonest_expires_at);
  }, [visibleStatus]);

  const pendingCount = visibleStatus?.pending_count ?? 0;
  if (!visibleStatus && !fetchFailed && !announcement) return null;
  if (pendingCount === 0 && !fetchFailed && !announcement) return null;

  return (
    <div className={`wake-status-bar${fetchFailed ? ' wake-status-bar--stale' : ''}`}>
      <div className="wake-status-bar__summary" role="status" aria-live="polite">
        {pendingCount > 0 && <span>⏰ {pendingCount} pending wake{pendingCount === 1 ? '' : 's'}</span>}
        {soonestExpiry && <span className="wake-status-bar__expiry">next expires {soonestExpiry}</span>}
        {fetchFailed && (
          <span className="wake-status-bar__stale-badge">
            {visibleStatus ? 'status unavailable • showing last known' : 'status unavailable'}
          </span>
        )}
      </div>
      <div className="wake-status-bar__contracts">
        {(visibleStatus?.contracts ?? []).map((contract) => {
          const cancelling = cancellingContractId === contract.contract_id;
          return (
            <button
              key={contract.workflow_id}
              type="button"
              className="wake-cancel-button"
              title={contractActionLabel(contract.contract_id, cancelling)}
              aria-label={contractActionLabel(contract.contract_id, cancelling)}
              disabled={fetchFailed || cancellingContractId !== null}
              aria-busy={cancelling}
              onClick={async () => {
                const actionGeneration = requestGeneration.current;
                setCancellingContractId(contract.contract_id);
                setAnnouncement(null);
                try {
                  await api.cancelWake(conversationId, contract.contract_id);
                  const next = await refresh(actionGeneration);
                  if (actionGeneration !== requestGeneration.current) return;
                  const resolved = next?.contracts.every((entry) => entry.contract_id !== contract.contract_id);
                  setAnnouncement({
                    kind: 'success',
                    text: resolved
                      ? `Wake ${contract.contract_id} cancelled.`
                      : `Cancel requested for wake ${contract.contract_id}.`,
                  });
                } catch {
                  const next = await refresh(actionGeneration);
                  if (actionGeneration !== requestGeneration.current) return;
                  const alreadyResolved = next?.contracts.every((entry) => entry.contract_id !== contract.contract_id);
                  setAnnouncement({
                    kind: alreadyResolved ? 'success' : 'error',
                    text: alreadyResolved
                      ? `Wake ${contract.contract_id} already resolved.`
                      : `Could not cancel wake ${contract.contract_id}. Wake status ${next ? 'refreshed' : 'unavailable'}.`,
                  });
                } finally {
                  if (actionGeneration === requestGeneration.current) setCancellingContractId(null);
                }
              }}
            >
              {cancelling ? `cancelling ${contract.contract_id}` : `cancel ${contract.contract_id}`}
            </button>
          );
        })}
      </div>
      <div className={`sr-only ${announcement?.kind === 'error' ? 'wake-status-bar__announcement--error' : ''}`} role={announcement?.kind === 'error' ? 'alert' : 'status'} aria-live="polite">
        {announcement?.text ?? ''}
      </div>
    </div>
  );
}
