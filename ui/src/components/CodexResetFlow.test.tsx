import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api } from '../api';
import { CodexResetFlow } from './CodexResetFlow';
import type { CodexResetCredits } from '../generated/CodexResetCredits';

vi.mock('../codexResets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../codexResets')>()),
  refreshCodexQuota: vi.fn(async () => undefined),
}));

const listing: CodexResetCredits = {
  available_count: 1,
  credits: [{
    id: 'credit-1',
    reset_type: 'codex_rate_limits',
    status: 'available',
    granted_at: 1_700_000_000,
    expires_at: null,
    title: 'Full reset (Weekly + 5 hr)',
    description: 'Ready to redeem',
  }],
};

describe('CodexResetFlow', () => {
  beforeEach(() => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'ok', credits: listing });
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  async function chooseAndConfirmView() {
    fireEvent.click(await screen.findByRole('button', { name: /full reset \(weekly/i }));
    expect(screen.getByText('Use this reset?')).toBeInTheDocument();
    expect(screen.getByText('Ready to redeem')).toBeInTheDocument();
  }

  it('never redeems without confirmation (REQ-CRR-004)', async () => {
    const redeem = vi.spyOn(api, 'codexRedeemReset');
    const onClose = vi.fn();
    render(<CodexResetFlow onClose={onClose} />);
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /no, go back/i }));
    fireEvent.click(screen.getByRole('button', { name: /cancel/i }));
    expect(onClose).toHaveBeenCalledOnce();
    expect(redeem).not.toHaveBeenCalled();
  });

  it('redeems the chosen credit and reports success once', async () => {
    const redeem = vi.spyOn(api, 'codexRedeemReset').mockResolvedValue({ outcome: 'reset', windows_reset: 2 });
    const onSucceeded = vi.fn();
    render(<CodexResetFlow onClose={vi.fn()} onSucceeded={onSucceeded} />);
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    expect(await screen.findByText(/usage reset/i)).toBeInTheDocument();
    expect(redeem).toHaveBeenCalledOnce();
    expect(redeem.mock.calls[0]?.[0].credit_id).toBe('credit-1');
    expect(redeem.mock.calls[0]?.[0].idempotency_key).toMatch(/^[0-9a-f-]{36}$/);
    expect(onSucceeded).toHaveBeenCalledOnce();
  });

  it('treats already_redeemed as success', async () => {
    vi.spyOn(api, 'codexRedeemReset').mockResolvedValue({ outcome: 'already_redeemed', windows_reset: 0 });
    const onSucceeded = vi.fn();
    render(<CodexResetFlow onClose={vi.fn()} onSucceeded={onSucceeded} />);
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    await waitFor(() => expect(onSucceeded).toHaveBeenCalledOnce());
  });

  it('reuses the idempotency key when retrying a failed redemption (REQ-CRR-007)', async () => {
    const redeem = vi.spyOn(api, 'codexRedeemReset')
      .mockRejectedValueOnce(new Error('network'))
      .mockResolvedValueOnce({ outcome: 'already_redeemed', windows_reset: 0 });
    render(<CodexResetFlow onClose={vi.fn()} />);
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    fireEvent.click(await screen.findByRole('button', { name: /try again/i }));
    await screen.findByText(/usage reset/i);
    expect(redeem).toHaveBeenCalledTimes(2);
    expect(redeem.mock.calls[1]?.[0]).toEqual(redeem.mock.calls[0]?.[0]);
  });

  it('mints a new key for a new confirmation', async () => {
    const redeem = vi.spyOn(api, 'codexRedeemReset')
      .mockResolvedValueOnce({ outcome: 'no_credit', windows_reset: 0 })
      .mockResolvedValueOnce({ outcome: 'reset', windows_reset: 1 });
    render(<CodexResetFlow onClose={vi.fn()} />);
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    expect(await screen.findByText(/no longer available/i)).toBeInTheDocument();
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    await screen.findByText(/usage reset/i);
    expect(redeem.mock.calls[1]?.[0].idempotency_key).not.toBe(redeem.mock.calls[0]?.[0].idempotency_key);
  });

  it('does not report success for nothing_to_reset', async () => {
    vi.spyOn(api, 'codexRedeemReset').mockResolvedValue({ outcome: 'nothing_to_reset', windows_reset: 0 });
    const onSucceeded = vi.fn();
    render(<CodexResetFlow onClose={vi.fn()} onSucceeded={onSucceeded} />);
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    expect(await screen.findByText(/does not need a reset/i)).toBeInTheDocument();
    expect(onSucceeded).not.toHaveBeenCalled();
  });

  it('reports no resets when a backend-chosen redemption answers no_credit', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({
      type: 'ok',
      credits: { available_count: 1, credits: [] },
    });
    const redeem = vi.spyOn(api, 'codexRedeemReset').mockResolvedValue({ outcome: 'no_credit', windows_reset: 0 });
    render(<CodexResetFlow onClose={vi.fn()} />);
    fireEvent.click(await screen.findByRole('button', { name: /full reset/i }));
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    expect(await screen.findByText(/no usage limit resets are available/i)).toBeInTheDocument();
    expect(redeem.mock.calls[0]?.[0].credit_id).toBeNull();
  });

  it('does not call onSucceeded after unmount (REQ-CRR-008 gate)', async () => {
    let resolve: (value: { outcome: 'reset'; windows_reset: number }) => void = () => undefined;
    vi.spyOn(api, 'codexRedeemReset').mockImplementation(() => new Promise((r) => { resolve = r; }));
    const onSucceeded = vi.fn();
    const { unmount } = render(<CodexResetFlow onClose={vi.fn()} onSucceeded={onSucceeded} />);
    await chooseAndConfirmView();
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    expect(screen.getByRole('button', { name: /resetting/i })).toBeDisabled();
    unmount();
    resolve({ outcome: 'reset', windows_reset: 1 });
    await Promise.resolve();
    expect(onSucceeded).not.toHaveBeenCalled();
  });

  it('shows a distinct unavailable state when the listing fails', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'unavailable' });
    render(<CodexResetFlow onClose={vi.fn()} />);
    expect(await screen.findByText(/couldn.t load your resets/i)).toBeInTheDocument();
  });
});
