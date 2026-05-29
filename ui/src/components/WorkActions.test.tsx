// Tests for WorkControlBar continuation gate (REQ-BED-031, task 24696 Phase 5).
//
// When the parent conversation has a continuation, abandon and mark-as-merged
// must be disabled on the parent — the action belongs on the continuation.
// Server enforces with 409 `error_type = "continuation_exists"`; the UI
// disables the controls so the user never sees that error path.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import type { ReactElement } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { WorkControlBar } from './WorkActions';
import { api, type PrStatusResponse } from '../api';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import {
  BrowserViewStateProvider,
  DiffViewerStateProvider,
  useDiffViewerState,
} from '../contexts/ViewerStateContext';
import type { DiffViewerPayload } from '../contexts/ViewerStateContext';
import { FileExplorerProvider } from './FileExplorer';

// All four providers are needed: FileExplorerProvider for the
// useFileExplorer().closeFile call WorkControlBar makes during the
// single-slot enforcement, ReviewNotesProvider for the diff viewer's
// notes context, and the two viewer-state providers (Diff + Browser)
// so the View-Diff and View-Browser controls can publish their state.
// MemoryRouter is required because FileExplorerProvider reads the
// open-file path from URL search params.
const renderWithProviders = (ui: ReactElement) =>
  render(
    <MemoryRouter>
      <FileExplorerProvider>
        <ReviewNotesProvider>
          <DiffViewerStateProvider>
            <BrowserViewStateProvider browserSessionActive={false}>
              {ui}
            </BrowserViewStateProvider>
          </DiffViewerStateProvider>
        </ReviewNotesProvider>
      </FileExplorerProvider>
    </MemoryRouter>,
  );

/** Test helper: subscribes to DiffViewerStateContext and forwards every
 *  payload to the provided callback so tests can assert on what the
 *  WorkControlBar push. */
function CapturePayload({ onPayload }: { onPayload: (p: DiffViewerPayload | null) => void }) {
  const { payload } = useDiffViewerState();
  useEffect(() => {
    onPayload(payload);
  }, [payload, onPayload]);
  return null;
}

vi.mock('../api', () => ({
  api: {
    abandonTask: vi.fn().mockResolvedValue({ success: true }),
    markMerged: vi.fn().mockResolvedValue({ success: true }),
    getConversationDiff: vi.fn(),
    createPrAutoFixContext: vi.fn().mockResolvedValue({ message: 'Address `.phoenix/pr-context/pr-12.json`' }),
  },
}));

function prStatusHandle(prStatus: PrStatusResponse = { found: false }, manualFallbackEnabled = false) {
  return {
    state: { status: 'ready' as const, prStatus },
    manualFallbackEnabled,
    enableManualFallback: vi.fn(),
  };
}

const loadingPrStatusHandle = {
  state: { status: 'loading' as const, prStatus: null },
  manualFallbackEnabled: false,
  enableManualFallback: vi.fn(),
};

describe('WorkControlBar — continuation gate (REQ-BED-031)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('disables Abandon and Mark-as-Merged when continuedInConvId is set', async () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId="continuation-id"
        prStatusHandle={prStatusHandle()}
      />
    );

    const abandon = screen.getByTestId('abandon-button') as HTMLButtonElement;
    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;

    expect(abandon.disabled).toBe(true);
    expect(mark.disabled).toBe(true);
    expect(abandon.title).toMatch(/continued/i);
    expect(mark.title).toMatch(/continued/i);

    // Visible inline note reinforces the reason
    expect(screen.getByText(/Continued — actions belong on the continuation/i)).toBeInTheDocument();
  });

  it('enables Abandon and Mark-as-Merged when continuedInConvId is null', async () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle()}
      />
    );

    const abandon = screen.getByTestId('abandon-button') as HTMLButtonElement;
    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;

    // Shared PR status is ready and no PR was found, so cleanup is allowed.
    await waitFor(() => {
      expect(abandon.disabled).toBe(false);
      expect(mark.disabled).toBe(false);
    });

    // Mark-as-merged is safe to click (no confirm dialog); assert it
    // actually wires through. Abandon triggers window.confirm which
    // happy-dom stubs to true by default but we avoid relying on that.
    fireEvent.click(mark);
    expect(api.markMerged).toHaveBeenCalledWith('conv-1');
  });
  it('requires an explicit manual fallback click when gh is unavailable', async () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, unavailable_reason: 'not_authenticated' })}
      />
    );

    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;
    expect(mark.textContent).toMatch(/manual fallback/i);
    fireEvent.click(mark);
    expect(api.markMerged).not.toHaveBeenCalled();
  });

  it('marks merged after explicit manual fallback is enabled', async () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, unavailable_reason: 'not_authenticated' }, true)}
      />
    );

    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;
    expect(mark.textContent).toMatch(/mark as merged/i);
    fireEvent.click(mark);
    expect(api.markMerged).toHaveBeenCalledWith('conv-1');
  });


  it('shows Checking PR while shared PR status is loading', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={loadingPrStatusHandle}
      />,
    );
    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;
    expect(mark.textContent).toMatch(/Checking PR/i);
    expect(mark.disabled).toBe(true);
  });

  it('shows Clean up merged PR when GitHub reports merged', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-merged"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: true, number: 136, display_state: 'merged' })}
      />,
    );
    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;
    expect(mark.textContent).toMatch(/Clean up merged PR/i);
    expect(mark.disabled).toBe(false);
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('guides closed-unmerged PRs toward Abandon instead of waiting for merge cleanup', async () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-closed"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: true, number: 133, display_state: 'closed' })}
      />,
    );

    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;
    const abandon = screen.getByTestId('abandon-button') as HTMLButtonElement;

    await waitFor(() => {
      expect(screen.getByText(/PR #133 is closed without merge\. Use Abandon/i)).toBeInTheDocument();
    });
    expect(screen.queryByText(/cleanup unlocks after GitHub reports merged/i)).not.toBeInTheDocument();
    expect(mark.disabled).toBe(true);
    expect(mark.textContent).toMatch(/closed without merge/i);
    expect(mark.title).toMatch(/Use Abandon/i);
    expect(abandon.disabled).toBe(false);
  });

  it.each([
    ['open', 134],
    ['draft', 135],
  ] as const)('keeps %s PRs blocked until GitHub reports merged', async (displayState, number) => {

    renderWithProviders(
      <WorkControlBar
        conversationId={`conv-${displayState}`}
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: true, number, display_state: displayState })}
      />,
    );

    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;

    await waitFor(() => {
      expect(
        screen.getByText(
          `PR #${number} is ${displayState}; cleanup unlocks after GitHub reports merged.`,
        ),
      ).toBeInTheDocument();
    });
    expect(mark.disabled).toBe(true);
    expect(mark.textContent).toMatch(/waiting for PR merge/i);
    expect(mark.title).toMatch(/merge it before cleanup/i);
  });
});

describe('WorkControlBar — View Diff (task 08641 + 08654 follow-on)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('fetches the diff and publishes the payload to DiffViewerStateContext', async () => {
    const { api } = await import('../api');
    (api.getConversationDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      comparator: 'origin/main',
      commit_log: 'abcdef0 feat: thing',
      committed_diff: 'diff --git a/x.txt b/x.txt\n+++ b/x.txt\n+hello',
      uncommitted_diff: '',
    });

    let captured: DiffViewerPayload | null = null;
    renderWithProviders(
      <>
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Branch"
          phaseType="idle"
          continuedInConvId={null}
        prStatusHandle={prStatusHandle()}
        />
        <CapturePayload onPayload={(p) => { captured = p; }} />
      </>,
    );

    fireEvent.click(screen.getByTestId('view-diff-button'));

    await waitFor(() => {
      expect(api.getConversationDiff).toHaveBeenCalledWith('conv-1');
    });
    await waitFor(() => {
      expect(captured).not.toBeNull();
    });
    // Once the fetch resolves, the loading label should clear back to "View Diff"
    // — the dialog itself is mounted by ConversationPage in production, not
    // here, so we don't assert on its DOM.
    await waitFor(() => {
      expect(
        (screen.getByTestId('view-diff-button') as HTMLButtonElement).textContent,
      ).toMatch(/view diff/i);
    });
    expect(captured!.comparator).toBe('origin/main');
    expect(captured!.commit_log).toBe('abcdef0 feat: thing');
  });

  it('shows the server error message when the fetch fails and does NOT publish a payload', async () => {
    const { api } = await import('../api');
    (api.getConversationDiff as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('Worktree no longer exists: /tmp/wt'),
    );

    let captured: DiffViewerPayload | null = null;
    renderWithProviders(
      <>
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Work"
          phaseType="idle"
          continuedInConvId={null}
        prStatusHandle={prStatusHandle()}
        />
        <CapturePayload onPayload={(p) => { captured = p; }} />
      </>,
    );

    fireEvent.click(screen.getByTestId('view-diff-button'));

    await waitFor(() => {
      expect(screen.getByText(/worktree no longer exists/i)).toBeInTheDocument();
    });
    // No payload published.
    expect(captured).toBeNull();
    // Button label returns to "View Diff" so the user can retry.
    const viewDiff = screen.getByTestId('view-diff-button') as HTMLButtonElement;
    expect(viewDiff.textContent).toMatch(/view diff/i);
  });

  it('does not render the View Diff button in Direct mode', async () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Direct"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle()}
      />
    );

    expect(screen.queryByTestId('view-diff-button')).not.toBeInTheDocument();
  });
});
