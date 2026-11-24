import type { ProductConversationSnapshotView } from '../api';

function displayLossIdentity(identity: string): string {
  const prefix = 'git_path_bytes_hex_v1:';
  if (!identity.startsWith(prefix)) return identity;
  const hex = identity.slice(prefix.length);
  if (!/^(?:[0-9a-f]{2})+$/i.test(hex)) return identity;
  const bytes = new Uint8Array(hex.match(/../g)?.map((pair) => Number.parseInt(pair, 16)) ?? []);
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return Array.from(bytes, (byte) => byte >= 0x20 && byte < 0x7f
      ? String.fromCharCode(byte)
      : `\\x${byte.toString(16).padStart(2, '0')}`).join('');
  }
}

const CANCELLABLE_CLOSE_PHASES = new Set([
  'awaiting_blocker_resolution',
  'awaiting_stop_work_confirmation',
  'settling_active_work',
  'cancel_requested_during_settlement',
  'awaiting_retirement_inspection',
]);

export function CloseStatusPanel({
  snapshot,
  busy,
  onConfirmLosses,
  onConfirmStopWork,
  onCancel,
  onRetry,
}: {
  snapshot: ProductConversationSnapshotView;
  busy: boolean;
  onConfirmLosses: () => void;
  onConfirmStopWork: () => void;
  onCancel: () => void;
  onRetry: () => void;
}) {
  const close = snapshot.close;
  if (!close || close.run_status === 'stopped' || close.phase === 'completed') return null;

  if (close.phase === 'awaiting_loss_confirmation') {
    return (
      <section className="work-actions-close-status" aria-label="Close loss confirmation">
        <strong>⚠ Closing will discard these exact items</strong>
        <ul>
          {close.losses.map((loss) => (
            <li key={`${loss.scope}:${loss.generation}:${loss.category}:${loss.identity}`}>
              <code>{loss.scope}: {displayLossIdentity(loss.identity)}</code> · {loss.category.replaceAll('_', ' ')}
            </li>
          ))}
        </ul>
        <p>No patch, stash, commit, branch deletion, or PR mutation will be created.</p>
        <button type="button" className="work-actions-btn work-actions-btn--danger" disabled={busy} onClick={onConfirmLosses}>
          {busy ? 'Closing…' : 'Discard exact items and close'}
        </button>
        <button type="button" className="work-actions-btn" disabled={busy} onClick={onCancel}>
          Keep work and cancel Close
        </button>
      </section>
    );
  }

  if (close.phase === 'awaiting_stop_work_confirmation') {
    return (
      <section className="work-actions-close-status" aria-label="Close stop-work confirmation">
        <strong>⚠ Active work must stop before closing</strong>
        <p>Continuing will cancel the active turn for this exact Close attempt.</p>
        <button type="button" className="work-actions-btn work-actions-btn--danger" disabled={busy} onClick={onConfirmStopWork}>
          {busy ? 'Stopping…' : 'Stop work and continue closing'}
        </button>
        <button type="button" className="work-actions-btn" disabled={busy} onClick={onCancel}>
          Keep work and cancel Close
        </button>
      </section>
    );
  }

  if (close.phase === 'needs_repair') {
    return (
      <section className="work-actions-close-status" aria-label="Close repair required">
        <strong>⚠ Close needs repair</strong>
        <p>Phoenix stopped before it could prove every owned resource was retired.</p>
        <ul>
          {close.residuals.map((residual) => (
            <li key={`${residual.scope}:${residual.resource_kind}:${residual.identity}`}>
              <code>{residual.scope}: {residual.resource_kind.replaceAll('_', ' ')} · {residual.identity}</code>
              {' · '}{residual.reason.replaceAll('_', ' ')}
              {residual.detail ? ` — ${residual.detail}` : ''}
            </li>
          ))}
        </ul>
        <button type="button" className="work-actions-btn" disabled={busy} onClick={onRetry}>
          {busy ? 'Retrying…' : 'Retry exact Close attempt'}
        </button>
      </section>
    );
  }

  if (CANCELLABLE_CLOSE_PHASES.has(close.phase)) {
    return (
      <section className="work-actions-close-status" aria-label="Close in progress">
        <strong>… Close in progress</strong>
        <p>{close.phase.replaceAll('_', ' ')}</p>
        <button type="button" className="work-actions-btn" disabled={busy} onClick={onCancel}>
          Keep work and cancel Close
        </button>
      </section>
    );
  }

  return null;
}
