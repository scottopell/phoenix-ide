import { useEffect, useMemo, useState } from 'react';
import { requestActivePrSelectorOpen } from './activePrSelectorIntent';
import { api } from '../api';
import type { ConversationPrStatusHandle } from '../hooks/useConversationPrStatus';
import { useViewerSlotCommands } from '../contexts/ViewerSlotContext';
import { prFeedbackFreshnessLabel, prFeedbackCoverageMarker } from './prBadge';
import { deriveWorkDisposition } from './workDisposition';
import { derivePrRailAvailability } from './prRailAvailability';
import { prReviewState } from './prReviewState';
import { useIsMobile } from '../hooks';
import './WorkActions.css';

interface WorkControlBarProps {
  conversationId: string;
  convModeLabel: string | undefined;
  phaseType: string;
  continuedInConvId: string | null | undefined;
  onSendMessage?: (text: string) => Promise<void> | void;
  showError?: (message: string) => void;
  prStatusHandle: ConversationPrStatusHandle;
}

function InfoHint({ text }: { text: string }) {
  return (
    <details className="work-actions-info-hint">
      <summary aria-label={text} title={text}>ⓘ</summary>
      <span role="tooltip">{text}</span>
    </details>
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

function PrReviewStateIndicator({ feedbackStatus }: { feedbackStatus: 'open' | 'in_progress' | 'approved' | null }) {
  const reviewState = prReviewState(feedbackStatus);
  if (!reviewState) return null;
  return (
    <span className={`pr-review-state ${reviewState.className}`} title={reviewState.label}>
      <span aria-hidden="true">{reviewState.symbol}</span>
      <span className="pr-review-state-label">{reviewState.label}</span>
    </span>
  );
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
  const [openSelectorAfterRefresh, setOpenSelectorAfterRefresh] = useState(false);
  const [expandedPrIdentity, setExpandedPrIdentity] = useState<string | null>(null);
  const [savingPrIdentity, setSavingPrIdentity] = useState<string | null>(null);
  const isMobile = useIsMobile();
  const isLoading = markingMerged || abandoning;
  const { openDiffFullscreen } = useViewerSlotCommands();

  const prLoading = prStatusHandle.state.status === 'loading';
  const prStatus = prStatusHandle.state.status === 'ready' ? prStatusHandle.state.prStatus : null;
  const activePr = prStatusHandle.activePrSummary;
  const legacyActivePrNumber = !prStatusHandle.activeSelection && prStatus?.found
    ? (prStatus.number ?? prStatus.pr?.number ?? null)
    : null;
  const activePrNumber = activePr?.pr_number ?? legacyActivePrNumber;
  const activePrLabel = activePrNumber ? `PR #${activePrNumber}` : 'PR';
  const prSpecificActionsEnabled = activePrNumber !== null && !prStatusHandle.ambiguous;
  const canShowPrDiff = !!activePr && prSpecificActionsEnabled;
  const diffLabel = canShowPrDiff ? `${activePrLabel} Diff` : 'Workspace Diff';
  const associatedPrs = useMemo(
    () => prStatusHandle.activeSelection?.associated_prs ?? [],
    [prStatusHandle.activeSelection?.associated_prs],
  );
  const { actionablePrs, canRepresentActiveSelection, shouldRender: shouldRenderPrRail } = derivePrRailAvailability(
    prStatusHandle,
    isMobile,
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
    canSendMessage: !!onSendMessage,
    workChange: prStatus?.work_change ?? null,
  });

  const isBranch = convModeLabel === 'Branch';
  const primaryClass = (role: 'review' | 'resolve' | 'clean_up' | 'abandon') =>
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

  const handleCleanUp = async () => {
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
  };

  const handleAbandon = async () => {
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
  };

  const resumePrInference = async () => {
    if (!prStatusHandle.resumeInference) return;
    setError(null);
    try {
      await prStatusHandle.resumeInference();
      setExpandedPrIdentity(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to resume automatic PR selection');
    }
  };

  const selectRailPr = async (pr: (typeof associatedPrs)[number], selected: boolean) => {
    const identity = `${pr.repo_owner}/${pr.repo_name}#${pr.pr_number}`;
    if (selected) {
      setExpandedPrIdentity((current) => current === identity ? null : identity);
      return;
    }
    if (!prStatusHandle.pinActivePr) return;
    setSavingPrIdentity(identity);
    setError(null);
    try {
      await prStatusHandle.pinActivePr({
        repo_owner: pr.repo_owner,
        repo_name: pr.repo_name,
        pr_number: pr.pr_number,
      });
      setExpandedPrIdentity(identity);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to select active PR');
    } finally {
      setSavingPrIdentity(null);
    }
  };

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
  const addressFeedbackLabel = capturing ? `Capturing ${activePrLabel}…` : `Address ${activePrLabel} feedback`;
  const addressFeedbackAriaLabel = canShowPrDiff
    ? `${addressFeedbackLabel}. Review ${activePrLabel} diff separately if needed.`
    : addressFeedbackLabel;

  if (isMobile && !canRepresentActiveSelection) {
      return (
        <div className="mobile-work-fallback" data-testid="mobile-work-fallback">
          {disposition.primary === 'resolve' && disposition.resolve && disposition.resolve.kind !== 'address_feedback' && (
            <ResolveLink verb={disposition.resolve} primary coverageMarker={coverageMarker} />
          )}
          {disposition.primary === 'resolve' && disposition.resolve?.kind === 'address_feedback' && prSpecificActionsEnabled && (
            <button
              type="button"
              className="mobile-pr-action mobile-pr-action--hero"
              data-testid="mobile-primary-address-feedback"
              disabled={capturing}
              onClick={handleAddressFeedback}
            >
              <span>{capturing ? `Capturing ${activePrLabel}…` : `Address feedback${freshnessLabel ? ` · ${freshnessLabel}` : ''}`}</span>
              <CoverageMarker marker={coverageMarker} />
            </button>
          )}
          {disposition.primary === 'review' && (
            <button type="button" className="mobile-pr-action mobile-pr-action--hero" onClick={() => openDiffFullscreen('workspace')}>
              Review workspace changes
            </button>
          )}
          {disposition.showCleanUp && !cleanupBlockedByAmbiguity && (
            <button
              type="button"
              className={`mobile-pr-action mobile-pr-action--cleanup${disposition.primary === 'clean_up' ? ' mobile-pr-action--hero' : ''}`}
              aria-label={`Clean up. ${cleanUpHintText(isBranch)}`}
              title={cleanUpHintText(isBranch)}
              disabled={isLoading}
              onClick={handleCleanUp}
            >
              Clean up
            </button>
          )}
          {disposition.showAbandon && !cleanupBlockedByAmbiguity && (
            <button
              type="button"
              className={`mobile-pr-action mobile-pr-action--danger${disposition.primary === 'abandon' ? ' mobile-pr-action--hero' : ''}`}
              aria-label={`Abandon. ${abandonHintText(isBranch)}`}
              title={abandonHintText(isBranch)}
              disabled={isLoading}
              onClick={handleAbandon}
            >
              Abandon
            </button>
          )}
          {disposition.note && <span className="work-actions-note">{disposition.note.text}</span>}
          {error && <div className="work-actions-error" role="alert">{error}</div>}
        </div>
      );
  }

  if (shouldRenderPrRail) {
    const activeIdentity = activePr
      ? `${activePr.repo_owner}/${activePr.repo_name}#${activePr.pr_number}`
      : null;
    const expanded = activeIdentity !== null && expandedPrIdentity === activeIdentity;
    const mobileHero = disposition.primary === 'clean_up' && !cleanupBlockedByAmbiguity ? (
      <button
        type="button"
        className="mobile-pr-action mobile-pr-action--hero mobile-pr-action--cleanup"
        disabled={isLoading}
        onClick={handleCleanUp}
      >
        Clean up
      </button>
    ) : disposition.primary === 'abandon' && !cleanupBlockedByAmbiguity ? (
      <button
        type="button"
        className="mobile-pr-action mobile-pr-action--hero mobile-pr-action--danger"
        disabled={isLoading}
        onClick={handleAbandon}
      >
        Abandon
      </button>
    ) : disposition.resolve?.kind === 'address_feedback' && prSpecificActionsEnabled ? (
      <button
        type="button"
        className="mobile-pr-action mobile-pr-action--hero"
        data-testid="mobile-primary-address-feedback"
        disabled={capturing}
        onClick={handleAddressFeedback}
      >
        <span>{capturing ? `Capturing ${activePrLabel}…` : `Address feedback${freshnessLabel ? ` · ${freshnessLabel}` : ''}`}</span>
        <CoverageMarker marker={coverageMarker} />
      </button>
    ) : disposition.primary === 'resolve' && disposition.resolve && disposition.resolve.kind !== 'address_feedback' && !prStatusHandle.ambiguous ? (
      <ResolveLink verb={disposition.resolve} primary coverageMarker={coverageMarker} />
    ) : disposition.primary === 'review' ? (
      <button type="button" className="mobile-pr-action mobile-pr-action--hero" onClick={() => openDiffFullscreen('workspace')}>
        Review workspace changes
      </button>
    ) : null;

    return (
      <div className={`mobile-pr-dock${isMobile ? '' : ' desktop-pr-dock'}`} data-testid={isMobile ? 'mobile-work-controls' : 'desktop-work-controls'}>
        {expanded && (
          <div className="mobile-pr-actions" data-testid="mobile-pr-actions">
            {mobileHero && <div className="mobile-pr-actions-hero">{mobileHero}</div>}
            <div className="mobile-pr-actions-secondary">
              <button type="button" className="mobile-pr-action mobile-pr-action--review" aria-label={`${activePrLabel} diff`} onClick={() => openDiffFullscreen('active_pr')}>
                <span className="mobile-pr-action-icon" aria-hidden="true">Δ</span><span>PR diff</span>
              </button>
              <button type="button" className="mobile-pr-action mobile-pr-action--workspace" aria-label="Workspace diff" onClick={() => openDiffFullscreen('workspace')}>
                <span className="mobile-pr-action-icon" aria-hidden="true">▱</span><span>Workspace</span>
              </button>
              {disposition.secondaryResolve && disposition.secondaryResolve.kind !== 'address_feedback' && (
                <a
                  className="mobile-pr-action mobile-pr-action--external"
                  href={disposition.secondaryResolve.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  aria-label={disposition.secondaryResolve.kind === 'merge_pr'
                    ? `Merge on GitHub #${disposition.secondaryResolve.number}`
                    : disposition.secondaryResolve.kind === 'open_pr'
                      ? `Open PR #${disposition.secondaryResolve.number}`
                      : 'Create PR on GitHub'}
                >
                  <span className="mobile-pr-action-icon" aria-hidden="true">↗</span><span>GitHub</span>
                </a>
              )}
              {!cleanupBlockedByAmbiguity && disposition.showCleanUp && disposition.primary !== 'clean_up' && (
                <button
                  type="button"
                  className="mobile-pr-action mobile-pr-action--cleanup"
                  aria-label={`Clean up. ${cleanUpHintText(isBranch)}`}
                  title={cleanUpHintText(isBranch)}
                  disabled={isLoading}
                  onClick={handleCleanUp}
                >
                  <span className="mobile-pr-action-icon" aria-hidden="true">—</span><span>Clean up</span>
                </button>
              )}
              {!cleanupBlockedByAmbiguity && disposition.showAbandon && disposition.primary !== 'abandon' && (
                <button
                  type="button"
                  className="mobile-pr-action mobile-pr-action--danger"
                  aria-label={`Abandon. ${abandonHintText(isBranch)}`}
                  title={abandonHintText(isBranch)}
                  disabled={isLoading}
                  onClick={handleAbandon}
                >
                  <span className="mobile-pr-action-icon" aria-hidden="true">!</span><span>Abandon</span>
                </button>
              )}
              {prStatusHandle.activeSelection?.active_pr?.provenance === 'pinned' && prStatusHandle.resumeInference && (
                <button
                  type="button"
                  className="mobile-pr-action mobile-pr-action--automatic"
                  onClick={resumePrInference}
                >
                  <span className="mobile-pr-action-icon" aria-hidden="true">↻</span><span>Auto</span>
                </button>
              )}
            </div>
            <div className="mobile-pr-actions-context">
              <strong>{activePrLabel}</strong>
              <span className="mobile-pr-actions-branch">{activePr?.head} → {activePr?.base}</span>
            </div>
          </div>
        )}
        <div className="mobile-pr-rail" aria-label="Open pull requests">
          {actionablePrs.map((pr) => {
            const identity = `${pr.repo_owner}/${pr.repo_name}#${pr.pr_number}`;
            const selected = identity === activeIdentity;
            const isExpanded = selected && expanded;
            return (
              <button
                key={identity}
                type="button"
                className={`mobile-pr-chip${selected ? ' mobile-pr-chip--active' : ''}`}
                data-pr-identity={identity}
                aria-pressed={selected}
                aria-expanded={isExpanded}
                disabled={savingPrIdentity !== null}
                onClick={() => selectRailPr(pr, selected)}
              >
                <span className={`mobile-pr-status-dot mobile-pr-status-dot--${pr.display_state}`} aria-hidden="true" />
                <span className="mobile-pr-chip-number">#{pr.pr_number}</span>
                {!isMobile && <span className="desktop-pr-chip-title">{pr.title}</span>}
                <span className="mobile-pr-chip-state">{savingPrIdentity === identity ? 'saving…' : pr.display_state}</span>
                {!isMobile && <span className="desktop-pr-chip-branch">{pr.head}</span>}
                <PrReviewStateIndicator feedbackStatus={pr.feedback_status} />
                {selected && freshnessLabel && (
                  <span className="mobile-pr-notification" aria-label={`${freshnessLabel} feedback`}>
                    {freshnessLabel.replace(' new', '')}
                  </span>
                )}
              </button>
            );
          })}
        </div>
        {error && <div className="work-actions-error" role="alert">{error}</div>}
        {note && <span className="work-actions-note mobile-pr-dock-note">{note.text}</span>}
      </div>
    );
  }

  return (
    <div className="desktop-work-actions-compact" data-testid="desktop-work-controls">
      <div className="desktop-work-actions-rail" aria-label="Work actions">
        {activePrNumber ? (
          <button
            type="button"
            className="mobile-pr-chip desktop-work-actions-identity"
            data-testid="desktop-work-actions-identity"
            onClick={requestActivePrSelectorOpen}
          >
            <span className="mobile-pr-status-dot" aria-hidden="true" />
            <span className="mobile-pr-chip-number">#{activePrNumber}</span>
            <span className="mobile-pr-chip-state">{activePr?.display_state ?? prStatus?.display_state ?? 'actions'}</span>
            <PrReviewStateIndicator feedbackStatus={activePr?.feedback_status ?? prStatus?.feedback_status ?? null} />
          </button>
        ) : (
          <span className="mobile-pr-chip desktop-work-actions-identity" data-testid="desktop-work-actions-identity">
            <span className={`mobile-pr-status-dot${prLoading ? ' mobile-pr-status-dot--loading' : ''}`} aria-hidden="true" />
            <span className="mobile-pr-chip-number">Workspace</span>
            <span className="mobile-pr-chip-state">{prLoading ? 'Checking PR…' : 'actions'}</span>
          </span>
        )}
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
        {disposition.primary === 'resolve' && disposition.resolve?.kind === 'address_feedback' && prSpecificActionsEnabled && (
          <button
            type="button"
            className={`work-actions-btn work-actions-address${primaryClass('resolve')}`}
            data-testid="address-feedback-button"
            aria-label={addressFeedbackAriaLabel}
            disabled={capturing}
            onClick={handleAddressFeedback}
          >
            <span className="work-actions-address-copy">{addressFeedbackLabel}</span>
            {freshnessLabel && <span className="work-actions-pr-freshness">{freshnessLabel}</span>}
            <CoverageMarker marker={coverageMarker} />
          </button>
        )}
        {!prStatusHandle.ambiguous && disposition.primary === 'resolve' && disposition.resolve && disposition.resolve.kind !== 'address_feedback' && (
          <ResolveLink verb={disposition.resolve} primary coverageMarker={coverageMarker} />
        )}
        {!prStatusHandle.ambiguous && disposition.secondaryResolve && disposition.secondaryResolve.kind !== 'address_feedback' && (
          <ResolveLink verb={disposition.secondaryResolve} primary={false} coverageMarker={coverageMarker} />
        )}
        {!cleanupBlockedByAmbiguity && disposition.showCleanUp && (
          <div className="desktop-work-actions-terminal">
            <button
              className={`work-actions-btn work-actions-clean-up${primaryClass('clean_up')}`}
              data-testid="clean-up-button"
              aria-label={`Clean up. ${cleanUpHintText(isBranch)}`}
              title={cleanUpHintText(isBranch)}
              disabled={isLoading}
              onClick={handleCleanUp}
            >
              {markingMerged ? 'Cleaning…' : 'Clean up'}
            </button>
            <InfoHint text={cleanUpHintText(isBranch)} />
          </div>
        )}
        {!cleanupBlockedByAmbiguity && disposition.showAbandon && (
          <div className="desktop-work-actions-terminal">
            <button
              className={`work-actions-btn work-actions-abandon${primaryClass('abandon')}`}
              data-testid="abandon-button"
              aria-label={`Abandon. ${abandonHintText(isBranch)}`}
              title={abandonHintText(isBranch)}
              disabled={isLoading}
              onClick={handleAbandon}
            >
              {abandoning ? 'Abandoning…' : 'Abandon'}
            </button>
            <InfoHint text={abandonHintText(isBranch)} />
          </div>
        )}
        {note && (
          <span className={`work-actions-note desktop-work-actions-note${note.kind === 'continued' ? ' work-actions-continuation-note' : ''}${note.kind === 'checking' ? ' work-actions-checking-note' : ''}${note.kind === 'gh_unavailable' ? ' work-actions-pr-note--warning' : ''}${note.kind === 'pr_closed' || note.kind === 'pr_open_stuck' || note.kind === 'no_pr_dirty' ? ' work-actions-pr-note' : ''}`}>
            {note.text}
          </span>
        )}
        {mixedAssociatedStateSummary && (
          <span className="work-actions-note desktop-work-actions-note" data-testid="mixed-associated-pr-summary">
            {mixedAssociatedStateSummary}
          </span>
        )}
      </div>
      {error && <div className="work-actions-error desktop-work-actions-error" role="alert">{error}</div>}
    </div>
  );
}

export const WorkActions = WorkControlBar;
