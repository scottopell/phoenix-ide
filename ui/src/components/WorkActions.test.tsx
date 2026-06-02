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
import { ViewerSlotProvider, useViewerSlot } from '../contexts/ViewerSlotContext';

// WorkViewerActions reads the unified viewer slot; MemoryRouter backs the
// slot's URL contract.
const renderWithProviders = (ui: ReactElement) =>
  render(
    <MemoryRouter>
      <ViewerSlotProvider browserSessionActive={false}>
        {ui}
      </ViewerSlotProvider>
    </MemoryRouter>,
  );

/** Subscribes to the viewer slot so tests can assert the kind the
 *  WorkControlBar transitioned it to. */
function CaptureSlotKind({ onKind }: { onKind: (kind: string) => void }) {
  const { slot } = useViewerSlot();
  useEffect(() => { onKind(slot.kind); }, [slot.kind, onKind]);
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

function prStatusHandle(prStatus: Partial<PrStatusResponse> = { found: false }, manualFallbackEnabled = false) {
  const status: PrStatusResponse = {
    found: false,
    refresh: {
      state: 'not_found',
      last_attempted_at: '2026-01-01T00:00:00Z',
      last_refreshed_at: '2026-01-01T00:00:00Z',
      stale: false,
    },
    ...prStatus,
  };
  return {
    state: { status: 'ready' as const, prStatus: status },
    manualFallbackEnabled,
    enableManualFallback: vi.fn(),
    refresh: vi.fn().mockResolvedValue(undefined),
  };
}

const loadingPrStatusHandle = {
  state: { status: 'loading' as const, prStatus: null },
  manualFallbackEnabled: false,
  enableManualFallback: vi.fn(),
  refresh: vi.fn().mockResolvedValue(undefined),
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

  it('keeps manual cleanup fallback reachable for stale PR data when gh is unavailable', async () => {
    const handle = prStatusHandle({
      found: true,
      number: 134,
      display_state: 'open',
      unavailable_reason: 'not_authenticated',
      refresh: {
        state: 'unavailable',
        reason: 'not_authenticated',
        last_attempted_at: '2026-01-01T00:00:00Z',
        last_refreshed_at: '2025-12-31T00:00:00Z',
        stale: true,
      },
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-stale-unavailable"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={handle}
      />,
    );

    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;
    expect(mark.disabled).toBe(false);
    expect(mark.textContent).toMatch(/manual fallback/i);
    fireEvent.click(mark);
    expect(api.markMerged).not.toHaveBeenCalled();
    expect(handle.enableManualFallback).toHaveBeenCalled();
  });

  it('keeps stale unavailable closed PRs directed to Abandon', async () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-stale-closed"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 135,
          display_state: 'closed',
          unavailable_reason: 'not_authenticated',
          refresh: {
            state: 'unavailable',
            reason: 'not_authenticated',
            last_attempted_at: '2026-01-01T00:00:00Z',
            last_refreshed_at: '2025-12-31T00:00:00Z',
            stale: true,
          },
        })}
      />,
    );

    const mark = screen.getByTestId('mark-merged-button') as HTMLButtonElement;
    expect(mark.disabled).toBe(true);
    expect(mark.textContent).toMatch(/closed without merge/i);
    expect(mark.title).toMatch(/Use Abandon/i);
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

  it('shows PR feedback freshness next to the remediation action', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-freshness"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 137,
          display_state: 'open',
          feedback_freshness: { state: 'new', new_count: 3 },
        })}
      />,
    );

    expect(screen.getByRole('button', { name: /Address CI & comments 3 new/i })).toBeInTheDocument();
  });

  it('shows a coarse updated marker when feedback cannot be counted', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-updated"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 138,
          display_state: 'open',
          feedback_freshness: { state: 'updated' },
        })}
      />,
    );

    expect(screen.getByRole('button', { name: /Address CI & comments updated/i })).toBeInTheDocument();
  });

  it('keeps remediation loading until send completes and then refreshes PR status', async () => {
    let resolveSend!: () => void;
    const sendPromise = new Promise<void>((resolve) => { resolveSend = resolve; });
    const onSendMessage = vi.fn(() => sendPromise);
    const handle = prStatusHandle({
      found: true,
      number: 139,
      display_state: 'open',
      feedback_freshness: { state: 'new', new_count: 2 },
    });

    renderWithProviders(
      <WorkControlBar
        conversationId="conv-remediate"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={onSendMessage}
        prStatusHandle={handle}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /Address CI & comments 2 new/i }));

    await waitFor(() => {
      expect(onSendMessage).toHaveBeenCalledWith('Address `.phoenix/pr-context/pr-12.json`');
    });
    expect(screen.getByRole('button', { name: /Capturing/i })).toBeDisabled();
    expect(handle.refresh).not.toHaveBeenCalled();

    resolveSend();

    await waitFor(() => {
      expect(handle.refresh).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /Address CI & comments 2 new/i })).not.toBeDisabled();
    });
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

  it('opens the diff slot when View Diff is clicked (payload fetched on mount by the viewer)', () => {
    let kind = 'none';
    renderWithProviders(
      <>
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Branch"
          phaseType="idle"
          continuedInConvId={null}
          prStatusHandle={prStatusHandle()}
        />
        <CaptureSlotKind onKind={(k) => { kind = k; }} />
      </>,
    );

    expect(kind).toBe('none');
    fireEvent.click(screen.getByTestId('view-diff-button'));
    expect(kind).toBe('diff');
    // WorkActions no longer fetches; the diff viewer fetches on mount.
    expect(api.getConversationDiff).not.toHaveBeenCalled();
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
