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
  return <div><span data-testid="pr-number">{number}</span><span data-testid="pr-title">{title}</span></div>;
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
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(screen.getByTestId('pr-number')).toHaveTextContent('2');
  });

  it('seeds ready state from cached PR while the fresh status loads', async () => {
    const fresh = deferred<PrStatusResponse>();
    const getPrStatus = api.getPrStatus as ReturnType<typeof vi.fn>;
    getPrStatus.mockReturnValue(fresh.promise);

    render(<Probe conversationId="conv-7" cached={cachedPr(7)} />);

    expect(screen.getByTestId('pr-number')).toHaveTextContent('7');
    expect(screen.getByTestId('pr-title')).toHaveTextContent('Cached PR 7');
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
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByTestId('pr-number')).toHaveTextContent('none');

    second.resolve(prStatus(2));
    await waitFor(() => {
      expect(screen.getByTestId('pr-number')).toHaveTextContent('2');
    });
  });

});
