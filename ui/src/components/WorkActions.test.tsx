// Tests for WorkActions continuation gate (REQ-BED-031, task 24696 Phase 5).
//
// When the parent conversation has a continuation, abandon and mark-as-merged
// must be disabled on the parent — the action belongs on the continuation.
// Server enforces with 409 `error_type = "continuation_exists"`; the UI
// disables the controls so the user never sees that error path.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import type { ReactElement } from 'react';
import { WorkActions } from './WorkActions';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import {
  DiffViewerStateProvider,
  useDiffViewerState,
} from '../contexts/ViewerStateContext';
import type { DiffViewerPayload } from '../contexts/ViewerStateContext';
import { FileExplorerProvider } from './FileExplorer';

// All three providers are needed: FileExplorerProvider for the
// useFileExplorer().closeFile call WorkActions makes during the
// single-slot enforcement, ReviewNotesProvider for the diff viewer's
// notes pile (when rendered by ConversationPage in production), and
// DiffViewerStateProvider so the View-Diff click can publish its
// payload.
const renderWithProviders = (ui: ReactElement) =>
  render(
    <FileExplorerProvider>
      <ReviewNotesProvider>
        <DiffViewerStateProvider>{ui}</DiffViewerStateProvider>
      </ReviewNotesProvider>
    </FileExplorerProvider>,
  );

/** Test helper: subscribes to DiffViewerStateContext and forwards every
 *  payload to the provided callback so tests can assert on what the
 *  WorkActions push. */
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
  },
}));

describe('WorkActions — continuation gate (REQ-BED-031)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('disables Abandon and Mark-as-Merged when continuedInConvId is set', async () => {
    renderWithProviders(
      <WorkActions
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        branchName="feat/x"
        baseBranch="main"
        continuedInConvId="continuation-id"
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
    const { api } = await import('../api');

    renderWithProviders(
      <WorkActions
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        branchName="feat/x"
        baseBranch="main"
        continuedInConvId={null}
      />
    );

    const abandon = screen.getByTestId('abandon-button') as HTMLButtonElement;
    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;

    expect(abandon.disabled).toBe(false);
    expect(mark.disabled).toBe(false);

    // Mark-as-merged is safe to click (no confirm dialog); assert it
    // actually wires through. Abandon triggers window.confirm which
    // happy-dom stubs to true by default but we avoid relying on that.
    fireEvent.click(mark);
    expect(api.markMerged).toHaveBeenCalledWith('conv-1');
  });
});

describe('WorkActions — View Diff (task 08641 + 08654 follow-on)', () => {
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
        <WorkActions
          conversationId="conv-1"
          convModeLabel="Branch"
          phaseType="idle"
          branchName="feat/x"
          baseBranch="main"
          continuedInConvId={null}
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
        <WorkActions
          conversationId="conv-1"
          convModeLabel="Work"
          phaseType="idle"
          branchName="feat/x"
          baseBranch="main"
          continuedInConvId={null}
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
      <WorkActions
        conversationId="conv-1"
        convModeLabel="Direct"
        phaseType="idle"
        branchName={undefined}
        baseBranch={null}
        continuedInConvId={null}
      />
    );

    expect(screen.queryByTestId('view-diff-button')).not.toBeInTheDocument();
  });
});
