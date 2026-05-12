import { useState, useEffect } from 'react';
import { api, type PrStatusResponse } from '../api';
import { useBrowserViewState, useDiffViewerState } from '../contexts/ViewerStateContext';
import { useFileExplorer } from '../hooks/useFileExplorer';

type FetchState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'error'; message: string };

interface WorkActionsProps {
  conversationId: string;
  convModeLabel: string | undefined;
  /** Live phase type from atom (authoritative, not derived from conversation row). */
  phaseType: string;
  branchName: string | undefined;
  baseBranch: string | null | undefined;
  /** When set, the parent has been continued into another conversation.
   *  REQ-BED-031 forbids abandon / mark-as-merged on a continued parent —
   *  the action belongs on the continuation. Server enforces with 409; UI
   *  disables the controls with a tooltip so the user never sees that
   *  error. */
  continuedInConvId: string | null | undefined;
  /** Send a user message to the conversation (for "ask agent to fix" flows) */
  onSendMessage?: (text: string) => void;
}

export function WorkActions({
  conversationId,
  convModeLabel,
  phaseType,
  branchName,
  continuedInConvId,
}: WorkActionsProps) {
  const [error, setError] = useState<string | null>(null);
  const [markingMerged, setMarkingMerged] = useState(false);
  const [abandoning, setAbandoning] = useState(false);
  const [prStatus, setPrStatus] = useState<PrStatusResponse | null>(null);
  const [manualFallback, setManualFallback] = useState(false);
  // Loading/error UI state for the GET diff fetch lives here; the
  // resolved payload is published into DiffViewerStateContext so
  // ConversationPage can mount the viewer in the split pane (or as
  // a full-screen overlay on narrow desktop).
  const [diffFetch, setDiffFetch] = useState<FetchState>({ status: 'idle' });
  const diffViewer = useDiffViewerState();
  const browserView = useBrowserViewState();
  const fileExplorer = useFileExplorer();

  // Clear stale errors when the agent runs (phaseType leaves idle then returns)
  useEffect(() => {
    setError(null);
  }, [phaseType]);

  useEffect(() => {
    // New conversation/branch → drop the previous one's PR status so the
    // cleanup affordance never reflects a stale PR, and require a fresh
    // explicit opt-in for the gh-unavailable manual fallback.
    setPrStatus(null);
    setManualFallback(false);
    if (!branchName || (convModeLabel !== 'Work' && convModeLabel !== 'Branch')) {
      return;
    }
    let cancelled = false;
    const fetchStatus = () => {
      api.getPrStatus(conversationId)
        .then(status => {
          if (cancelled) return;
          setPrStatus(status);
          // gh became available again → drop any stale manual-fallback opt-in.
          if (!status.unavailable_reason) setManualFallback(false);
        })
        .catch(() => { if (!cancelled) setPrStatus({ found: false, unavailable_reason: 'command_failed' }); });
    };
    fetchStatus();
    // Re-fetch when the tab regains focus so a merge done elsewhere unsticks
    // the cleanup button without a page reload.
    const onVisible = () => { if (document.visibilityState === 'visible') fetchStatus(); };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      cancelled = true;
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [conversationId, branchName, convModeLabel]);

  const isBranch = convModeLabel === 'Branch';
  if (convModeLabel !== 'Work' && !isBranch) return null;
  if (phaseType !== 'idle') return null;

  const isLoading = markingMerged || abandoning;
  const hasContinuation = !!continuedInConvId;
  // prStatus === null means the PR-status fetch is still in flight (or being
  // reset on a conversation switch). Block cleanup until we know — otherwise a
  // fast click could mark-merged a conversation whose PR is still open.
  const prChecking = prStatus === null;
  const prMerged = prStatus?.found && prStatus.display_state === 'merged';
  const prUnavailable = !!prStatus?.unavailable_reason;
  const prBlocksCleanup = prStatus?.found && !prMerged;
  const completeDisabled =
    isLoading || hasContinuation || prChecking || (!!prBlocksCleanup && !manualFallback);
  const completeLabel = prChecking
    ? 'Checking PR…'
    : prMerged
      ? 'Clean up merged PR'
      : prBlocksCleanup
        ? 'Waiting for PR merge'
        : prUnavailable && !manualFallback
          ? 'Use manual fallback'
          : 'Mark as Merged';
  const completeTitle = hasContinuation
    ? 'This conversation has been continued. Abandon the continuation instead.'
    : prChecking
      ? 'Checking PR status…'
      : prMerged
        ? 'GitHub reports this PR is merged. Clean up Phoenix local state.'
        : prBlocksCleanup
          ? `GitHub reports PR #${prStatus?.number} is ${prStatus?.display_state}; merge it before cleanup.`
          : prUnavailable && !manualFallback
            ? 'GitHub CLI status is unavailable. Click to enable the explicit manual cleanup fallback.'
            : 'Manual fallback: assert the PR was merged outside Phoenix and clean up local state.';
  const continuationTooltip = hasContinuation
    ? 'This conversation has been continued. Abandon the continuation instead.'
    : undefined;

  return (
    <div className="work-actions-bar">
      <span className="work-actions-label">Done?</span>
      <button
        className="work-actions-btn work-actions-view-diff"
        disabled={diffFetch.status === 'loading'}
        data-testid="view-diff-button"
        onClick={async () => {
          setDiffFetch({ status: 'loading' });
          try {
            const resp = await api.getConversationDiff(conversationId);
            // Single-slot: ensure the file viewer (if any) closes so
            // the diff takes the split pane / overlay slot. The
            // ConversationPage effect handles the reverse case
            // (file click while diff is open).
            fileExplorer.closeFile();
            diffViewer.open(resp);
            setDiffFetch({ status: 'idle' });
          } catch (err) {
            setDiffFetch({
              status: 'error',
              message: err instanceof Error ? err.message : 'Failed to load diff',
            });
          }
        }}
      >
        {diffFetch.status === 'loading' ? 'Loading...' : 'View Diff'}
      </button>
      {browserView.browserSessionActive && !browserView.open && (
        <button
          type="button"
          className="work-actions-btn work-actions-view-browser"
          data-testid="view-browser-button"
          onClick={() => {
            // REQ-BT-018: opening the browser view is mutually exclusive
            // with the prose / diff slot. Close the others first so the
            // ConversationPage mutex effect doesn't immediately bounce us.
            fileExplorer.closeFile();
            diffViewer.close();
            browserView.openPanel();
          }}
          title="Show the live browser view"
        >
          View Browser
        </button>
      )}
      <button
        className="work-actions-btn work-actions-complete"
        disabled={completeDisabled}
        title={completeTitle}
        data-testid="mark-merged-button"
        onClick={async () => {
          if (prUnavailable && !manualFallback) {
            setManualFallback(true);
            return;
          }
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
        {markingMerged ? 'Cleaning...' : completeLabel}
      </button>
      <button
        className="work-actions-btn work-actions-abandon"
        disabled={isLoading || hasContinuation}
        title={continuationTooltip}
        data-testid="abandon-button"
        onClick={async () => {
          const confirmText = isBranch
            ? 'Abandon this conversation? The worktree will be deleted but your branch will be kept.'
            : 'Abandon this task? The worktree and task branch will be deleted.';
          const confirmed = window.confirm(confirmText);
          if (!confirmed) return;
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
      {prBlocksCleanup && !manualFallback && (
        <span className="work-actions-pr-note">
          PR #{prStatus.number} is {prStatus.display_state}; cleanup unlocks after GitHub reports merged.
        </span>
      )}
      {prUnavailable && manualFallback && (
        <span className="work-actions-pr-note work-actions-pr-note--warning">
          gh unavailable — manual cleanup fallback enabled.
        </span>
      )}
      {hasContinuation && (
        <span className="work-actions-continuation-note">
          Continued — actions belong on the continuation.
        </span>
      )}
      {error && (
        <div className="work-actions-error">{error}</div>
      )}
      {diffFetch.status === 'error' && (
        <div className="work-actions-error">{diffFetch.message}</div>
      )}
    </div>
  );
}
