import { describe, it, expect, beforeEach, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { CoordinatorPage } from './CoordinatorPage';
import type { Conversation, GlobalOpenWorkResponse } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    ensureGlobalCoordinator: vi.fn(),
    getGlobalOpenWork: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: apiMock,
  };
});

vi.mock('./ConversationPage', () => ({
  ConversationPage: ({ routePrefix }: { routePrefix?: string }) => (
    <div>Shared conversation runtime {routePrefix}</div>
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
          state: 'working',
          task_id: '08700',
          task_title: 'Replace Global Recall with Coordinator',
          task_status: 'in-progress',
          branch_name: 'task-08700',
          base_branch: 'main',
          worktree_path: '/wt/task-08700',
          member_count: 2,
          signals: ['active runtime', 'task open'],
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
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  it('loads the coordinator contract and renders a compact fleet snapshot', async () => {
    renderPage();

    await waitFor(() => {
      expect(apiMock.ensureGlobalCoordinator).toHaveBeenCalledTimes(1);
      expect(screen.getByRole('heading', { name: 'Coordinator' })).toBeInTheDocument();
    });

    expect(screen.getByText('Shared conversation runtime /global')).toBeInTheDocument();
    expect(screen.getByText('Fix coordinator page')).toBeInTheDocument();
    expect(screen.getByText('TASK 08700')).toBeInTheDocument();
    expect(screen.queryByText(/ROOT conv-root/i)).not.toBeInTheDocument();
  });

  it('replaces a stale Coordinator continuation URL with the singleton route', async () => {
    render(
      <MemoryRouter initialEntries={['/global/stale-coordinator']}>
        <Routes>
          <Route path="/global/:slug" element={<><CoordinatorPage /><CurrentPath /></>} />
        </Routes>
      </MemoryRouter>,
    );

    expect(await screen.findByText('/global/conv-coordinator')).toBeInTheDocument();
  });

  it('expands a fleet row to reveal audit detail and copies the durable reference', async () => {
    renderPage();

    await screen.findByText('Fix coordinator page');
    fireEvent.click(screen.getByRole('button', { name: 'Show details' }));

    expect(screen.getByText('ROOT conv-roo')).toBeInTheDocument();
    expect(screen.getByText(/WORKTREE \/wt\/task-08700/)).toBeInTheDocument();
    expect(screen.getByText('REF @work:item-1')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Copy ref' }));
    await waitFor(() => {
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith('@work:item-1');
    });
  });
});
