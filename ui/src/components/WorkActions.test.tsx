// Tests for the redesigned WorkControlBar.
//
// The bar renders from `deriveWorkDisposition` (see workDisposition.ts), which
// is the source of truth for which disposition row yields which testid /
// primary / note. These tests drive the real component through a render
// harness, asserting the rendered affordances per disposition case.
//
// Affordances (NEW design):
//   - view-diff-button ("View Diff") — always in the REVIEW zone on Work/Branch.
//   - address-feedback-button ("Address feedback" / "Capturing...") — carries
//     the #288 freshness (.work-actions-pr-freshness) + coverage
//     (.work-actions-pr-coverage[--auth] ⚠) spans.
//   - merge-pr-link / open-pr-link — honest <a> links to the PR url.
//   - clean-up-button ("Clean up" / "Cleaning...") — single click → api.markMerged.
//   - abandon-button ("Abandon" / "Abandoning...") — window.confirm → api.abandonTask.
//   - Exactly one element carries work-actions-btn--primary.
//   - Notes are muted spans, never buttons.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import type { ReactElement } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { WorkControlBar } from './WorkActions';
import { api, type PrStatusResponse } from '../api';
import { ViewerSlotProvider, useViewerSlot } from '../contexts/ViewerSlotContext';

// WorkControlBar reads the unified viewer slot; MemoryRouter backs the slot's
// URL contract.
const renderWithProviders = (ui: ReactElement) =>
  render(
    <MemoryRouter>
      <ViewerSlotProvider browserSessionActive={false}>
        {ui}
      </ViewerSlotProvider>
    </MemoryRouter>,
  );

function CaptureSlot({ onSlot }: { onSlot: (slot: ReturnType<typeof useViewerSlot>['slot']) => void }) {
  const { slot } = useViewerSlot();
  useEffect(() => { onSlot(slot); }, [slot, onSlot]);
  return null;
}

vi.mock('../api', () => ({
  api: {
    abandonTask: vi.fn().mockResolvedValue({ success: true }),
    markMerged: vi.fn().mockResolvedValue({ success: true }),
    getConversationDiff: vi.fn(),
    createPrAutoFixContext: vi
      .fn()
      .mockResolvedValue({ message: 'Address `.phoenix/pr-context/pr-12.json`' }),
  },
}));

function prStatusHandle(prStatus: Partial<PrStatusResponse> = { found: false }) {
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
    manualFallbackEnabled: false,
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

/** Count of glowing primaries across the whole bar — must always be exactly 1
 *  when the bar is in a dispositive (non-continued) state. */
function primaryCount() {
  return document.querySelectorAll('.work-actions-btn--primary').length;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('WorkControlBar — visibility (REQ-WAB-001)', () => {
  it('is hidden in a non-Work/Branch mode (Direct)', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Direct"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle()}
      />,
    );
    expect(screen.queryByTestId('view-diff-button')).not.toBeInTheDocument();
    expect(screen.queryByTestId('abandon-button')).not.toBeInTheDocument();
  });

  it('is hidden when the phase is running (not a disposable phase)', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="running"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle()}
      />,
    );
    expect(screen.queryByTestId('abandon-button')).not.toBeInTheDocument();
  });

  it.each(['idle', 'error', 'context_exhausted'] as const)(
    'is visible for a %s phase on Work',
    (phaseType) => {
      renderWithProviders(
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Work"
          phaseType={phaseType}
          continuedInConvId={null}
          prStatusHandle={prStatusHandle()}
        />,
      );
      expect(screen.getByTestId('abandon-button')).toBeInTheDocument();
      expect(screen.getByTestId('view-diff-button')).toBeInTheDocument();
    },
  );
});

describe('WorkControlBar — continuation gate (REQ-WAB-009)', () => {
  it('hides both terminal verbs, shows only the continuation note, glows nothing', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId="continuation-id"
        prStatusHandle={prStatusHandle()}
      />,
    );

    // FINISH zone fully suppressed — no dead disabled button (REQ-WAB-008/009).
    expect(screen.queryByTestId('abandon-button')).not.toBeInTheDocument();
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();

    expect(
      document.querySelector('.work-actions-continuation-note'),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Continued — actions belong on the continuation/i),
    ).toBeInTheDocument();

    // No primary glow in the continued case.
    expect(primaryCount()).toBe(0);
  });
});

describe('WorkControlBar — stuck phases suppress RESOLVE (REQ-WAB-005)', () => {
  it.each(['error', 'context_exhausted'] as const)(
    'exposes Clean up + Abandon but NO address-feedback even with an open PR (%s)',
    (phaseType) => {
      renderWithProviders(
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Work"
          phaseType={phaseType}
          continuedInConvId={null}
          onSendMessage={vi.fn()}
          prStatusHandle={prStatusHandle({
            found: true,
            number: 12,
            url: 'https://gh/pr/12',
            display_state: 'open',
            check_state: 'failing',
          })}
        />,
      );

      // Stuck + open PR → primary collapses to Abandon (an open PR can't be
      // cleaned up), RESOLVE is suppressed.
      expect(screen.getByTestId('abandon-button')).toBeInTheDocument();
      expect(screen.queryByTestId('address-feedback-button')).not.toBeInTheDocument();
      expect(screen.queryByTestId('merge-pr-link')).not.toBeInTheDocument();
    },
  );

  it('stuck with no PR → Clean up + Abandon both present, no RESOLVE', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="error"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({ found: false })}
      />,
    );
    expect(screen.getByTestId('clean-up-button')).toBeInTheDocument();
    expect(screen.getByTestId('abandon-button')).toBeInTheDocument();
    expect(screen.queryByTestId('address-feedback-button')).not.toBeInTheDocument();
  });
});

describe('WorkControlBar — idle disposition cases (REQ-WAB-004)', () => {
  it('merged PR → Clean up is present and is the primary; Abandon present', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-merged"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: true, number: 136, display_state: 'merged' })}
      />,
    );

    const clean = screen.getByTestId('clean-up-button');
    expect(clean).toBeInTheDocument();
    expect(clean).toHaveClass('work-actions-btn--primary');
    expect(screen.getByTestId('abandon-button')).toBeInTheDocument();
    expect(primaryCount()).toBe(1);
  });

  it('closed-unmerged PR → Abandon is primary, note says closed/use Abandon, NO Clean up', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-closed"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: true, number: 133, display_state: 'closed' })}
      />,
    );

    const abandon = screen.getByTestId('abandon-button');
    expect(abandon).toHaveClass('work-actions-btn--primary');
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();

    const note = document.querySelector('.work-actions-pr-note');
    expect(note).toBeInTheDocument();
    expect(note?.textContent).toMatch(/PR #133 is closed without merge\. Use Abandon/i);
  });

  it('open PR, failing checks + onSendMessage → address-feedback present + primary', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-fail"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 12,
          url: 'https://gh/pr/12',
          display_state: 'open',
          check_state: 'failing',
        })}
      />,
    );

    const resolve = screen.getByTestId('address-feedback-button');
    expect(resolve).toBeInTheDocument();
    expect(resolve).toHaveClass('work-actions-btn--primary');
    expect(resolve.textContent).toMatch(/Address feedback/i);
    expect(primaryCount()).toBe(1);
  });

  it('open PR, fresh feedback + onSendMessage → address-feedback present + primary', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-fresh"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 12,
          url: 'https://gh/pr/12',
          display_state: 'open',
          check_state: 'passing',
          feedback_freshness: { state: 'new', count: 2 },
        })}
      />,
    );

    expect(screen.getByTestId('address-feedback-button')).toHaveClass(
      'work-actions-btn--primary',
    );
  });

  it('open PR, passing checks, no fresh feedback → merge-pr-link present + primary, honest href', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-green"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 77,
          url: 'https://github.com/o/r/pull/77',
          display_state: 'open',
          check_state: 'passing',
        })}
      />,
    );

    const link = screen.getByTestId('merge-pr-link') as HTMLAnchorElement;
    expect(link).toBeInTheDocument();
    expect(link.textContent).toMatch(/Merge PR #77 ↗/);
    expect(link.getAttribute('href')).toBe('https://github.com/o/r/pull/77');
    expect(link).toHaveClass('work-actions-btn--primary');
    expect(screen.queryByTestId('address-feedback-button')).not.toBeInTheDocument();
  });

  it('open PR, pending checks → open-pr-link ("Open PR #N ↗")', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-pending"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 88,
          url: 'https://github.com/o/r/pull/88',
          display_state: 'open',
          check_state: 'pending',
        })}
      />,
    );

    const link = screen.getByTestId('open-pr-link') as HTMLAnchorElement;
    expect(link).toBeInTheDocument();
    expect(link.textContent).toMatch(/Open PR #88 ↗/);
    expect(link.getAttribute('href')).toBe('https://github.com/o/r/pull/88');
    expect(screen.queryByTestId('merge-pr-link')).not.toBeInTheDocument();
  });

  it('draft PR → open-pr-link ("Open PR #N ↗")', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-draft"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 89,
          url: 'https://github.com/o/r/pull/89',
          display_state: 'draft',
        })}
      />,
    );

    const link = screen.getByTestId('open-pr-link') as HTMLAnchorElement;
    expect(link).toBeInTheDocument();
    expect(link.textContent).toMatch(/Open PR #89 ↗/);
  });

  it('no PR found (refresh ok) → Clean up present and primary', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-none"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false })}
      />,
    );

    const clean = screen.getByTestId('clean-up-button');
    expect(clean).toBeInTheDocument();
    expect(clean).toHaveClass('work-actions-btn--primary');
    expect(primaryCount()).toBe(1);
  });
});

describe('WorkControlBar — gh unavailable (single-click manual fallback)', () => {
  it('no PR + refresh unavailable → Clean up present; a SINGLE click calls api.markMerged; warning note shown', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, unavailable_reason: 'not_authenticated' })}
      />,
    );

    const clean = screen.getByTestId('clean-up-button');
    expect(clean).toBeInTheDocument();
    expect(
      document.querySelector('.work-actions-pr-note--warning'),
    ).toBeInTheDocument();

    // Single click marks merged — no two-step enable-then-mark fallback.
    fireEvent.click(clean);
    expect(api.markMerged).toHaveBeenCalledTimes(1);
    expect(api.markMerged).toHaveBeenCalledWith('conv-1');
  });
});

describe('WorkControlBar — checking / loading', () => {
  it('PR status loading → checking note shown, Abandon present, NO Clean up', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={loadingPrStatusHandle}
      />,
    );

    expect(document.querySelector('.work-actions-checking-note')).toBeInTheDocument();
    expect(screen.getByText(/Checking PR/i)).toBeInTheDocument();
    expect(screen.getByTestId('abandon-button')).toBeInTheDocument();
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
  });
});

describe('WorkControlBar — terminal cleanup actions', () => {
  it('Clean up is a single click that calls api.markMerged (no two-step)', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false })}
      />,
    );
    fireEvent.click(screen.getByTestId('clean-up-button'));
    expect(api.markMerged).toHaveBeenCalledTimes(1);
    expect(api.markMerged).toHaveBeenCalledWith('conv-1');
  });

  it('Abandon confirms then calls api.abandonTask', () => {
    const confirmSpy = vi.fn().mockReturnValue(true);
    const prevConfirm = window.confirm;
    window.confirm = confirmSpy;
    try {
      renderWithProviders(
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Work"
          phaseType="idle"
          continuedInConvId={null}
          prStatusHandle={prStatusHandle({ found: false })}
        />,
      );
      fireEvent.click(screen.getByTestId('abandon-button'));
      expect(confirmSpy).toHaveBeenCalled();
      expect(api.abandonTask).toHaveBeenCalledWith('conv-1');
    } finally {
      window.confirm = prevConfirm;
    }
  });

  it('Abandon does NOT call api.abandonTask when the confirm is declined', () => {
    const confirmSpy = vi.fn().mockReturnValue(false);
    const prevConfirm = window.confirm;
    window.confirm = confirmSpy;
    try {
      renderWithProviders(
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Work"
          phaseType="idle"
          continuedInConvId={null}
          prStatusHandle={prStatusHandle({ found: false })}
        />,
      );
      fireEvent.click(screen.getByTestId('abandon-button'));
      expect(confirmSpy).toHaveBeenCalled();
      expect(api.abandonTask).not.toHaveBeenCalled();
    } finally {
      window.confirm = prevConfirm;
    }
  });
});

describe('WorkControlBar — View Diff (View Browser gone)', () => {
  it('View Browser is gone; View Diff opens the fullscreen diff slot', () => {
    let slot: ReturnType<typeof useViewerSlot>['slot'] = { kind: 'none' };
    renderWithProviders(
      <>
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Branch"
          phaseType="idle"
          continuedInConvId={null}
          prStatusHandle={prStatusHandle()}
        />
        <CaptureSlot onSlot={(s) => { slot = s; }} />
      </>,
    );

    expect(screen.queryByTestId('view-browser-button')).toBeNull();

    expect(slot).toEqual({ kind: 'none' });
    fireEvent.click(screen.getByTestId('view-diff-button'));
    expect(slot).toEqual({ kind: 'diff', presentation: 'fullscreen' });
    expect(api.getConversationDiff).not.toHaveBeenCalled();
  });
});

describe('WorkControlBar — invariants', () => {
  it('exactly one element glows in a representative case (merged PR)', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: true, number: 90, display_state: 'merged' })}
      />,
    );
    expect(primaryCount()).toBe(1);
  });

  it('no disabled-as-status: old morphed labels are absent', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 91,
          url: 'https://gh/pr/91',
          display_state: 'open',
          check_state: 'pending',
        })}
      />,
    );

    expect(
      screen.queryByText(/Waiting for PR merge|Clean up merged PR|Use manual fallback|Checking PR…/i),
    ).toBeNull();
  });
});

describe('WorkControlBar — PR feedback freshness + coverage (#288)', () => {
  it('shows a "N new" freshness marker inside the address-feedback button', () => {
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
          url: 'https://gh/pr/137',
          display_state: 'open',
          feedback_freshness: { state: 'new', count: 3 },
        })}
      />,
    );

    const button = screen.getByTestId('address-feedback-button');
    const freshness = button.querySelector('.work-actions-pr-freshness');
    expect(freshness).toBeInTheDocument();
    expect(freshness?.textContent).toBe('3 new');
  });

  it('shows an edited-comment marker when existing feedback changed', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-edited"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 138,
          url: 'https://gh/pr/138',
          display_state: 'open',
          feedback_freshness: { state: 'edited', count: 1 },
        })}
      />,
    );

    const button = screen.getByTestId('address-feedback-button');
    expect(button.querySelector('.work-actions-pr-freshness')?.textContent).toBe(
      '1 comment updated',
    );
  });

  it('renders the count as a lower bound and a ⚠ warning when feedback coverage is degraded', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-incomplete"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 140,
          url: 'https://gh/pr/140',
          display_state: 'open',
          feedback_freshness: { state: 'new', count: 2 },
          feedback_coverage: { kind: 'incomplete', surfaces: ['review_threads'] },
        })}
      />,
    );

    const button = screen.getByTestId('address-feedback-button');
    // Lower-bound prefix from the degraded coverage.
    expect(button.querySelector('.work-actions-pr-freshness')?.textContent).toBe(
      'at least 2 new',
    );
    // Transient (non-actionable) coverage gap → icon-only ⚠, no --auth class.
    const coverage = button.querySelector('.work-actions-pr-coverage');
    expect(coverage).toBeInTheDocument();
    expect(coverage).not.toHaveClass('work-actions-pr-coverage--auth');
    expect(coverage?.textContent).toContain('⚠');
    expect(coverage).toHaveAttribute('title', expect.stringContaining('review threads'));
  });

  it('renders the coverage marker on the Merge PR link when checks pass but a coverage gap exists (codex #C)', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-merge-cov"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 142,
          url: 'https://gh/pr/142',
          display_state: 'open',
          check_state: 'passing',
          // Coverage gap, no fresh feedback → not addressable; routes to Merge.
          feedback_coverage: { kind: 'auth_required', surfaces: ['review_threads'] },
        })}
      />,
    );

    // Coverage is orthogonal: the PR is mergeable (Merge link), and the auth
    // coverage marker still surfaces — on the link, not hidden.
    const merge = screen.getByTestId('merge-pr-link');
    expect(screen.queryByTestId('address-feedback-button')).not.toBeInTheDocument();
    const coverage = merge.querySelector('.work-actions-pr-coverage');
    expect(coverage).toBeInTheDocument();
    expect(coverage).toHaveClass('work-actions-pr-coverage--auth');
    expect(coverage?.textContent).toContain('GitHub sign-in needed');
  });

  it('shows an actionable "GitHub sign-in needed" auth marker when feedback auth failed', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-auth"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 141,
          url: 'https://gh/pr/141',
          display_state: 'open',
          check_state: 'failing',
          feedback_coverage: { kind: 'auth_required', surfaces: ['review_threads'] },
        })}
      />,
    );

    const button = screen.getByTestId('address-feedback-button');
    const coverage = button.querySelector('.work-actions-pr-coverage');
    expect(coverage).toBeInTheDocument();
    expect(coverage).toHaveClass('work-actions-pr-coverage--auth');
    expect(coverage?.textContent).toContain('⚠ GitHub sign-in needed');
  });

  it('keeps remediation loading until send completes and then refreshes PR status', async () => {
    let resolveSend!: () => void;
    const sendPromise = new Promise<void>((resolve) => { resolveSend = resolve; });
    const onSendMessage = vi.fn(() => sendPromise);
    const handle = prStatusHandle({
      found: true,
      number: 139,
      url: 'https://gh/pr/139',
      display_state: 'open',
      feedback_freshness: { state: 'new', count: 2 },
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

    const button = screen.getByTestId('address-feedback-button');
    fireEvent.click(button);

    // createPrAutoFixContext → onSendMessage with the captured message.
    await waitFor(() => {
      expect(api.createPrAutoFixContext).toHaveBeenCalledWith('conv-remediate');
      expect(onSendMessage).toHaveBeenCalledWith('Address `.phoenix/pr-context/pr-12.json`');
    });

    // Loading state holds while send is in flight; refresh not yet called.
    expect(button.textContent).toMatch(/Capturing/i);
    // Button is disabled while capturing — no double-submit (codex #2).
    expect((screen.getByTestId('address-feedback-button') as HTMLButtonElement).disabled).toBe(true);
    expect(handle.refresh).not.toHaveBeenCalled();

    resolveSend();

    // Once send completes, PR status refreshes and the label settles back.
    await waitFor(() => {
      expect(handle.refresh).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(screen.getByTestId('address-feedback-button').textContent).toMatch(
        /Address feedback/i,
      );
    });
  });
});
