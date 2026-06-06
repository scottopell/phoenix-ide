import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/react';
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
  work_scope_key: `conversation:${id}`,
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
  let originalScrollDescriptor: PropertyDescriptor | undefined;

  beforeEach(() => {
    localStorage.clear();
    originalScrollDescriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      configurable: true,
      value: vi.fn(),
    });
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
    if (originalScrollDescriptor) {
      Object.defineProperty(Element.prototype, 'scrollIntoView', originalScrollDescriptor);
    } else {
      delete (Element.prototype as unknown as { scrollIntoView?: unknown }).scrollIntoView;
    }
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

  it('preserves a manually selected project tab after the active route has been revealed', async () => {
    const conversations = [
      makeConv('active-id', 'active-project-one', { project_id: 'proj-1', cwd: '/home/user/one' }),
      makeConv('browse-id', 'browse-project-two', { project_id: 'proj-2', cwd: '/home/user/two' }),
    ];

    const { container, getByRole } = render(
      <MemoryRouter initialEntries={['/c/active-project-one']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={conversations}
          archivedConversations={[]}
          activeSlug="active-project-one"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(container.querySelector('[data-id="active-id"]')).not.toBeNull();
    });

    fireEvent.click(getByRole('button', { name: 'two' }));

    await waitFor(() => {
      expect(localStorage.getItem('phoenix:sidebar-project-filter')).toBe('proj-2');
    });
    expect(container.querySelector('[data-id="active-id"]')).toBeNull();
    expect(container.querySelector('[data-id="browse-id"]')).not.toBeNull();
  });

  it('shows the archived list when the active conversation is archived', async () => {
    const archived = makeConv('archived-id', 'archived-active', {
      archived: true,
      project_id: 'proj-1',
    });

    const { container } = render(
      <MemoryRouter initialEntries={['/c/archived-active']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[]}
          archivedConversations={[archived]}
          activeSlug="archived-active"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(container.querySelector('[data-id="archived-id"]')).not.toBeNull();
    });
    expect(container.querySelector('[data-id="archived-id"]')!.classList.contains('active')).toBe(true);
    expect(container.querySelector('.archive-toggle')!.textContent).toBe('Active');
  });
});
