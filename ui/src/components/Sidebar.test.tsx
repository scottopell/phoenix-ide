import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { Conversation, ProductConversationListRow } from '../api';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    codexLoginPreflight: vi.fn(),
    deploymentInfo: vi.fn(),
    getProjects: vi.fn(),
    listProductConversations: vi.fn(),
    getLocalServices: vi.fn(),
    archiveConversation: vi.fn(),
    archiveChain: vi.fn(),
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

const makeProductConversation = (id: string, overrides: Partial<ProductConversationListRow> = {}): ProductConversationListRow => ({
  product_conversation_id: id,
  canonical_route: `/product-conversations/${id}`,
  canonical_root: {
    transcript_row_id: `root-${id}`,
    slug: `root-${id}`,
    title: `Root ${id}`,
  },
  ordinary_lifecycle: 'open',
  latest_transcript_row_id: `latest-${id}`,
  updated_at: '2024-01-01T00:00:00Z',
  presentation: { kind: 'state', display_name: `Display ${id}`, presentation_mode: 'idle' },
  ...overrides,
});

describe('Sidebar — ProductConversation navigation', () => {
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
    apiMock.deploymentInfo.mockResolvedValue({ local_access: true });
    apiMock.getLocalServices.mockResolvedValue({ services: [] });
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [
        makeProductConversation('pc-open', { canonical_root: { transcript_row_id: 'root-open', slug: 'root-open', title: 'Open Root' } }),
        makeProductConversation('pc-archived', { ordinary_lifecycle: 'history', canonical_root: { transcript_row_id: 'root-archived', slug: 'root-archived', title: 'Archived Root' } }),
      ],
    });
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



  it('shows History when the active aggregate product conversation is archived', async () => {
    const { container, getByRole } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-archived']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[]}
          archivedConversations={[]}
          activeSlug="pc-archived"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(container.querySelector('[data-product-conversation-id="pc-archived"]')).not.toBeNull();
    });
    const archivedTab = getByRole('button', { name: /History 1/ });
    expect(archivedTab.getAttribute('aria-pressed')).toBe('true');
  });



  it('marks a collapsed aggregate dot active for its latest-row identity', async () => {
    const onToggle = vi.fn();

    const { container, getByRole } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-open']}>
        <Sidebar
          collapsed
          onToggle={onToggle}
          conversations={[]}
          archivedConversations={[]}
          activeSlug="latest-pc-open"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(container.querySelector('[aria-label="Open Display pc-open"]')).toHaveClass('active');
    });

    fireEvent.click(getByRole('button', { name: /Expand sidebar/ }));
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it('falls back to cached member rows and retries a failed aggregate refresh', async () => {
    apiMock.listProductConversations
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({ product_conversations: [makeProductConversation('pc-recovered')] });
    const cached = makeConv('cached-id', 'cached-slug');

    const { container, getByRole } = render(
      <MemoryRouter initialEntries={['/c/cached-slug']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[cached]}
          archivedConversations={[]}
          activeSlug="cached-slug"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => expect(getByRole('status')).toHaveTextContent('Showing cached conversations'));
    expect(container.querySelector('[data-id="cached-id"]')).not.toBeNull();
    fireEvent.click(getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-recovered"]')).not.toBeNull());
    expect(apiMock.listProductConversations).toHaveBeenCalledTimes(2);
  });

  it('labels the global nav entry as Coordinator', async () => {
    const conversations = [makeConv('active-id', 'active-project-one')];

    const { getAllByLabelText, queryByLabelText } = render(
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
      expect(getAllByLabelText('Coordinator').length).toBeGreaterThan(0);
    });
    expect(queryByLabelText('Global Recall')).toBeNull();
  });


});
