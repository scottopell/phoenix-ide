import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api, type CodexRedeemResult, type CodexResetCreditsResult } from '../api';
import { CodexResetAction } from './CodexResetAction';
import { ErrorBanner } from './ErrorBanner';
import { getErrorPresentation } from '../errorPresentation';
import type { CodexResetCredits } from '../generated/CodexResetCredits';

vi.mock('../codexResets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../codexResets')>()),
  refreshCodexQuota: vi.fn(async () => undefined),
}));

const twoResets: CodexResetCredits = {
  account_id: 'acct',
  available_count: 2,
  credits: [
    { id: 'a', reset_type: 'codex_rate_limits', status: 'available', granted_at: 1, expires_at: null, title: null, description: null },
    { id: 'b', reset_type: 'codex_rate_limits', status: 'available', granted_at: 1, expires_at: 1_900_000_000, title: null, description: null },
  ],
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('CodexResetAction on the error surface (REQ-CRR-005)', () => {
  it('shows the count and a use action when resets are available', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'ok', credits: twoResets });
    render(<CodexResetAction surface="error" />);
    expect(await screen.findByText('2 resets available')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /use reset/i })).toBeInTheDocument();
  });

  it.each<[string, CodexResetCreditsResult]>([
    ['zero', { type: 'ok', credits: { account_id: 'acct', available_count: 0, credits: [] } }],
    ['signed out', { type: 'signed_out' }],
    ['unavailable', { type: 'unavailable' }],
  ])('shows nothing when %s', async (_label, result) => {
    const listing = vi.spyOn(api, 'codexResetCredits').mockResolvedValue(result);
    const { container } = render(<CodexResetAction surface="error" />);
    await waitFor(() => expect(listing).toHaveBeenCalled());
    await act(async () => { await listing.mock.results[0]?.value; });
    expect(container).toBeEmptyDOMElement();
  });
});

describe('CodexResetAction on the settings surface (REQ-CRR-009)', () => {
  it('distinguishes failure from zero', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'unavailable' });
    render(<CodexResetAction surface="settings" />);
    expect(await screen.findByText('Resets unavailable')).toBeInTheDocument();
  });

  it('shows zero without an action', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'ok', credits: { account_id: 'acct', available_count: 0, credits: [] } });
    render(<CodexResetAction surface="settings" />);
    expect(await screen.findByText('No resets available')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /use reset/i })).not.toBeInTheDocument();
  });
});

describe('ErrorBanner reset integration (REQ-CRR-008)', () => {
  it('resumes through Retry after a successful redemption', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'ok', credits: twoResets });
    vi.spyOn(api, 'codexRedeemReset').mockResolvedValue({ type: 'ok', response: { outcome: 'reset', windows_reset: 2 } });
    const onRetry = vi.fn();
    render(
      <ErrorBanner
        message="You've hit your usage limit."
        error={getErrorPresentation('usage_limit_reached')}
        onRetry={onRetry}
        onDismiss={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByRole('button', { name: /use reset/i }));
    fireEvent.click(await screen.findByRole('button', { name: /full reset.*expires/i }));
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    await waitFor(() => expect(onRetry).toHaveBeenCalledOnce());
  });

  it('does not resume when the redemption spends nothing', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'ok', credits: twoResets });
    vi.spyOn(api, 'codexRedeemReset').mockResolvedValue({ type: 'ok', response: { outcome: 'nothing_to_reset', windows_reset: 0 } });
    const onRetry = vi.fn();
    render(
      <ErrorBanner
        message="You've hit your usage limit."
        error={getErrorPresentation('usage_limit_reached')}
        onRetry={onRetry}
        onDismiss={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByRole('button', { name: /use reset/i }));
    fireEvent.click(await screen.findByRole('button', { name: /full reset.*expires/i }));
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    expect(await screen.findByText(/does not need a reset/i)).toBeInTheDocument();
    expect(onRetry).not.toHaveBeenCalled();
  });

  it('does not list resets for other errors or read-only banners', () => {
    const listing = vi.spyOn(api, 'codexResetCredits');
    render(
      <ErrorBanner message="boom" error={getErrorPresentation('server_error')} onRetry={vi.fn()} onDismiss={vi.fn()} />,
    );
    render(
      <ErrorBanner message="limit" error={getErrorPresentation('usage_limit_reached')} />,
    );
    expect(listing).not.toHaveBeenCalled();
  });

  it('does not resume a different conversation whose banner replaced the original (keyed remount)', async () => {
    vi.spyOn(api, 'codexResetCredits').mockResolvedValue({ type: 'ok', credits: twoResets });
    let resolve: (value: CodexRedeemResult) => void = () => undefined;
    vi.spyOn(api, 'codexRedeemReset').mockImplementation(() => new Promise((r) => { resolve = r; }));
    const retryA = vi.fn();
    const retryB = vi.fn();
    const banner = (key: string, onRetry: () => void) => (
      <ErrorBanner
        key={key}
        message="You've hit your usage limit."
        error={getErrorPresentation('usage_limit_reached')}
        onRetry={onRetry}
        onDismiss={vi.fn()}
      />
    );
    const { rerender } = render(banner('conv-a', retryA));
    fireEvent.click(await screen.findByRole('button', { name: /use reset/i }));
    fireEvent.click(await screen.findByRole('button', { name: /full reset.*expires/i }));
    fireEvent.click(screen.getByRole('button', { name: /yes, use reset/i }));
    rerender(banner('conv-b', retryB));
    await act(async () => { resolve({ type: 'ok', response: { outcome: 'reset', windows_reset: 2 } }); });
    expect(retryA).not.toHaveBeenCalled();
    expect(retryB).not.toHaveBeenCalled();
  });
});
