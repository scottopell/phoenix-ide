import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  notifyProductConversationListMayHaveChanged,
  subscribeCloseSnapshotChanged,
  subscribeProductConversationListRevision,
  subscribeProductConversationSnapshotChanged,
} from '../notifications';
import { render, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { ApiResponseError, ConflictError } from '../api';
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
    closeProductConversation: vi.fn(),
    getChain: vi.fn(),
    deleteChain: vi.fn(),
    deleteConversation: vi.fn(),
    renameConversation: vi.fn(),
    renameProductConversation: vi.fn(),
    getProductConversationSnapshot: vi.fn(),
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
  lifecycle: { state: 'open', close_action: { availability: 'available' } },
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
    apiMock.getProductConversationSnapshot.mockRejectedValue(new Error('not a product route'));
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [
        makeProductConversation('pc-open', { canonical_root: { transcript_row_id: 'root-open', slug: 'root-open', title: 'Open Root' } }),
        makeProductConversation('pc-archived', { lifecycle: { state: 'history' }, canonical_root: { transcript_row_id: 'root-archived', slug: 'root-archived', title: 'Archived Root' } }),
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



  it('shows a History aggregate with no Archived terminology or legacy Archive action', async () => {
    const { container, getByRole, getAllByText, queryByText, queryByTitle } = render(
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
    const historyTab = getByRole('button', { name: /^History 1$/ });
    expect(historyTab.getAttribute('aria-pressed')).toBe('true');
    expect(getAllByText('History').length).toBeGreaterThan(0);
    expect(queryByText('Archived')).toBeNull();
    expect(queryByTitle(/Archive conversation/)).toBeNull();
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
    fireEvent.click(getByRole('button', { name: 'Conversation actions' }));
    expect(container.querySelector('.conv-item-actions')?.textContent).not.toContain('Rename');
    fireEvent.click(getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-recovered"]')).not.toBeNull());
    expect(apiMock.listProductConversations).toHaveBeenCalledTimes(2);
  });

  it('renames product conversations through the canonical-root transcript id', async () => {
    apiMock.renameConversation.mockResolvedValue({ conversation: makeConv('unused', 'renamed-product') });
    const row = makeProductConversation('pc-continued', {
      canonical_root: { transcript_row_id: 'canonical-root-row', slug: 'old-product', title: 'Old Product' },
      latest_transcript_row_id: 'latest-continuation-row',
    });
    apiMock.listProductConversations.mockResolvedValue({ product_conversations: [row] });
    apiMock.renameProductConversation.mockResolvedValue({
      ...row,
      canonical_root: { ...row.canonical_root, title: 'Renamed Product Title' },
      presentation: { kind: 'state', display_name: 'Renamed Product Title', presentation_mode: 'idle' },
    });
    const listRevisionListener = vi.fn();
    const unsubscribe = subscribeProductConversationListRevision(listRevisionListener);

    const { getByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-continued']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-continued" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-continued"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Rename product conversation Old Product/ }));
    const input = getByRole('textbox');
    fireEvent.change(input, { target: { value: 'Renamed Product Title' } });
    fireEvent.click(getByRole('button', { name: 'Rename' }));

    await waitFor(() => expect(apiMock.renameProductConversation).toHaveBeenCalledWith('pc-continued', 'Renamed Product Title'));
    expect(apiMock.renameConversation).not.toHaveBeenCalled();
    await waitFor(() => expect(listRevisionListener).toHaveBeenCalledTimes(1));
    unsubscribe();
  });

  it('closes product conversations through the canonical root chain', async () => {
    apiMock.closeProductConversation.mockResolvedValue(undefined);
    const row = makeProductConversation('pc-continued', {
      canonical_root: { transcript_row_id: 'canonical-root-row', slug: 'old-product', title: 'Old Product' },
      latest_transcript_row_id: 'latest-continuation-row',
    });
    apiMock.listProductConversations.mockResolvedValue({ product_conversations: [row] });

    const { getByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-continued']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-continued" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-continued"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Close product conversation Old Product/ }));
    fireEvent.click(getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(apiMock.closeProductConversation).toHaveBeenCalledWith('pc-continued'));
  });

  it('closes single-row products through the ordinary conversation endpoint', async () => {
    apiMock.closeProductConversation.mockResolvedValue(undefined);
    const row = makeProductConversation('pc-single', {
      canonical_root: { transcript_row_id: 'single-row', slug: 'single', title: 'Single Product' },
      latest_transcript_row_id: 'single-row',
    });
    apiMock.listProductConversations.mockResolvedValue({ product_conversations: [row] });

    const { getByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-single']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-single" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-single"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Close product conversation Single Product/ }));
    fireEvent.click(getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(apiMock.closeProductConversation).toHaveBeenCalledWith('pc-single'));
  });

  it('re-resolves close topology when a continuation appears during confirmation', async () => {
    apiMock.closeProductConversation.mockResolvedValue(undefined);
    const single = makeProductConversation('pc-race', {
      canonical_root: { transcript_row_id: 'race-root', slug: 'race', title: 'Race Product' },
      latest_transcript_row_id: 'race-root',
    });
    const continued = { ...single, latest_transcript_row_id: 'race-continuation' };
    apiMock.listProductConversations
      .mockResolvedValueOnce({ product_conversations: [single] })
      .mockResolvedValueOnce({ product_conversations: [continued] })
      .mockResolvedValue({ product_conversations: [] });

    const { getByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-race']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-race" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-race"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Close product conversation Race Product/ }));
    fireEvent.click(getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(apiMock.closeProductConversation).toHaveBeenCalledWith('pc-race'));
  });

  it('does not expose product conversation actions for history rows', async () => {
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [makeProductConversation('pc-history', {
        lifecycle: { state: 'history' },
      })],
    });

    const { queryByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-history']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-history" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-history"]')).not.toBeNull());
    expect(queryByRole('button', { name: /Rename product conversation/ })).toBeNull();
    expect(queryByRole('button', { name: /Close product conversation/ })).toBeNull();
  });

  it('keeps History Delete open with an error and gates duplicate submissions', async () => {
    let rejectDelete!: (error: Error) => void;
    apiMock.deleteChain.mockReturnValueOnce(new Promise((_, reject) => { rejectDelete = reject; }));
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [makeProductConversation('pc-history', {
        lifecycle: { state: 'history' },
        canonical_root: { transcript_row_id: 'history-root', slug: 'history-root', title: 'History Product' },
        latest_transcript_row_id: 'history-latest',
      })],
    });

    const { getByRole, findByText, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-history']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-history" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-history"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Delete product conversation History Product/ }));
    const confirm = getByRole('button', { name: 'Delete' });
    fireEvent.click(confirm);
    fireEvent.click(confirm);

    expect(apiMock.deleteChain).toHaveBeenCalledTimes(1);
    expect(confirm).toBeDisabled();
    rejectDelete(new Error('server refused deletion'));

    expect(await findByText('server refused deletion')).toBeInTheDocument();
    expect(container.querySelector('.confirm-dialog[title="Delete Product Conversation"]')).not.toBeNull();
    expect(getByRole('button', { name: 'Delete' })).not.toBeDisabled();
  });

  it('treats an authoritative History Delete 404 as idempotent success', async () => {
    apiMock.deleteChain.mockRejectedValueOnce(new ApiResponseError('not found', 404));
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [makeProductConversation('pc-history', {
        lifecycle: { state: 'history' },
        canonical_root: { transcript_row_id: 'history-root', slug: 'history-root', title: 'History Product' },
        latest_transcript_row_id: 'history-latest',
      })],
    });

    const { getByRole, queryByText, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-history']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-history" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );
    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-history"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Delete product conversation History Product/ }));
    fireEvent.click(getByRole('button', { name: 'Delete' }));

    await waitFor(() => expect(container.querySelector('.confirm-dialog[title="Delete Product Conversation"]')).toBeNull());
    expect(queryByText('not found')).toBeNull();
  });

  it('leaves an intermediate aggregate member route after History Delete', async () => {
    const row = makeProductConversation('pc-history', {
      lifecycle: { state: 'history' },
      canonical_root: { transcript_row_id: 'history-root', slug: 'history-root', title: 'History Product' },
      latest_transcript_row_id: 'history-latest',
    });
    apiMock.listProductConversations.mockResolvedValue({ product_conversations: [row] });
    apiMock.getProductConversationSnapshot.mockResolvedValue({
      product_conversation_id: 'pc-history',
      segments: [{ transcript_row_id: 'history-root', slug: 'history-root' }, { transcript_row_id: 'middle-id', slug: 'middle-slug' }, { transcript_row_id: 'history-latest', slug: 'latest-slug' }],
    });

    const { getByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/middle-slug']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="middle-slug" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );
    await waitFor(() => expect(apiMock.getProductConversationSnapshot).toHaveBeenCalledWith('middle-slug', { message_limit: 1 }));
    fireEvent.click(getByRole('button', { name: /Delete product conversation History Product/ }));
    fireEvent.click(getByRole('button', { name: 'Delete' }));

    await waitFor(() => expect(container.querySelector('.confirm-dialog[title="Delete Product Conversation"]')).toBeNull());
  });

  it('publishes the successful authoritative title to the active aggregate snapshot', async () => {
    const renamed = makeProductConversation('pc-rename', {
      canonical_root: { transcript_row_id: 'root-rename', slug: 'old-product', title: 'New Product' },
    });
    const original = makeProductConversation('pc-rename', {
      canonical_root: { transcript_row_id: 'root-rename', slug: 'old-product', title: 'Old Product' },
    });
    let serverRow = original;
    apiMock.renameProductConversation.mockImplementationOnce(async () => {
      serverRow = renamed;
      return renamed;
    });
    apiMock.listProductConversations.mockImplementation(async () => ({
      product_conversations: [serverRow],
    }));
    const snapshotListener = vi.fn();
    const unsubscribe = subscribeProductConversationSnapshotChanged('pc-rename', snapshotListener);

    const { getByRole, findByRole, findByText } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-rename']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-rename" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    fireEvent.click(await findByRole('button', { name: /Rename product conversation Old Product/ }));
    fireEvent.change(getByRole('textbox'), { target: { value: 'New Product' } });
    fireEvent.click(getByRole('button', { name: 'Rename' }));

    expect(await findByText('New Product')).toBeInTheDocument();
    expect(snapshotListener).toHaveBeenCalledTimes(1);
    unsubscribe();
  });

  it('keeps product rename dialog open and unchanged on conflict', async () => {
    apiMock.renameProductConversation.mockRejectedValue(new ConflictError({ error_type: 'conflict', error: 'stale rename' }));
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [makeProductConversation('pc-conflict', { canonical_root: { transcript_row_id: 'root-conflict', slug: 'old-product', title: 'Old Product' } })],
    });

    const { getByRole, findByText, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-conflict']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-conflict" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-conflict"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Rename product conversation Old Product/ }));
    fireEvent.change(getByRole('textbox'), { target: { value: 'new-product' } });
    fireEvent.click(getByRole('button', { name: 'Rename' }));

    expect(await findByText('stale rename')).toBeInTheDocument();
    expect(container.querySelector('[data-product-conversation-id="pc-conflict"]')).not.toBeNull();
  });

  it('clears the starter close target when durable product close reports a typed conflict', async () => {
    apiMock.closeProductConversation.mockRejectedValueOnce(new ConflictError({
      error: 'close loss confirmation required',
      error_type: 'close_loss_confirmation_required',
    }));
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [makeProductConversation('pc-close-conflict', {
        canonical_root: { transcript_row_id: 'root-close-conflict', slug: 'close-product', title: 'Close Product' },
      })],
    });

    const { getByRole, queryByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-close-conflict']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-close-conflict" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-close-conflict"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /Close product conversation Close Product/ }));
    fireEvent.click(getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(apiMock.closeProductConversation).toHaveBeenCalledWith('pc-close-conflict'));
    await waitFor(() => expect(queryByRole('dialog', { name: 'Close Product Conversation' })).toBeNull());
  });

  it('ignores older product-list responses after a newer refresh wins', async () => {
    let resolveFirst!: (value: { product_conversations: ProductConversationListRow[] }) => void;
    const first = new Promise<{ product_conversations: ProductConversationListRow[] }>((resolve) => { resolveFirst = resolve; });
    apiMock.listProductConversations.mockReturnValueOnce(first);
    apiMock.listProductConversations.mockResolvedValueOnce({ product_conversations: [makeProductConversation('pc-newer')] });

    const { container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-newer']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-newer" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    notifyProductConversationListMayHaveChanged();
    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-newer"]')).not.toBeNull());
    resolveFirst({ product_conversations: [makeProductConversation('pc-older')] });

    await waitFor(() => {
      expect(container.querySelector('[data-product-conversation-id="pc-newer"]')).not.toBeNull();
      expect(container.querySelector('[data-product-conversation-id="pc-older"]')).toBeNull();
    });
  });

  it('coalesces product-list refresh triggers from visibility, focus, and online without request loops', async () => {
    const { container } = render(
      <MemoryRouter initialEntries={['/c/cached-slug']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[makeConv('cached-id', 'cached-slug')]}
          archivedConversations={[]}
          activeSlug="cached-slug"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-open"]')).not.toBeNull());
    apiMock.listProductConversations.mockResolvedValueOnce({
      product_conversations: [makeProductConversation('pc-refreshed')],
    });

    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('focus'));
    window.dispatchEvent(new Event('online'));

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-refreshed"]')).not.toBeNull());
    expect(apiMock.listProductConversations).toHaveBeenCalledTimes(2);
  });

  it('coalesces product-list refresh triggers from conversation store mutations without request loops', async () => {
    const { container } = render(
      <MemoryRouter initialEntries={['/c/cached-slug']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[makeConv('cached-id', 'cached-slug')]}
          archivedConversations={[]}
          activeSlug="cached-slug"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-open"]')).not.toBeNull());
    apiMock.listProductConversations.mockResolvedValueOnce({
      product_conversations: [makeProductConversation('pc-updated')],
    });

    notifyProductConversationListMayHaveChanged();
    notifyProductConversationListMayHaveChanged();
    notifyProductConversationListMayHaveChanged();

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-updated"]')).not.toBeNull());
    expect(apiMock.listProductConversations).toHaveBeenCalledTimes(2);
  });

  it('notifies the active Close surface when Close starts a server-authoritative attempt', async () => {
    apiMock.listProductConversations.mockRejectedValueOnce(new Error('offline'));
    apiMock.archiveConversation.mockRejectedValueOnce(new ConflictError({
      error: 'close loss confirmation required',
      error_type: 'close_loss_confirmation_required',
    }));
    const listener = vi.fn();
    const unsubscribe = subscribeCloseSnapshotChanged('cached-id', listener);

    const { getByTitle } = render(
      <MemoryRouter initialEntries={['/c/cached-slug']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[makeConv('cached-id', 'cached-slug')]}
          archivedConversations={[]}
          activeSlug="cached-slug"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    fireEvent.click(await waitFor(() => getByTitle('Actions')));
    fireEvent.click(await waitFor(() => getByTitle('Close conversation "cached-slug"')));
    await waitFor(() => expect(listener).toHaveBeenCalledTimes(1));
    unsubscribe();
  });

  it('does not notify the Close surface for non-close compatibility failures', async () => {
    apiMock.listProductConversations.mockRejectedValueOnce(new Error('offline'));
    apiMock.archiveConversation.mockRejectedValueOnce(new ConflictError({
      error: 'other conflict',
      error_type: 'proposal_resolved',
    }));
    const listener = vi.fn();
    const unsubscribe = subscribeCloseSnapshotChanged('cached-id', listener);

    const { getByTitle } = render(
      <MemoryRouter initialEntries={['/c/cached-slug']}>
        <Sidebar
          collapsed={false}
          onToggle={vi.fn()}
          conversations={[makeConv('cached-id', 'cached-slug')]}
          archivedConversations={[]}
          activeSlug="cached-slug"
          onConversationCreated={vi.fn()}
        />
      </MemoryRouter>,
    );

    fireEvent.click(await waitFor(() => getByTitle('Actions')));
    fireEvent.click(await waitFor(() => getByTitle('Close conversation "cached-slug"')));
    await waitFor(() => expect(apiMock.archiveConversation).toHaveBeenCalledTimes(1));
    expect(listener).toHaveBeenCalledTimes(0);
    unsubscribe();
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
