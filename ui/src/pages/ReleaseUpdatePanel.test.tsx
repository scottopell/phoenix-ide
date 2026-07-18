import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ReleaseUpdatePanel } from './ReleaseUpdatePanel';

const snapshot = {
  backend: 'launchd' as const,
  current_version: '1.0.0',
  current_git_sha: '1'.repeat(40),
  preview: {
    kind: 'available' as const,
    tag: 'v1.1.0',
    version: '1.1.0',
    commit: '2'.repeat(40),
    asset_name: 'phoenix_ide-aarch64-apple-darwin',
    asset_sha256: '3'.repeat(64),
    release_url: 'https://example.test/release',
    notes: 'Safer updates',
    newer_than_current: true,
  },
  authority: { kind: 'allowed' as const },
  transaction: { kind: 'none' as const },
  sampled_at: '2026-07-18T00:00:00Z',
};

function json(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } }));
}

describe('ReleaseUpdatePanel', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.stubGlobal('fetch', vi.fn(() => json(snapshot)));
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('shows immutable release identity and requires explicit confirmation', async () => {
    render(<ReleaseUpdatePanel />);
    expect(await screen.findByText('v1.1.0')).toBeInTheDocument();
    expect(screen.getByText(snapshot.preview.commit)).toBeInTheDocument();
    expect(screen.getByText(snapshot.preview.asset_sha256)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Approve and install' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Review and install v1.1.0' }));
    expect(screen.getByRole('button', { name: 'Approve and install' })).toBeInTheDocument();
  });

  it('posts the approved tag and full commit', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock
      .mockImplementationOnce(() => json(snapshot))
      .mockImplementationOnce(() => json({ transaction_id: 'tx-1' }, 202))
      .mockImplementation(() => json(snapshot));
    render(<ReleaseUpdatePanel />);
    await screen.findByText('v1.1.0');
    fireEvent.click(screen.getByRole('button', { name: 'Review and install v1.1.0' }));
    fireEvent.click(screen.getByRole('button', { name: 'Approve and install' }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/release-updates/approve', expect.objectContaining({
      method: 'POST',
      body: JSON.stringify({ tag: 'v1.1.0', commit: snapshot.preview.commit, asset_name: snapshot.preview.asset_name, asset_sha256: snapshot.preview.asset_sha256 }),
    })));
  });

  it('explains remote approval denial while preserving release review', async () => {
    vi.mocked(fetch).mockImplementation(() => json({
      ...snapshot,
      authority: { kind: 'remote_browser' },
    }));
    render(<ReleaseUpdatePanel />);
    expect(await screen.findByText(/approval is available only from a browser on the Phoenix host/i)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /install v1.1.0/i })).not.toBeInTheDocument();
    expect(screen.getByText(snapshot.preview.commit)).toBeInTheDocument();
  });

  it('surfaces unreadable durable status instead of treating it as absent', async () => {
    vi.mocked(fetch).mockImplementation(() => json({
      ...snapshot,
      transaction: { kind: 'unreadable', reason: 'status permissions denied' },
    }));
    render(<ReleaseUpdatePanel />);
    expect(await screen.findByText(/status permissions denied/i)).toBeInTheDocument();
    expect(screen.queryByText(/no deployment transaction/i)).not.toBeInTheDocument();
  });

  it('distinguishes verified rollback from rollback failure', async () => {
    const rolledBack = {
      ...snapshot,
      transaction: {
        kind: 'present' as const,
        transaction_id: 'tx-rollback', state: 'activation_failed_rolled_back', source_commit: null,
        release_tag: 'v1.1.0', expected_version: '1.1.0', expected_git_sha: snapshot.preview.commit,
        created_at: null, updated_at: null, failure: 'health timeout', rollback_failure: null, stale: false,
      },
    };
    vi.mocked(fetch).mockImplementation(() => json(rolledBack));
    const view = render(<ReleaseUpdatePanel />);
    expect(await screen.findByText(/previous release restored and verified/i)).toBeInTheDocument();

    vi.mocked(fetch).mockImplementation(() => json({
      ...rolledBack,
      transaction: { ...rolledBack.transaction, state: 'activation_failed_rollback_failed', rollback_failure: 'old runtime unhealthy' },
    }));
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(await screen.findByText(/offline recovery required/i)).toBeInTheDocument();
    expect(screen.getByText(/claim remains retained/i)).toBeInTheDocument();
    view.unmount();
  });
});
