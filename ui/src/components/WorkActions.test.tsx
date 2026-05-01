// Tests for WorkActions continuation gate (REQ-BED-031, task 24696 Phase 5).
//
// When the parent conversation has a continuation, abandon and mark-as-merged
// must be disabled on the parent — the action belongs on the continuation.
// Server enforces with 409 `error_type = "continuation_exists"`; the UI
// disables the controls so the user never sees that error path.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { WorkActions } from './WorkActions';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import type { ReactElement } from 'react';

// All tests need the review-notes provider since the diff viewer (lazily
// mounted on View Diff click) reads from it. Wrap renders in this helper
// instead of repeating the provider at every callsite.
const renderWithProvider = (ui: ReactElement) =>
  render(<ReviewNotesProvider>{ui}</ReviewNotesProvider>);

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
    renderWithProvider(
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

    renderWithProvider(
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

describe('WorkActions — View Diff (task 08641)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('fetches the diff and opens the modal on success', async () => {
    const { api } = await import('../api');
    (api.getConversationDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      comparator: 'origin/main',
      commit_log: 'abcdef0 feat: thing',
      committed_diff: 'diff --git a/x.txt b/x.txt\n+++ b/x.txt\n+hello',
      uncommitted_diff: '',
    });

    renderWithProvider(
      <WorkActions
        conversationId="conv-1"
        convModeLabel="Branch"
        phaseType="idle"
        branchName="feat/x"
        baseBranch="main"
        continuedInConvId={null}
      />
    );

    const viewDiff = screen.getByTestId('view-diff-button');
    fireEvent.click(viewDiff);

    await waitFor(() => {
      expect(api.getConversationDiff).toHaveBeenCalledWith('conv-1');
    });

    // Modal mounts a dialog with comparator in the title and the commit
    // log + committed diff in body.
    await waitFor(() => {
      expect(screen.getByRole('dialog', { name: /worktree diff/i })).toBeInTheDocument();
    });
    // `origin/main` appears in both the title and the "Committed changes
    // (vs origin/main)" section header — getAllByText avoids the
    // single-match assertion failure.
    expect(screen.getAllByText(/origin\/main/).length).toBeGreaterThan(0);
    expect(screen.getByText(/abcdef0 feat: thing/)).toBeInTheDocument();
  });

  it('shows the server error message when the fetch fails', async () => {
    const { api } = await import('../api');
    (api.getConversationDiff as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('Worktree no longer exists: /tmp/wt'),
    );

    renderWithProvider(
      <WorkActions
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        branchName="feat/x"
        baseBranch="main"
        continuedInConvId={null}
      />
    );

    fireEvent.click(screen.getByTestId('view-diff-button'));

    await waitFor(() => {
      expect(screen.getByText(/worktree no longer exists/i)).toBeInTheDocument();
    });
    // Modal must NOT be open when the fetch errored.
    expect(screen.queryByRole('dialog', { name: /worktree diff/i })).not.toBeInTheDocument();
    // Button label returns to "View Diff" so the user can retry.
    const viewDiff = screen.getByTestId('view-diff-button') as HTMLButtonElement;
    expect(viewDiff.textContent).toMatch(/view diff/i);
  });

  it('does not render the View Diff button in Direct mode', async () => {
    renderWithProvider(
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
