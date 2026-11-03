import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PluginStatus, PluginsListResponse } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    getPlugins: vi.fn(),
    addPlugin: vi.fn(),
    setPluginEnabled: vi.fn(),
    removePlugin: vi.fn(),
    purgePluginData: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: apiMock,
  };
});

import { PluginsPage } from './PluginsPage';

function listResponse(plugins: PluginStatus[]): PluginsListResponse {
  return { plugins, root: '/home/u/.agents/plugins' };
}

describe('PluginsPage', () => {
  beforeEach(() => vi.resetAllMocks());

  it('enables and disables a retained missing identity while keeping purge available', async () => {
    const missing: PluginStatus = { name: 'retained', enabled: false, provenance: 'managed', missing: true };
    apiMock.getPlugins.mockResolvedValueOnce(listResponse([missing]))
      .mockResolvedValueOnce(listResponse([{ ...missing, enabled: true }]))
      .mockResolvedValueOnce(listResponse([missing]));
    apiMock.setPluginEnabled.mockResolvedValue({ ok: true });
    render(<PluginsPage />);
    fireEvent.click(await screen.findByRole('button', { name: 'Enable' }));
    await waitFor(() => expect(apiMock.setPluginEnabled).toHaveBeenCalledWith('retained', true));
    fireEvent.click(await screen.findByRole('button', { name: 'Disable' }));
    await waitFor(() => expect(apiMock.setPluginEnabled).toHaveBeenCalledWith('retained', false));
    await screen.findByRole('button', { name: 'Enable' });
    expect(screen.getByRole('button', { name: 'Purge data' })).toBeInTheDocument();
  });

  it('renders installed plugins with status, provenance, and counts', async () => {
    apiMock.getPlugins.mockResolvedValue(
      listResponse([
        {
          name: 'devtools',
          version: '1.2.0',
          description: 'Developer tools',
          enabled: true,
          provenance: 'managed',
          skills: [{ name: 'deploy', status: 'loaded' }],
          mcp_servers: [{ name: 'validator', status: 'loaded' }],
        },
        {
          name: 'muted',
          enabled: false,
          provenance: 'unmanaged',
        },
        {
          name: 'vanished',
          enabled: false,
          provenance: 'managed',
          missing: true,
        },
      ]),
    );

    render(<PluginsPage />);

    await waitFor(() => expect(screen.getByText('devtools')).toBeInTheDocument());
    expect(screen.getByText('1 skills · 1 servers')).toBeInTheDocument();
    // 'managed' matches both the managed badge and (as a substring query is
    // exact-match, but two plugins carry badges) — query by class instead.
    expect(document.querySelectorAll('.plugins-page__provenance')).toHaveLength(3);
    expect(document.querySelector('.plugins-page__provenance--managed')).not.toBeNull();
    expect(document.querySelector('.plugins-page__provenance--unmanaged')).not.toBeNull();
    // Missing identities retain activation controls alongside purge.
    expect(screen.getByText('Purge data')).toBeInTheDocument();
    expect(screen.getByText('Disable')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Enable' })).toHaveLength(2);
  });

  it('counts only admitted components and shows skip and disable reasons', async () => {
    apiMock.getPlugins.mockResolvedValue(listResponse([{
      name: 'broken', enabled: true, provenance: 'unmanaged',
      skills: [
        { name: 'ok', status: 'loaded' },
        { name: 'bad', status: 'skipped', reason: 'invalid SKILL.md frontmatter' },
      ],
      mcp_servers: [
        { name: 'escape', status: 'skipped', reason: 'cwd escapes plugin root' },
        { name: 'muted', status: 'disabled', reason: 'plugin disabled' },
      ],
      skills_disabled_reason: 'skills location unreadable',
    }]));
    render(<PluginsPage />);
    expect(await screen.findByText('1 skills · 0 servers')).toBeInTheDocument();
    expect(screen.getByText('Skill bad: skipped — invalid SKILL.md frontmatter')).toBeInTheDocument();
    expect(screen.getByText('MCP escape: skipped — cwd escapes plugin root')).toBeInTheDocument();
    expect(screen.getByText('MCP muted: disabled — plugin disabled')).toBeInTheDocument();
    expect(screen.getByText('Skills disabled: skills location unreadable')).toBeInTheDocument();
  });

  it('shows a present but unloadable installation as an error', async () => {
    apiMock.getPlugins.mockResolvedValue(listResponse([{
      name: 'malformed', enabled: true, provenance: 'managed', missing: false,
      load_error: 'invalid plugin manifest',
    }]));
    render(<PluginsPage />);
    expect(await screen.findByText('Load failed: invalid plugin manifest')).toBeInTheDocument();
    expect(document.querySelector('.plugins-page__status')).toHaveTextContent('✗');
    expect(screen.queryByText('Purge data')).not.toBeInTheDocument();
    expect(screen.getByText('Disable')).toBeInTheDocument();
  });

  it('shows the empty state when no plugins are installed', async () => {
    apiMock.getPlugins.mockResolvedValue(listResponse([]));

    render(<PluginsPage />);

    await waitFor(() =>
      expect(screen.getByText('No plugins installed.')).toBeInTheDocument(),
    );
  });
});
