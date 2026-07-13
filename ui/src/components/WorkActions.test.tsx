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
import { StateBar } from './StateBar';
import { api, type AssociatedPrStatusEnvelope, type PrStatusResponse } from '../api';
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
    getPrStatus: vi.fn(),
    createPrAutoFixContext: vi
      .fn()
      .mockResolvedValue({ message: 'Address `.phoenix/pr-context/pr-12.json`' }),
  },
}));

function cleanWorkChange(): PrStatusResponse['work_change'] {
  return { kind: 'clean' };
}

function selection(overrides: Partial<AssociatedPrStatusEnvelope> = {}): AssociatedPrStatusEnvelope {
  return {
    associated_prs: [
      {
        repo_owner: 'o',
        repo_name: 'r',
        pr_number: 12,
        title: 'Fix CI',
        url: 'https://github.com/o/r/pull/12',
        state: 'OPEN',
        draft: false,
        display_state: 'open',
        base: 'main',
        head: 'task-123',
        feedback_status: 'open',
      },
    ],
    active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 12 }, provenance: 'inferred' },
    ...overrides,
  };
}

function prStatusHandle(prStatus: Partial<PrStatusResponse> = { found: false }, overrides: Record<string, unknown> = {}) {
  const status: PrStatusResponse = {
    found: false,
    refresh: {
      state: 'not_found',
      last_attempted_at: '2026-01-01T00:00:00Z',
      last_refreshed_at: '2026-01-01T00:00:00Z',
      stale: false,
    },
    work_change: cleanWorkChange(),
    ...prStatus,
  };
  const selectionValue = (status.selection ?? selection()) as NonNullable<PrStatusResponse['selection']>;
  const associated = selectionValue?.associated_prs ?? [];
  return {
    state: { status: 'ready' as const, prStatus: selectionValue ? { ...status, selection: selectionValue } : status },
    refresh: vi.fn().mockResolvedValue(undefined),
    activeSelection: selectionValue,
    activePrSummary: selectionValue?.active_pr
      ? associated.find((pr) => pr.repo_owner === selectionValue.active_pr?.pr.repo_owner
        && pr.repo_name === selectionValue.active_pr?.pr.repo_name
        && pr.pr_number === selectionValue.active_pr?.pr.pr_number) ?? null
      : null,
    ambiguous: !!selectionValue && !selectionValue.active_pr && associated.filter((pr) => pr.display_state === 'open' || pr.display_state === 'draft').length > 1,
    pinActivePr: vi.fn().mockResolvedValue(undefined),
    resumeInference: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

const loadingPrStatusHandle = {
  state: { status: 'loading' as const, prStatus: null },
  refresh: vi.fn().mockResolvedValue(undefined),
};

/** Count of glowing primaries across the whole bar — must always be exactly 1
 *  when the bar is in a dispositive (non-continued) state. */
function primaryCount() {
  return document.querySelectorAll('.work-actions-btn--primary').length;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.getPrStatus).mockResolvedValue({
    found: false,
    refresh: { state: 'fresh', stale: false, last_attempted_at: '', last_refreshed_at: '' },
    associated_prs: [],
    work_change: cleanWorkChange(),
  });
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
    expect(resolve.textContent).toMatch(/Address PR #12 feedback/i);
    expect(primaryCount()).toBe(1);
  });

  it('cached open PR seed keeps address-feedback primary while fresh status loads', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-cached-open"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 120,
          url: 'https://gh/pr/120',
          display_state: 'open',
          check_state: 'passing',
          refresh: {
            state: 'unavailable',
            reason: 'command_failed',
            last_attempted_at: '2026-01-01T00:00:00Z',
            stale: true,
          },
          unavailable_reason: 'command_failed',
        })}
      />,
    );

    const address = screen.getByTestId('address-feedback-button');
    expect(address).toHaveClass('work-actions-btn--primary');
    expect(screen.getByTestId('open-pr-link')).not.toHaveClass('work-actions-btn--primary');
    expect(screen.queryByTestId('merge-pr-link')).not.toBeInTheDocument();
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

  it('open PR, passing checks → address-feedback primary, Merge rides as a non-primary secondary link', () => {
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

    const address = screen.getByTestId('address-feedback-button');
    expect(address).toHaveClass('work-actions-btn--primary');

    const link = screen.getByTestId('merge-pr-link') as HTMLAnchorElement;
    expect(link).toBeInTheDocument();
    expect(link.textContent).toMatch(/Merge on GitHub #77 ↗/);
    expect(link.getAttribute('href')).toBe('https://github.com/o/r/pull/77');
    // The Merge link is the secondary — it must NOT glow as a second primary.
    expect(link).not.toHaveClass('work-actions-btn--primary');
    expect(primaryCount()).toBe(1);
  });

  it('open PR, pending checks → address-feedback primary, Open PR secondary', () => {
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

    expect(screen.getByTestId('address-feedback-button')).toHaveClass(
      'work-actions-btn--primary',
    );
    expect(screen.queryByTestId('merge-pr-link')).not.toBeInTheDocument();
    expect(screen.getByTestId('open-pr-link')).not.toHaveClass('work-actions-btn--primary');
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

  it('no PR found + clean work → Clean up present and primary', () => {
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
  it('no PR + dirty PR-ready work → Create PR external link is primary and Clean up hidden', () => {
    const createUrl = 'https://github.com/o/r/compare/main...task-1?expand=1';
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-none"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({
          found: false,
          work_change: {
            kind: 'dirty_pr_ready',
            create_pr_url: createUrl,
            branch_name: 'task-1',
            base_branch: 'main',
          },
        })}
      />,
    );

    const link = screen.getByTestId('create-pr-link') as HTMLAnchorElement;
    expect(link).toHaveClass('work-actions-btn--primary');
    expect(link).toHaveAttribute('href', createUrl);
    expect(link.textContent).toMatch(/Create PR on GitHub ↗/);
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.getByTestId('abandon-button')).toBeInTheDocument();
    expect(primaryCount()).toBe(1);
  });

  it('no PR + dirty needs review → View Diff is primary and Clean up hidden', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-none"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({
          found: false,
          work_change: { kind: 'dirty_needs_review', reason: 'uncommitted_changes' },
        })}
      />,
    );

    const viewDiff = screen.getByTestId('view-diff-button');
    expect(viewDiff).toHaveClass('work-actions-btn--primary');
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.getByText(/Uncommitted changes found/i)).toBeInTheDocument();
    expect(primaryCount()).toBe(1);
  });
  it('no PR + refresh unavailable → Clean up present; a SINGLE click calls api.markMerged; warning note shown', async () => {
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

    // Single click marks merged — no enable-then-cleanup state.
    fireEvent.click(clean);
    await waitFor(() => expect(api.markMerged).toHaveBeenCalledTimes(1));
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
  it('Clean up is a single click that calls api.markMerged (no two-step)', async () => {
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
    await waitFor(() => expect(api.markMerged).toHaveBeenCalledTimes(1));
    expect(api.markMerged).toHaveBeenCalledWith('conv-1');
  });

  it('Abandon confirms then calls api.abandonTask', async () => {
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
      await waitFor(() => expect(api.abandonTask).toHaveBeenCalledWith('conv-1'));
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
    expect(slot).toEqual({ kind: 'diff', presentation: 'fullscreen', target: 'workspace' });
    expect(api.getConversationDiff).not.toHaveBeenCalled();
  });
});

describe('WorkControlBar — active PR interactions', () => {
  it('retargets ambiguity guidance to the selector and opens focus there', async () => {
    const handle = prStatusHandle(
      { found: false },
      {
        activeSelection: selection({
          associated_prs: [
            { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://github.com/o/r/pull/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'task-123', feedback_status: 'open' },
            { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Follow-up', url: 'https://github.com/o/r/pull/34', state: 'OPEN', draft: false, display_state: 'open', base: 'task-123', head: 'task-123-follow-up', feedback_status: 'open' },
          ],
        }),
        activePrSummary: null,
        ambiguous: true,
      },
    );
    delete handle.activeSelection.active_pr;

    renderWithProviders(
      <>
        <StateBar
          conversation={{
            id: 'conv-1', slug: 'slug', model: 'claude-sonnet-5', cwd: '/repo/.phoenix/worktrees/conv-1', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z', message_count: 1, state: { type: 'idle' }, branch_name: 'task-123', base_branch: 'main', worktree_path: '/repo/.phoenix/worktrees/conv-1', task_title: 'Task', conv_mode_label: 'Work', browser_session_active: false, terminal_uses_tmux: false, work_scope_key: 'worktree:/repo/.phoenix/worktrees/conv-1',
          }}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          prStatusHandle={handle}
        />
        <WorkControlBar
          conversationId="conv-1"
          convModeLabel="Work"
          phaseType="idle"
          continuedInConvId={null}
          onSendMessage={vi.fn()}
          prStatusHandle={handle}
        />
      </>,
    );

    fireEvent.click(screen.getByTestId('active-pr-ambiguity-note'));
    expect(screen.getByTestId('active-pr-choice-12')).toHaveFocus();
  });

  it('shows mixed associated PR cleanup summary while keeping cleanup task-scoped', () => {
    const handle = prStatusHandle({ found: true, number: 12, display_state: 'merged' }, {
      activeSelection: selection({
        associated_prs: [
          { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://github.com/o/r/pull/12', state: 'CLOSED', draft: false, display_state: 'merged', base: 'main', head: 'task-123', feedback_status: 'approved' },
          { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Still open', url: 'https://github.com/o/r/pull/34', state: 'OPEN', draft: false, display_state: 'open', base: 'task-123', head: 'task-123-follow-up', feedback_status: 'open' },
          { repo_owner: 'o', repo_name: 'r', pr_number: 55, title: 'Closed', url: 'https://github.com/o/r/pull/55', state: 'CLOSED', draft: false, display_state: 'closed', base: 'main', head: 'old-branch', feedback_status: 'open' },
        ],
      }),
      activePrSummary: { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://github.com/o/r/pull/12', state: 'CLOSED', draft: false, display_state: 'merged', base: 'main', head: 'task-123', feedback_status: 'approved' },
    });

    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={handle}
      />,
    );

    expect(screen.getByTestId('mixed-associated-pr-summary')).toHaveTextContent('Associated PRs: 1 open/draft · 1 merged · 1 closed. Cleanup still applies only to this task branch.');
    expect(screen.getByTestId('clean-up-button')).toBeInTheDocument();
  });

  it('suppresses terminal cleanup while multiple actionable associated PRs are ambiguous', () => {
    const ambiguousHandle = prStatusHandle({
      found: false,
      work_change: { kind: 'clean' },
    }, {
      activeSelection: selection({
        associated_prs: [
          { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://gh/pr/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'task-123', feedback_status: 'open' },
          { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Follow-up', url: 'https://gh/pr/34', state: 'OPEN', draft: false, display_state: 'open', base: 'task-123', head: 'task-123-follow-up', feedback_status: 'open' },
          { repo_owner: 'o', repo_name: 'r', pr_number: 55, title: 'Closed', url: 'https://gh/pr/55', state: 'CLOSED', draft: false, display_state: 'closed', base: 'main', head: 'old-branch', feedback_status: 'open' },
        ],
      }),
      activePrSummary: null,
      ambiguous: true,
    });
    delete ambiguousHandle.activeSelection.active_pr;

    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={ambiguousHandle}
      />,
    );

    expect(screen.getByTestId('view-diff-button')).toHaveTextContent('Workspace Diff');
    expect(screen.getByTestId('active-pr-ambiguity-note')).toBeInTheDocument();
    expect(screen.getByTestId('mixed-associated-pr-summary')).toHaveTextContent('Associated PRs: 2 open/draft · 1 closed. Cleanup still applies only to this task branch.');
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.queryByTestId('abandon-button')).not.toBeInTheDocument();
  });

  it('uses concise address-feedback copy and a full accessible target', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({ found: true, number: 12, url: 'https://gh/pr/12', display_state: 'open', check_state: 'failing' })}
      />,
    );

    expect(screen.getByTestId('address-feedback-button')).toHaveTextContent('Address PR #12 feedback');
    expect(screen.getByTestId('address-feedback-button')).toHaveAttribute('aria-label', expect.stringContaining('Address PR #12 feedback'));
  });

  it('gates PR-specific resolve link-outs when the active selection is ambiguous', () => {
    const ambiguousHandle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://gh/pr/12',
      display_state: 'open',
      check_state: 'passing',
      selection: selection({
        associated_prs: [
          { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://gh/pr/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'task-123', feedback_status: 'open' },
          { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Follow-up', url: 'https://gh/pr/34', state: 'OPEN', draft: false, display_state: 'open', base: 'task-123', head: 'task-123-follow-up', feedback_status: 'open' },
        ],
      }),
    }, {
      activeSelection: selection({
        associated_prs: [
          { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://gh/pr/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'task-123', feedback_status: 'open' },
          { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Follow-up', url: 'https://gh/pr/34', state: 'OPEN', draft: false, display_state: 'open', base: 'task-123', head: 'task-123-follow-up', feedback_status: 'open' },
        ],
      }),
      activePrSummary: null,
      ambiguous: true,
    });
    delete ambiguousHandle.activeSelection.active_pr;

    renderWithProviders(
      <WorkControlBar conversationId="conv-1" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={ambiguousHandle} />,
    );

    expect(screen.queryByTestId('address-feedback-button')).not.toBeInTheDocument();
    expect(screen.queryByTestId('merge-pr-link')).not.toBeInTheDocument();
    expect(screen.queryByTestId('open-pr-link')).not.toBeInTheDocument();
    expect(screen.getByTestId('active-pr-ambiguity-note')).toBeInTheDocument();
  });

  it('gates active PR diff behind ambiguity and otherwise uses PR-specific comparator context', () => {
    const ambiguousHandle = prStatusHandle({ found: false }, {
      activeSelection: selection({ associated_prs: [
        { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://github.com/o/r/pull/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'task-123', feedback_status: 'open' },
        { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Follow-up', url: 'https://github.com/o/r/pull/34', state: 'OPEN', draft: false, display_state: 'open', base: 'task-123', head: 'task-123-follow-up', feedback_status: 'open' },
      ] }),
      activePrSummary: null,
      ambiguous: true,
    });
    delete ambiguousHandle.activeSelection.active_pr;
    const { rerender } = renderWithProviders(
      <WorkControlBar conversationId="conv-1" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={ambiguousHandle} />,
    );
    expect(screen.queryByTestId('view-active-pr-diff-button')).not.toBeInTheDocument();

    rerender(
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId="conv-1" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={prStatusHandle({ found: true, number: 12, url: 'https://gh/pr/12', display_state: 'open', check_state: 'failing' })} />
        </ViewerSlotProvider>
      </MemoryRouter>,
    );
    expect(screen.getByTestId('view-active-pr-diff-button')).toHaveAttribute('aria-label', 'View PR #12 diff compared with its base branch');
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
      '1 updated',
    );
  });

  it('renders no freshness badge when there is no actionable freshness signal', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-resolved-only"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 144,
          url: 'https://gh/pr/144',
          display_state: 'open',
        })}
      />,
    );

    const button = screen.getByTestId('address-feedback-button');
    expect(button.querySelector('.work-actions-pr-freshness')).toBeNull();
  });

  it('renders edited actionable feedback as "N updated" with degraded coverage prefix', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-edited-incomplete"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={prStatusHandle({
          found: true,
          number: 143,
          url: 'https://gh/pr/143',
          display_state: 'open',
          feedback_freshness: { state: 'edited', count: 2 },
          feedback_coverage: { kind: 'incomplete', surfaces: ['review_threads'] },
        })}
      />,
    );

    const button = screen.getByTestId('address-feedback-button');
    expect(button.querySelector('.work-actions-pr-freshness')?.textContent).toBe(
      'at least 2 updated',
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

  it('rides the coverage marker on the Address-feedback primary when a passing PR has a coverage gap', () => {
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
          feedback_coverage: { kind: 'auth_required', surfaces: ['review_threads'] },
        })}
      />,
    );

    // Address feedback is the primary (open + can send); Merge rides as the
    // secondary link. The coverage marker rides on the primary verb only, never
    // duplicated onto the secondary link.
    const address = screen.getByTestId('address-feedback-button');
    const merge = screen.getByTestId('merge-pr-link');
    const coverage = address.querySelector('.work-actions-pr-coverage');
    expect(coverage).toBeInTheDocument();
    expect(coverage).toHaveClass('work-actions-pr-coverage--auth');
    expect(coverage?.textContent).toContain('GitHub sign-in needed');
    expect(merge.querySelector('.work-actions-pr-coverage')).toBeNull();
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
      selection: selection({
        associated_prs: [{ repo_owner: 'o', repo_name: 'r', pr_number: 139, title: 'Fix CI', url: 'https://gh/pr/139', state: 'OPEN', draft: false, display_state: 'open' as const, base: 'main', head: 'task-123', feedback_status: 'open' }],
        active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 139 }, provenance: 'inferred' },
      }),
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
        /Address PR #139 feedback/i,
      );
    });
  });
});
