import '@testing-library/jest-dom/vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import type { WakeStatusSnapshot } from '../generated/WakeStatusSnapshot';
import { WakeIndicator } from './WakeIndicator';

const snapshot: WakeStatusSnapshot = {
  conversation_id: 'conv/1',
  pending_count: 1,
  soonest_expiry: '2026-07-10T12:30:00Z',
  lifecycle_blocked: true,
  contracts: [
    {
      id: 'wake/1',
      handle: { kind: 'bash', id: 'b-3' },
      registered_at: '2026-07-10T12:00:00Z',
      expires_at: '2026-07-10T12:30:00Z',
      status: 'pending', cause: null, forgotten_reason: null,
    },
    {
      id: 'wake-2',
      handle: { kind: 'tmux_window', id: 'phoenix:2' },
      registered_at: '2026-07-10T11:00:00Z',
      expires_at: '2026-07-10T12:00:00Z',
      status: 'fired', cause: 'fired', forgotten_reason: null,
    },
  ],
};

afterEach(() => vi.restoreAllMocks());

describe('WakeIndicator', () => {
  it('shows pending count and compact soonest expiry, then lists full contract details', () => {
    vi.setSystemTime(new Date('2026-07-10T12:00:00Z'));
    render(<WakeIndicator conversationId="conv/1" snapshot={snapshot} onError={vi.fn()} />);
    expect(screen.getByRole('button', { name: /1 wake/ })).toHaveTextContent('≤ 30m');
    fireEvent.click(screen.getByRole('button', { name: /1 wake/ }));
    expect(screen.getByRole('dialog', { name: 'Wake contracts' })).toBeVisible();
    expect(screen.getByText('bash · b-3')).toBeVisible();
    expect(screen.getByText('tmux window · phoenix:2')).toBeVisible();
    expect(screen.getByText('cause fired')).toBeVisible();
  });

  it('keeps terminal wake history visible without implying success or failure', () => {
    const terminalSnapshot: WakeStatusSnapshot = {
      ...snapshot,
      pending_count: 0,
      soonest_expiry: null,
      lifecycle_blocked: false,
      contracts: [
        { ...snapshot.contracts[0]!, status: 'cancelled', cause: 'cancelled' },
        snapshot.contracts[1]!,
        { ...snapshot.contracts[0]!, id: 'wake-3', status: 'expired', cause: 'expired' },
        {
          ...snapshot.contracts[0]!,
          id: 'wake-4',
          status: 'forgotten',
          cause: null,
          forgotten_reason: 'handle_missing',
        },
      ],
    };

    render(<WakeIndicator conversationId="conv/1" snapshot={terminalSnapshot} onError={vi.fn()} />);

    const trigger = screen.getByRole('button', { name: 'Wake history' });
    expect(trigger).not.toHaveClass('wake-indicator__trigger--pending');
    fireEvent.click(trigger);
    expect(screen.getByText('cancelled')).toBeVisible();
    expect(screen.getByText('fired')).toBeVisible();
    expect(screen.getByText('expired')).toBeVisible();
    expect(screen.getByText('forgotten')).toBeVisible();
    expect(screen.getByText('cause handle missing')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
  });

  it('renders nothing when the snapshot has no contracts', () => {
    const { container } = render(
      <WakeIndicator
        conversationId="conv/1"
        snapshot={{ ...snapshot, pending_count: 0, soonest_expiry: null, contracts: [] }}
        onError={vi.fn()}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('posts cancel, disables optimistically, and waits for the SSE snapshot', async () => {
    let resolve!: (value: { outcome: 'cancelled'; contract_id: string }) => void;
    const pending = new Promise<{ outcome: 'cancelled'; contract_id: string }>((done) => { resolve = done; });
    const cancelSpy = vi.spyOn(api, 'cancelWake').mockReturnValue(pending);
    const { rerender } = render(
      <WakeIndicator conversationId="conv/1" snapshot={snapshot} onError={vi.fn()} />,
    );
    fireEvent.click(screen.getByRole('button', { name: /1 wake/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(cancelSpy).toHaveBeenCalledWith('conv/1', 'wake/1');
    expect(screen.getByRole('button', { name: 'Cancelling…' })).toBeDisabled();
    resolve({ outcome: 'cancelled', contract_id: 'wake/1' });
    await pending;
    expect(screen.getByRole('button', { name: 'Cancelling…' })).toBeDisabled();
    rerender(
      <WakeIndicator
        conversationId="conv/1"
        snapshot={{
          ...snapshot, pending_count: 0, soonest_expiry: null,
          contracts: snapshot.contracts.map((contract) =>
            contract.id === 'wake/1' ? { ...contract, status: 'cancelled', cause: 'cancelled' } : contract),
        }}
        onError={vi.fn()}
      />,
    );
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Cancelling…' })).not.toBeInTheDocument());
  });
});
