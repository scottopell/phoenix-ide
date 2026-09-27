import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';

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

describe('SettingsDropdown version footer', () => {
  beforeEach(() => {
    apiMock.getVersion.mockReset();
    apiMock.getLlmLanguageSetting.mockReset().mockReturnValue(new Promise(() => {}));
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
