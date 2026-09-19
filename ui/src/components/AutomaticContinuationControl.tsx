import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type AutomaticContinuationAdmission, type AutomaticContinuationView } from '../api';
import './AutomaticContinuationControl.css';

const PHASE_LABELS: Record<AutomaticContinuationAdmission['phase'], string> = {
  admitted: 'Admitted',
  successor_reserved: 'Successor reserved',
  ownership_transferred: 'Ownership transferred',
  dispatch_accepted: 'Dispatch accepted',
  message_settled: 'Continued',
  superseded: 'Continued manually',
  failed: 'Failed',
};

export type AutomaticContinuationScope =
  | { kind: 'ordinary'; reference: string }
  | { kind: 'coordinator' };

interface AutomaticContinuationControlProps {
  scope: AutomaticContinuationScope;
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export function AutomaticContinuationControl({ scope }: AutomaticContinuationControlProps) {
  const [view, setView] = useState<AutomaticContinuationView | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [failedValue, setFailedValue] = useState<boolean | null>(null);
  const [retrying, setRetrying] = useState(false);
  const requestGeneration = useRef(0);
  const viewRevision = useRef(0);
  const savePending = useRef(false);
  const scopeKind = scope.kind;
  const reference = scope.kind === 'ordinary' ? scope.reference : null;

  useEffect(() => {
    const generation = ++requestGeneration.current;
    viewRevision.current += 1;
    let refreshPending = false;
    setView(null);
    setLoading(true);
    setSaving(false);
    setFeedback(null);
    savePending.current = false;
    setFailedValue(null);
    setRetrying(false);
    const refresh = (initial: boolean) => {
      if (refreshPending || savePending.current) return;
      refreshPending = true;
      const revision = ++viewRevision.current;
      const request = scopeKind === 'ordinary'
        ? api.getProductConversationAutomaticContinuation(reference!)
        : api.getCoordinatorAutomaticContinuation();
      void request
        .then((next) => {
          if (requestGeneration.current === generation && viewRevision.current === revision) {
            setView(next);
          }
        })
        .catch((error: unknown) => {
          if (initial && requestGeneration.current === generation) {
            setFeedback(errorMessage(error, 'Failed to load automatic continuation setting'));
          }
        })
        .finally(() => {
          refreshPending = false;
          if (initial && requestGeneration.current === generation) setLoading(false);
        });
    };
    refresh(true);
    const interval = window.setInterval(() => refresh(false), 5_000);
    return () => {
      window.clearInterval(interval);
      requestGeneration.current += 1;
    };
  }, [reference, scopeKind]);

  const save = useCallback(async (enabled: boolean) => {
    const generation = requestGeneration.current;
    const revision = ++viewRevision.current;
    savePending.current = true;
    setSaving(true);
    setFeedback(null);
    setFailedValue(null);
    try {
      const next = scopeKind === 'ordinary'
        ? await api.updateProductConversationAutomaticContinuation(reference!, enabled)
        : await api.updateCoordinatorAutomaticContinuation(enabled);
      if (requestGeneration.current === generation && viewRevision.current === revision) {
        setView(next);
        setFeedback('Saved');
      }
    } catch (error) {
      if (requestGeneration.current === generation && viewRevision.current === revision) {
        setFailedValue(enabled);
        setFeedback(errorMessage(error, 'Failed to save automatic continuation setting'));
      }
    } finally {
      if (requestGeneration.current === generation) {
        savePending.current = false;
        setSaving(false);
      }
    }
  }, [reference, scopeKind]);

  const retryFailedAdmission = useCallback(async () => {
    const admission = view?.admission;
    if (!admission?.actionable_failure) return;
    const generation = requestGeneration.current;
    const authorityLabel = admission.actionable_failure.opening_authority === 'generated_predecessor_context' ? 'generated' : 'manual';
    setRetrying(true);
    setFeedback(null);
    try {
      const response = await api.continueConversation(
        admission.predecessor_transcript_row_id,
        {
          handoff: admission.actionable_failure.accepted_handoff,
          message_id: admission.actionable_failure.first_message_id,
        },
      );
      if (response.status === 'dispatch_failed') {
        throw new Error(response.error ?? `Failed to retry ${authorityLabel} handoff`);
      }
      if (requestGeneration.current === generation) {
        setFeedback(`${authorityLabel === 'generated' ? 'Generated' : 'Manual'} handoff retry accepted`);
      }
    } catch (error) {
      if (requestGeneration.current === generation) {
        setFeedback(errorMessage(error, `Failed to retry ${authorityLabel} handoff`));
      }
    } finally {
      if (requestGeneration.current === generation) setRetrying(false);
    }
  }, [view]);

  const enabled = view?.auto_continue_on_context_exhaustion ?? false;
  const admission = view?.admission ?? null;
  const phaseLabel = admission ? PHASE_LABELS[admission.phase] : null;
  const failedAdmission = admission?.phase === 'failed' ? admission : null;
  const detailsRef = useRef<HTMLDetailsElement>(null);

  useEffect(() => {
    if (failedAdmission?.actionable_failure) detailsRef.current?.setAttribute('open', '');
  }, [failedAdmission?.actionable_failure]);

  return (
    <details ref={detailsRef} className="automatic-continuation" data-testid="automatic-continuation-control">
      <summary>
        Auto-continue <span className={enabled ? 'automatic-continuation__on' : ''}>{loading ? '…' : enabled ? 'On' : 'Off'}</span>
        {admission && <span className="automatic-continuation__phase"> · {phaseLabel}</span>}
      </summary>
      <div className="automatic-continuation__panel">
        <label className="automatic-continuation__toggle">
          <input
            type="checkbox"
            checked={enabled}
            disabled={loading || saving || view === null}
            onChange={(event) => void save(event.currentTarget.checked)}
          />
          <span>Automatically accept future generated handoffs and continue</span>
        </label>
        <p className="automatic-continuation__help">
          Applies only to future entries into context exhaustion. Changing this setting does not start or resume an already-exhausted conversation, or cancel continuation work already admitted.
        </p>
        {admission && (
          <div className={`automatic-continuation__admission automatic-continuation__admission--${admission.phase}`}>
            <strong>{phaseLabel}</strong>
            {admission.phase !== 'message_settled' && admission.phase !== 'superseded' && admission.phase !== 'failed' && (
              <span> · automatic handoff in progress</span>
            )}
            {admission.no_progress_attempts > 0 && (
              <span> · {admission.no_progress_attempts} no-progress {admission.no_progress_attempts === 1 ? 'attempt' : 'attempts'}</span>
            )}
            {failedAdmission?.actionable_failure && (
              <div role="alert" className="automatic-continuation__failure">
                <span>{failedAdmission.actionable_failure.message}</span>
                <pre>{failedAdmission.actionable_failure.accepted_handoff}</pre>
                <span> Retry safely with the same persisted {failedAdmission.actionable_failure.opening_authority === 'generated_predecessor_context' ? 'generated handoff' : 'manual handoff'} and message identity. Automatic continuation remains enabled for future exhaustions.</span>
                <button
                  type="button"
                  disabled={retrying}
                  onClick={() => { void retryFailedAdmission(); }}
                >
                  {retrying ? 'Retrying…' : `Retry ${failedAdmission.actionable_failure.opening_authority === 'generated_predecessor_context' ? 'generated' : 'manual'} handoff`}
                </button>
              </div>
            )}
          </div>
        )}
        {(loading || saving || feedback) && (
          <div className="automatic-continuation__feedback" role={feedback && feedback !== 'Saved' ? 'alert' : 'status'} aria-live="polite">
            {loading ? 'Loading setting…' : saving ? 'Saving…' : feedback}
            {!saving && failedValue !== null && (
              <button type="button" onClick={() => void save(failedValue)}>Retry save</button>
            )}
          </div>
        )}
      </div>
    </details>
  );
}
