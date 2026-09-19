import { describe, it, expect, beforeEach, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { CoordinatorPage } from './CoordinatorPage';
import { COORDINATOR_BRIEFING_PROMPT } from './coordinatorBriefing';
import type { Conversation } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    ensureGlobalCoordinator: vi.fn(),
    resolveCoordinatorRoute: vi.fn(),
    getCoordinatorAutomaticContinuation: vi.fn(),
    updateCoordinatorAutomaticContinuation: vi.fn(),
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
  const location = useLocation();
  return <div>{`${location.pathname}${location.search}${location.hash}`}</div>;
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
    apiMock.getCoordinatorAutomaticContinuation.mockResolvedValue({
      aggregate: { kind: 'coordinator', product_conversation_id: 'coordinator-product' },
      auto_continue_on_context_exhaustion: false,
      admission: null,
    });
    apiMock.updateCoordinatorAutomaticContinuation.mockResolvedValue({
      aggregate: { kind: 'coordinator', product_conversation_id: 'coordinator-product' },
      auto_continue_on_context_exhaustion: true,
      admission: null,
    });
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

  it('loads and immediately persists the Global Coordinator automatic-continuation toggle', async () => {
    renderPage();

    const control = await screen.findByTestId('automatic-continuation-control');
    fireEvent.click(control.querySelector('summary')!);
    const checkbox = screen.getByRole('checkbox', { name: 'Automatically accept future generated handoffs and continue' });
    await waitFor(() => {
      expect(apiMock.getCoordinatorAutomaticContinuation).toHaveBeenCalledTimes(1);
      expect(checkbox).toBeEnabled();
      expect(checkbox).not.toBeChecked();
    });

    fireEvent.click(checkbox);
    await waitFor(() => expect(apiMock.updateCoordinatorAutomaticContinuation).toHaveBeenCalledWith(true));
    expect(await screen.findByText('Saved')).toBeInTheDocument();
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

  it('canonicalizes a historical Coordinator chain member without exposing aggregate controls', async () => {
    render(
      <MemoryRouter initialEntries={['/global/old-coordinator?view=history#message-source']}>
        <Routes>
          <Route path="/global/:slug" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('Shared conversation runtime /global')).toBeInTheDocument();
    expect(screen.getByText('/global/old-coordinator?view=history#message-source')).toBeInTheDocument();
    expect(apiMock.resolveCoordinatorRoute).toHaveBeenCalledWith('old-coordinator');
    expect(screen.queryByTestId('automatic-continuation-control')).not.toBeInTheDocument();
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
