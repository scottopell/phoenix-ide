import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { Conversation, Project } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    codexLoginPreflight: vi.fn(),
    getProjects: vi.fn(),
    archiveConversation: vi.fn(),
    unarchiveConversation: vi.fn(),
    archiveChain: vi.fn(),
    unarchiveChain: vi.fn(),
    getChain: vi.fn(),
    deleteChain: vi.fn(),
    deleteConversation: vi.fn(),
    renameConversation: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: apiMock,
  };
});

vi.mock('../modelsPoller', () => ({
  subscribeModels: vi.fn(() => () => {}),
  refreshModels: vi.fn(),
}));

import { Sidebar } from './Sidebar';

const makeConv = (id: string, slug: string, overrides: Partial<Conversation> = {}): Conversation => ({
  id,
  slug,
  model: 'claude-3-5-sonnet',
  cwd: '/home/user/project',
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  message_count: 5,
  project_id: 'proj-1',
  conv_mode_label: 'EXPLORE',
  browser_session_active: false,
  terminal_uses_tmux: false,
  ...overrides,
});

const makeProject = (id: string, path: string): Project => ({
  id,
  canonical_path: path,
  main_ref: 'main',
  created_at: '2024-01-01T00:00:00Z',
  conversation_count: 1,
});

describe('Sidebar — active conversation project filter', () => {
  beforeEach(() => {
    localStorage.clear();
    apiMock.codexLoginPreflight.mockResolvedValue({
      configured: false,
      account_id: null,
      auth_path: null,
    });
    apiMock.getProjects.mockResolvedValue([
      makeProject('proj-1', '/home/user/one'),
      makeProject('proj-2', '/home/user/two'),
    ]);
  });

  afterEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it('clears the project filter when it hides the active conversation', async () => {
    localStorage.setItem('phoenix:sidebar-project-filter', 'proj-1');
    const conversations = [
      makeConv('hidden-active-id', 'hidden-active', { project_id: 'proj-2', cwd: '/home/user/two' }),
      makeConv('visible-other-id', 'visible-other', { project_id: 'proj-1', cwd: '/home/user/one' }),
    ];

    const { container } = render(
      <MemoryRouter initialEntries={['/c/hidden-active']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={conversations}
          archivedConversations={[]}
          activeSlug="hidden-active"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(localStorage.getItem('phoenix:sidebar-project-filter')).toBeNull();
    });
    await waitFor(() => {
      expect(container.querySelector('[data-id="hidden-active-id"]')).not.toBeNull();
    });
    expect(container.querySelector('[data-id="hidden-active-id"]')!.classList.contains('active')).toBe(true);
  });

  it('keeps existing chain grouping and active-row rendering after filter clearing', async () => {
    localStorage.setItem('phoenix:sidebar-project-filter', 'proj-1');
    const root = makeConv('root-id', 'root-slug', {
      project_id: 'proj-2',
      cwd: '/home/user/two',
      continued_in_conv_id: 'leaf-id',
      chain_name: 'filtered chain',
    });
    const leaf = makeConv('leaf-id', 'leaf-slug', {
      project_id: 'proj-2',
      cwd: '/home/user/two',
    });

    const { container } = render(
      <MemoryRouter initialEntries={['/c/leaf-slug']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[leaf, root]}
          archivedConversations={[]}
          activeSlug="leaf-slug"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(container.querySelector('.conv-chain-block')).not.toBeNull();
    });
    expect(container.querySelector('.conv-chain-name-label')!.textContent).toBe('filtered chain');
    expect(container.querySelectorAll('.conv-item-chain-member').length).toBe(2);
    expect(container.querySelector('[data-id="leaf-id"]')!.classList.contains('active')).toBe(true);
  });
});
