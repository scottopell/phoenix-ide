import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import { WakeStatusBar } from './WakeStatusBar';

vi.mock('../api', () => ({
  api: {
    getWakeStatus: vi.fn(),
    cancelWake: vi.fn(),
  },
}));

const now = new Date('2026-01-01T00:00:00Z').getTime();
const active = {
  pending_count: 1,
  soonest_expires_at: Math.floor(now / 1000) + 120,
  contracts: [{
    workflow_id: 1,
    contract_id: 'wake-1',
    expires_at: Math.floor(now / 1000) + 120,
  }],
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.setSystemTime(now);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('WakeStatusBar', () => {
  it('preserves last known data and disables cancellation when a refresh fails', async () => {
    vi.useFakeTimers();
    vi.mocked(api.getWakeStatus)
      .mockResolvedValueOnce(active)
      .mockRejectedValueOnce(new Error('offline'));

    render(<WakeStatusBar conversationId="conv" />);
    await act(async () => {});
    expect(screen.getByText('⏰ 1 pending wake')).toBeInTheDocument();
    expect(screen.getByText('next expires in 2 minutes')).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });

    expect(screen.getByText('status unavailable • showing last known')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Cancel wake wake-1' })).toBeDisabled();
  });

  it('does not carry stale status across conversations', async () => {
    vi.mocked(api.getWakeStatus)
      .mockResolvedValueOnce(active)
      .mockRejectedValueOnce(new Error('offline'));

    const view = render(<WakeStatusBar conversationId="conv-a" />);
    expect(await screen.findByText('⏰ 1 pending wake')).toBeInTheDocument();

    view.rerender(<WakeStatusBar conversationId="conv-b" />);

    expect(await screen.findByText('status unavailable')).toBeInTheDocument();
    expect(screen.queryByText('⏰ 1 pending wake')).not.toBeInTheDocument();
    expect(screen.queryByText(/showing last known/)).not.toBeInTheDocument();
  });

  it('announces successful cancellation after refresh removes the contract', async () => {
    vi.mocked(api.getWakeStatus)
      .mockResolvedValueOnce(active)
      .mockResolvedValueOnce({
        pending_count: 0,
        soonest_expires_at: null,
        contracts: [],
      });
    vi.mocked(api.cancelWake).mockResolvedValueOnce({ success: true });

    render(<WakeStatusBar conversationId="conv" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel wake wake-1' }));

    await waitFor(() =>
      expect(screen.getByText('Wake wake-1 cancelled.')).toBeInTheDocument()
    );
  });

  it('keeps the contract and announces a cancellation failure', async () => {
    vi.mocked(api.getWakeStatus).mockResolvedValue(active);
    vi.mocked(api.cancelWake).mockRejectedValueOnce(new Error('conflict'));

    render(<WakeStatusBar conversationId="conv" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel wake wake-1' }));

    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent(
        'Could not cancel wake wake-1. Wake status refreshed.'
      )
    );
    expect(screen.getByRole('button', { name: 'Cancel wake wake-1' })).toBeEnabled();
  });
});
