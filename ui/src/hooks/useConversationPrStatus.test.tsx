import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { useConversationPrStatus } from './useConversationPrStatus';
import { api, type PrStatusResponse } from '../api';

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

function Probe({ conversationId }: { conversationId: string }) {
  const handle = useConversationPrStatus({
    conversationId,
    convModeLabel: 'Work',
    branchName: `branch-${conversationId}`,
  });
  const number = handle.state.status === 'ready' ? handle.state.prStatus.number : 'none';
  return <div data-testid="pr-number">{number}</div>;
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
});
