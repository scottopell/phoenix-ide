import { useEffect, useRef, useState } from 'react';
import { api } from '../api';
import type { WakeContractStatus } from '../generated/WakeContractStatus';
import type { WakeStatusSnapshot } from '../generated/WakeStatusSnapshot';
import './WakeIndicator.css';

interface WakeIndicatorProps {
  conversationId: string;
  snapshot: WakeStatusSnapshot | null;
  onError: (message: string) => void;
}

function compactExpiry(expiresAt: string, now = Date.now()): string {
  const remaining = Math.max(0, Date.parse(expiresAt) - now);
  const minutes = Math.ceil(remaining / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.ceil(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.ceil(hours / 24)}d`;
}

function detailedExpiry(expiresAt: string): string {
  const date = new Date(expiresAt);
  return Number.isNaN(date.getTime()) ? expiresAt : date.toLocaleString();
}

function detail(contract: WakeContractStatus): string {
  if (contract.forgotten_reason) return contract.forgotten_reason.replaceAll('_', ' ');
  if (contract.cause) return contract.cause;
  return '—';
}

export function WakeIndicator({ conversationId, snapshot, onError }: WakeIndicatorProps) {
  const [open, setOpen] = useState(false);
  const [cancelling, setCancelling] = useState<Set<string>>(() => new Set());
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [open]);

  useEffect(() => {
    if (!snapshot) return;
    setCancelling((current) => {
      const pending = new Set(snapshot.contracts.filter((c) => c.status === 'pending').map((c) => c.id));
      const next = new Set([...current].filter((id) => pending.has(id)));
      const unchanged = next.size === current.size && [...next].every((id) => current.has(id));
      return unchanged ? current : next;
    });
  }, [snapshot]);

  if (!snapshot || snapshot.contracts.length === 0) return null;

  const hasPending = snapshot.pending_count > 0;

  const cancel = async (contractId: string) => {
    setCancelling((current) => new Set(current).add(contractId));
    try {
      await api.cancelWake(conversationId, contractId);
    } catch (error) {
      setCancelling((current) => {
        const next = new Set(current);
        next.delete(contractId);
        return next;
      });
      onError(error instanceof Error ? error.message : 'Failed to cancel wake');
    }
  };

  return (
    <div className="wake-indicator" ref={rootRef}>
      <button
        type="button"
        className={`wake-indicator__trigger${hasPending ? ' wake-indicator__trigger--pending' : ''}`}
        onClick={(event) => {
          event.stopPropagation();
          setOpen((value) => !value);
        }}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={hasPending
          ? 'Pending wake contracts block archive, delete, abandon, and mark merged. Open to review or cancel.'
          : 'Wake contract history. Open to review terminal outcomes.'}
      >
        <span aria-hidden="true">◷</span>
        <span>{hasPending
          ? `${snapshot.pending_count} wake${snapshot.pending_count === 1 ? '' : 's'}`
          : 'Wake history'}</span>
        {snapshot.soonest_expiry && (
          <span className="wake-indicator__expiry">≤ {compactExpiry(snapshot.soonest_expiry)}</span>
        )}
      </button>
      {open && (
        <div className="wake-indicator__popover" role="dialog" aria-label="Wake contracts">
          <div className="wake-indicator__heading">
            <strong>Wake contracts</strong>
            <span>{hasPending
              ? 'Cancel pending wakes before archiving, deleting, abandoning, or marking merged.'
              : 'Terminal wake outcomes retained for this conversation.'}</span>
          </div>
          <ul className="wake-indicator__list">
            {snapshot.contracts.map((contract) => (
              <li className="wake-indicator__contract" key={contract.id}>
                <div className="wake-indicator__contract-main">
                  <span className={`wake-indicator__status wake-indicator__status--${contract.status}`}>
                    {contract.status}
                  </span>
                  <span className="wake-indicator__handle">
                    {contract.handle.kind === 'tmux_window' ? 'tmux window' : 'bash'} · {contract.handle.id}
                  </span>
                </div>
                <div className="wake-indicator__meta">
                  <span title={contract.id}>contract {contract.id}</span>
                  <span>expires {detailedExpiry(contract.expires_at)}</span>
                  <span>cause {detail(contract)}</span>
                </div>
                {contract.status === 'pending' && (
                  <button
                    type="button"
                    className="wake-indicator__cancel"
                    disabled={cancelling.has(contract.id)}
                    onClick={() => void cancel(contract.id)}
                  >
                    {cancelling.has(contract.id) ? 'Cancelling…' : 'Cancel'}
                  </button>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
