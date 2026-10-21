import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    getVersion: vi.fn(),
    getLlmLanguageSetting: vi.fn(),
  },
}));

vi.mock('../api', () => ({ api: apiMock }));
vi.mock('../notifications', () => ({
  getBrowserNotificationPermission: () => 'unsupported',
  useNotificationSettings: () => ({
    settings: {
      enabled: false,
      notify_task_approval: false,
      notify_question: false,
      notify_error: false,
      notify_idle: false,
    },
    saving: false,
    error: null,
    save: vi.fn(),
  }),
}));
vi.mock('../hooks/useDensity', () => ({
  useDensity: () => ({ density: 'full', setDensity: vi.fn() }),
}));
vi.mock('../hooks/useFocusScope', () => ({
  useRegisterFocusScope: vi.fn(),
}));

import { SettingsDropdown } from './SettingsDropdown';
import { OPEN_FEDERATION_CONNECTIONS_EVENT } from './settingsEvents';

function Location() {
  const location = useLocation();
  return <output data-testid="location">{JSON.stringify(location)}</output>;
}

describe('SettingsDropdown', () => {
  beforeEach(() => {
    apiMock.getVersion.mockReset().mockReturnValue(new Promise(() => {}));
    apiMock.getLlmLanguageSetting.mockReset().mockReturnValue(new Promise(() => {}));
  });

  afterEach(() => vi.unstubAllGlobals());

  it('opens Settings and expands and focuses Connections for the shared action event', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify([]))));
    render(
      <MemoryRouter>
        <SettingsDropdown
          theme="dark"
          onToggleTheme={vi.fn()}
          codexPreflight={null}
          onPreflightInvalidated={vi.fn()}
        />
      </MemoryRouter>,
    );

    const request = new Event(OPEN_FEDERATION_CONNECTIONS_EVENT, { cancelable: true });
    fireEvent(window, request);

    expect(request.defaultPrevented).toBe(true);
    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeVisible();
    const connections = screen.getByRole('button', { name: /Connections/ });
    expect(connections).toHaveAttribute('aria-expanded', 'true');
    expect(connections).toHaveFocus();
    expect(screen.getByRole('textbox', { name: 'Receiver-issued enrollment bundle' })).toBeVisible();
    expect(await screen.findByText('No saved peer connections.')).toBeVisible();
  });

  it('opens Connections from the fallback query and consumes only that query', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify([]))));
    render(
      <MemoryRouter initialEntries={['/?settings=connections&filter=active#saved-view']}>
        <SettingsDropdown
          theme="dark"
          onToggleTheme={vi.fn()}
          codexPreflight={null}
          onPreflightInvalidated={vi.fn()}
        />
        <Location />
      </MemoryRouter>,
    );

    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeVisible();
    const connections = screen.getByRole('button', { name: /Connections/ });
    expect(connections).toHaveAttribute('aria-expanded', 'true');
    expect(connections).toHaveFocus();
    expect(screen.getByRole('textbox', { name: 'Receiver-issued enrollment bundle' })).toBeVisible();
    expect(await screen.findByText('No saved peer connections.')).toBeVisible();
    await waitFor(() => {
      expect(screen.getByTestId('location')).toHaveTextContent('"pathname":"/"');
      expect(screen.getByTestId('location')).toHaveTextContent('"search":"?filter=active"');
      expect(screen.getByTestId('location')).toHaveTextContent('"hash":"#saved-view"');
      expect(screen.getByTestId('location')).not.toHaveTextContent('settings=connections');
    });
  });

  it('shows an abbreviated dirty SHA while exposing the full identity', async () => {
    const fullGitSha = '0123456789abcdef0123456789abcdef01234567-dirty';
    apiMock.getVersion.mockResolvedValue({
      version: '1.2.3',
      git_sha: fullGitSha,
      socket_activated: false,
    });

    render(
      <MemoryRouter>
        <SettingsDropdown
          theme="dark"
          onToggleTheme={vi.fn()}
          codexPreflight={null}
          onPreflightInvalidated={vi.fn()}
        />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));

    const commit = await screen.findByLabelText(`Git SHA ${fullGitSha}`);
    expect(commit).toHaveAttribute('title', fullGitSha);
    expect(commit).toHaveTextContent('0123456789ab-dirty');
    expect(commit).not.toHaveTextContent(fullGitSha);
  });
});
