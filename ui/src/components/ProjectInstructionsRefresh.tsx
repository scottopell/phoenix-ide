import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ConflictError, type ConversationState } from '../api';
import type { ProjectInstructionRefreshStatus } from '../generated/ProjectInstructionRefreshStatus';
import type { ProjectInstructionSourceChangeKind } from '../generated/ProjectInstructionSourceChangeKind';
import './ProjectInstructionsRefresh.css';

interface ProjectInstructionsRefreshProps {
  conversationId: string;
  conversationState: ConversationState;
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
}: ProjectInstructionsRefreshProps) {
  const [status, setStatus] = useState<ProjectInstructionRefreshStatus | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const refreshButtonRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);

  const refreshStatus = useCallback(async () => {
    try {
      const next = await api.getProjectInstructionRefreshStatus(conversationId);
      setStatus(next);
      return next;
    } catch {
      return null;
    }
  }, [conversationId]);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus, conversationState.type]);

  useEffect(() => {
    if (!dialogOpen) return;
    confirmRef.current?.focus();
    const trigger = refreshButtonRef.current;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setDialogOpen(false);
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
  }, [dialogOpen]);

  const openPreview = async () => {
    setLoadingPreview(true);
    setError(null);
    const next = await refreshStatus();
    setLoadingPreview(false);
    if (next?.candidate_bundle_id && hasChanges(next)) setDialogOpen(true);
  };

  const confirm = async () => {
    const candidateId = status?.candidate_bundle_id;
    if (!candidateId) return;
    setConfirming(true);
    setError(null);
    try {
      const response = await api.confirmProjectInstructionRefresh(conversationId, candidateId);
      setStatus(response.status);
      setDialogOpen(false);
    } catch (confirmError) {
      if (confirmError instanceof ConflictError && confirmError.detail.error_type === 'stale_project_instruction_candidate') {
        const next = await refreshStatus();
        setError(next
          ? 'The project instructions changed while this preview was open. Review the updated preview before confirming.'
          : 'The preview became stale and could not be refreshed. Close and try again.');
      } else {
        setError(confirmError instanceof Error ? confirmError.message : 'Could not queue the refresh.');
      }
    } finally {
      setConfirming(false);
    }
  };

  const changed = status ? hasChanges(status) : false;
  const manifest = status?.changed_manifest;
  const working = conversationState.type !== 'idle';

  return (
    <div className="project-instructions-refresh">
      <span className="project-instructions-label">Project instructions</span>
      {status?.is_queued ? (
        <span className="project-instructions-queued">queued for next user turn</span>
      ) : changed ? (
        <>
          <span className="project-instructions-changed">↻ changed</span>
          <button ref={refreshButtonRef} type="button" className="project-instructions-refresh-button" onClick={() => void openPreview()} disabled={loadingPreview}>
            {loadingPreview ? 'Checking…' : 'Refresh'}
          </button>
        </>
      ) : (
        <span className="project-instructions-current" aria-label="Project instructions current">current</span>
      )}

      {dialogOpen && status && manifest && (
        <div className="project-instructions-dialog-backdrop" onMouseDown={(event) => {
          if (event.target === event.currentTarget) setDialogOpen(false);
        }}>
          <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="project-instructions-dialog-title" className="project-instructions-dialog">
            <div className="project-instructions-dialog-header">
              <h2 id="project-instructions-dialog-title">Refresh project instructions?</h2>
              <button type="button" className="project-instructions-dialog-close" aria-label="Close" onClick={() => setDialogOpen(false)}>×</button>
            </div>
            <p className="project-instructions-dialog-copy">
              {working
                ? 'The conversation is working. Activation waits for the next user turn.'
                : 'The refresh activates before the next user turn.'}
            </p>

            {manifest.guidance.length > 0 && (
              <section aria-labelledby="project-guidance-changes">
                <h3 id="project-guidance-changes">Guidance</h3>
                <ul>
                  {manifest.guidance.map((change) => (
                    <li key={change.relative_path}><code>{change.relative_path}</code><ChangeStatus status={change.status} /></li>
                  ))}
                </ul>
              </section>
            )}
            {manifest.skills.length > 0 && (
              <section aria-labelledby="project-skill-changes">
                <h3 id="project-skill-changes">Skills</h3>
                <ul>
                  {manifest.skills.map((change) => (
                    <li key={change.name}><span>{change.name}</span><ChangeStatus status={change.status} /></li>
                  ))}
                </ul>
              </section>
            )}
            {(manifest.unchanged_guidance_count > 0 || manifest.unchanged_skill_count > 0) && (
              <details className="project-instructions-unchanged">
                <summary>Unchanged: {manifest.unchanged_guidance_count} guidance, {manifest.unchanged_skill_count} skills</summary>
              </details>
            )}

            <div className="project-instructions-estimate">
              <strong>May rewarm {estimateLabel(status.estimated_rewarm_tokens)} once.</strong>
              {status.rewarm_tokens_are_estimate && <span>{status.rewarm_estimate_notice}</span>}
            </div>
            {error && <p role="alert" className="project-instructions-error">{error}</p>}
            <div className="project-instructions-dialog-actions">
              <button type="button" className="btn-secondary" onClick={() => setDialogOpen(false)}>Cancel</button>
              <button ref={confirmRef} type="button" className="btn-primary" onClick={() => void confirm()} disabled={confirming}>
                {confirming ? 'Queuing…' : 'Confirm refresh'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
