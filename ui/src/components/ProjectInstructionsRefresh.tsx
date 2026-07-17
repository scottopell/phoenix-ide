import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { api, ConflictError, type ConversationState } from '../api';
import type { ProjectInstructionRefreshStatus } from '../generated/ProjectInstructionRefreshStatus';
import type { ProjectInstructionSourceChangeKind } from '../generated/ProjectInstructionSourceChangeKind';
import './ProjectInstructionsRefresh.css';

interface ProjectInstructionsRefreshProps {
  conversationId: string;
  conversationState: ConversationState;
  activationMessageId?: string | undefined;
}

function hasChanges(status: ProjectInstructionRefreshStatus): boolean {
  return status.changed_manifest.guidance.length > 0 || status.changed_manifest.skills.length > 0;
}

function estimateLabel(tokens: number): string {
  return `~${Math.max(1, Math.round(tokens / 1000))}K input tokens`;
}

function ChangeStatus({ status }: { status: ProjectInstructionSourceChangeKind }) {
  return <span className={`project-instructions-change-status ${status}`}>{status}</span>;
}

export function ProjectInstructionsRefresh({
  conversationId,
  conversationState,
  activationMessageId,
}: ProjectInstructionsRefreshProps) {
  const [status, setStatus] = useState<ProjectInstructionRefreshStatus | null>(null);
  const [preview, setPreview] = useState<ProjectInstructionRefreshStatus | null>(null);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [checking, setChecking] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const conversationGenerationRef = useRef(0);
  const statusRequestRef = useRef(0);
  const previewRequestRef = useRef(0);
  const statusAbortRef = useRef<AbortController | null>(null);
  const previewAbortRef = useRef<AbortController | null>(null);
  const previewInFlightRef = useRef(false);

  useLayoutEffect(() => {
    conversationGenerationRef.current += 1;
    statusRequestRef.current += 1;
    previewRequestRef.current += 1;
    statusAbortRef.current?.abort();
    previewAbortRef.current?.abort();
    previewInFlightRef.current = false;
    setStatus(null);
    setPreview(null);
    setLoadingPreview(false);
    setChecking(false);
    setConfirming(false);
    setError(null);
  }, [conversationId]);

  useEffect(() => () => {
    statusAbortRef.current?.abort();
    previewAbortRef.current?.abort();
  }, []);

  const refreshStatus = useCallback(async (explicitCheck = false) => {
    const generation = conversationGenerationRef.current;
    const requestId = ++statusRequestRef.current;
    statusAbortRef.current?.abort();
    const controller = new AbortController();
    statusAbortRef.current = controller;
    if (explicitCheck) setChecking(true);
    try {
      const next = await api.getProjectInstructionRefreshStatus(conversationId, controller.signal);
      if (generation !== conversationGenerationRef.current || requestId !== statusRequestRef.current) return null;
      setStatus(next);
      return next;
    } catch (requestError) {
      if (!(requestError instanceof DOMException && requestError.name === 'AbortError')) return null;
      return null;
    } finally {
      if (generation === conversationGenerationRef.current && requestId === statusRequestRef.current) {
        if (explicitCheck) setChecking(false);
      }
    }
  }, [conversationId]);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus, conversationState.type, activationMessageId]);

  useEffect(() => {
    if (!preview) return;
    confirmRef.current?.focus();
    const trigger = triggerRef.current;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setPreview(null);
        setError(null);
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), details > summary, [href], input:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ) ?? []);
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      trigger?.focus();
    };
  }, [preview]);

  const requestPreview = useCallback(async (stale = false) => {
    if (previewInFlightRef.current) return null;
    previewInFlightRef.current = true;
    const generation = conversationGenerationRef.current;
    const requestId = ++previewRequestRef.current;
    previewAbortRef.current?.abort();
    const controller = new AbortController();
    previewAbortRef.current = controller;
    setLoadingPreview(true);
    if (!stale) setError(null);
    try {
      const next = await api.previewProjectInstructionRefresh(conversationId, controller.signal);
      if (generation !== conversationGenerationRef.current || requestId !== previewRequestRef.current) return null;
      setStatus(next);
      if (next.candidate_bundle_id && hasChanges(next)) {
        setPreview(next);
        if (stale) {
          setError('The preview changed while it was open. Review these newer changes before confirming again.');
        }
      } else {
        setPreview(null);
      }
      return next;
    } catch (requestError) {
      if (!(requestError instanceof DOMException && requestError.name === 'AbortError')) {
        setError('Could not load the project-instruction preview. Try again.');
      }
      return null;
    } finally {
      if (generation === conversationGenerationRef.current && requestId === previewRequestRef.current) {
        previewInFlightRef.current = false;
        setLoadingPreview(false);
      }
    }
  }, [conversationId]);

  const confirm = async () => {
    const reviewedPreview = preview;
    const candidateId = reviewedPreview?.candidate_bundle_id;
    if (!candidateId || confirming) return;
    const generation = conversationGenerationRef.current;
    const reviewedRequestId = previewRequestRef.current;
    setConfirming(true);
    setError(null);
    try {
      const response = await api.confirmProjectInstructionRefresh(conversationId, candidateId);
      if (
        generation !== conversationGenerationRef.current
        || reviewedRequestId !== previewRequestRef.current
      ) return;
      setStatus(response.status);
      setPreview(null);
    } catch (confirmError) {
      if (generation !== conversationGenerationRef.current) return;
      if (confirmError instanceof ConflictError && confirmError.detail.error_type === 'stale_project_instruction_candidate') {
        await requestPreview(true);
      } else {
        setError(confirmError instanceof Error ? confirmError.message : 'Could not queue the refresh.');
      }
    } finally {
      if (generation === conversationGenerationRef.current) setConfirming(false);
    }
  };

  const changed = status ? hasChanges(status) : false;
  const working = conversationState.type !== 'idle';
  const closePreview = () => {
    setPreview(null);
    setError(null);
  };

  return (
    <div className="project-instructions-refresh">
      <span className="project-instructions-label">Project instructions</span>
      {status?.is_queued && (
        <span className="project-instructions-queued">queued for next user turn</span>
      )}
      {changed ? (
        <>
          <span className="project-instructions-changed">↻ changed</span>
          <button ref={triggerRef} type="button" className="project-instructions-refresh-button" onClick={() => void requestPreview()} disabled={loadingPreview}>
            {loadingPreview ? 'Checking…' : status?.is_queued ? 'Review newer changes' : 'Review changes'}
          </button>
        </>
      ) : (
        <>
          {!status?.is_queued && <span className="project-instructions-current" aria-label="Project instructions current">current</span>}
          <button ref={triggerRef} type="button" className="project-instructions-check-button" onClick={() => void refreshStatus(true)} disabled={checking}>
            {checking ? 'Checking…' : 'Check'}
          </button>
        </>
      )}

      {preview && (
        <div className="project-instructions-dialog-backdrop" onMouseDown={(event) => {
          if (event.target === event.currentTarget) closePreview();
        }}>
          <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="project-instructions-dialog-title" className="project-instructions-dialog">
            <div className="project-instructions-dialog-header">
              <h2 id="project-instructions-dialog-title">Refresh project instructions?</h2>
              <button type="button" className="project-instructions-dialog-close" aria-label="Close" onClick={closePreview}>×</button>
            </div>
            <p className="project-instructions-dialog-copy">
              {working
                ? 'The conversation is working. Activation waits for the next user turn.'
                : 'The refresh activates before the next user turn.'}
            </p>

            {preview.changed_manifest.guidance.length > 0 && (
              <section aria-labelledby="project-guidance-changes">
                <h3 id="project-guidance-changes">Guidance</h3>
                <ul>
                  {preview.changed_manifest.guidance.map((change) => (
                    <li key={change.relative_path}><code>{change.relative_path}</code><ChangeStatus status={change.status} /></li>
                  ))}
                </ul>
              </section>
            )}
            {preview.changed_manifest.skills.length > 0 && (
              <section aria-labelledby="project-skill-changes">
                <h3 id="project-skill-changes">Skills</h3>
                <ul>
                  {preview.changed_manifest.skills.map((change) => (
                    <li key={change.name}><span>{change.name}</span><ChangeStatus status={change.status} /></li>
                  ))}
                </ul>
              </section>
            )}
            {(preview.changed_manifest.unchanged_guidance_count > 0 || preview.changed_manifest.unchanged_skill_count > 0) && (
              <details className="project-instructions-unchanged">
                <summary>Unchanged: {preview.changed_manifest.unchanged_guidance_count} guidance, {preview.changed_manifest.unchanged_skill_count} skills</summary>
              </details>
            )}

            <div className="project-instructions-estimate">
              <strong>May rewarm {estimateLabel(preview.estimated_rewarm_tokens)} once.</strong>
              {preview.rewarm_tokens_are_estimate && <span>{preview.rewarm_estimate_notice}</span>}
            </div>
            {error && <p role="alert" className="project-instructions-error">{error}</p>}
            <div className="project-instructions-dialog-actions">
              <button type="button" className="btn-secondary" onClick={closePreview}>Cancel</button>
              <button ref={confirmRef} type="button" className="btn-primary" onClick={() => void confirm()} disabled={confirming || loadingPreview}>
                {confirming ? 'Queuing…' : 'Confirm refresh'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
