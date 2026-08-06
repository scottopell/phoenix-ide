import { StrictMode } from 'react';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import { useConversationPrStatus } from './useConversationPrStatus';
import { api, type CachedPrSummary, type PrStatusResponse } from '../api';

vi.mock('../api', () => ({
  api: {
    getPrStatus: vi.fn(),
    resumeAssociatedPrInference: vi.fn(),
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
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve;
    reject = innerReject;
  });
  return { promise, resolve, reject };
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

function Probe({ conversationId, cached }: { conversationId: string | null; cached?: CachedPrSummary | null }) {
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
  return <div><span data-testid="pr-number">{number}</span><span data-testid="pr-title">{title}</span><span data-testid="refresh-state">{refreshState}</span><span data-testid="associated-count">{associatedCount}</span><span data-testid="active-pr-number">{activePrNumber}</span><span data-testid="ambiguous">{ambiguous}</span><button type="button" onClick={() => void handle.refresh()}>Refresh</button><button type="button" onClick={() => void handle.refreshForSafety()}>Safety refresh</button><button type="button" onClick={() => void handle.resumeInference?.()}>Resume inference</button></div>;
}

describe('useConversationPrStatus', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('starts an explicit safety refresh instead of reusing a background read', async () => {
    const background = deferred<PrStatusResponse>();
    const explicit = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValueOnce(background.promise).mockReturnValueOnce(explicit.promise);

    render(<Probe conversationId="conv-explicit" />);
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(1));

    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));

    await act(async () => {
      background.resolve(prStatus(40));
      await background.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('none');

    explicit.resolve(prStatus(41));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('41'));
  });

  it('starts a safety read instead of reusing an ordinary explicit read', async () => {
    const ordinary = deferred<PrStatusResponse>();
    const safety = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus
      .mockResolvedValueOnce(prStatus(57))
      .mockReturnValueOnce(ordinary.promise)
      .mockReturnValueOnce(safety.promise);

    render(<Probe conversationId="conv-safety" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('57'));
    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));

    screen.getByRole('button', { name: 'Safety refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(3));

    await act(async () => {
      ordinary.resolve(prStatus(58));
      await ordinary.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('57');
    safety.resolve(prStatus(59));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('59'));
  });

  it('retries a safety read after a later state-driven refresh supersedes it', async () => {
    const safety = deferred<PrStatusResponse>();
    const stateUpdate = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus
      .mockResolvedValueOnce(prStatus(65))
      .mockReturnValueOnce(safety.promise)
      .mockReturnValueOnce(stateUpdate.promise);

    render(<Probe conversationId="conv-safety-priority" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('65'));
    screen.getByRole('button', { name: 'Safety refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));

    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(3));
    await act(async () => {
      safety.resolve(prStatus(66));
      await safety.promise;
    });
    expect(getPrStatus).toHaveBeenCalledTimes(3);

    await act(async () => {
      stateUpdate.resolve(prStatus(67));
      await stateUpdate.promise;
    });
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('67'));
  });

  it('cancels a safety result after navigating away and back to the same scope', async () => {
    const oldSafety = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus
      .mockResolvedValueOnce(prStatus(72))
      .mockReturnValueOnce(oldSafety.promise)
      .mockResolvedValueOnce(prStatus(73));

    const { rerender } = render(<Probe conversationId="conv-aba" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('72'));
    screen.getByRole('button', { name: 'Safety refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));

    rerender(<Probe conversationId={null} />);
    rerender(<Probe conversationId="conv-aba" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('73'));

    await act(async () => {
      oldSafety.resolve(prStatus(74));
      await oldSafety.promise;
    });
    expect(getPrStatus).toHaveBeenCalledTimes(3);
    expect(screen.getByTestId('pr-number')).toHaveTextContent('73');
  });

  it('starts a new read for each state-driven explicit refresh', async () => {
    const earlier = deferred<PrStatusResponse>();
    const later = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus
      .mockResolvedValueOnce(prStatus(60))
      .mockReturnValueOnce(earlier.promise)
      .mockReturnValueOnce(later.promise);

    render(<Probe conversationId="conv-explicit-order" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('60'));
    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));
    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(3));

    await act(async () => {
      earlier.resolve(prStatus(61));
      await earlier.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('60');
    later.resolve(prStatus(62));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('62'));
  });

  it('starts a valid replacement request after StrictMode invalidates effect setup', async () => {
    const stale = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValueOnce(stale.promise).mockResolvedValueOnce(prStatus(46));

    render(<StrictMode><Probe conversationId="conv-strict" /></StrictMode>);

    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('46'));
    await act(async () => {
      stale.resolve(prStatus(45));
      await stale.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('46');
  });

  it('allows a routine retry immediately after a failed refresh', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockRejectedValueOnce(new Error('network failed')).mockResolvedValueOnce(prStatus(47));

    render(<Probe conversationId="conv-retry" />);
    await waitFor(() => expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable'));

    document.dispatchEvent(new Event('visibilitychange'));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('47'));
    expect(getPrStatus).toHaveBeenCalledTimes(2);
  });

  it('starts a new status read after a mutation when an older read is pending', async () => {
    const beforeMutation = deferred<PrStatusResponse>();
    const afterMutation = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    const resumeInference = api.resumeAssociatedPrInference as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValueOnce(beforeMutation.promise).mockReturnValueOnce(afterMutation.promise);
    resumeInference.mockResolvedValue(undefined);

    render(<Probe conversationId="conv-mutation" />);
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(1));
    screen.getByRole('button', { name: 'Resume inference' }).click();
    await waitFor(() => expect(resumeInference).toHaveBeenCalledWith('conv-mutation'));
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));

    await act(async () => {
      beforeMutation.resolve(prStatus(48));
      await beforeMutation.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('none');
    afterMutation.resolve(prStatus(49));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('49'));
  });

  it('does not reuse a pre-mutation explicit read for post-mutation state', async () => {
    const beforeMutation = deferred<PrStatusResponse>();
    const afterMutation = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    const resumeInference = api.resumeAssociatedPrInference as ReturnType<typeof vi.fn>;
    getPrStatus
      .mockResolvedValueOnce(prStatus(53))
      .mockReturnValueOnce(beforeMutation.promise)
      .mockReturnValueOnce(afterMutation.promise);
    resumeInference.mockResolvedValue(undefined);

    render(<Probe conversationId="conv-mutation-explicit" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('53'));
    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));

    screen.getByRole('button', { name: 'Resume inference' }).click();
    await waitFor(() => expect(resumeInference).toHaveBeenCalledWith('conv-mutation-explicit'));
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(3));

    await act(async () => {
      beforeMutation.resolve(prStatus(54));
      await beforeMutation.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('53');
    afterMutation.resolve(prStatus(55));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('55'));
  });

  it('does not reuse a pre-mutation safety read for post-mutation state', async () => {
    const safety = deferred<PrStatusResponse>();
    const afterMutation = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    const resumeInference = api.resumeAssociatedPrInference as ReturnType<typeof vi.fn>;
    getPrStatus
      .mockResolvedValueOnce(prStatus(69))
      .mockReturnValueOnce(safety.promise)
      .mockReturnValueOnce(afterMutation.promise);
    resumeInference.mockResolvedValue(undefined);

    render(<Probe conversationId="conv-mutation-safety" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('69'));
    screen.getByRole('button', { name: 'Safety refresh' }).click();
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));

    screen.getByRole('button', { name: 'Resume inference' }).click();
    await waitFor(() => expect(resumeInference).toHaveBeenCalledWith('conv-mutation-safety'));
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(3));

    await act(async () => {
      safety.resolve(prStatus(70));
      await safety.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('69');
    expect(getPrStatus).toHaveBeenCalledTimes(3);
    afterMutation.resolve(prStatus(71));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('71'));
  });

  it('coalesces a visibility refresh with a scheduled poll', async () => {
    vi.useFakeTimers();
    try {
      const poll = deferred<PrStatusResponse>();
      const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
      getPrStatus.mockResolvedValueOnce(prStatus(44)).mockReturnValueOnce(poll.promise);

      render(<Probe conversationId="conv-poll" />);
      await act(async () => { await Promise.resolve(); });
      expect(getPrStatus).toHaveBeenCalledTimes(1);

      await act(async () => { vi.advanceTimersByTime(60_000); });
      expect(getPrStatus).toHaveBeenCalledTimes(2);

      document.dispatchEvent(new Event('visibilitychange'));
      expect(getPrStatus).toHaveBeenCalledTimes(2);

      await act(async () => {
        poll.resolve(prStatus(45));
        await poll.promise;
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it('replaces a stalled in-flight refresh instead of reusing it indefinitely', async () => {
    const stalled = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    const dateNow = vi.spyOn(Date, 'now').mockReturnValueOnce(1_000).mockReturnValue(40_000);
    getPrStatus.mockReturnValueOnce(stalled.promise).mockResolvedValueOnce(prStatus(67));
    try {
      render(<Probe conversationId="conv-stalled" />);
      await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(1));

      document.dispatchEvent(new Event('visibilitychange'));
      await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('67'));

      await act(async () => {
        stalled.resolve(prStatus(68));
        await stalled.promise;
      });
      expect(screen.getByTestId('pr-number')).toHaveTextContent('67');
    } finally {
      dateNow.mockRestore();
    }
  });

  it('suppresses recent routine visibility refreshes but preserves explicit refreshes', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockResolvedValueOnce(prStatus(42)).mockResolvedValueOnce(prStatus(43));

    render(<Probe conversationId="conv-fresh" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('42'));

    document.dispatchEvent(new Event('visibilitychange'));
    expect(getPrStatus).toHaveBeenCalledTimes(1);

    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('43'));
    expect(getPrStatus).toHaveBeenCalledTimes(2);
  });

  it('suppresses routine refreshes after a successful not-found result', async () => {
    const notFound: PrStatusResponse = {
      found: false,
      refresh: {
        state: 'not_found',
        last_attempted_at: '2026-01-01T00:00:00Z',
        stale: false,
      },
      work_change: { kind: 'clean' },
    };
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockResolvedValue(notFound);

    render(<Probe conversationId="conv-not-found" />);
    await waitFor(() => expect(screen.getByTestId('refresh-state')).toHaveTextContent('not_found'));

    document.dispatchEvent(new Event('visibilitychange'));
    expect(getPrStatus).toHaveBeenCalledTimes(1);
  });

  it('expires routine freshness when the wall clock moves backward', async () => {
    const dateNow = vi.spyOn(Date, 'now')
      .mockReturnValueOnce(20_000)
      .mockReturnValueOnce(20_000)
      .mockReturnValue(0);
    try {
      const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
      getPrStatus.mockResolvedValueOnce(prStatus(56)).mockResolvedValueOnce(prStatus(57));

      render(<Probe conversationId="conv-clock-correction" />);
      await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('56'));

      document.dispatchEvent(new Event('visibilitychange'));
      await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('57'));
      expect(getPrStatus).toHaveBeenCalledTimes(2);
    } finally {
      dateNow.mockRestore();
    }
  });

  it('expires routine freshness when wall time advances across system sleep', async () => {
    const dateNow = vi.spyOn(Date, 'now')
      .mockReturnValueOnce(1_000)
      .mockReturnValueOnce(1_000)
      .mockReturnValue(20_000);
    try {
      const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
      getPrStatus.mockResolvedValueOnce(prStatus(63)).mockResolvedValueOnce(prStatus(64));

      render(<Probe conversationId="conv-system-sleep" />);
      await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('63'));

      document.dispatchEvent(new Event('visibilitychange'));
      await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('64'));
      expect(getPrStatus).toHaveBeenCalledTimes(2);
    } finally {
      dateNow.mockRestore();
    }
  });

  it('reads immediately when a recently refreshed scope is reactivated', async () => {
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockResolvedValueOnce(prStatus(50)).mockResolvedValueOnce(prStatus(51));

    const { rerender } = render(<Probe conversationId="conv-reactivate" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('50'));

    rerender(<Probe conversationId={null} />);
    expect(screen.getByTestId('pr-number')).toHaveTextContent('none');
    rerender(<Probe conversationId="conv-reactivate" />);

    await waitFor(() => expect(getPrStatus).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('51'));
  });

  it('clears prior freshness when a later HTTP response is unavailable', async () => {
    const unavailable: PrStatusResponse = {
      found: false,
      refresh: {
        state: 'unavailable',
        reason: 'command_failed',
        last_attempted_at: '2026-01-01T00:00:00Z',
        stale: true,
      },
      work_change: { kind: 'unavailable', reason: 'command_failed' },
    };
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus
      .mockResolvedValueOnce(prStatus(52))
      .mockResolvedValueOnce(unavailable)
      .mockResolvedValueOnce(prStatus(53));

    render(<Probe conversationId="conv-unavailable" />);
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('52'));
    screen.getByRole('button', { name: 'Refresh' }).click();
    await waitFor(() => expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable'));

    document.dispatchEvent(new Event('visibilitychange'));
    await waitFor(() => expect(screen.getByTestId('pr-number')).toHaveTextContent('53'));
    expect(getPrStatus).toHaveBeenCalledTimes(3);
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

    await act(async () => {
      first.resolve(prStatus(1));
      await first.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('2');
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

    await act(async () => {
      first.resolve(prStatus(1));
      await first.promise;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('none');

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
    const failure = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValue(failure.promise);

    render(<Probe conversationId="conv-9" cached={cachedPr(9)} />);
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledWith('conv-9'));

    await act(async () => {
      const rejected = expect(failure.promise).rejects.toThrow('network failed');
      failure.reject(new Error('network failed'));
      await rejected;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('9');
    expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 9');
    expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable');
  });

  it('keeps cached PR display-only after the fresh status request fails', async () => {
    const failure = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValue(failure.promise);

    render(<Probe conversationId="conv-9" cached={cachedPr(9)} />);
    await waitFor(() => expect(getPrStatus).toHaveBeenCalledWith('conv-9'));

    await act(async () => {
      const rejected = expect(failure.promise).rejects.toThrow('network failed');
      failure.reject(new Error('network failed'));
      await rejected;
    });
    expect(screen.getByTestId('pr-number')).toHaveTextContent('9');
    expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 9');
    expect(screen.getByTestId('refresh-state')).toHaveTextContent('unavailable');
    expect(screen.getByTestId('associated-count')).toHaveTextContent('0');
    expect(screen.getByTestId('active-pr-number')).toHaveTextContent('none');
    expect(screen.getByTestId('ambiguous')).toHaveTextContent('no');
  });

});
