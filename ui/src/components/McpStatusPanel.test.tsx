import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api, type McpReloadResult } from '../api';
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

describe('McpStatusPanel', () => {
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
