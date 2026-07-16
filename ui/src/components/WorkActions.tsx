import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { requestActivePrSelectorOpen } from './activePrSelectorIntent';
import { api } from '../api';
import type { ConversationPrStatusHandle } from '../hooks/useConversationPrStatus';
import { useViewerSlotCommands } from '../contexts/ViewerSlotContext';
import { prFeedbackFreshnessLabel, prFeedbackCoverageMarker } from './prBadge';
import { deriveWorkDisposition } from './workDisposition';
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
  onSendMessage,
  showError,
  prStatusHandle,
}: WorkControlBarProps) {
  const [error, setError] = useState<string | null>(null);
  const [markingMerged, setMarkingMerged] = useState(false);
  const [abandoning, setAbandoning] = useState(false);
  const [capturing, setCapturing] = useState(false);
  const [openSelectorAfterRefresh, setOpenSelectorAfterRefresh] = useState(false);
  const [mobileSheetOpen, setMobileSheetOpen] = useState(false);
  const [savingPrIdentity, setSavingPrIdentity] = useState<string | null>(null);
  const isMobile = useIsMobile();
  const mobileSheetTitleId = useId();
  const mobileSheetRef = useRef<HTMLDivElement>(null);
  const mobileSheetTriggerRef = useRef<HTMLButtonElement>(null);
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

  useEffect(() => {
    if (!mobileSheetOpen) return;
    const dialog = mobileSheetRef.current;
    const trigger = mobileSheetTriggerRef.current;
    const focusable = dialog?.querySelector<HTMLElement>('button:not([disabled]), a[href]');
    (focusable ?? dialog)?.focus();
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setMobileSheetOpen(false);
        return;
      }
      if (event.key !== 'Tab' || !dialog) return;
      const items = [...dialog.querySelectorAll<HTMLElement>('button:not([disabled]), a[href]')];
      if (items.length === 0) return;
      const first = items[0]!;
      const last = items[items.length - 1]!;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    const onFocusIn = (event: FocusEvent) => {
      if (dialog && event.target instanceof Node && !dialog.contains(event.target)) {
        (dialog.querySelector<HTMLElement>('button:not([disabled]), a[href]') ?? dialog).focus();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    document.addEventListener('focusin', onFocusIn);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      document.removeEventListener('focusin', onFocusIn);
      document.body.style.overflow = previousOverflow;
      trigger?.focus();
    };
  }, [mobileSheetOpen]);

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

  const selectMobilePr = async (pr: (typeof associatedPrs)[number]) => {
    if (!prStatusHandle.pinActivePr) return;
    const identity = `${pr.repo_owner}/${pr.repo_name}#${pr.pr_number}`;
    setSavingPrIdentity(identity);
    setError(null);
    try {
      await prStatusHandle.pinActivePr({
        repo_owner: pr.repo_owner,
        repo_name: pr.repo_name,
        pr_number: pr.pr_number,
      });
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

  if (isMobile) {
    const openSheet = () => setMobileSheetOpen(true);
    const mobilePrimary = prStatusHandle.ambiguous && actionablePrs.length > 1 ? (
      <button type="button" className="mobile-work-primary work-actions-btn--primary" onClick={openSheet}>
        Choose active PR · {actionablePrs.length} open
      </button>
    ) : disposition.primary === 'resolve' && disposition.resolve?.kind === 'address_feedback' && prSpecificActionsEnabled ? (
      <button
        type="button"
        className="mobile-work-primary work-actions-btn--primary"
        data-testid="mobile-primary-address-feedback"
        disabled={capturing}
        onClick={handleAddressFeedback}
      >
        {capturing ? `Capturing ${activePrLabel}…` : `Address ${freshnessLabel ?? ''} feedback on ${activePrLabel}`.replace('  ', ' ')}
      </button>
    ) : disposition.primary === 'resolve' && disposition.resolve && disposition.resolve.kind !== 'address_feedback' ? (
      <ResolveLink verb={disposition.resolve} primary coverageMarker={coverageMarker} />
    ) : disposition.primary === 'review' ? (
      <button type="button" className="mobile-work-primary work-actions-btn--primary" onClick={() => openDiffFullscreen('workspace')}>
        Review workspace changes
      </button>
    ) : disposition.primary === 'clean_up' && !cleanupBlockedByAmbiguity ? (
      <button type="button" className="mobile-work-primary work-actions-btn--primary" disabled={isLoading} onClick={handleCleanUp}>
        {markingMerged ? 'Cleaning…' : 'Clean up'}
      </button>
    ) : disposition.primary === 'abandon' && !cleanupBlockedByAmbiguity ? (
      <button type="button" className="mobile-work-primary work-actions-btn--primary" disabled={isLoading} onClick={handleAbandon}>
        {abandoning ? 'Abandoning…' : 'Abandon'}
      </button>
    ) : null;

    return (
      <div className="mobile-work-controls" data-testid="mobile-work-controls">
        <div className="mobile-work-rail">
          {mobilePrimary}
          <button
            ref={mobileSheetTriggerRef}
            type="button"
            className="mobile-work-sheet-trigger"
            aria-haspopup="dialog"
            aria-expanded={mobileSheetOpen}
            onClick={openSheet}
          >
            Work details
          </button>
        </div>
        {error && <div className="work-actions-error">{error}</div>}
        {mobileSheetOpen && (
          <div className="mobile-work-sheet-backdrop" onMouseDown={(event) => {
            if (event.target === event.currentTarget) setMobileSheetOpen(false);
          }}>
            <div
              ref={mobileSheetRef}
              className="mobile-work-sheet"
              role="dialog"
              aria-modal="true"
              aria-labelledby={mobileSheetTitleId}
              tabIndex={-1}
            >
              <header className="mobile-work-sheet-header">
                <div>
                  <h2 id={mobileSheetTitleId}>Work details</h2>
                  <p>{activePr ? `${activePrLabel} · ${activePr.head}` : `${associatedPrs.length} associated PRs`}</p>
                </div>
                <button type="button" className="mobile-work-sheet-close" aria-label="Close work details" onClick={() => setMobileSheetOpen(false)}>×</button>
              </header>

              {associatedPrs.length > 0 && (
                <section className="mobile-work-sheet-section" aria-labelledby={`${mobileSheetTitleId}-prs`}>
                  <h3 id={`${mobileSheetTitleId}-prs`}>Pull requests</h3>
                  {prStatusHandle.ambiguous && <p className="mobile-work-sheet-guidance">Choose one open PR to enable PR-specific actions.</p>}
                  <div className="mobile-work-pr-list">
                    {associatedPrs.map((pr) => {
                      const identity = `${pr.repo_owner}/${pr.repo_name}#${pr.pr_number}`;
                      const selected = activePr?.pr_number === pr.pr_number && activePr.repo_owner === pr.repo_owner && activePr.repo_name === pr.repo_name;
                      const actionable = pr.display_state === 'open' || pr.display_state === 'draft';
                      const content = (
                        <>
                          <span className="mobile-work-pr-title">#{pr.pr_number} {pr.title}</span>
                          <span className="mobile-work-pr-meta">{pr.head} → {pr.base}</span>
                          <span className="mobile-work-pr-state">{pr.display_state}{selected ? ' · active' : ''}{savingPrIdentity === identity ? ' · saving…' : ''}</span>
                        </>
                      );
                      return actionable ? (
                        <button
                          key={identity}
                          type="button"
                          className={`mobile-work-pr${selected ? ' mobile-work-pr--active' : ''}`}
                          aria-pressed={selected}
                          disabled={savingPrIdentity !== null}
                          onClick={() => selectMobilePr(pr)}
                        >
                          {content}
                        </button>
                      ) : (
                        <div key={identity} className="mobile-work-pr mobile-work-pr--history">
                          {content}
                        </div>
                      );
                    })}
                  </div>
                </section>
              )}

              <section className="mobile-work-sheet-section" aria-labelledby={`${mobileSheetTitleId}-review`}>
                <h3 id={`${mobileSheetTitleId}-review`}>Review</h3>
                <button type="button" className="mobile-work-sheet-action" onClick={() => openDiffFullscreen('workspace')}>Workspace diff</button>
                {canShowPrDiff && <button type="button" className="mobile-work-sheet-action" onClick={() => openDiffFullscreen('active_pr')}>{activePrLabel} diff</button>}
              </section>

              {disposition.resolve && !prStatusHandle.ambiguous && (
                <section className="mobile-work-sheet-section" aria-labelledby={`${mobileSheetTitleId}-resolve`}>
                  <h3 id={`${mobileSheetTitleId}-resolve`}>Pull request</h3>
                  {disposition.resolve.kind === 'address_feedback' && prSpecificActionsEnabled ? (
                    <button type="button" className="mobile-work-sheet-action mobile-work-sheet-action--primary" disabled={capturing} onClick={handleAddressFeedback}>
                      {addressFeedbackLabel}{freshnessLabel ? ` · ${freshnessLabel}` : ''}
                    </button>
                  ) : disposition.resolve.kind !== 'address_feedback' ? (
                    <ResolveLink verb={disposition.resolve} primary={false} coverageMarker={coverageMarker} />
                  ) : null}
                  {disposition.secondaryResolve && disposition.secondaryResolve.kind !== 'address_feedback' && (
                    <ResolveLink verb={disposition.secondaryResolve} primary={false} coverageMarker={coverageMarker} />
                  )}
                </section>
              )}

              {(disposition.showCleanUp || disposition.showAbandon) && !cleanupBlockedByAmbiguity && (
                <section className="mobile-work-sheet-section mobile-work-sheet-section--finish" aria-labelledby={`${mobileSheetTitleId}-finish`}>
                  <h3 id={`${mobileSheetTitleId}-finish`}>Finish</h3>
                  {disposition.showCleanUp && (
                    <div className="mobile-work-finish-action">
                      <button type="button" className="mobile-work-sheet-action" disabled={isLoading} onClick={handleCleanUp}>Clean up</button>
                      <p>{cleanUpHintText(isBranch)}</p>
                    </div>
                  )}
                  {disposition.showAbandon && (
                    <div className="mobile-work-finish-action">
                      <button type="button" className="mobile-work-sheet-action mobile-work-sheet-action--danger" disabled={isLoading} onClick={handleAbandon}>Abandon</button>
                      <p>{abandonHintText(isBranch)}</p>
                    </div>
                  )}
                </section>
              )}
            </div>
          </div>
        )}
      </div>
    );
  }

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
              disabled={capturing}
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
              onClick={handleCleanUp}
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
              onClick={handleAbandon}
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
