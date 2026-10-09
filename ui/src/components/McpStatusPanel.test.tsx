import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api, type McpReloadResult, type McpServerStatus } from '../api';
import { McpStatusPanel } from './McpStatusPanel';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      getMcpStatus: vi.fn(),
      reloadMcp: vi.fn(),
    },
  };
});

const getMcpStatus = vi.mocked(api.getMcpStatus);
const reloadMcp = vi.mocked(api.reloadMcp);
const emptyReload: McpReloadResult = {
  added: [],
  removed: [],
  pending_removals: [],
  restarted: [],
  unchanged: [],
  failed: [],
};

beforeEach(() => {
  getMcpStatus.mockReset().mockResolvedValue([]);
  reloadMcp.mockReset().mockResolvedValue(emptyReload);
});

afterEach(() => vi.useRealTimers());

const healthy: McpServerStatus = {
  name: 'healthy', state: 'ready', transport: 'http', auth: 'none',
  tool_count: 1, tools: ['report'], enabled: true,
};

describe('McpStatusPanel', () => {
  it('keeps polling a deferred removal until its later authorization and cleanup finish', async () => {
    vi.useFakeTimers();
    for (const [anotherReady, deferred] of [[true, true], [false, true], [true, false]]) {
    const survivors = anotherReady ? [healthy] : [];
    const showToast = vi.fn();
    getMcpStatus.mockResolvedValue([...survivors, { ...healthy, name: 'remote' }]);
    const { unmount } = render(<McpStatusPanel showToast={showToast} showError={vi.fn()} />);
    await act(async () => {});
    fireEvent.click(screen.getByRole('button', { name: /^MCP / }));
    reloadMcp.mockResolvedValue(deferred
      ? { ...emptyReload, pending_removals: ['remote'], unchanged: survivors.map(s => s.name) }
      : { ...emptyReload, failed: [{ server: 'remote', action: 'remove', error: 'transient refresh' }] });
    getMcpStatus.mockResolvedValue([...survivors, { ...healthy, name: 'remote', state: 'removing' }]);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Reload MCP servers' })); });
    if (deferred) expect(showToast).toHaveBeenCalledWith(expect.stringContaining('removal pending'), 3000);
    const afterReload = getMcpStatus.mock.calls.length;
    getMcpStatus.mockResolvedValue([...survivors, { ...healthy, name: 'remote', state: 'unauthorized', auth: 'oauth', pending_oauth_url: 'https://example.com/authorize' }]);
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    expect(getMcpStatus.mock.calls.length).toBe(afterReload + 1);
    expect(screen.getByRole('link', { name: /Sign in/ })).toHaveAttribute('href', 'https://example.com/authorize');
    getMcpStatus.mockResolvedValue(survivors);
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    const afterCleanup = getMcpStatus.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(6000); });
    expect(getMcpStatus.mock.calls.length).toBe(afterCleanup);
    unmount();
    }
  });

  it('reloads config from the writable empty state', async () => {
    render(<McpStatusPanel showToast={vi.fn()} showError={vi.fn()} />);

    await waitFor(() => expect(getMcpStatus).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole('button', { name: 'Reload MCP servers' }));

    await waitFor(() => expect(reloadMcp).toHaveBeenCalledOnce());
  });

  it('does not expose reload in read-only mode', async () => {
    render(<McpStatusPanel showToast={vi.fn()} showError={vi.fn()} readOnly />);

    await waitFor(() => expect(getMcpStatus).toHaveBeenCalledOnce());

    expect(screen.queryByRole('button', { name: 'Reload MCP servers' })).not.toBeInTheDocument();
  });
});
