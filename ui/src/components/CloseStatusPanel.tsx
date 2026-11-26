import type { ProductConversationSnapshotView } from '../api';
import { displayLossIdentity } from './closeLossIdentity';

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
  onConfirmStopWork,
  onCancel,
  onRetry,
}: {
  snapshot: ProductConversationSnapshotView;
  busy: boolean;
  onConfirmStopWork: () => void;
  onCancel: () => void;
  onRetry: () => void;
}) {
  const close = snapshot.close;
  if (!close || close.phase === 'completed') return null;

  const lossRows = close.losses.map((loss) => (
    <li key={`${loss.scope}:${loss.generation}:${loss.category}:${loss.identity}`}>
      <code>{loss.scope}: {displayLossIdentity(loss.identity)}</code> · {loss.category.replaceAll('_', ' ')}
    </li>
  ));
  const residualRows = close.residuals.map((residual) => (
    <li key={`${residual.scope}:${residual.resource_kind}:${residual.identity}`}>
      <code>{residual.scope}: {residual.resource_kind.replaceAll('_', ' ')} · {residual.identity}</code>
      {' · '}{residual.reason.replaceAll('_', ' ')}
      {residual.detail ? ` — ${residual.detail}` : ''}
    </li>
  ));
  const remainingResourceRows = (close.failure?.remaining_resources ?? []).map((resource, index) => (
    <li key={`${resource.scope}:${resource.resource_kind}:${resource.identity}:${resource.disposition}:${index}`}>
      <code>scope: {resource.scope} · resource_kind: {resource.resource_kind} · identity: {resource.identity} · disposition: {resource.disposition}</code>
    </li>
  ));
  const failure = close.failure && (
    <p>
      {close.failure.reason.replaceAll('_', ' ')} — {close.failure.detail}
      {' · '}{close.failure.stop_certainty.kind.replaceAll('_', ' ')}
    </p>
  );

  if (close.run_status === 'stopped') {
    const shutdownConfirmed = close.failure?.stop_certainty.kind === 'conversation_and_processes_stopped';
    return (
      <section className="work-actions-close-status" aria-label={close.phase === 'needs_repair' ? 'Close repair required' : 'Close incomplete'}>
        <strong>{shutdownConfirmed
          ? '⚠ Close incomplete — cleanup not confirmed'
          : '⚠ Close incomplete — shutdown could not be confirmed'}</strong>
        {lossRows.length > 0 && <><p>Exact work items:</p><ul>{lossRows}</ul></>}
        {residualRows.length > 0 && <ul>{residualRows}</ul>}
        {remainingResourceRows.length > 0 && (
          <>
            <p>Remaining resources ({remainingResourceRows.length}):</p>
            <ol>{remainingResourceRows}</ol>
          </>
        )}
        {failure}
        <p>
          To retry, navigate to the Coordinator and send one explicit fresh safe retry request.
          Nothing is admitted automatically; this request does not retry, use a stale fallback, or discard anything.
        </p>
        <a href="/global/coordinator">Open Coordinator to request retry</a>
        <label>
          <span>Copy this request and send it normally in the Coordinator:</span>
          <textarea
            aria-label="Explicit fresh safe retry request"
            readOnly
            value={`Please make ONE explicit fresh safe retry via close_conversation for ProductConversation ${snapshot.product_conversation_id}, attempt_id ${close.attempt_id}, failed_run_ordinal ${close.run_ordinal}. Do not run an automatic loop, use a stale fallback, or discard anything.`}
          />
        </label>
      </section>
    );
  }

  if (close.phase === 'awaiting_loss_confirmation') {
    return (
      <section className="work-actions-close-status" aria-label="Close incomplete">
        <strong>⚠ Close blocked — content retained</strong>
        <p>Close will not discard these items. Any cleanup must happen separately under the owning agent’s authority.</p>
        {lossRows.length > 0 && <ul>{lossRows}</ul>}
        {residualRows.length > 0 && <ul>{residualRows}</ul>}
        {failure}
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
        {lossRows.length > 0 && <ul>{lossRows}</ul>}
        {residualRows.length > 0 && <ul>{residualRows}</ul>}
        {failure}
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
