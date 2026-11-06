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
  it('shows background recovery and keeps expanded diagnostics current after readiness', async () => {
    vi.useFakeTimers();
    getMcpStatus.mockResolvedValue([{ ...healthy, state: 'recovering', tool_count: 0, tools: [], last_error: 'HTTP 503', next_retry_at: 2000000000 }]);
    render(<McpStatusPanel showToast={vi.fn()} showError={vi.fn()} />);
    await act(async () => {});
    fireEvent.click(screen.getByRole('button', { name: /^MCP / }));
    await act(async () => {});
    fireEvent.click(screen.getByRole('button', { name: /healthy/ }));
    expect(screen.getByText(/HTTP 503/)).toBeTruthy();
    expect(screen.getByText(/Next retry/)).toBeTruthy();
    const beforeRecovery = getMcpStatus.mock.calls.length;
    getMcpStatus.mockResolvedValue([healthy]);
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    expect(getMcpStatus.mock.calls.length).toBeGreaterThan(beforeRecovery);
    expect(screen.queryByText(/HTTP 503/)).toBeNull();
    getMcpStatus.mockResolvedValue([{ ...healthy, state: 'unauthorized', pending_oauth_url: 'https://example.com/authorize' }]);
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    expect(screen.getByRole('link', { name: /Sign in/ })).toHaveAttribute('href', 'https://example.com/authorize');
    getMcpStatus.mockResolvedValue([healthy]);
    await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
    fireEvent.click(screen.getByRole('button', { name: /^MCP / }));
    const settled = getMcpStatus.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(9000); });
    expect(getMcpStatus.mock.calls.length).toBeLessThanOrEqual(settled + 1);
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
