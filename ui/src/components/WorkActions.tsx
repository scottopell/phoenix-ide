import { useState } from 'react';
import { api, type PrStatusResponse } from '../api';
import type { ConversationPrStatusHandle } from '../hooks/useConversationPrStatus';
import { useBrowserViewState, useDiffViewerState } from '../contexts/ViewerStateContext';
import { useFileExplorer } from '../hooks/useFileExplorer';

type FetchState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'error'; message: string };

interface WorkControlBarProps {
  conversationId: string;
  convModeLabel: string | undefined;
  phaseType: string;
  continuedInConvId: string | null | undefined;
  onSendMessage?: (text: string) => void;
  showError?: (message: string) => void;
  prStatusHandle: ConversationPrStatusHandle;
}

function deriveWorkLifecycleControls({
  convModeLabel,
  phaseType,
  continuedInConvId,
  prStatusState,
  manualFallbackEnabled,
  isLoading = false,
}: {
  convModeLabel: string | undefined;
  phaseType: string;
  continuedInConvId: string | null | undefined;
  prStatusState: ConversationPrStatusHandle['state'];
  manualFallbackEnabled: boolean;
  isLoading?: boolean;
}) {
  const visible = (convModeLabel === 'Work' || convModeLabel === 'Branch') && phaseType === 'idle';
  const hasContinuation = !!continuedInConvId;
  const prChecking = prStatusState.status === 'loading';
  const prStatus = prStatusState.status === 'ready' ? prStatusState.prStatus : null;
  const prMerged = !!prStatus?.found && prStatus.display_state === 'merged';
  const prClosedUnmerged = !!prStatus?.found && prStatus.display_state === 'closed';
  const prUnavailable = !!prStatus?.unavailable_reason;
  const prBlocksCleanup = !!prStatus?.found && !prMerged;
  const completeDisabled = isLoading || hasContinuation || prChecking || (!!prBlocksCleanup && !manualFallbackEnabled);
  const completeLabel = prChecking
    ? 'Checking PR…'
    : prMerged
      ? 'Clean up merged PR'
      : prClosedUnmerged
        ? 'PR closed without merge'
        : prBlocksCleanup
          ? 'Waiting for PR merge'
          : prUnavailable && !manualFallbackEnabled
            ? 'Use manual fallback'
            : 'Mark as Merged';
  const completeTitle = hasContinuation
    ? 'This conversation has been continued. Abandon the continuation instead.'
    : prChecking
      ? 'Checking PR status…'
      : prMerged
        ? 'GitHub reports this PR is merged. Clean up Phoenix local state.'
        : prClosedUnmerged
          ? `GitHub reports PR #${prStatus?.number} is closed without merge. Use Abandon to clean up Phoenix local state.`
          : prBlocksCleanup
            ? `GitHub reports PR #${prStatus?.number} is ${prStatus?.display_state}; merge it before cleanup.`
            : prUnavailable && !manualFallbackEnabled
              ? 'GitHub CLI status is unavailable. Click to enable the explicit manual cleanup fallback.'
              : 'Manual fallback: assert the PR was merged outside Phoenix and clean up local state.';
  return {
    visible,
    hasContinuation,
    prStatus,
    prClosedUnmerged,
    prUnavailable,
    prBlocksCleanup,
    completeDisabled,
    completeLabel,
    completeTitle,
    continuationTooltip: hasContinuation ? 'This conversation has been continued. Abandon the continuation instead.' : undefined,
  };
}

function WorkViewerActions({ conversationId, diffFetch, setDiffFetch }: { conversationId: string; diffFetch: FetchState; setDiffFetch: (state: FetchState) => void }) {
  const diffViewer = useDiffViewerState();
  const browserView = useBrowserViewState();
  const fileExplorer = useFileExplorer();
  return <>
    <button
      className="work-actions-btn work-actions-view-diff"
      disabled={diffFetch.status === 'loading'}
      data-testid="view-diff-button"
      onClick={async () => {
        setDiffFetch({ status: 'loading' });
        try {
          const resp = await api.getConversationDiff(conversationId);
          fileExplorer.closeFile();
          diffViewer.open(resp);
          setDiffFetch({ status: 'idle' });
        } catch (err) {
          setDiffFetch({ status: 'error', message: err instanceof Error ? err.message : 'Failed to load diff' });
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
          fileExplorer.closeFile();
          diffViewer.close();
          browserView.openPanel();
        }}
        title="Show the live browser view"
      >
        View Browser
      </button>
    )}
  </>;
}

function PrRemediationActions({ conversationId, prStatus, onSendMessage, showError }: { conversationId: string; prStatus: PrStatusResponse | null; onSendMessage?: ((text: string) => void) | undefined; showError?: ((message: string) => void) | undefined }) {
  const [loading, setLoading] = useState(false);
  const canAddress = !!prStatus?.found && prStatus.display_state === 'open' && !!onSendMessage;
  if (!prStatus?.found) return null;
  return (
    <button
      type="button"
      className="work-actions-btn work-actions-pr-remediate"
      disabled={!canAddress || loading}
      title={!canAddress ? 'Available for open PRs when conversation input is available' : undefined}
      onClick={async () => {
        if (!canAddress) return;
        setLoading(true);
        try {
          const context = await api.createPrAutoFixContext(conversationId);
          onSendMessage(context.message);
        } catch (err) {
          showError?.(err instanceof Error ? err.message : 'Failed to capture PR context');
        } finally {
          setLoading(false);
        }
      }}
    >
      {loading ? 'Capturing...' : 'Address CI & comments'}
    </button>
  );
}

export function WorkControlBar({ conversationId, convModeLabel, phaseType, continuedInConvId, onSendMessage, showError, prStatusHandle }: WorkControlBarProps) {
  const [error, setError] = useState<string | null>(null);
  const [markingMerged, setMarkingMerged] = useState(false);
  const [abandoning, setAbandoning] = useState(false);
  const [diffFetch, setDiffFetch] = useState<FetchState>({ status: 'idle' });
  const isLoading = markingMerged || abandoning;
  const lifecycle = deriveWorkLifecycleControls({
    convModeLabel,
    phaseType,
    continuedInConvId,
    prStatusState: prStatusHandle.state,
    manualFallbackEnabled: prStatusHandle.manualFallbackEnabled,
    isLoading,
  });
  if (!lifecycle.visible) return null;
  const isBranch = convModeLabel === 'Branch';
  return (
    <div className="work-actions-bar">
      <span className="work-actions-label">Done?</span>
      <WorkViewerActions conversationId={conversationId} diffFetch={diffFetch} setDiffFetch={setDiffFetch} />
      <PrRemediationActions conversationId={conversationId} prStatus={lifecycle.prStatus} onSendMessage={onSendMessage} showError={showError} />
      <button
        className="work-actions-btn work-actions-complete"
        disabled={lifecycle.completeDisabled}
        title={lifecycle.completeTitle}
        data-testid="mark-merged-button"
        onClick={async () => {
          if (lifecycle.prUnavailable && !prStatusHandle.manualFallbackEnabled) {
            prStatusHandle.enableManualFallback();
            return;
          }
          setError(null);
          setMarkingMerged(true);
          try { await api.markMerged(conversationId); }
          catch (err) { setError(err instanceof Error ? err.message : 'Failed to mark as merged'); }
          finally { setMarkingMerged(false); }
        }}
      >
        {markingMerged ? 'Cleaning...' : lifecycle.completeLabel}
      </button>
      <button
        className="work-actions-btn work-actions-abandon"
        disabled={isLoading || lifecycle.hasContinuation}
        title={lifecycle.continuationTooltip}
        data-testid="abandon-button"
        onClick={async () => {
          const confirmText = isBranch
            ? 'Abandon this conversation? The worktree will be deleted but your branch will be kept.'
            : 'Abandon this task? The worktree and task branch will be deleted.';
          if (!window.confirm(confirmText)) return;
          setError(null);
          setAbandoning(true);
          try { await api.abandonTask(conversationId); }
          catch (err) { setError(err instanceof Error ? err.message : 'Failed to abandon task'); }
          finally { setAbandoning(false); }
        }}
      >
        {abandoning ? 'Abandoning...' : 'Abandon'}
      </button>
      {lifecycle.prBlocksCleanup && !prStatusHandle.manualFallbackEnabled && lifecycle.prStatus?.found && (
        <span className="work-actions-pr-note">
          {lifecycle.prClosedUnmerged
            ? `PR #${lifecycle.prStatus.number} is closed without merge. Use Abandon to clean up local Phoenix state.`
            : `PR #${lifecycle.prStatus.number} is ${lifecycle.prStatus.display_state}; cleanup unlocks after GitHub reports merged.`}
        </span>
      )}
      {lifecycle.prUnavailable && prStatusHandle.manualFallbackEnabled && (
        <span className="work-actions-pr-note work-actions-pr-note--warning">gh unavailable — manual cleanup fallback enabled.</span>
      )}
      {lifecycle.hasContinuation && <span className="work-actions-continuation-note">Continued — actions belong on the continuation.</span>}
      {error && <div className="work-actions-error">{error}</div>}
      {diffFetch.status === 'error' && <div className="work-actions-error">{diffFetch.message}</div>}
    </div>
  );
}

export const WorkActions = WorkControlBar;
