import { useEffect, useMemo, useState } from 'react';
import { requestActivePrSelectorOpen } from './activePrSelectorIntent';
import { api } from '../api';
import type { ConversationPrStatusHandle } from '../hooks/useConversationPrStatus';
import { useViewerSlotCommands } from '../contexts/ViewerSlotContext';
import { prFeedbackFreshnessLabel, prFeedbackCoverageMarker } from './prBadge';
import { deriveWorkDisposition } from './workDisposition';
import { generateUUID } from '../utils/uuid';

interface WorkControlBarProps {
  conversationId: string;
  convModeLabel: string | undefined;
  phaseType: string;
  continuedInConvId: string | null | undefined;
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

/** An honest GitHub link-out RESOLVE verb (Merge / Open PR). `primary` glows it;
 *  a secondary link rides beside the Address-feedback primary without glowing. */
function ResolveLink({
  verb,
  primary,
  coverageMarker,
}: {
  verb:
    | { kind: 'merge_pr' | 'open_pr'; url: string; number: number }
    | { kind: 'create_pr'; url: string; branchName: string };
  primary: boolean;
  coverageMarker: ReturnType<typeof prFeedbackCoverageMarker>;
}) {
  const isMerge = verb.kind === 'merge_pr';
  const isCreate = verb.kind === 'create_pr';
  const cls = isMerge ? 'work-actions-merge-link' : 'work-actions-open-link';
  const testId = isMerge ? 'merge-pr-link' : isCreate ? 'create-pr-link' : 'open-pr-link';
  const label = isMerge
    ? `Merge on GitHub #${verb.number}`
    : isCreate
      ? 'Create PR on GitHub'
      : `Open PR #${verb.number}`;
  return (
    <a
      href={verb.url}
      target="_blank"
      rel="noopener noreferrer"
      className={`work-actions-btn ${cls}${primary ? ' work-actions-btn--primary' : ''}`}
      data-testid={testId}
    >
      {label} ↗
      {/* The coverage marker rides on the primary verb only — never duplicated
          across both the primary and the secondary link. */}
      {primary && !isCreate && <CoverageMarker marker={coverageMarker} />}
    </a>
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
  showError,
  prStatusHandle,
}: WorkControlBarProps) {
  const [error, setError] = useState<string | null>(null);
  const [markingMerged, setMarkingMerged] = useState(false);
  const [abandoning, setAbandoning] = useState(false);
  const [capturing, setCapturing] = useState(false);
  const [openSelectorAfterRefresh, setOpenSelectorAfterRefresh] = useState(false);
  const [addressMessageId, setAddressMessageId] = useState<string | null>(null);
  const [addressSubmitted, setAddressSubmitted] = useState(false);
  const isLoading = markingMerged || abandoning;
  const { openDiffFullscreen } = useViewerSlotCommands();

  useEffect(() => {
    setCapturing(false);
    setAddressSubmitted(false);
    setAddressMessageId(null);
  }, [conversationId]);

  useEffect(() => {
    if (addressSubmitted && phaseType !== 'idle') {
      setAddressSubmitted(false);
      setAddressMessageId(null);
    }
  }, [addressSubmitted, phaseType]);

  const prLoading = prStatusHandle.state.status === 'loading';
  const prStatus = prStatusHandle.state.status === 'ready' ? prStatusHandle.state.prStatus : null;
  const activePr = prStatusHandle.activePrSummary;
  const legacyActivePrNumber = !prStatusHandle.activeSelection && prStatus?.found
    ? (prStatus.number ?? prStatus.pr?.number ?? null)
    : null;
  const activePrNumber = activePr?.pr_number ?? legacyActivePrNumber;
  const activePrLabel = activePrNumber ? `PR #${activePrNumber}` : 'PR';
  const selection = prStatusHandle.activeSelection;
  const prSpecificActionsEnabled = activePrNumber !== null && !prStatusHandle.ambiguous;
  const canShowPrDiff = !!activePr && prSpecificActionsEnabled;
  const associatedPrs = useMemo(() => selection?.associated_prs ?? [], [selection?.associated_prs]);
  const actionablePrs = useMemo(
    () => associatedPrs.filter((pr) => pr.display_state === 'open' || pr.display_state === 'draft'),
    [associatedPrs],
  );
  const diffLabel = useMemo(
    () => (canShowPrDiff ? `${activePrLabel} Diff` : 'Workspace Diff'),
    [activePrLabel, canShowPrDiff],
  );
  const cleanupBlockedByAmbiguity = prStatusHandle.ambiguous && actionablePrs.length > 1 && !activePr;
  useEffect(() => {
    if (!openSelectorAfterRefresh || !prStatusHandle.ambiguous) return;
    requestActivePrSelectorOpen();
    setOpenSelectorAfterRefresh(false);
  }, [openSelectorAfterRefresh, prStatusHandle.ambiguous]);
  const disposition = deriveWorkDisposition({
    convModeLabel,
    phaseType,
    continuedInConvId,
    prStatus,
    prLoading,
    canSendMessage: true,
    workChange: prStatus?.work_change ?? null,
  });

  const isBranch = convModeLabel === 'Branch';
  const primaryClass = (role: 'review' | 'resolve' | 'clean_up' | 'abandon') =>
    disposition.primary === role ? ' work-actions-btn--primary' : '';

  const freshnessLabel = prStatus ? prFeedbackFreshnessLabel(prStatus) : null;
  const coverageMarker = prStatus ? prFeedbackCoverageMarker(prStatus) : null;

  const addressLocked = capturing || addressSubmitted;
  const handleAddressFeedback = async () => {
    if (addressLocked) return;
    const messageId = addressMessageId ?? generateUUID();
    if (!addressMessageId) setAddressMessageId(messageId);
    setCapturing(true);
    try {
      await api.addressPrFeedback(conversationId, messageId);
      setAddressSubmitted(true);
      prStatusHandle.refresh().catch((err) => {
        console.warn('[WorkActions] failed to refresh PR status after addressing feedback', err);
      });
    } catch (err) {
      showError?.(err instanceof Error ? err.message : 'Failed to address PR feedback');
    } finally {
      setCapturing(false);
    }
  };

  const mixedAssociatedStateSummary = useMemo(() => {
    if (associatedPrs.length < 2) return null;
    const states = new Set(associatedPrs.map((pr) => pr.display_state));
    if (states.size < 2) return null;
    const labels = [
      actionablePrs.length > 0 ? `${actionablePrs.length} open/draft` : null,
      associatedPrs.some((pr) => pr.display_state === 'merged') ? `${associatedPrs.filter((pr) => pr.display_state === 'merged').length} merged` : null,
      associatedPrs.some((pr) => pr.display_state === 'closed') ? `${associatedPrs.filter((pr) => pr.display_state === 'closed').length} closed` : null,
    ].filter(Boolean);
    return labels.length > 1 ? `Associated PRs: ${labels.join(' · ')}. Cleanup still applies only to this task branch.` : null;
  }, [actionablePrs, associatedPrs]);

  if (!disposition.visible) return null;

  const terminalActionStillSafe = async (): Promise<boolean> => {
    const latest = await prStatusHandle.refresh();
    if (!latest) return false;
    const actionable = (latest.associated_prs ?? []).filter(
      (pr) => pr.display_state === 'open' || pr.display_state === 'draft',
    );
    if (actionable.length > 1 && !latest.active_pr) {
      setOpenSelectorAfterRefresh(true);
      setError('Select an active PR before cleaning up or abandoning this task.');
      return false;
    }
    return true;
  };

  const note = disposition.note;
  const addressLocked = capturing || addressSubmitted;
  const addressFeedbackLabel = capturing
    ? `Capturing ${activePrLabel}…`
    : addressSubmitted
      ? `Addressing ${activePrLabel}…`
      : `Address ${activePrLabel} feedback`;
  const addressFeedbackAriaLabel = canShowPrDiff
    ? `${addressFeedbackLabel}. Review ${activePrLabel} diff separately if needed.`
    : addressFeedbackLabel;

  return (
    <div className="work-actions-bar">
      <span className="work-actions-label">Done?</span>

      {/* REVIEW zone */}
      <div className="work-actions-zone work-actions-zone--review">
        <button
          className={`work-actions-btn work-actions-view-diff${primaryClass('review')}`}
          data-testid="view-diff-button"
          onClick={() => openDiffFullscreen('workspace')}
        >
          Workspace Diff
        </button>
        {canShowPrDiff && (
          <button
            className="work-actions-btn work-actions-view-diff"
            data-testid="view-active-pr-diff-button"
            aria-label={`View ${activePrLabel} diff compared with its base branch`}
            onClick={() => openDiffFullscreen('active_pr')}
          >
            {diffLabel}
          </button>
        )}
      </div>

      {/* RESOLVE zone — only when the disposition pushes forward (idle). */}
      {disposition.primary === 'resolve' && disposition.resolve && (
        <div className="work-actions-zone work-actions-zone--resolve">
          {disposition.resolve.kind === 'address_feedback' && prSpecificActionsEnabled && (
            <button
              type="button"
              className={`work-actions-btn work-actions-address${primaryClass('resolve')}`}
              data-testid="address-feedback-button"
              aria-label={addressFeedbackAriaLabel}
              disabled={addressLocked}
              onClick={handleAddressFeedback}
            >
              <span className="work-actions-address-copy">{addressFeedbackLabel}</span>
              {freshnessLabel && <span className="work-actions-pr-freshness">{freshnessLabel}</span>}
              <CoverageMarker marker={coverageMarker} />
            </button>
          )}
          {!prStatusHandle.ambiguous &&
            (disposition.resolve.kind === 'merge_pr' ||
              disposition.resolve.kind === 'open_pr' ||
              disposition.resolve.kind === 'create_pr') && (
              <ResolveLink verb={disposition.resolve} primary coverageMarker={coverageMarker} />
            )}
          {/* Non-glowing secondary link-out beside the Address-feedback primary
              (e.g. Merge on a passing PR). REQ-WAB-003: never a second primary. */}
          {!prStatusHandle.ambiguous &&
            disposition.secondaryResolve &&
            (disposition.secondaryResolve.kind === 'merge_pr' ||
              disposition.secondaryResolve.kind === 'open_pr') && (
              <ResolveLink
                verb={disposition.secondaryResolve}
                primary={false}
                coverageMarker={coverageMarker}
              />
            )}
        </div>
      )}

      {/* FINISH zone */}
      <div className="work-actions-zone work-actions-zone--finish">
        {!cleanupBlockedByAmbiguity && disposition.showCleanUp && (
          <>
            <button
              className={`work-actions-btn work-actions-clean-up${primaryClass('clean_up')}`}
              data-testid="clean-up-button"
              disabled={isLoading}
              onClick={async () => {
                setError(null);
                setMarkingMerged(true);
                try {
                  if (!(await terminalActionStillSafe())) return;
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
        {!cleanupBlockedByAmbiguity && disposition.showAbandon && (
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
                  if (!(await terminalActionStillSafe())) return;
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
      {(note?.kind === 'pr_closed' || note?.kind === 'pr_open_stuck' || note?.kind === 'no_pr_dirty') && (
        <span className="work-actions-pr-note">{note.text}</span>
      )}

      {prStatusHandle.ambiguous && selection && (
        <button
          type="button"
          className="work-actions-pr-note work-actions-pr-note--warning work-actions-pr-note-button"
          data-testid="active-pr-ambiguity-note"
          onClick={() => requestActivePrSelectorOpen()}
        >
          Multiple actionable PRs are associated with this work. Select one before PR-specific actions.
        </button>
      )}
      {mixedAssociatedStateSummary && (
        <span className="work-actions-pr-note" data-testid="mixed-associated-pr-summary">{mixedAssociatedStateSummary}</span>
      )}
      {error && <div className="work-actions-error">{error}</div>}
    </div>
  );
}

export const WorkActions = WorkControlBar;
