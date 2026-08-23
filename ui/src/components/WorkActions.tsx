import { useEffect, useMemo, useRef, useState } from 'react';
import { requestActivePrSelectorOpen } from './activePrSelectorIntent';
import { api } from '../api';
import type { AssociatedPrSummaryResponse, PrStatusResponse } from '../api';
import type { ConversationPrStatusHandle } from '../hooks/useConversationPrStatus';
import { useViewerSlotCommands } from '../contexts/ViewerSlotContext';
import { prFeedbackFreshnessLabel, prFeedbackCoverageMarker } from './prBadge';
import { deriveWorkDisposition } from './workDisposition';
import type { WorkDisposition } from './workDisposition';
import { derivePrRailAvailability } from './prRailAvailability';
import { prReviewState } from './prReviewState';
import { useIsCompactLayout } from '../hooks';
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

type CompactFallbackStatus = {
  icon: '⚠' | '✓' | '…';
  label: string;
  tone: 'attention' | 'success' | 'muted';
};

function compactFallbackStatus(
  disposition: WorkDisposition,
  prStatus: PrStatusResponse | null,
  explicitSelectionUnresolved: boolean,
  activePr: AssociatedPrSummaryResponse | null | undefined,
): CompactFallbackStatus {
  if (disposition.note?.kind === 'continued') return { icon: '✓', label: 'Continued elsewhere', tone: 'muted' };
  if (explicitSelectionUnresolved) return { icon: '⚠', label: 'Active PR unavailable', tone: 'attention' };
  if (activePr?.display_state === 'merged') return { icon: '✓', label: 'PR merged', tone: 'success' };
  if (activePr?.display_state === 'closed') return { icon: '⚠', label: 'PR closed', tone: 'attention' };
  if (activePr?.display_state === 'draft') return { icon: '…', label: 'Draft PR', tone: 'muted' };
  if (activePr?.display_state === 'open') return { icon: '…', label: 'PR open', tone: 'muted' };
  if (disposition.note?.kind === 'checking') return { icon: '…', label: 'Checking PR', tone: 'muted' };
  if (disposition.note?.kind === 'pr_closed') return { icon: '⚠', label: 'PR closed', tone: 'attention' };
  if (disposition.note?.kind === 'pr_open_stuck') return { icon: '⚠', label: 'PR still open', tone: 'attention' };
  if (disposition.note?.kind === 'gh_unavailable') return { icon: '⚠', label: 'GitHub unavailable', tone: 'attention' };

  if (prStatus?.display_state === 'merged') return { icon: '✓', label: 'PR merged', tone: 'success' };
  if (disposition.resolve?.kind === 'address_feedback' && (prStatus?.feedback_freshness?.count ?? 0) > 0) {
    return { icon: '⚠', label: 'PR feedback ready', tone: 'attention' };
  }
  if (prStatus?.found && prStatus.display_state === 'draft') {
    return { icon: '…', label: 'Draft PR', tone: 'muted' };
  }
  if (prStatus?.found && prStatus.display_state === 'open') {
    return { icon: '…', label: 'PR open', tone: 'muted' };
  }

  switch (prStatus?.work_change?.kind) {
    case 'dirty_needs_review': {
      const labels: Record<typeof prStatus.work_change.reason, string> = {
        uncommitted_changes: 'Uncommitted changes',
        branch_not_pushed: 'Branch not pushed',
        local_ahead_of_remote: 'Unpushed commits',
        remote_diverged: 'Branch diverged',
        non_github_remote: 'Non-GitHub remote',
        unknown_remote: 'Remote status unknown',
        unknown: 'Changes need review',
      };
      return { icon: '⚠', label: labels[prStatus.work_change.reason], tone: 'attention' };
    }
    case 'dirty_pr_ready':
      return { icon: '✓', label: 'Ready to open PR', tone: 'success' };
    case 'unavailable':
      return { icon: '⚠', label: 'Work status unavailable', tone: 'attention' };
    case 'loading':
      return { icon: '…', label: 'Checking changes', tone: 'muted' };
    case 'clean':
      return { icon: '✓', label: 'Workspace ready', tone: 'success' };
    case undefined:
      break;
  }

  if (disposition.primary === 'abandon') return { icon: '⚠', label: 'Cleanup required', tone: 'attention' };
  return { icon: '✓', label: 'Workspace ready', tone: 'success' };
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
  const [fallbackPanel, setFallbackPanel] = useState<'info' | 'menu' | null>(null);
  const [fallbackMenuAction, setFallbackMenuAction] = useState<'clean_up' | 'abandon' | null>(null);
  const fallbackDockRef = useRef<HTMLDivElement>(null);
  const fallbackInfoButtonRef = useRef<HTMLButtonElement>(null);
  const fallbackMenuButtonRef = useRef<HTMLButtonElement>(null);
  const fallbackMenuRef = useRef<HTMLDivElement>(null);
  const fallbackSelectorOriginRef = useRef(false);
  const fallbackWasVisibleRef = useRef(false);
  const fallbackOwnedFocusRef = useRef(false);
  const usesCompactLayout = useIsCompactLayout();
  const isLoading = markingMerged || abandoning;
  const { openDiffFullscreen } = useViewerSlotCommands();

  useEffect(() => {
    const fallbackOwnedFocus = fallbackOwnedFocusRef.current;
    setFallbackPanel(null);
    setFallbackMenuAction(null);
    if (fallbackOwnedFocus) {
      requestAnimationFrame(() => fallbackInfoButtonRef.current?.focus());
    }
  }, [conversationId]);

  useEffect(() => {
    if (!fallbackPanel) return;

    const closeOnOutsideClick = (event: MouseEvent) => {
      if (!fallbackDockRef.current?.contains(event.target as Node)) setFallbackPanel(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      const dismissedPanel = fallbackPanel;
      setFallbackPanel(null);
      if (dismissedPanel === 'info') fallbackInfoButtonRef.current?.focus();
      else fallbackMenuButtonRef.current?.focus();
    };

    document.addEventListener('mousedown', closeOnOutsideClick);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('mousedown', closeOnOutsideClick);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [fallbackPanel]);

  const prLoading = prStatusHandle.state.status === 'loading';
  const prStatus = prStatusHandle.state.status === 'ready' ? prStatusHandle.state.prStatus : null;
  const activePr = prStatusHandle.activePrSummary;
  const legacyActivePrNumber = !prStatusHandle.activeSelection && prStatus?.found
    ? (prStatus.number ?? prStatus.pr?.number ?? null)
    : null;
  const activePrNumber = activePr?.pr_number ?? legacyActivePrNumber;
  const canOpenActivePrSelector = activePr !== null
    || (prStatusHandle.activeSelection?.associated_prs.some(
      (pr) => pr.display_state === 'open' || pr.display_state === 'draft',
    ) ?? false)
    || prStatusHandle.ambiguous;
  const activePrLabel = activePrNumber ? `PR #${activePrNumber}` : 'PR';
  const prSpecificActionsEnabled = activePrNumber !== null && !prStatusHandle.ambiguous;
  const cachedPrCanTargetFeedback = prStatusHandle.activeSelection == null
    && !!prStatus?.found
    && prStatus.number != null
    && !prStatusHandle.ambiguous;
  const canAddressFeedback = prSpecificActionsEnabled || cachedPrCanTargetFeedback;
  const addressFeedbackPrLabel = activePrNumber != null
    ? `PR #${activePrNumber}`
    : prStatus?.found && prStatus.number != null
      ? `PR #${prStatus.number}`
      : 'PR';
  const canShowPrDiff = !!activePr && prSpecificActionsEnabled;
  const diffLabel = canShowPrDiff ? `${activePrLabel} Diff` : 'Workspace Diff';
  const associatedPrs = useMemo(
    () => prStatusHandle.activeSelection?.associated_prs ?? [],
    [prStatusHandle.activeSelection?.associated_prs],
  );
  const { actionablePrs, canRepresentActiveSelection, shouldRender: shouldRenderPrRail } = derivePrRailAvailability(
    prStatusHandle,
    usesCompactLayout,
  );

  const explicitSelectionUnresolved = prStatusHandle.activeSelection?.active_pr !== undefined && activePr === null;
  const cleanupBlockedByAmbiguity = explicitSelectionUnresolved
    || (prStatusHandle.ambiguous && actionablePrs.length > 1 && !activePr);
  useEffect(() => {
    if (!openSelectorAfterRefresh || !prStatusHandle.ambiguous) return;
    requestActivePrSelectorOpen();
    setOpenSelectorAfterRefresh(false);
  }, [openSelectorAfterRefresh, prStatusHandle.ambiguous]);
  const activePrOverridesCached = !!activePr && prStatus?.found === true;
  const activePrMatchesCachedIdentity = !!activePr
    && prStatus?.number === activePr.pr_number
    && prStatus.url === activePr.url;
  const activePrMatchesCachedState = activePrMatchesCachedIdentity
    && prStatus?.display_state === activePr?.display_state
    && (prStatus.draft ?? false) === activePr.draft;
  const dispositionPrStatus: PrStatusResponse | null = activePrOverridesCached && activePr && prStatus
    ? {
        ...(activePrMatchesCachedIdentity ? prStatus : {}),
        found: true,
        number: activePr.pr_number,
        title: activePr.title,
        url: activePr.url,
        state: activePr.state,
        draft: activePr.draft,
        base: activePr.base,
        head: activePr.head,
        display_state: activePr.display_state,
        feedback_status: activePr.feedback_status,
        ...(activePrMatchesCachedState ? {} : { check_state: 'unknown' as const }),
        refresh: activePrMatchesCachedState
          ? prStatus.refresh
          : { ...prStatus.refresh, state: 'unavailable' },
        work_change: prStatus.work_change,
        pr: {
          number: activePr.pr_number,
          title: activePr.title,
          url: activePr.url,
          state: activePr.state,
          draft: activePr.draft,
          display_state: activePr.display_state,
          base: activePr.base,
          head: activePr.head,
        },
      }
    : prStatus;
  const disposition = deriveWorkDisposition({
    convModeLabel,
    phaseType,
    continuedInConvId,
    prStatus: dispositionPrStatus,
    prLoading,
    canSendMessage: !!onSendMessage,
    workChange: prStatus?.work_change ?? null,
  });

  const isBranch = convModeLabel === 'Branch';
  const fallbackOverflowAction = !cleanupBlockedByAmbiguity && disposition.showCleanUp && disposition.primary !== 'clean_up'
    ? 'clean_up'
    : !cleanupBlockedByAmbiguity && disposition.showAbandon && disposition.primary !== 'abandon'
      ? 'abandon'
      : null;
  const fallbackHasOverflowActions = fallbackOverflowAction !== null;
  const fallbackMenuIsOpen = fallbackPanel === 'menu' && fallbackMenuAction === fallbackOverflowAction;

  useEffect(() => {
    if (!disposition.visible || !usesCompactLayout || canRepresentActiveSelection) {
      setFallbackPanel(null);
      return;
    }
    if (fallbackPanel === 'menu' && fallbackMenuAction !== fallbackOverflowAction) {
      const menuOwnedFocus = fallbackMenuRef.current?.contains(document.activeElement) ?? false;
      setFallbackPanel(null);
      if (menuOwnedFocus) {
        requestAnimationFrame(() => (fallbackMenuButtonRef.current ?? fallbackInfoButtonRef.current)?.focus());
      }
    }
  }, [canRepresentActiveSelection, disposition.visible, fallbackMenuAction, fallbackOverflowAction, fallbackPanel, usesCompactLayout]);

  const fallbackVisible = disposition.visible && usesCompactLayout && !canRepresentActiveSelection;

  useEffect(() => {
    if (!fallbackVisible) return;
    const trackFocus = (event: FocusEvent) => {
      fallbackOwnedFocusRef.current = fallbackDockRef.current?.contains(event.target as Node) ?? false;
    };
    document.addEventListener('focusin', trackFocus);
    return () => document.removeEventListener('focusin', trackFocus);
  }, [fallbackVisible]);

  useEffect(() => {
    const shouldTransferFocus = fallbackOwnedFocusRef.current || fallbackSelectorOriginRef.current;
    const fallbackWasReplaced = canRepresentActiveSelection || !usesCompactLayout;
    if (fallbackWasVisibleRef.current && !fallbackVisible && fallbackWasReplaced && shouldTransferFocus) {
      requestAnimationFrame(() => {
        const activeChip = document.querySelector<HTMLElement>('.mobile-pr-chip--active');
        const firstChip = document.querySelector<HTMLElement>('.mobile-pr-chip');
        const desktopControl = document.querySelector<HTMLElement>('[data-testid="desktop-work-controls"] button');
        (activeChip ?? firstChip ?? desktopControl)?.focus();
      });
    }
    if (!fallbackVisible) {
      fallbackOwnedFocusRef.current = false;
      fallbackSelectorOriginRef.current = false;
    }
    fallbackWasVisibleRef.current = fallbackVisible;
  }, [canRepresentActiveSelection, fallbackVisible, usesCompactLayout]);


  const freshnessLabel = dispositionPrStatus ? prFeedbackFreshnessLabel(dispositionPrStatus) : null;
  const coverageMarker = dispositionPrStatus ? prFeedbackCoverageMarker(dispositionPrStatus) : null;

  const handleAddressFeedback = async () => {
    if (!onSendMessage) return;
    setCapturing(true);
    try {
      const ctx = await api.createPrAutoFixContext(conversationId);
      await onSendMessage(ctx.message);
      await prStatusHandle.refreshAfterMutation();
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

  const handleCleanUp = async (): Promise<boolean> => {
    setError(null);
    setMarkingMerged(true);
    try {
      if (!(await terminalActionStillSafe())) return false;
      await api.markMerged(conversationId);
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to mark as merged');
      return false;
    } finally {
      setMarkingMerged(false);
    }
  };

  const handleAbandon = async (): Promise<boolean> => {
    const confirmText = isBranch
      ? 'Abandon this conversation? The worktree will be deleted but your branch will be kept.'
      : 'Abandon this task? The worktree and task branch will be deleted.';
    if (!window.confirm(confirmText)) return false;
    setError(null);
    setAbandoning(true);
    try {
      if (!(await terminalActionStillSafe())) return false;
      await api.abandonTask(conversationId);
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to abandon task');
      return false;
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

  const primaryClass = (role: 'review' | 'resolve' | 'clean_up' | 'abandon') =>
    !explicitSelectionUnresolved && disposition.primary === role ? ' work-actions-btn--primary' : '';

  const terminalActionStillSafe = async (): Promise<boolean> => {
    const latest = await prStatusHandle.refreshForSafety();
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

  if (usesCompactLayout && !canRepresentActiveSelection) {
    const status = compactFallbackStatus(disposition, prStatus, explicitSelectionUnresolved, activePr);
    const primaryGuidance = explicitSelectionUnresolved
      ? 'The selected PR is unavailable. Select an active PR or resume automatic PR inference.'
      : disposition.primary === 'review'
        ? 'Review workspace changes before deciding how to finish this work.'
        : disposition.resolve?.kind === 'address_feedback'
        ? `Review and address feedback on ${activePrLabel}.`
        : disposition.resolve?.kind === 'merge_pr'
          ? `Open PR #${disposition.resolve.number} on GitHub to merge it.`
          : disposition.resolve?.kind === 'open_pr'
            ? `Open PR #${disposition.resolve.number} on GitHub to review its current state.`
            : disposition.resolve?.kind === 'create_pr'
              ? `Create a PR on GitHub for branch ${disposition.resolve.branchName}.`
              : null;
    const detailItems = [
      disposition.note?.text ?? primaryGuidance,
      disposition.showCleanUp ? cleanUpHintText(isBranch) : null,
      disposition.showAbandon ? abandonHintText(isBranch) : null,
    ].filter((item): item is string => item !== null);
    const closeFallbackPanel = () => setFallbackPanel(null);

    return (
      <div
        ref={fallbackDockRef}
        className={`mobile-work-fallback${disposition.primary === 'none' && !fallbackHasOverflowActions ? ' mobile-work-fallback--status-only' : ''}`}
        data-testid="mobile-work-fallback"
      >
        <div className="mobile-work-fallback-status">
          <span className={`mobile-work-fallback-status-copy mobile-work-fallback-status-copy--${status.tone}`}>
            <span aria-hidden="true">{status.icon}</span>
            <span>{status.label}</span>
          </span>
          <button
            type="button"
            className="mobile-work-fallback-info-button"
            ref={fallbackInfoButtonRef}
            aria-label="Work status details"
            aria-expanded={fallbackPanel === 'info'}
            onClick={() => setFallbackPanel((panel) => panel === 'info' ? null : 'info')}
          >
            ⓘ
          </button>
        </div>

        {(disposition.primary !== 'none' || fallbackHasOverflowActions) && <div className="mobile-work-fallback-actions">
          {cleanupBlockedByAmbiguity && (disposition.primary === 'clean_up' || disposition.primary === 'abandon') && (
            actionablePrs.length > 0 ? (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                onClick={requestActivePrSelectorOpen}
              >
                Select active PR
              </button>
            ) : (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                disabled={!prStatusHandle.resumeInference}
                onClick={() => void resumePrInference()}
              >
                Resume PR inference
              </button>
            )
          )}
          {disposition.primary === 'resolve' && disposition.resolve && disposition.resolve.kind !== 'address_feedback' && !explicitSelectionUnresolved && (
            <ResolveLink verb={disposition.resolve} primary coverageMarker={coverageMarker} />
          )}
          {disposition.primary === 'resolve' && disposition.resolve && disposition.resolve.kind !== 'address_feedback' && explicitSelectionUnresolved && (
            actionablePrs.length > 0 ? (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                onClick={requestActivePrSelectorOpen}
              >
                Select active PR
              </button>
            ) : (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                disabled={!prStatusHandle.resumeInference}
                onClick={() => void resumePrInference()}
              >
                Resume PR inference
              </button>
            )
          )}
          {disposition.primary === 'resolve' && disposition.resolve?.kind === 'address_feedback' && canAddressFeedback && (
            <button
              type="button"
              className="mobile-pr-action mobile-pr-action--hero"
              data-testid="mobile-primary-address-feedback"
              disabled={capturing}
              onClick={handleAddressFeedback}
            >
              <span>{capturing ? `Capturing ${addressFeedbackPrLabel}…` : `Address feedback${freshnessLabel ? ` · ${freshnessLabel}` : ''}`}</span>
              <CoverageMarker marker={coverageMarker} />
            </button>
          )}
          {disposition.primary === 'resolve' && disposition.resolve?.kind === 'address_feedback' && !canAddressFeedback && (
            actionablePrs.length > 0 ? (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                onClick={() => {
                  fallbackSelectorOriginRef.current = true;
                  requestActivePrSelectorOpen();
                }}
              >
                Select active PR
              </button>
            ) : (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                disabled={!prStatusHandle.resumeInference}
                onClick={() => void resumePrInference()}
              >
                Resume PR inference
              </button>
            )
          )}
          {disposition.primary === 'review' && !explicitSelectionUnresolved && (
            <button
              type="button"
              className="mobile-pr-action mobile-pr-action--hero"
              aria-label="Review workspace changes"
              onClick={() => openDiffFullscreen('workspace')}
            >
              Review changes
            </button>
          )}
          {disposition.primary === 'review' && explicitSelectionUnresolved && (
            actionablePrs.length > 0 ? (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                onClick={requestActivePrSelectorOpen}
              >
                Select active PR
              </button>
            ) : (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--hero"
                disabled={!prStatusHandle.resumeInference}
                onClick={() => void resumePrInference()}
              >
                Resume PR inference
              </button>
            )
          )}
          {disposition.primary === 'clean_up' && !cleanupBlockedByAmbiguity && (
            <button
              type="button"
              className="mobile-pr-action mobile-pr-action--cleanup mobile-pr-action--hero"
              aria-label={`Clean up. ${cleanUpHintText(isBranch)}`}
              title={cleanUpHintText(isBranch)}
              disabled={isLoading}
              onClick={handleCleanUp}
            >
              {markingMerged ? 'Cleaning…' : 'Clean up'}
            </button>
          )}
          {disposition.primary === 'abandon' && !cleanupBlockedByAmbiguity && (
            <button
              type="button"
              className="mobile-pr-action mobile-pr-action--danger mobile-pr-action--hero"
              aria-label={`Abandon. ${abandonHintText(isBranch)}`}
              title={abandonHintText(isBranch)}
              disabled={isLoading}
              onClick={handleAbandon}
            >
              {abandoning ? 'Abandoning…' : 'Abandon'}
            </button>
          )}
          {fallbackHasOverflowActions && (
            <button
              ref={fallbackMenuButtonRef}
              type="button"
              className="mobile-work-fallback-menu-button"
              aria-label="More work actions"
              aria-expanded={fallbackMenuIsOpen}
              aria-controls="mobile-work-fallback-more-actions"
              onClick={() => {
                if (fallbackMenuIsOpen) {
                  setFallbackPanel(null);
                  setFallbackMenuAction(null);
                } else {
                  setFallbackMenuAction(fallbackOverflowAction);
                  setFallbackPanel('menu');
                }
              }}
            >
              •••
            </button>
          )}
        </div>}

        {fallbackPanel === 'info' && (
          <div className="mobile-work-fallback-panel" role="status">
            {detailItems.map((item, index) => (
              <span key={item} className={index > 0 ? 'mobile-work-fallback-panel-item--terminal' : undefined}>
                {item}
              </span>
            ))}
          </div>
        )}
        {fallbackMenuIsOpen && fallbackHasOverflowActions && (
          <div ref={fallbackMenuRef} id="mobile-work-fallback-more-actions" className="mobile-work-fallback-menu" aria-label="More work actions">
            {disposition.showCleanUp && disposition.primary !== 'clean_up' && !cleanupBlockedByAmbiguity && (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--cleanup"
                disabled={isLoading}
                aria-label={`Clean up. ${cleanUpHintText(isBranch)}`}
                title={cleanUpHintText(isBranch)}
                onClick={async () => {
                  if (await handleCleanUp()) closeFallbackPanel();
                }}
              >
                Clean up
              </button>
            )}
            {disposition.showAbandon && disposition.primary !== 'abandon' && !cleanupBlockedByAmbiguity && (
              <button
                type="button"
                className="mobile-pr-action mobile-pr-action--danger"
                aria-label={`Abandon. ${abandonHintText(isBranch)}`}
                title={abandonHintText(isBranch)}
                disabled={isLoading}
                onClick={async () => {
                  if (await handleAbandon()) closeFallbackPanel();
                }}
              >
                Abandon
              </button>
            )}
          </div>
        )}
        {error && <div className="work-actions-error mobile-work-fallback-error" role="alert">{error}</div>}
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
      <div className={`mobile-pr-dock${usesCompactLayout ? '' : ' desktop-pr-dock'}`} data-testid={usesCompactLayout ? 'mobile-work-controls' : 'desktop-work-controls'}>
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
              {disposition.secondaryResolve && (
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
                {!usesCompactLayout && <span className="desktop-pr-chip-title">{pr.title}</span>}
                <span className="mobile-pr-chip-state">{savingPrIdentity === identity ? 'saving…' : pr.display_state}</span>
                {!usesCompactLayout && <span className="desktop-pr-chip-branch">{pr.head}</span>}
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
        {activePrNumber && canOpenActivePrSelector ? (
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
            <span className="mobile-pr-chip-number">{activePrNumber ? `#${activePrNumber}` : 'Workspace'}</span>
            <span className="mobile-pr-chip-state">
              {activePrNumber ? prStatus?.display_state ?? 'actions' : prLoading ? 'Checking PR…' : 'actions'}
            </span>
            {activePrNumber && <PrReviewStateIndicator feedbackStatus={prStatus?.feedback_status ?? null} />}
          </span>
        )}
        <button
          className={`work-actions-btn work-actions-view-diff${primaryClass('review')}`}
          data-testid="view-diff-button"
          onClick={() => openDiffFullscreen('workspace')}
        >
          Workspace Diff
        </button>
        {explicitSelectionUnresolved && disposition.primary !== 'none' && (
          <button
            type="button"
            className="work-actions-btn work-actions-resolve work-actions-btn--primary"
            disabled={!prStatusHandle.resumeInference}
            onClick={() => void resumePrInference()}
          >
            Resume PR inference
          </button>
        )}
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
        {!prStatusHandle.ambiguous && !explicitSelectionUnresolved && disposition.primary === 'resolve' && disposition.resolve && disposition.resolve.kind !== 'address_feedback' && (
          <ResolveLink verb={disposition.resolve} primary coverageMarker={coverageMarker} />
        )}
        {!prStatusHandle.ambiguous && !explicitSelectionUnresolved && disposition.secondaryResolve && (
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
