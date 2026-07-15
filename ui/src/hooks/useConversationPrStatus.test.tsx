import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { useConversationPrStatus } from './useConversationPrStatus';
import { api, type CachedPrSummary, type PrStatusResponse } from '../api';

vi.mock('../api', () => ({
  api: {
    getPrStatus: vi.fn(),
  },
}));

function prStatus(number: number): PrStatusResponse {
  return {
    found: true,
    number,
    display_state: 'open',
    refresh: {
      state: 'fresh',
      last_attempted_at: '2026-01-01T00:00:00Z',
      last_refreshed_at: '2026-01-01T00:00:00Z',
      stale: false,
    },
    work_change: { kind: 'clean' },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((innerResolve) => { resolve = innerResolve; });
  return { promise, resolve };
}

function cachedPr(number: number): CachedPrSummary {
  return {
    number,
    title: `Cached PR ${number}`,
    url: `https://github.com/example/repo/pull/${number}`,
    display_state: 'open',
    base: 'main',
    head: `branch-conv-${number}`,
  };
}

function Probe({ conversationId, cached }: { conversationId: string; cached?: CachedPrSummary | null }) {
  const handle = useConversationPrStatus({
    conversationId,
    convModeLabel: 'Work',
    branchName: `branch-${conversationId}`,
    cachedPr: cached,
  });
  const number = handle.state.status === 'ready' ? handle.state.prStatus.number : 'none';
  const title = handle.state.status === 'ready' ? handle.state.prStatus.title : 'none';
  const refreshState = handle.state.status === 'ready' ? handle.state.prStatus.refresh.state : 'none';
  const associatedCount = handle.activeSelection?.associated_prs.length ?? 0;
  const activePrNumber = handle.activePrSummary?.pr_number ?? 'none';
  const ambiguous = handle.ambiguous ? 'yes' : 'no';
  return <div><span data-testid="pr-number">{number}</span><span data-testid="pr-title">{title}</span><span data-testid="refresh-state">{refreshState}</span><span data-testid="associated-count">{associatedCount}</span><span data-testid="active-pr-number">{activePrNumber}</span><span data-testid="ambiguous">{ambiguous}</span></div>;
}

describe('useConversationPrStatus', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('ignores stale refresh results after the conversation scope changes', async () => {
    const first = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockImplementation((conversationId: string) => {
      if (conversationId === 'conv-1') return first.promise;
      if (conversationId === 'conv-2') return Promise.resolve(prStatus(2));
      return Promise.reject(new Error(`unexpected conversation ${conversationId}`));
    });

    const { rerender } = render(<Probe conversationId="conv-1" />);
    await waitFor(() => {
      expect(getPrStatus).toHaveBeenCalledWith('conv-1');
    });

    rerender(<Probe conversationId="conv-2" />);
    await waitFor(() => {
      expect(screen.getByTestId('pr-number')).toHaveTextContent('2');
    });

    first.resolve(prStatus(1));
    await waitFor(() => {
      expect(screen.getByTestId('pr-number')).toHaveTextContent('2');
    });
  });

  it('seeds ready state from cached PR while the fresh status loads', async () => {
    const fresh = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValue(fresh.promise);

    render(<Probe conversationId="conv-7" cached={cachedPr(7)} />);

    expect(screen.getByTestId('pr-number')).toHaveTextContent('7');
    expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 7');
    expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable');
    await waitFor(() => {
      expect(getPrStatus).toHaveBeenCalledWith('conv-7');
    });

    fresh.resolve({ ...prStatus(8), title: 'Fresh PR 8' });
    await waitFor(() => {
      expect(screen.getByTestId('pr-number')).toHaveTextContent('8');
      expect(screen.getByTestId('pr-title')).toHaveTextContent('Fresh PR 8');
    });
  });

  it('keeps cached PR selection display-only while the fresh status loads', async () => {
    const fresh = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValue(fresh.promise);

    render(<Probe conversationId="conv-7" cached={cachedPr(7)} />);

    expect(screen.getByTestId('pr-number')).toHaveTextContent('7');
    expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 7');
    expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable');
    expect(screen.getByTestId('associated-count')).toHaveTextContent('0');
    expect(screen.getByTestId('active-pr-number')).toHaveTextContent('none');
    expect(screen.getByTestId('ambiguous')).toHaveTextContent('no');
    await waitFor(() => {
      expect(getPrStatus).toHaveBeenCalledWith('conv-7');
    });

    fresh.resolve({ ...prStatus(8), title: 'Fresh PR 8' });
    await waitFor(() => {
      expect(screen.getByTestId('pr-number')).toHaveTextContent('8');
      expect(screen.getByTestId('pr-title')).toHaveTextContent('Fresh PR 8');
    });
  });

  it('does not show a previous cached seed after switching to a conversation without cached PR', async () => {
    const first = deferred<PrStatusResponse>();
    const second = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockImplementation((conversationId: string) => {
      if (conversationId === 'conv-1') return first.promise;
      if (conversationId === 'conv-2') return second.promise;
      return Promise.reject(new Error(`unexpected conversation ${conversationId}`));
    });

    const { rerender } = render(<Probe conversationId="conv-1" cached={cachedPr(1)} />);
    expect(screen.getByTestId('pr-number')).toHaveTextContent('1');

    rerender(<Probe conversationId="conv-2" cached={null} />);
    expect(screen.getByTestId('pr-number')).toHaveTextContent('none');

    first.resolve(prStatus(1));
    await waitFor(() => {
      expect(screen.getByTestId('pr-number')).toHaveTextContent('none');
    });

    second.resolve(prStatus(2));
    await waitFor(() => {
      expect(screen.getByTestId('pr-number')).toHaveTextContent('2');
    });
  });


  it('does not replace fresh status when cached snapshot object churns', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockResolvedValue({ ...prStatus(8), title: 'Fresh PR 8' });

    const { rerender } = render(<Probe conversationId="conv-8" cached={cachedPr(8)} />);
    await waitFor(() => {
      expect(screen.getByTestId('pr-title')).toHaveTextContent('Fresh PR 8');
    });

    rerender(<Probe conversationId="conv-8" cached={cachedPr(99)} />);

    expect(screen.getByTestId('pr-title')).toHaveTextContent('Fresh PR 8');
    expect(getPrStatus).toHaveBeenCalledTimes(1);
  });



  it('shows a semantically new cached PR while current status is a stale seed', async () => {
    const fresh = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValue(fresh.promise);

    const { rerender } = render(<Probe conversationId="conv-10" cached={cachedPr(10)} />);
    expect(screen.getByTestId('pr-number')).toHaveTextContent('10');

    rerender(<Probe conversationId="conv-10" cached={cachedPr(11)} />);

    expect(screen.getByTestId('pr-number')).toHaveTextContent('11');
    expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 11');
  });

  it('shows a cached PR that arrives after a not-found refresh result', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockResolvedValue({
      found: false,
      refresh: {
        state: 'not_found',
        last_attempted_at: '2026-01-01T00:00:00Z',
        stale: false,
      },
      work_change: { kind: 'clean' },
    } satisfies PrStatusResponse);

    const { rerender } = render(<Probe conversationId="conv-12" cached={null} />);
    await waitFor(() => {
      expect(screen.getByTestId('refresh-state')).toHaveTextContent('not_found');
    });

    rerender(<Probe conversationId="conv-12" cached={cachedPr(12)} />);

    expect(screen.getByTestId('pr-number')).toHaveTextContent('12');
    expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 12');
  });

  it('reads flattened selection fields from the PR status response', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockResolvedValue({
      found: true,
      number: 12,
      title: 'Flattened PR 12',
      display_state: 'open',
      refresh: {
        state: 'fresh',
        last_attempted_at: '2026-01-01T00:00:00Z',
        last_refreshed_at: '2026-01-01T00:00:00Z',
        stale: false,
      },
      work_change: { kind: 'clean' },
      associated_prs: [{
        repo_owner: 'o',
        repo_name: 'r',
        pr_number: 12,
        title: 'Flattened PR 12',
        url: 'https://github.com/o/r/pull/12',
        state: 'OPEN',
        draft: false,
        display_state: 'open',
        base: 'main',
        head: 'branch-conv-12',
        feedback_status: 'open',
      }],
      active_pr: {
        pr: { repo_owner: 'o', repo_name: 'r', pr_number: 12 },
        provenance: 'inferred',
      },
      latest_observed_branch: {
        repository_identity: 'o/r',
        branch_name: 'branch-conv-12',
      },
    } satisfies PrStatusResponse);

    render(<Probe conversationId="conv-12" />);

    await waitFor(() => {
      expect(screen.getByTestId('associated-count')).toHaveTextContent('1');
      expect(screen.getByTestId('active-pr-number')).toHaveTextContent('12');
    });
  });

  it('keeps the cached PR link when the fresh status request fails', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockRejectedValue(new Error('network failed'));

    render(<Probe conversationId="conv-9" cached={cachedPr(9)} />);

    await waitFor(() => {
      expect(getPrStatus).toHaveBeenCalledWith('conv-9');
      expect(screen.getByTestId('pr-number')).toHaveTextContent('9');
      expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 9');
      expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable');
    });
  });

  it('keeps cached PR display-only after the fresh status request fails', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockRejectedValue(new Error('network failed'));

    render(<Probe conversationId="conv-9" cached={cachedPr(9)} />);

    await waitFor(() => {
      expect(getPrStatus).toHaveBeenCalledWith('conv-9');
      expect(screen.getByTestId('pr-number')).toHaveTextContent('9');
      expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 9');
      expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable');
      expect(screen.getByTestId('associated-count')).toHaveTextContent('0');
      expect(screen.getByTestId('active-pr-number')).toHaveTextContent('none');
      expect(screen.getByTestId('ambiguous')).toHaveTextContent('no');
    });
  });

});
