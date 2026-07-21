import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { CoordinatorPage } from './CoordinatorPage';
import { COORDINATOR_BRIEFING_PROMPT } from './coordinatorBriefing';
import type { Conversation } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    ensureGlobalCoordinator: vi.fn(),
    resolveCoordinatorRoute: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return { ...actual, api: apiMock };
});

vi.mock('./ConversationPage', () => ({
  ConversationPage: ({
    routePrefix,
    composerQuickAction,
  }: {
    routePrefix?: string;
    composerQuickAction?: { label: string; compactLabel: string; prompt: string };
  }) => (
    <div>
      Shared conversation runtime {routePrefix}
      {composerQuickAction && (
        <button type="button" data-prompt={composerQuickAction.prompt}>
          {composerQuickAction.compactLabel}
          <span className="sr-only">{composerQuickAction.label}</span>
        </button>
      )}
    </div>
  ),
}));

function renderPage(initialEntry = '/global/conv-coordinator') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <Routes>
        <Route path="/global/:slug" element={<CoordinatorPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

function CurrentPath() {
  return <div>{useLocation().pathname}</div>;
}

const coordinatorConversation = (): Conversation => ({
  id: 'conv-coordinator',
  slug: 'coordinator',
  title: 'Coordinator',
  model: 'claude-3-5-sonnet',
  cwd: '/coordinator',
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  message_count: 3,
  browser_session_active: false,
  terminal_uses_tmux: false,
  work_scope_key: 'global:',
});

describe('CoordinatorPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMock.ensureGlobalCoordinator.mockResolvedValue({ conversation: coordinatorConversation() });
    apiMock.resolveCoordinatorRoute.mockResolvedValue({ coordinator_id: 'conv-coordinator' });
  });

  it('mounts only the shared conversation runtime with the briefing action', async () => {
    renderPage();

    expect(await screen.findByText('Shared conversation runtime /global')).toBeInTheDocument();
    const action = screen.getByRole('button', { name: /Brief me/ });
    expect(action).toHaveAttribute('data-prompt', COORDINATOR_BRIEFING_PROMPT);
    expect(COORDINATOR_BRIEFING_PROMPT).toContain('Do not send messages or change anything.');

    expect(screen.queryByRole('heading', { name: 'Coordinator' })).not.toBeInTheDocument();
    expect(screen.queryByRole('tablist', { name: 'Coordinator view' })).not.toBeInTheDocument();
    expect(screen.queryByRole('navigation', { name: 'Coordinator sections' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Coordinator work')).not.toBeInTheDocument();
    expect(screen.queryByText('Current work context is attached to each Coordinator message.')).not.toBeInTheDocument();
  });

  it('marks bootstrap loading and errors for overlay placement', async () => {
    let resolveCoordinator!: (value: { conversation: Conversation }) => void;
    apiMock.ensureGlobalCoordinator.mockReturnValueOnce(new Promise((resolve) => {
      resolveCoordinator = resolve;
    }));

    const { unmount } = renderPage();
    expect(screen.getByText('Loading…')).toHaveClass('coordinator-page-status');
    resolveCoordinator({ conversation: coordinatorConversation() });
    await screen.findByText('Shared conversation runtime /global');
    unmount();

    apiMock.ensureGlobalCoordinator.mockRejectedValueOnce(new Error('Coordinator unavailable'));
    renderPage();
    expect(await screen.findByText('Coordinator unavailable')).toHaveClass('coordinator-page-status');
  });

  it('redirects an ordinary conversation away from the Coordinator shell', async () => {
    apiMock.resolveCoordinatorRoute.mockResolvedValueOnce({ coordinator_id: null });
    render(
      <MemoryRouter initialEntries={['/global/ordinary-conversation']}>
        <Routes>
          <Route path="/global/:slug" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('/global/conv-coordinator')).toBeInTheDocument();
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith('ordinary-conversation');
  });

  it('mounts a historical Coordinator chain member without canonicalizing it', async () => {
    render(
      <MemoryRouter initialEntries={['/global/old-coordinator#message-source']}>
        <Routes>
          <Route path="/global/:slug" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('Shared conversation runtime /global')).toBeInTheDocument();
    expect(screen.getByText('/global/old-coordinator')).toBeInTheDocument();
  });

  it('replaces a stale Coordinator continuation URL with the singleton route', async () => {
    apiMock.resolveCoordinatorRoute.mockResolvedValueOnce({ coordinator_id: null });
    render(
      <MemoryRouter initialEntries={['/global/stale-coordinator']}>
        <Routes>
          <Route path="/global/:slug" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('/global/conv-coordinator')).toBeInTheDocument();
  });
});
