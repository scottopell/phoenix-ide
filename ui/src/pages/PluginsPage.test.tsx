import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
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
  it('renders installed plugins with status, provenance, and counts', async () => {
    apiMock.getPlugins.mockResolvedValue(
      listResponse([
        {
          name: 'devtools',
          version: '1.2.0',
          description: 'Developer tools',
          enabled: true,
          provenance: 'managed',
          skills: ['deploy'],
          mcp_servers: ['validator'],
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
    // Missing roots offer only the purge action.
    expect(screen.getByText('Purge data')).toBeInTheDocument();
    expect(screen.getByText('Disable')).toBeInTheDocument();
    expect(screen.getByText('Enable')).toBeInTheDocument();
  });

  it('shows the empty state when no plugins are installed', async () => {
    apiMock.getPlugins.mockResolvedValue(listResponse([]));

    render(<PluginsPage />);

    await waitFor(() =>
      expect(screen.getByText('No plugins installed.')).toBeInTheDocument(),
    );
  });
});
