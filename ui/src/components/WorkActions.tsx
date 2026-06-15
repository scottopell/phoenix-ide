import { useState } from 'react';
import { api } from '../api';
import type { ConversationPrStatusHandle } from '../hooks/useConversationPrStatus';
import { useViewerSlot } from '../contexts/ViewerSlotContext';
import { prFeedbackFreshnessLabel, prFeedbackCoverageMarker } from './prBadge';
import { deriveWorkDisposition } from './workDisposition';

interface WorkControlBarProps {
  conversationId: string;
  convModeLabel: string | undefined;
  phaseType: string;
  continuedInConvId: string | null | undefined;
  onSendMessage?: (text: string) => Promise<void> | void;
  showError?: (message: string) => void;
  prStatusHandle: ConversationPrStatusHandle;
}

/** A small ⓘ affordance carrying a hover/focus tooltip (REQ-WAB-007). */
function InfoHint({ text }: { text: string }) {
  return (
    <span
      className="work-actions-info-hint"
      tabIndex={0}
      role="img"
      aria-label={text}
      title={text}
    >
      ⓘ
    </span>
  );
}

/** The orthogonal PR feedback coverage marker (e.g. "⚠ GitHub sign-in needed").
 *  Rendered on whichever RESOLVE verb shows, since a coverage gap is independent
 *  of which action the PR routes to (it never forces auto-fix routing). */
function CoverageMarker({
  marker,
}: {
  marker: ReturnType<typeof prFeedbackCoverageMarker>;
}) {
  if (!marker) return null;
  return (
    <span
      className={`work-actions-pr-coverage${marker.actionable ? ' work-actions-pr-coverage--auth' : ''}`}
      title={marker.tooltip}
    >
      {'⚠'}
      {marker.label ? ` ${marker.label}` : ''}
    </span>
  );
}

function cleanUpHintText(isBranch: boolean): string {
  return isBranch
    ? 'Mark as merged. Deletes the worktree; your branch is kept. No confirmation — use Abandon if you want a diff snapshot first.'
    : 'Mark as merged. Deletes the worktree and the task branch Phoenix created. No confirmation — use Abandon if you want a diff snapshot first.';
}

function abandonHintText(isBranch: boolean): string {
  return isBranch
    ? 'Captures a diff snapshot and deletes the worktree; your branch is kept. Asks for confirmation.'
    : 'Captures a diff snapshot, then deletes the worktree and the task branch. Asks for confirmation.';
}

export function WorkControlBar({
  conversationId,
  convModeLabel,
  phaseType,
  continuedInConvId,
  onSendMessage,
  showError,
  prStatusHandle,
}: WorkControlBarProps) {
  const [error, setError] = useState<string | null>(null);
  const [markingMerged, setMarkingMerged] = useState(false);
  const [abandoning, setAbandoning] = useState(false);
  const [capturing, setCapturing] = useState(false);
  const isLoading = markingMerged || abandoning;
  const viewerSlot = useViewerSlot();

  const prLoading = prStatusHandle.state.status === 'loading';
  const prStatus = prStatusHandle.state.status === 'ready' ? prStatusHandle.state.prStatus : null;
  const disposition = deriveWorkDisposition({
    convModeLabel,
    phaseType,
    continuedInConvId,
    prStatus,
    prLoading,
    canSendMessage: !!onSendMessage,
  });
  if (!disposition.visible) return null;

  const isBranch = convModeLabel === 'Branch';
  const primaryClass = (role: 'resolve' | 'clean_up' | 'abandon') =>
    disposition.primary === role ? ' work-actions-btn--primary' : '';

  const freshnessLabel = prStatus ? prFeedbackFreshnessLabel(prStatus) : null;
  const coverageMarker = prStatus ? prFeedbackCoverageMarker(prStatus) : null;

  const handleAddressFeedback = async () => {
    if (!onSendMessage) return;
    setCapturing(true);
    try {
      const ctx = await api.createPrAutoFixContext(conversationId);
      await onSendMessage(ctx.message);
      await prStatusHandle.refresh();
    } catch (err) {
      showError?.(err instanceof Error ? err.message : 'Failed to capture PR context');
    } finally {
      setCapturing(false);
    }
  };

  const note = disposition.note;

  return (
    <div className="work-actions-bar">
      <span className="work-actions-label">Done?</span>

      {/* REVIEW zone */}
      <div className="work-actions-zone work-actions-zone--review">
        <button
          className="work-actions-btn work-actions-view-diff"
          data-testid="view-diff-button"
          onClick={() => viewerSlot.openDiffFullscreen()}
        >
          View Diff
        </button>
      </div>

      {/* RESOLVE zone — only when the disposition pushes forward (idle). */}
      {disposition.primary === 'resolve' && disposition.resolve && (
        <div className="work-actions-zone work-actions-zone--resolve">
          {disposition.resolve.kind === 'address_feedback' && (
            <button
              type="button"
              className={`work-actions-btn work-actions-address${primaryClass('resolve')}`}
              data-testid="address-feedback-button"
              disabled={capturing}
              onClick={handleAddressFeedback}
            >
              {capturing ? 'Capturing...' : 'Address feedback'}
              {freshnessLabel && <span className="work-actions-pr-freshness">{freshnessLabel}</span>}
              <CoverageMarker marker={coverageMarker} />
            </button>
          )}
          {disposition.resolve.kind === 'merge_pr' && (
            <a
              href={disposition.resolve.url}
              target="_blank"
              rel="noopener noreferrer"
              className={`work-actions-btn work-actions-merge-link${primaryClass('resolve')}`}
              data-testid="merge-pr-link"
            >
              Merge PR #{disposition.resolve.number} ↗
              <CoverageMarker marker={coverageMarker} />
            </a>
          )}
          {disposition.resolve.kind === 'open_pr' && (
            <a
              href={disposition.resolve.url}
              target="_blank"
              rel="noopener noreferrer"
              className={`work-actions-btn work-actions-open-link${primaryClass('resolve')}`}
              data-testid="open-pr-link"
            >
              Open PR #{disposition.resolve.number} ↗
              <CoverageMarker marker={coverageMarker} />
            </a>
          )}
        </div>
      )}

      {/* FINISH zone */}
      <div className="work-actions-zone work-actions-zone--finish">
        {disposition.showCleanUp && (
          <>
            <button
              className={`work-actions-btn work-actions-clean-up${primaryClass('clean_up')}`}
              data-testid="clean-up-button"
              disabled={isLoading}
              onClick={async () => {
                setError(null);
                setMarkingMerged(true);
                try {
                  await api.markMerged(conversationId);
                } catch (err) {
                  setError(err instanceof Error ? err.message : 'Failed to mark as merged');
                } finally {
                  setMarkingMerged(false);
                }
              }}
            >
              {markingMerged ? 'Cleaning...' : 'Clean up'}
            </button>
            <InfoHint text={cleanUpHintText(isBranch)} />
          </>
        )}
        {disposition.showAbandon && (
          <>
            <button
              className={`work-actions-btn work-actions-abandon${primaryClass('abandon')}`}
              data-testid="abandon-button"
              disabled={isLoading}
              onClick={async () => {
                const confirmText = isBranch
                  ? 'Abandon this conversation? The worktree will be deleted but your branch will be kept.'
                  : 'Abandon this task? The worktree and task branch will be deleted.';
                if (!window.confirm(confirmText)) return;
                setError(null);
                setAbandoning(true);
                try {
                  await api.abandonTask(conversationId);
                } catch (err) {
                  setError(err instanceof Error ? err.message : 'Failed to abandon task');
                } finally {
                  setAbandoning(false);
                }
              }}
            >
              {abandoning ? 'Abandoning...' : 'Abandon'}
            </button>
            <InfoHint text={abandonHintText(isBranch)} />
          </>
        )}
      </div>

      {/* Inline note — muted text, never a button. */}
      {note?.kind === 'continued' && (
        <span className="work-actions-continuation-note">{note.text}</span>
      )}
      {note?.kind === 'checking' && (
        <span className="work-actions-checking-note">{note.text}</span>
      )}
      {note?.kind === 'gh_unavailable' && (
        <span className="work-actions-pr-note work-actions-pr-note--warning">{note.text}</span>
      )}
      {(note?.kind === 'pr_closed' || note?.kind === 'pr_open_stuck') && (
        <span className="work-actions-pr-note">{note.text}</span>
      )}

      {error && <div className="work-actions-error">{error}</div>}
    </div>
  );
}

export const WorkActions = WorkControlBar;
