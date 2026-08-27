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
//   - clean-up-button / abandon-button initiate the server-authoritative Close contract.
//   - persisted loss-confirmation and repair phases surface exact follow-up actions.
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
import { requestActivePrSelectorOpen } from './activePrSelectorIntent';

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

vi.mock('./activePrSelectorIntent', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./activePrSelectorIntent')>();
  return { ...actual, requestActivePrSelectorOpen: vi.fn() };
});

vi.mock('../api', () => ({
  api: {
    abandonTask: vi.fn().mockResolvedValue({ success: true }),
    markMerged: vi.fn().mockResolvedValue({ success: true }),
    getProductConversationSnapshot: vi.fn().mockResolvedValue({ close: null }),
    confirmCloseLossRetirement: vi.fn().mockResolvedValue({ success: true }),
    cancelCloseBeforeRetirement: vi.fn().mockResolvedValue({ success: true }),
    retryCloseRetirement: vi.fn().mockResolvedValue({ success: true }),
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
  const defaultSelection = status.found && status.number !== undefined
    ? selection({
        associated_prs: [{
          ...selection().associated_prs[0]!,
          pr_number: status.number,
          title: status.title ?? selection().associated_prs[0]!.title,
          url: status.url ?? `https://github.com/o/r/pull/${status.number}`,
          state: status.state ?? selection().associated_prs[0]!.state,
          draft: status.draft ?? false,
          display_state: status.display_state ?? selection().associated_prs[0]!.display_state,
          base: status.base ?? selection().associated_prs[0]!.base,
          head: status.head ?? selection().associated_prs[0]!.head,
          ...(status.feedback_status === undefined ? {} : { feedback_status: status.feedback_status }),
        }],
        active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: status.number }, provenance: 'inferred' },
      })
    : selection();
  const selectionValue = (status.selection ?? defaultSelection) as NonNullable<PrStatusResponse['selection']>;
  const committedStatus = selectionValue ? { ...status, selection: selectionValue } : status;
  const associated = selectionValue?.associated_prs ?? [];
  return {
    state: { status: 'ready' as const, prStatus: committedStatus },
    refresh: vi.fn().mockResolvedValue(committedStatus),
    refreshForSafety: vi.fn().mockResolvedValue(committedStatus),
    refreshAfterMutation: vi.fn().mockResolvedValue(committedStatus),
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
  refreshForSafety: vi.fn().mockResolvedValue(undefined),
  refreshAfterMutation: vi.fn().mockResolvedValue(undefined),
};

/** Count of glowing primaries across the whole bar — must always be exactly 1
 *  when the bar is in a dispositive (non-continued) state. */
function primaryCount() {
  return document.querySelectorAll('.work-actions-btn--primary').length;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({ close: null } as never);
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
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

  it.each(['idle', 'error'] as const)(
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

  it('is hidden for a context_exhausted phase on Work', () => {
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-1"
        convModeLabel="Work"
        phaseType="context_exhausted"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle()}
      />,
    );
    expect(screen.queryByTestId('abandon-button')).not.toBeInTheDocument();
    expect(screen.queryByTestId('view-diff-button')).not.toBeInTheDocument();
  });
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
  it.each(['error'] as const)(
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

  it('refreshes status after feedback capture with post-mutation ordering', async () => {
    const onSendMessage = vi.fn().mockResolvedValue(undefined);
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://gh/pr/12',
      display_state: 'open',
      check_state: 'failing',
    });

    renderWithProviders(
      <WorkControlBar
        conversationId="conv-capture"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={onSendMessage}
        prStatusHandle={handle}
      />,
    );

    fireEvent.click(screen.getByTestId('address-feedback-button'));

    await waitFor(() => expect(onSendMessage).toHaveBeenCalledWith('Address `.phoenix/pr-context/pr-12.json`'));
    await waitFor(() => expect(handle.refreshAfterMutation).toHaveBeenCalledTimes(1));
    expect(handle.refresh).not.toHaveBeenCalled();
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
    expect(screen.getAllByText(/Checking PR/i)).toHaveLength(2);
    expect(screen.getByTestId('abandon-button')).toBeInTheDocument();
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.getByTestId('desktop-work-controls')).toBeInTheDocument();
    expect(screen.queryByText('Done?')).not.toBeInTheDocument();
    expect(screen.getByTestId('desktop-work-actions-identity')).toHaveTextContent('WorkspaceChecking PR…');
    expect(screen.getByTestId('desktop-work-actions-identity').tagName).toBe('SPAN');
    expect(screen.getByTestId('desktop-work-actions-identity')).not.toHaveAttribute('tabindex');
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

  it('Abandon starts the server-authoritative Close contract without a generic browser confirmation', async () => {
    const confirmSpy = vi.fn();
    const previousConfirm = window.confirm;
    window.confirm = confirmSpy;
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
    await waitFor(() => expect(api.abandonTask).toHaveBeenCalledWith('conv-1'));
    expect(confirmSpy).not.toHaveBeenCalled();
    window.confirm = previousConfirm;
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
  it('resolves desktop ambiguity directly through the persistent PR rail', async () => {
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

    expect(screen.queryByTestId('active-pr-selector-trigger')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /#12 Fix CI open task-123/ }));
    await waitFor(() => expect(handle.pinActivePr).toHaveBeenCalledWith({ repo_owner: 'o', repo_name: 'r', pr_number: 12 }));
  });

  it('uses the compact PR rail as selector owner on tablet widths', () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query === '(max-width: 1024px)',
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });
    const handle = prStatusHandle();

    renderWithProviders(
      <>
        <StateBar
          conversation={{
            id: 'conv-tablet', slug: 'slug', model: 'claude-sonnet-5', cwd: '/repo/.phoenix/worktrees/conv-tablet', created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z', message_count: 1, state: { type: 'idle' }, branch_name: 'task-123', base_branch: 'main', worktree_path: '/repo/.phoenix/worktrees/conv-tablet', task_title: 'Task', conv_mode_label: 'Work', browser_session_active: false, terminal_uses_tmux: false, work_scope_key: 'worktree:/repo/.phoenix/worktrees/conv-tablet',
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
          conversationId="conv-tablet"
          convModeLabel="Work"
          phaseType="idle"
          continuedInConvId={null}
          onSendMessage={vi.fn()}
          prStatusHandle={handle}
        />
      </>,
    );

    expect(screen.queryByTestId('active-pr-selector-trigger')).not.toBeInTheDocument();
    expect(screen.getByTestId('mobile-work-controls')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#12 open/i })).toBeInTheDocument();
  });

  it('commits a newly ambiguous safety refresh before opening the selector', async () => {
    const latest = {
      found: false,
      refresh: { state: 'fresh' as const, stale: false, last_attempted_at: '', last_refreshed_at: '' },
      work_change: cleanWorkChange(),
      associated_prs: [
        { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://gh/pr/12', state: 'OPEN', draft: false, display_state: 'open' as const, base: 'main', head: 'task-123', feedback_status: 'open' as const },
        { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Follow-up', url: 'https://gh/pr/34', state: 'OPEN', draft: false, display_state: 'open' as const, base: 'task-123', head: 'follow-up', feedback_status: 'open' as const },
      ],
    };
    const handle = prStatusHandle({ found: false }, {
      refreshForSafety: vi.fn().mockResolvedValue(latest),
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

    fireEvent.click(screen.getByTestId('clean-up-button'));

    await waitFor(() => expect(handle.refreshForSafety).toHaveBeenCalledTimes(1));
    expect(api.markMerged).not.toHaveBeenCalled();
    expect(screen.getAllByLabelText(/Closes this conversation/, { selector: 'summary' })).toHaveLength(2);
    expect(screen.getAllByText('ⓘ')).toHaveLength(2);
    expect(screen.getByText('Select an active PR before cleaning up or abandoning this task.')).toBeInTheDocument();
    const alert = screen.getByRole('alert');
    expect(alert.closest('.desktop-work-actions-rail')).toBeNull();
    expect(alert.closest('.desktop-work-actions-compact')).toBeInTheDocument();
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

    expect(screen.getByTestId('desktop-work-controls')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#12 Fix CI open task-123/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#34 Follow-up open task-123-follow-up/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Clean up/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Abandon/ })).not.toBeInTheDocument();
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
    expect(screen.getByTestId('desktop-work-controls')).toBeInTheDocument();
    expect(screen.queryByTestId('mobile-pr-actions')).not.toBeInTheDocument();
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
    expect(handle.refreshAfterMutation).not.toHaveBeenCalled();

    resolveSend();

    // Once send completes, PR status refreshes and the label settles back.
    await waitFor(() => {
      expect(handle.refreshAfterMutation).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(screen.getByTestId('address-feedback-button').textContent).toMatch(
        /Address PR #139 feedback/i,
      );
    });
  });
});

describe('WorkControlBar — desktop multi-PR rail', () => {
  it('replaces the wrapped action bar with rich PR chips when multiple PRs are actionable', () => {
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
      selection: {
        ...selection(),
        associated_prs: [
          ...selection().associated_prs,
          { ...selection().associated_prs[0]!, pr_number: 13, title: 'Second desktop PR', url: 'https://github.com/o/r/pull/13', head: 'task-124' },
        ],
      },
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-multi" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('desktop-work-controls')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#12 Fix CI open task-123/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#13 Second desktop PR open task-124/ })).toBeInTheDocument();
    expect(screen.queryByText('Done?')).not.toBeInTheDocument();
  });

  it('expands the active desktop PR into hero and supporting actions', () => {
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_freshness: { state: 'new', count: 2 },
      feedback_status: 'open',
      selection: {
        ...selection(),
        associated_prs: [
          ...selection().associated_prs,
          { ...selection().associated_prs[0]!, pr_number: 13, title: 'Second desktop PR', url: 'https://github.com/o/r/pull/13', head: 'task-124' },
        ],
      },
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-expand" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#12 Fix CI open task-123/ }));
    expect(screen.getByTestId('mobile-primary-address-feedback')).toHaveTextContent('Address feedback · 2 new');
    expect(screen.getByRole('button', { name: 'PR #12 diff' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Workspace diff' })).toBeInTheDocument();
  });

  it('shows each desktop PR feedback status without requiring selection', () => {
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      feedback_status: 'approved',
      selection: {
        ...selection(),
        associated_prs: [
          { ...selection().associated_prs[0]!, feedback_status: 'approved' },
          { ...selection().associated_prs[0]!, pr_number: 13, title: 'Needs review', url: 'https://github.com/o/r/pull/13', head: 'task-124', feedback_status: 'open' },
          { ...selection().associated_prs[0]!, pr_number: 14, title: 'Being handled', url: 'https://github.com/o/r/pull/14', head: 'task-125', feedback_status: 'in_progress' },
        ],
      },
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-feedback" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByRole('button', { name: /#13 Needs review open task-124$/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#14 Being handled open task-125 feedback in progress \(eyes reaction\)/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#12 Fix CI open task-123 feedback approved \(thumbs-up reaction\)/ })).toBeInTheDocument();
    expect(screen.getByText('👀').parentElement).toHaveAttribute('title', 'feedback in progress (eyes reaction)');
    expect(screen.getByText('👍').parentElement).toHaveAttribute('title', 'feedback approved (thumbs-up reaction)');
  });

  it('shows the active PR review state in the compact desktop rail', () => {
    const handle = prStatusHandle({
      found: true,
      number: 12,
      display_state: 'open',
      feedback_status: 'approved',
      selection: {
        ...selection(),
        associated_prs: [
          { ...selection().associated_prs[0]!, feedback_status: 'approved' },
        ],
      },
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-approved" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('desktop-work-actions-identity')).toHaveTextContent('#12open👍');
    expect(screen.getByText('feedback approved (thumbs-up reaction)')).toHaveClass('pr-review-state-label');
  });

  it('renders legacy cached PR identity as status when no selector can open', () => {
    const handle = prStatusHandle(
      { found: true, number: 12, display_state: 'open', feedback_status: 'approved' },
      { activeSelection: null, activePrSummary: null, ambiguous: false },
    );
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-legacy-pr" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    const identity = screen.getByTestId('desktop-work-actions-identity');
    expect(identity.tagName).toBe('SPAN');
    expect(identity).toHaveTextContent('#12open👍');
    expect(identity).not.toHaveAttribute('tabindex');
  });

  it('uses the active summary for compact chip state and review status', () => {
    const handle = prStatusHandle({
      found: true,
      number: 12,
      display_state: 'open',
      feedback_status: 'open',
      selection: {
        ...selection(),
        associated_prs: [
          { ...selection().associated_prs[0]!, display_state: 'draft', feedback_status: 'approved' },
        ],
      },
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-summary-authority" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('desktop-work-actions-identity')).toHaveTextContent('#12draft👍');
    expect(screen.getByRole('button', { name: '#12 draft feedback approved (thumbs-up reaction)' })).toBeInTheDocument();
  });

  it('shows review state on compact PR chips independently of freshness', () => {
    const handle = prStatusHandle({
      found: true,
      number: 12,
      display_state: 'open',
      feedback_status: 'in_progress',
      selection: {
        ...selection(),
        associated_prs: [
          { ...selection().associated_prs[0]!, feedback_status: 'in_progress' },
        ],
      },
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-review-state" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByRole('button', { name: /#12 open feedback in progress \(eyes reaction\)/ })).toBeInTheDocument();
    expect(screen.getByText('👀')).toBeInTheDocument();
  });

  it('uses the compact desktop rail when an explicit active PR is absent from associated summaries', () => {
    const handle = prStatusHandle({
      found: false,
      selection: {
        ...selection(),
        associated_prs: [
          ...selection().associated_prs,
          { ...selection().associated_prs[0]!, pr_number: 13, title: 'Second PR', url: 'https://github.com/o/r/pull/13', head: 'task-124' },
        ],
        active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 99 }, provenance: 'pinned' },
      },
    });
    handle.activePrSummary = null;
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-stale-active" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('desktop-work-controls')).toBeInTheDocument();
    expect(screen.queryByText('Done?')).not.toBeInTheDocument();
  });

  it('uses the compact desktop rail for a single actionable PR', () => {
    renderWithProviders(
      <WorkControlBar conversationId="conv-desktop-single" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={prStatusHandle()} />,
    );

    expect(screen.getByTestId('desktop-work-controls')).toBeInTheDocument();
    expect(screen.queryByText('Done?')).not.toBeInTheDocument();
    expect(screen.getByTestId('view-diff-button')).toBeInTheDocument();
  });
});

describe('WorkControlBar — mobile PR rail (REQ-WAB-011)', () => {
  const enableMobile = () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query === '(max-width: 1024px)',
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });
  };

  const twoOpenPrSelection = (active = true): AssociatedPrStatusEnvelope => ({
    associated_prs: [
      ...selection().associated_prs,
      { ...selection().associated_prs[0]!, pr_number: 13, title: 'Second PR', url: 'https://github.com/o/r/pull/13', head: 'task-124' },
    ],
    ...(active ? { active_pr: selection().active_pr } : {}),
  });

  it('shows a thin rail of open PRs and expands the selected PR actions upward', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_freshness: { state: 'new', count: 3 },
      feedback_status: 'open',
      selection: twoOpenPrSelection(),
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByLabelText('Open pull requests')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#12 open 3 new feedback/ })).toHaveAttribute('aria-expanded', 'false');
    expect(screen.getByRole('button', { name: /#13 open/ })).toBeInTheDocument();
    expect(screen.queryByTestId('mobile-pr-actions')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /#12 open 3 new feedback/ }));
    expect(screen.getByTestId('mobile-pr-actions')).toBeInTheDocument();
    expect(screen.getByTestId('mobile-primary-address-feedback')).toHaveTextContent('Address feedback · 3 new');
    expect(screen.getByRole('button', { name: 'PR #12 diff' })).toHaveTextContent('PR diff');
    expect(screen.getByRole('button', { name: 'Workspace diff' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Clean up' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Abandon\./ })).toHaveClass('mobile-pr-action--danger');
  });

  it('falls back to disposition lifecycle actions when no actionable PR exists', () => {
    enableMobile();
    const handle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-no-pr" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('mobile-work-fallback')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Clean up\./ })).toBeInTheDocument();
    expect(screen.queryByLabelText('Open pull requests')).not.toBeInTheDocument();
  });

  it('keeps dirty no-PR guidance compact and discloses secondary actions', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: false,
      work_change: { kind: 'dirty_needs_review', reason: 'uncommitted_changes' },
      selection: { associated_prs: [] },
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-dirty" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    const dock = screen.getByTestId('mobile-work-fallback');
    expect(dock).toHaveTextContent('Uncommitted changes');
    expect(screen.getByRole('button', { name: 'Review workspace changes' })).toHaveTextContent('Review changes');
    expect(screen.queryByText(/Review, commit, and push/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Abandon\./ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Work status details' }));
    const details = screen.getByRole('status');
    expect(details).toHaveTextContent('Review, commit, and push before opening a PR.');
    expect(details).toHaveTextContent('Closes this conversation');
    const infoTrigger = screen.getByRole('button', { name: 'Work status details' });
    fireEvent.click(infoTrigger);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    fireEvent.click(infoTrigger);

    fireEvent.click(screen.getByRole('button', { name: 'Work status details' }));
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    expect(screen.getByRole('button', { name: /^Abandon\./ })).toBeInTheDocument();
  });

  it('closes the fallback overflow with Escape and returns focus to its trigger', () => {
    enableMobile();
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-mobile-menu"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
      />,
    );

    const trigger = screen.getByRole('button', { name: 'More work actions' });
    fireEvent.click(trigger);
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByLabelText('More work actions', { selector: 'div' })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('keeps overflow open and focus in place when Abandon fails', async () => {
    enableMobile();
    vi.mocked(api.abandonTask).mockRejectedValueOnce(new Error('abandon failed'));
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-mobile-cancel"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    const abandon = screen.getByRole('button', { name: /^Abandon\./ });
    abandon.focus();
    fireEvent.click(abandon);

    expect(await screen.findByRole('alert')).toHaveTextContent('abandon failed');
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();
    expect(abandon).toHaveFocus();
  });

  it('keeps overflow open and focus in place when another Abandon fails', async () => {
    enableMobile();
    const prevConfirm = window.confirm;
    window.confirm = vi.fn(() => true);
    vi.mocked(api.abandonTask).mockRejectedValueOnce(new Error('abandon failed'));
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-mobile-abandon-failure"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    const abandon = screen.getByRole('button', { name: /^Abandon\./ });
    abandon.focus();
    fireEvent.click(abandon);

    expect(await screen.findByRole('alert')).toHaveTextContent('abandon failed');
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();
    expect(abandon).toHaveFocus();
    window.confirm = prevConfirm;
  });

  it('keeps overflow open when Abandon safety refresh requires PR selection', async () => {
    enableMobile();
    const prevConfirm = window.confirm;
    window.confirm = vi.fn(() => true);
    const handle = prStatusHandle(
      { found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } },
      {
        refreshForSafety: vi.fn().mockResolvedValue({
          found: false,
          associated_prs: twoOpenPrSelection(false).associated_prs,
        }),
      },
    );
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-mobile-abandon-safety"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={handle}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    const abandon = screen.getByRole('button', { name: /^Abandon\./ });
    abandon.focus();
    fireEvent.click(abandon);

    expect(await screen.findByRole('alert')).toHaveTextContent('Select an active PR');
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();
    expect(abandon).toHaveFocus();
    expect(api.abandonTask).not.toHaveBeenCalled();
    window.confirm = prevConfirm;
  });

  it('returns Escape focus to the info trigger that opened the details panel', () => {
    enableMobile();
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-mobile-info"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
      />,
    );

    const globalEscapeHandler = vi.fn();
    window.addEventListener('keydown', globalEscapeHandler);
    const trigger = screen.getByRole('button', { name: 'Work status details' });
    fireEvent.click(trigger);
    expect(screen.getByRole('status')).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(globalEscapeHandler).not.toHaveBeenCalled();
    window.removeEventListener('keydown', globalEscapeHandler);
  });

  it('resets an open fallback panel when the conversation identity changes', () => {
    enableMobile();
    const handle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const { rerender } = renderWithProviders(
      <WorkControlBar conversationId="conv-a" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();

    rerender(
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId="conv-b" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>,
    );
    expect(screen.queryByLabelText('More work actions', { selector: 'div' })).not.toBeInTheDocument();
  });

  it('restores owned overflow focus after the conversation identity changes', async () => {
    enableMobile();
    const handle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const renderBar = (conversationId: string) => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId={conversationId} convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar('conv-focus-a'));

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    const abandon = screen.getByRole('button', { name: /^Abandon\./ });
    abandon.focus();
    fireEvent.focusIn(abandon);
    rerender(renderBar('conv-focus-b'));

    expect(screen.queryByLabelText('More work actions', { selector: 'div' })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Work status details' })).toHaveFocus());
  });

  it('resets open details while the Work Actions bar is hidden during an LLM turn', () => {
    enableMobile();
    const handle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const renderBar = (phaseType: 'idle' | 'llm_requesting') => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId="conv-hide-show" convModeLabel="Work" phaseType={phaseType} continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar('idle'));

    fireEvent.click(screen.getByRole('button', { name: 'Work status details' }));
    expect(screen.getByRole('status')).toBeInTheDocument();
    rerender(renderBar('llm_requesting'));
    expect(screen.queryByTestId('mobile-work-fallback')).not.toBeInTheDocument();

    rerender(renderBar('idle'));
    expect(screen.getByTestId('mobile-work-fallback')).toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Clean up\./ })).toBeInTheDocument();
  });

  it('does not reopen overflow after the PR rail temporarily replaces the fallback', () => {
    enableMobile();
    const renderBar = (handle: ReturnType<typeof prStatusHandle>) => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId="conv-stable" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const fallbackHandle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const { rerender } = render(renderBar(fallbackHandle));

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();

    const railHandle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'passing',
      selection: twoOpenPrSelection(),
    });
    rerender(renderBar(railHandle));
    expect(screen.getByLabelText('Open pull requests')).toBeInTheDocument();
    expect(screen.queryByLabelText('More work actions', { selector: 'div' })).not.toBeInTheDocument();

    rerender(renderBar(fallbackHandle));
    expect(screen.getByTestId('mobile-work-fallback')).toBeInTheDocument();
    expect(screen.queryByLabelText('More work actions', { selector: 'div' })).not.toBeInTheDocument();
  });

  it('moves focus to the active PR chip when fallback details become a PR rail', async () => {
    enableMobile();
    const fallbackHandle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const railSelection = twoOpenPrSelection();
    const railHandle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'passing',
      selection: railSelection,
    });
    const renderBar = (handle: ReturnType<typeof prStatusHandle>) => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId="conv-focus-transition" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar(fallbackHandle));

    const detailsTrigger = screen.getByRole('button', { name: 'Work status details' });
    detailsTrigger.focus();
    fireEvent.click(detailsTrigger);
    expect(screen.getByRole('status')).toBeInTheDocument();
    rerender(renderBar(railHandle));

    const activeChip = screen.getByRole('button', { name: /#12/ });
    await waitFor(() => expect(activeChip).toHaveFocus());
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('prefers the active PR chip when it follows another chip in the rail', async () => {
    enableMobile();
    const fallbackHandle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const selectionWithSecondActive: AssociatedPrStatusEnvelope = {
      ...twoOpenPrSelection(false),
      active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 13 }, provenance: 'pinned' },
    };
    const railHandle = prStatusHandle({
      found: true,
      number: 13,
      url: 'https://github.com/o/r/pull/13',
      display_state: 'open',
      check_state: 'passing',
      selection: selectionWithSecondActive,
    });
    const renderBar = (handle: ReturnType<typeof prStatusHandle>) => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId="conv-second-active-focus" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar(fallbackHandle));

    const trigger = screen.getByRole('button', { name: 'Work status details' });
    trigger.focus();
    rerender(renderBar(railHandle));

    const activeChip = screen.getByRole('button', { name: /#13/ });
    await waitFor(() => expect(activeChip).toHaveFocus());
    expect(screen.getByRole('button', { name: /#12/ })).not.toHaveFocus();
  });

  it('moves owned fallback focus to the first PR chip when the rail has no active selection', async () => {
    enableMobile();
    const fallbackHandle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const railHandle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'passing',
      selection: twoOpenPrSelection(false),
    });
    const renderBar = (handle: ReturnType<typeof prStatusHandle>) => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar conversationId="conv-no-active-focus" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar(fallbackHandle));

    const trigger = screen.getByRole('button', { name: 'Work status details' });
    trigger.focus();
    rerender(renderBar(railHandle));

    const firstChip = screen.getAllByRole('button', { name: /#/ })[0]!;
    await waitFor(() => expect(firstChip).toHaveFocus());
  });

  it('does not steal unrelated focus when a background refresh replaces the fallback', async () => {
    enableMobile();
    const fallbackHandle = prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } });
    const railHandle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'passing',
      selection: twoOpenPrSelection(),
    });
    const renderBar = (handle: ReturnType<typeof prStatusHandle>) => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <input aria-label="Composer" />
          <WorkControlBar conversationId="conv-no-focus-steal" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar(fallbackHandle));

    const composer = screen.getByRole('textbox', { name: 'Composer' });
    composer.focus();
    rerender(renderBar(railHandle));

    await waitFor(() => expect(screen.getByLabelText('Open pull requests')).toBeInTheDocument());
    expect(composer).toHaveFocus();
  });

  it('does not render a stale overflow menu after terminal actions disappear', () => {
    enableMobile();
    const renderBar = (continuedInConvId: string | null) => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar
            conversationId="conv-stable"
            convModeLabel="Work"
            phaseType="idle"
            continuedInConvId={continuedInConvId}
            prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
          />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar(null));

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();
    rerender(renderBar('conv-next'));

    expect(screen.getByTestId('mobile-work-fallback')).toHaveTextContent('Continued elsewhere');
    expect(screen.queryByLabelText('More work actions', { selector: 'div' })).not.toBeInTheDocument();
    expect(screen.getByTestId('mobile-work-fallback')).toHaveClass('mobile-work-fallback--status-only');
    expect(screen.queryByRole('button', { name: 'More work actions' })).not.toBeInTheDocument();
  });

  it('closes overflow when its secondary terminal action changes identity', async () => {
    enableMobile();
    const renderBar = (phaseType: 'idle' | 'stuck') => (
      <MemoryRouter>
        <ViewerSlotProvider browserSessionActive={false}>
          <WorkControlBar
            conversationId="conv-changing-secondary"
            convModeLabel="Work"
            phaseType={phaseType}
            continuedInConvId={null}
            prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
          />
        </ViewerSlotProvider>
      </MemoryRouter>
    );
    const { rerender } = render(renderBar('idle'));

    const trigger = screen.getByRole('button', { name: 'More work actions' });
    fireEvent.click(trigger);
    const abandon = screen.getByRole('button', { name: /^Abandon\./ });
    abandon.focus();
    fireEvent.focusIn(abandon);
    rerender(renderBar('stuck'));

    expect(screen.queryByLabelText('More work actions', { selector: 'div' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Clean up\./ })).not.toBeInTheDocument();
  });

  it('replaces Address feedback with PR selection recovery when its target is unresolved', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
    }, {
      activeSelection: {
        associated_prs: [{ ...selection().associated_prs[0]!, pr_number: 13 }],
        active_pr: selection().active_pr,
      },
      activePrSummary: null,
      ambiguous: true,
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-unresolved-active-summary"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={handle}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Select active PR' }));
    expect(requestActivePrSelectorOpen).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('mobile-primary-address-feedback')).not.toBeInTheDocument();
  });

  it('recovers unresolved selection from review disposition without cached PR guidance', async () => {
    enableMobile();
    const resumeInference = vi.fn().mockResolvedValue(undefined);
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'passing',
      work_change: { kind: 'dirty_needs_review', reason: 'uncommitted_changes' },
    }, {
      activeSelection: {
        associated_prs: [],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
      resumeInference,
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-review-recovery" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Resume PR inference' }));
    await waitFor(() => expect(resumeInference).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: 'Work status details' }));
    expect(screen.getByRole('status')).toHaveTextContent('The selected PR is unavailable.');
    expect(screen.getByRole('status')).not.toHaveTextContent('Open PR #12');
    expect(screen.queryByRole('button', { name: 'Review workspace changes' })).not.toBeInTheDocument();
  });

  it('derives compact status from the resolved explicit PR instead of stale cached status', () => {
    enableMobile();
    const explicitPr = { ...selection().associated_prs[0]!, pr_number: 13, display_state: 'merged' as const };
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'passing',
    }, {
      activeSelection: {
        associated_prs: [explicitPr],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: explicitPr,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-explicit-status" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('mobile-work-fallback')).toHaveTextContent('PR merged');
    expect(screen.getByTestId('mobile-work-fallback')).not.toHaveTextContent('PR open');
  });

  it('derives terminal actions from the resolved explicit PR instead of stale cached status', () => {
    const explicitPr = { ...selection().associated_prs[0]!, pr_number: 13, url: 'https://github.com/o/r/pull/13', display_state: 'open' as const };
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
    }, {
      activeSelection: {
        associated_prs: [explicitPr],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: explicitPr,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-explicit-actions" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open PR #13/ })).toBeInTheDocument();
  });

  it('uses every resolved inferred active PR without borrowing cached checks', () => {
    const inferredPr = { ...selection().associated_prs[0]!, pr_number: 13, url: 'https://github.com/o/r/pull/13', display_state: 'open' as const };
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
      feedback_freshness: { state: 'new', count: 4 },
      feedback_coverage: { kind: 'auth_required', surfaces: ['review_threads'] },
    }, {
      activeSelection: {
        associated_prs: [inferredPr],
        active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 13 }, provenance: 'inferred' },
      },
      activePrSummary: inferredPr,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-inferred-actions" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open PR #13/ })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: /Merge on GitHub #13/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/4 new feedback/)).not.toBeInTheDocument();
    expect(document.querySelector('.work-actions-pr-coverage')).not.toBeInTheDocument();
  });

  it('uses the active summary when same-identity cached state is stale', () => {
    const activePr = { ...selection().associated_prs[0]!, display_state: 'open' as const };
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
    }, {
      activeSelection: selection({ associated_prs: [activePr] }),
      activePrSummary: activePr,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-stale-state" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open PR #12/ })).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: /Merge on GitHub #12/ })).not.toBeInTheDocument();
  });

  it('does not reuse cached status for a same-number active PR in another repository', () => {
    const activePr = {
      ...selection().associated_prs[0]!,
      repo_owner: 'other-owner',
      repo_name: 'other-repo',
      url: 'https://github.com/other-owner/other-repo/pull/12',
      display_state: 'open' as const,
    };
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
    }, {
      activeSelection: {
        associated_prs: [activePr],
        active_pr: {
          pr: { repo_owner: 'other-owner', repo_name: 'other-repo', pr_number: 12 },
          provenance: 'pinned',
        },
      },
      activePrSummary: activePr,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-cross-repo-actions" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    const openPr = screen.getByRole('link', { name: /Open PR #12/ });
    expect(openPr).toHaveAttribute('href', 'https://github.com/other-owner/other-repo/pull/12');
    expect(screen.queryByRole('link', { name: /Merge on GitHub #12/ })).not.toBeInTheDocument();
  });

  it('does not render a stale cached PR link beside an unresolved explicit selection', () => {
    enableMobile();
    const resumeInference = vi.fn().mockResolvedValue(undefined);
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'draft',
      check_state: 'pending',
    }, {
      activeSelection: {
        associated_prs: [],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
      resumeInference,
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-stale-link" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.queryByRole('link', { name: /Open PR #12/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Resume PR inference' }));
    expect(resumeInference).toHaveBeenCalledTimes(1);
  });

  it('does not target a stale cached PR beside an unresolved explicit selection', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
    }, {
      activeSelection: {
        associated_prs: [{ ...selection().associated_prs[0]!, pr_number: 13 }],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-explicit-summary-missing"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={handle}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Select active PR' }));
    expect(requestActivePrSelectorOpen).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('mobile-primary-address-feedback')).not.toBeInTheDocument();
  });

  it('resumes inference for unresolved feedback selection without actionable PR choices', async () => {
    enableMobile();
    const resumeInference = vi.fn().mockResolvedValue(undefined);
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
    }, {
      activeSelection: {
        associated_prs: [],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
      resumeInference,
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-feedback-no-options"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={handle}
      />,
    );

    expect(screen.queryByRole('button', { name: 'Select active PR' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Resume PR inference' }));
    await waitFor(() => expect(resumeInference).toHaveBeenCalledTimes(1));
  });

  it('blocks cached-derived cleanup while an explicit selected PR summary is unresolved', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
    }, {
      activeSelection: {
        associated_prs: [{ ...selection().associated_prs[0]!, pr_number: 13 }],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-stale-merged-cache"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={handle}
      />,
    );

    expect(screen.queryByRole('button', { name: /^Clean up\./ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Abandon\./ })).not.toBeInTheDocument();
    expect(screen.getByTestId('mobile-work-fallback')).toHaveTextContent('Active PR unavailable');
    fireEvent.click(screen.getByRole('button', { name: 'Select active PR' }));
    expect(requestActivePrSelectorOpen).toHaveBeenCalledTimes(1);
    expect(api.markMerged).not.toHaveBeenCalled();
  });

  it('resumes inference when an unresolved selection has no actionable PR choices', async () => {
    enableMobile();
    const resumeInference = vi.fn().mockResolvedValue(undefined);
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
    }, {
      activeSelection: {
        associated_prs: [],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
      resumeInference,
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-unresolved-no-options"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={handle}
      />,
    );

    expect(screen.queryByRole('button', { name: 'Select active PR' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Resume PR inference' }));
    await waitFor(() => expect(resumeInference).toHaveBeenCalledTimes(1));
    expect(api.markMerged).not.toHaveBeenCalled();
  });

  it('suppresses desktop recovery after the conversation continues elsewhere', () => {
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
    }, {
      activeSelection: {
        associated_prs: [],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
      resumeInference: vi.fn().mockResolvedValue(undefined),
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-desktop-continued"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId="conv-next"
        prStatusHandle={handle}
      />,
    );

    expect(screen.queryByRole('button', { name: 'Resume PR inference' })).not.toBeInTheDocument();
    expect(screen.getByText('Continued — actions belong on the continuation.')).toBeInTheDocument();
  });

  it('offers desktop inference recovery for an unresolved explicit selection', async () => {
    const resumeInference = vi.fn().mockResolvedValue(undefined);
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'merged',
      check_state: 'passing',
    }, {
      activeSelection: {
        associated_prs: [],
        active_pr: { mode: 'explicit', pr_number: 13, active_source: 'user_pinned' },
      },
      activePrSummary: null,
      ambiguous: false,
      resumeInference,
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-desktop-unresolved"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={handle}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Resume PR inference' }));
    await waitFor(() => expect(resumeInference).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: /Open PR #12/ })).not.toBeInTheDocument();
    expect(screen.getByTestId('view-diff-button')).not.toHaveClass('work-actions-btn--primary');
    expect(screen.getByRole('button', { name: 'Resume PR inference' })).toHaveClass('work-actions-btn--primary');
    expect(screen.queryByRole('link', { name: /Merge PR #12|Open PR #12/ })).not.toBeInTheDocument();
  });

  it('does not target cached feedback when the live PR envelope is empty', () => {
    enableMobile();
    const onSendMessage = vi.fn();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
    }, {
      activeSelection: { associated_prs: [] },
      activePrSummary: null,
      ambiguous: false,
    });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-cached-empty-envelope"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={onSendMessage}
        prStatusHandle={handle}
      />,
    );

    expect(screen.queryByTestId('mobile-primary-address-feedback')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Resume PR inference' })).toBeInTheDocument();
    expect(onSendMessage).not.toHaveBeenCalled();
  });

  it('prioritizes feedback status over cached workspace-change status', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
      feedback_freshness: { state: 'new', count: 1 },
      work_change: { kind: 'loading' },
    }, { activeSelection: null, activePrSummary: null });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-cached-feedback"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={handle}
      />,
    );

    expect(screen.getByTestId('mobile-work-fallback')).toHaveTextContent('PR feedback ready');
    expect(screen.getByTestId('mobile-primary-address-feedback')).toBeInTheDocument();
    expect(screen.getByTestId('mobile-work-fallback')).not.toHaveTextContent('Checking changes');
  });

  it('does not claim actionable feedback from review state alone', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      work_change: { kind: 'loading' },
    }, { activeSelection: null, activePrSummary: null });
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-cached-no-feedback"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        onSendMessage={vi.fn()}
        prStatusHandle={handle}
      />,
    );

    expect(screen.getByTestId('mobile-work-fallback')).toHaveTextContent('PR open');
    expect(screen.getByTestId('mobile-primary-address-feedback')).toBeInTheDocument();
    expect(screen.getByTestId('mobile-work-fallback')).not.toHaveTextContent('PR feedback ready');
  });

  it('keeps overflow cleanup and focus available when cleanup fails', async () => {
    enableMobile();
    vi.mocked(api.markMerged).mockRejectedValueOnce(new Error('cleanup failed'));
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-cleanup-failure"
        convModeLabel="Work"
        phaseType="error"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'More work actions' }));
    const cleanup = screen.getByRole('button', { name: /^Clean up\./ });
    cleanup.focus();
    fireEvent.click(cleanup);

    expect(await screen.findByRole('alert')).toHaveTextContent('cleanup failed');
    expect(screen.getByLabelText('More work actions', { selector: 'div' })).toBeInTheDocument();
    expect(cleanup).toHaveFocus();
  });

  it('keeps a terminal-action error visible while details are open', async () => {
    enableMobile();
    vi.mocked(api.markMerged).mockRejectedValueOnce(new Error('cleanup remains visible'));
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-error-details"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /^Clean up\./ }));
    expect(await screen.findByRole('alert')).toHaveTextContent('cleanup remains visible');
    fireEvent.click(screen.getByRole('button', { name: 'Work status details' }));

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('cleanup remains visible');
    expect(screen.getByRole('status').nextElementSibling).toBe(screen.getByRole('alert'));
  });

  it('keeps the primary and overflow actions available while details are open', () => {
    enableMobile();
    renderWithProviders(
      <WorkControlBar
        conversationId="conv-covered-actions"
        convModeLabel="Work"
        phaseType="idle"
        continuedInConvId={null}
        prStatusHandle={prStatusHandle({ found: false, work_change: { kind: 'clean' }, selection: { associated_prs: [] } })}
      />,
    );

    expect(screen.getByRole('button', { name: /^Clean up\./ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'More work actions' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Work status details' }));
    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Clean up\./ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'More work actions' })).toBeInTheDocument();
  });

  it('describes an unrepresentable PR resolve action instead of cleanup', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
    }, { activeSelection: null, activePrSummary: null });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-open-link" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.getByRole('link', { name: /Open PR #12/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Work status details' }));
    const details = screen.getByRole('status');
    expect(details).toHaveTextContent('Open PR #12 on GitHub to review its current state.');
    expect(details).toHaveTextContent('Closes this conversation');
    expect(details).not.toHaveTextContent('Mark as merged');
  });

  it('keeps cleanup presence aligned with WorkDisposition for an open PR', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
      selection: twoOpenPrSelection(),
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-open" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#12 open/ }));
    expect(screen.queryByRole('button', { name: 'Clean up' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Abandon\./ })).toBeInTheDocument();
  });

  it('renders feedback coverage warnings on the mobile Address feedback hero', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
      feedback_coverage: { kind: 'auth_required', surfaces: ['review_threads'] },
      selection: twoOpenPrSelection(),
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-coverage" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#12 open/ }));
    const hero = screen.getByTestId('mobile-primary-address-feedback');
    expect(hero.querySelector('.work-actions-pr-coverage')).toHaveTextContent('GitHub sign-in needed');
  });

  it('preserves terminal action explanations on mobile', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
      selection: twoOpenPrSelection(),
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-hints" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#12 open/ }));
    expect(screen.getByRole('button', { name: /^Abandon\./ })).toHaveAttribute('title', expect.stringContaining('Closes this conversation'));
  });

  it('uses lifecycle fallback when the active PR is terminal but another PR is open', () => {
    enableMobile();
    const mixedSelection = selection({
      associated_prs: [
        { ...selection().associated_prs[0]!, state: 'CLOSED', display_state: 'merged' },
        { ...selection().associated_prs[0]!, pr_number: 13, title: 'Still open', url: 'https://github.com/o/r/pull/13', head: 'task-124' },
      ],
    });
    const handle = prStatusHandle({ found: true, number: 12, display_state: 'merged', selection: mixedSelection });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-terminal-active" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('mobile-work-fallback')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Clean up\./ })).toHaveAttribute('title', expect.stringContaining('Exact tracked, untracked'));
    expect(screen.queryByLabelText('Open pull requests')).not.toBeInTheDocument();
  });

  it('renders disposition guidance beside an actionable PR rail', () => {
    enableMobile();
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-continued" convModeLabel="Work" phaseType="idle" continuedInConvId="continued-conversation" prStatusHandle={prStatusHandle()} />,
    );

    expect(screen.getByLabelText('Open pull requests')).toBeInTheDocument();
    expect(screen.getByText('Continued — actions belong on the continuation.')).toBeInTheDocument();
  });

  it('keeps Address feedback for a cached PR before associations load', () => {
    enableMobile();
    const handle = prStatusHandle(
      { found: true, number: 12, url: 'https://github.com/o/r/pull/12', display_state: 'open', check_state: 'failing', feedback_status: 'open' },
      { activeSelection: null, activePrSummary: null, ambiguous: false },
    );
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-cached" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    expect(screen.getByTestId('mobile-work-fallback')).toBeInTheDocument();
    expect(screen.getByTestId('mobile-primary-address-feedback')).toHaveTextContent('Address feedback');
  });

  it('keeps stuck-phase Abandon as the mobile hero action', () => {
    enableMobile();
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      selection: twoOpenPrSelection(),
    });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-stuck" convModeLabel="Work" phaseType="error" continuedInConvId={null} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#12 open/ }));
    expect(screen.getByRole('button', { name: /^Abandon\./ })).toHaveClass('mobile-pr-action--hero');
    expect(screen.getByRole('button', { name: /^Clean up\./ })).not.toHaveClass('mobile-pr-action--hero');
  });

  it('keeps terminal cleanup primary in the mobile fallback', () => {
    enableMobile();
    const terminalSelection = selection({
      associated_prs: [{ ...selection().associated_prs[0]!, state: 'MERGED', display_state: 'merged' }],
    });
    const handle = prStatusHandle({ found: true, number: 12, display_state: 'merged', selection: terminalSelection });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-merged" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    expect(screen.getByRole('button', { name: /^Clean up\./ })).toHaveClass('mobile-pr-action--hero');
    expect(screen.getByTestId('mobile-work-fallback')).toHaveTextContent('PR merged');
    expect(screen.getByRole('button', { name: 'More work actions' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Abandon\./ })).not.toBeInTheDocument();
  });

  it('lets a pinned mobile selection resume automatic inference', async () => {
    enableMobile();
    const resumeInference = vi.fn().mockResolvedValue(undefined);
    const pinnedSelection = twoOpenPrSelection();
    pinnedSelection.active_pr = { ...pinnedSelection.active_pr!, provenance: 'pinned' };
    const handle = prStatusHandle({
      found: true,
      number: 12,
      url: 'https://github.com/o/r/pull/12',
      display_state: 'open',
      check_state: 'failing',
      feedback_status: 'open',
      selection: pinnedSelection,
    }, { resumeInference });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-pinned" convModeLabel="Work" phaseType="idle" continuedInConvId={null} onSendMessage={vi.fn()} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#12 open/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Auto' }));
    await waitFor(() => expect(resumeInference).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId('mobile-pr-actions')).not.toBeInTheDocument();
  });

  it('pins a different open PR through the shared handle', async () => {
    enableMobile();
    const pinActivePr = vi.fn().mockResolvedValue(undefined);
    const handle = prStatusHandle({ found: false, selection: twoOpenPrSelection(false) }, { pinActivePr });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-select" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#13 open/ }));
    await waitFor(() => expect(pinActivePr).toHaveBeenCalledWith({ repo_owner: 'o', repo_name: 'r', pr_number: 13 }));
  });

  it('reports a failed active-PR mutation without inventing a selection', async () => {
    enableMobile();
    const pinActivePr = vi.fn().mockRejectedValue(new Error('Could not save selection'));
    const handle = prStatusHandle({ found: false, selection: twoOpenPrSelection(false) }, { pinActivePr });
    renderWithProviders(
      <WorkControlBar conversationId="conv-mobile-error" convModeLabel="Work" phaseType="idle" continuedInConvId={null} prStatusHandle={handle} />,
    );

    fireEvent.click(screen.getByRole('button', { name: /#13 open/ }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not save selection');
    expect(screen.queryByTestId('mobile-pr-actions')).not.toBeInTheDocument();
  });
});
