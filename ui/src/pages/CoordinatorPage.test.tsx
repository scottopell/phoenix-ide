import { describe, it, expect, beforeEach, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { CoordinatorPage } from './CoordinatorPage';
import type { Conversation, GlobalOpenWorkResponse } from '../api';

const { apiMock, conversationSnapshotMock, conversationPhaseMock } = vi.hoisted(() => ({
  apiMock: {
    ensureGlobalCoordinator: vi.fn(),
    getGlobalOpenWork: vi.fn(),
    resolveCoordinatorRoute: vi.fn(),
  },
  conversationSnapshotMock: vi.fn(),
  conversationPhaseMock: vi.fn(),
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: apiMock,
  };
});

vi.mock('../hooks', async () => {
  const actual = await vi.importActual<typeof import('../hooks')>('../hooks');
  return {
    ...actual,
    useMediaQuery: () => true,
  };
});

vi.mock('../conversation', () => ({
  useConversationSnapshot: (slug: string | null) => conversationSnapshotMock(slug),
  useConversationPhase: (slug: string | null) => conversationPhaseMock(slug),
}));

vi.mock('./ConversationPage', () => ({
  ConversationPage: ({ routePrefix }: { routePrefix?: string }) => (
    <div>
      Shared conversation runtime {routePrefix}
      <label>
        Coordinator draft
        <input aria-label="Coordinator draft" defaultValue="preserve me" />
      </label>
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

const openWork = (): GlobalOpenWorkResponse => ({
  generated_at: '2024-01-01T00:00:00Z',
  has_more: false,
  groups: [
    {
      project_id: 'proj-1',
      project_name: 'Phoenix',
      canonical_path: '/work/phoenix',
      items: [
        {
          id: 'item-1',
          source: 'chain',
          title: 'Fix coordinator page',
          project_id: 'proj-1',
          current_conversation_id: 'conv-12345678',
          current_conversation_slug: 'fix-coordinator',
          root_conversation_id: 'conv-root0001',
          root_conversation_slug: 'root-fix-coordinator',
          updated_at: '2024-01-02T10:00:00Z',
          mode: 'WORK',
          state: 'needs_action',
          task_id: '08700',
          task_title: 'Replace Global Recall with Coordinator',
          task_status: 'in-progress',
          branch_name: 'task-08700',
          base_branch: 'main',
          worktree_path: '/wt/task-08700',
          member_count: 2,
          signals: ['needs action', 'task open'],
          href: '/c/fix-coordinator',
          reference: '@work:item-1',
        },
      ],
    },
  ],
});

describe('CoordinatorPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMock.ensureGlobalCoordinator.mockResolvedValue({ conversation: coordinatorConversation() });
    apiMock.getGlobalOpenWork.mockResolvedValue(openWork());
    apiMock.resolveCoordinatorRoute.mockResolvedValue({ coordinator_id: 'conv-coordinator' });
    conversationSnapshotMock.mockReturnValue({ state: { type: 'idle' } });
    conversationPhaseMock.mockReturnValue({ type: 'idle' });
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      value: 'visible',
    });
  });

  it('keeps an applied find-work query stable without rerunning bootstrap', async () => {
    renderPage();
    await screen.findByText('Shared conversation runtime /global');
    fireEvent.click(screen.getByRole('tab', { name: /^Work \d+$/ }));

    fireEvent.change(screen.getByPlaceholderText('task id, branch, title, signal'), {
      target: { value: 'auth' },
    });
    fireEvent.submit(screen.getByPlaceholderText('task id, branch, title, signal').closest('form')!);

    await waitFor(() => expect(apiMock.getGlobalOpenWork).toHaveBeenCalledWith(0, 'auth'));
    expect(apiMock.ensureGlobalCoordinator).toHaveBeenCalledTimes(1);
    expect(screen.getByDisplayValue('auth')).toBeInTheDocument();
  });

  it('treats approval and recovery signals as attention and links to the owning conversation', async () => {
    const data = openWork();
    const group = data.groups[0];
    if (!group) throw new Error('fixture group missing');
    const item = group.items[0];
    if (!item) throw new Error('fixture item missing');
    group.items[0] = {
      ...item,
      source: 'chain',
      href: '/chains/root-1',
      current_conversation_slug: 'latest-owner',
      state: 'awaiting_task_approval',
      signals: ['task approval pending', 'recovery needed'],
    };
    apiMock.getGlobalOpenWork.mockResolvedValue(data);

    renderPage();
    await screen.findByText('Shared conversation runtime /global');
    fireEvent.click(screen.getByRole('tab', { name: /^Work \d+$/ }));

    expect(screen.getByRole('heading', { name: '1 conversation need attention' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Fix coordinator page/ })).toHaveAttribute('href', '/c/latest-owner');
    expect(screen.getByText('awaiting_task_approval')).toHaveClass('coordinator-state-pill--urgent');
  });

  it('loads the coordinator contract and renders attention-first work', async () => {
    renderPage();

    await waitFor(() => {
      expect(apiMock.ensureGlobalCoordinator).toHaveBeenCalledTimes(1);
      expect(screen.getByRole('heading', { name: 'Coordinator' })).toBeInTheDocument();
    });

    expect(await screen.findByText('Shared conversation runtime /global')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: /^Work \d+$/ }));
    expect(screen.getByRole('heading', { name: '1 conversation need attention' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Fix coordinator page/ })).toHaveAttribute('href', '/c/fix-coordinator');
    expect(screen.getByText('Showing all open work')).toBeInTheDocument();
  });

  it('defaults to Conversation and preserves its mounted state while switching to Work', async () => {
    renderPage();

    const conversation = await screen.findByRole('region', { name: 'Coordinator conversation' });
    const workPane = document.querySelector('.coordinator-work-pane');
    expect(workPane).not.toBeNull();
    const draft = screen.getByRole('textbox', { name: 'Coordinator draft' });
    fireEvent.change(draft, { target: { value: 'unsent follow-up' } });

    expect(screen.getByRole('tab', { name: 'Conversation' })).toHaveAttribute('aria-selected', 'true');
    expect(conversation).not.toHaveAttribute('hidden');
    expect(workPane).toHaveAttribute('hidden');

    fireEvent.click(screen.getByRole('tab', { name: /^Work \d+$/ }));
    expect(conversation).toHaveAttribute('hidden');
    expect(workPane).not.toHaveAttribute('hidden');

    fireEvent.click(screen.getByRole('tab', { name: 'Conversation' }));
    expect(screen.getByRole('textbox', { name: 'Coordinator draft' })).toHaveValue('unsent follow-up');
  });

  it('restores Conversation when a compact deep link reuses the route', async () => {
    function DeepLinkHarness() {
      const navigate = useNavigate();
      return (
        <>
          <button type="button" onClick={() => navigate('/global/conv-coordinator#message-42')}>Open citation</button>
          <CoordinatorPage />
        </>
      );
    }
    render(
      <MemoryRouter initialEntries={['/global/conv-coordinator']}>
        <Routes><Route path="/global/:slug" element={<DeepLinkHarness />} /></Routes>
      </MemoryRouter>,
    );

    await screen.findByText('Shared conversation runtime /global');
    fireEvent.click(screen.getByRole('tab', { name: /^Work \d+$/ }));
    const conversation = document.querySelector('.coordinator-conversation');
    expect(conversation).not.toBeNull();
    expect(conversation).toHaveAttribute('hidden');

    fireEvent.click(screen.getByRole('button', { name: 'Open citation' }));
    await waitFor(() => expect(conversation).not.toHaveAttribute('hidden'));
  });

  it('refreshes open work when the window regains focus', async () => {
    renderPage();
    await screen.findByText('Shared conversation runtime /global');

    fireEvent.focus(window);

    await waitFor(() => expect(apiMock.getGlobalOpenWork).toHaveBeenCalledTimes(2));
  });

  it('refreshes open work when the coordinator turn completes', async () => {
    let phase: { type: 'seeded_llm_requesting'; attempt: number } | { type: 'idle' } = { type: 'seeded_llm_requesting', attempt: 1 };
    conversationPhaseMock.mockImplementation(() => phase);

    const { rerender } = render(
      <MemoryRouter initialEntries={['/global/conv-coordinator']}>
        <Routes><Route path="/global/:slug" element={<CoordinatorPage />} /></Routes>
      </MemoryRouter>,
    );
    await screen.findByText('Shared conversation runtime /global');
    expect(apiMock.getGlobalOpenWork).toHaveBeenCalledTimes(1);

    phase = { type: 'idle' };
    rerender(
      <MemoryRouter initialEntries={['/global/conv-coordinator']}>
        <Routes><Route path="/global/:slug" element={<CoordinatorPage />} /></Routes>
      </MemoryRouter>,
    );

    await waitFor(() => expect(apiMock.getGlobalOpenWork).toHaveBeenCalledTimes(2));
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
