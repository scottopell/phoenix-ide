import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  notifyProductConversationListMayHaveChanged,
  subscribeProductConversationListRevision,
  subscribeProductConversationSnapshotChanged,
} from '../notifications';
import { act, render, fireEvent, waitFor } from '@testing-library/react';
import { Link, MemoryRouter, useLocation } from 'react-router-dom';
import { refreshProductConversationList } from '../conversation/productConversationList';
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
    regenerateProductConversationName: vi.fn(),
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
  presentation: { kind: 'state', display_name: overrides.canonical_root?.title ?? `Root ${id}`, presentation_mode: 'idle' },
  read_state: { state: 'read' },
  cached_pr: null,
  monitored: false,
  question_with_coordinator_since: null,
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
    apiMock.listProductConversations.mockReset();
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



  it('opens New without discarding the saved draft language or recovery identity', async () => {
    localStorage.setItem('phoenix-new-conversation-draft', 'Resume this draft');
    localStorage.setItem('phoenix-create-llm-language', 'adhd');
    sessionStorage.setItem('phoenix-active-product-create-request', 'request-pending');
    function NavigationProbe() {
      return <output data-testid="new-location">{useLocation().pathname}</output>;
    }
    const { getByRole, getByTestId } = render(
      <MemoryRouter initialEntries={['/c/existing']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]}
          activeSlug="existing" onConversationCreated={vi.fn()} />
        <NavigationProbe />
      </MemoryRouter>,
    );
    await waitFor(() => expect(apiMock.listProductConversations).toHaveBeenCalled());
    fireEvent.click(getByRole('button', { name: '+ New' }));
    expect(getByTestId('new-location')).toHaveTextContent('/new');
    expect(localStorage.getItem('phoenix-new-conversation-draft')).toBe('Resume this draft');
    expect(localStorage.getItem('phoenix-create-llm-language')).toBe('adhd');
    expect(sessionStorage.getItem('phoenix-active-product-create-request')).toBe('request-pending');
    sessionStorage.removeItem('phoenix-active-product-create-request');
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
      expect(container.querySelector('[aria-label="Open Open Root"]')).toHaveClass('active');
    });

    fireEvent.click(getByRole('button', { name: /Expand sidebar/ }));
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it('keeps an uncached empty list in loading state across route changes until the first snapshot succeeds', async () => {
    let resolveList!: (value: { product_conversations: ProductConversationListRow[]; coordinator: null }) => void;
    apiMock.listProductConversations.mockReturnValueOnce(new Promise((resolve) => { resolveList = resolve; }));
    function RouteHarness() {
      const { pathname } = useLocation();
      return <>
        <output aria-label="Current route">{pathname}</output>
        <Link to="/global">Global</Link>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug={pathname.startsWith('/c/') ? pathname.slice(3) : null} onConversationCreated={vi.fn()} />
      </>;
    }
    const { getByRole, queryByText } = render(
      <MemoryRouter initialEntries={['/c/some-conversation']}>
        <RouteHarness />
      </MemoryRouter>,
    );
    const signal = apiMock.listProductConversations.mock.calls[0]![0] as AbortSignal;

    expect(getByRole('button', { name: /Open …/ })).toBeInTheDocument();
    expect(getByRole('button', { name: /History …/ })).toBeInTheDocument();
    expect(getByRole('list', { name: '' })).toHaveTextContent('Loading conversations');
    expect(queryByText('No Open conversations')).toBeNull();
    fireEvent.click(getByRole('link', { name: 'Global' }));
    expect(getByRole('status', { name: 'Current route' })).toHaveTextContent('/global');
    expect(getByRole('button', { name: /Open …/ })).toBeInTheDocument();
    expect(getByRole('button', { name: /History …/ })).toBeInTheDocument();
    expect(getByRole('list', { name: '' })).toHaveTextContent('Loading conversations');
    expect(queryByText('No Open conversations')).toBeNull();
    expect(apiMock.listProductConversations).toHaveBeenCalledTimes(1);
    expect(signal.aborted).toBe(false);

    await act(async () => {
      resolveList({ product_conversations: [], coordinator: null });
    });
    expect(getByRole('button', { name: 'Open 0' })).toBeInTheDocument();
    expect(getByRole('list', { name: '' })).toHaveTextContent('No Open conversations');
  });

  it.each([
    { label: 'empty', rows: [] },
    { label: 'nonempty', rows: [makeProductConversation('pc-recovered')] },
  ])('shows an initial failure and retries to a successful $label snapshot', async ({ rows }) => {
    let resolveList!: (value: { product_conversations: ProductConversationListRow[]; coordinator: null }) => void;
    apiMock.listProductConversations.mockReset();
    apiMock.listProductConversations
      .mockRejectedValueOnce(new Error('offline'))
      .mockReturnValueOnce(new Promise((resolve) => { resolveList = resolve; }));
    const { container, getByRole, queryByText } = render(
      <MemoryRouter initialEntries={['/']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug={null} onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    expect(await waitFor(() => getByRole('status'))).toHaveTextContent('Unable to load conversations');
    expect(queryByText('No Open conversations')).toBeNull();
    fireEvent.click(getByRole('button', { name: 'Retry' }));
    expect(getByRole('list', { name: '' })).toHaveTextContent('Loading conversations');
    expect(queryByText('No Open conversations')).toBeNull();
    await act(async () => {
      resolveList({ product_conversations: rows, coordinator: null });
    });
    if (rows.length > 0) {
      expect(container.querySelector('[data-product-conversation-id="pc-recovered"]')).not.toBeNull();
    } else {
      expect(getByRole('list', { name: '' })).toHaveTextContent('No Open conversations');
    }
  });

  it('preserves cached-empty provenance and count when refresh fails', async () => {
    localStorage.setItem('phoenix:product-conversation-list:v1', JSON.stringify({ product_conversations: [], coordinator: null }));
    (globalThis as { __phoenixResetProductConversationListSnapshot?: () => void })
      .__phoenixResetProductConversationListSnapshot?.();
    apiMock.listProductConversations.mockRejectedValueOnce(new Error('offline'));
    const { getByRole } = render(
      <MemoryRouter initialEntries={['/']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug={null} onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    expect(await waitFor(() => getByRole('status'))).toHaveTextContent('Showing cached conversations');
    expect(getByRole('button', { name: 'Open 0' })).toBeInTheDocument();
    expect(getByRole('list', { name: '' })).toHaveTextContent('No Open conversations');
  });

  it('renders cached product rows when the refresh fails and recovers on retry', async () => {
    localStorage.setItem('phoenix:product-conversation-list:v1', JSON.stringify({
      product_conversations: [makeProductConversation('pc-cached')],
      coordinator: null,
    }));
    (globalThis as { __phoenixResetProductConversationListSnapshot?: () => void })
      .__phoenixResetProductConversationListSnapshot?.();
    apiMock.listProductConversations.mockReset();
    apiMock.listProductConversations
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({ product_conversations: [makeProductConversation('pc-recovered')], coordinator: null });

    const { container, getByRole } = render(
      <MemoryRouter initialEntries={['/c/root-pc-cached']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="root-pc-cached" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(getByRole('status')).toHaveTextContent('Showing cached conversations'));
    expect(container.querySelector('[data-product-conversation-id="pc-cached"]')).not.toBeNull();
    fireEvent.click(getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-recovered"]')).not.toBeNull());
    expect(apiMock.listProductConversations).toHaveBeenCalledTimes(2);
  });

  it.each(['manual', 'generated'] as const)('names product conversations through the canonical aggregate (%s)', async (operation) => {
    const row = makeProductConversation('pc-continued', {
      canonical_root: { transcript_row_id: 'canonical-root-row', slug: 'old-product', title: 'Old Product' },
      latest_transcript_row_id: 'latest-continuation-row',
    });
    apiMock.listProductConversations.mockResolvedValue({ product_conversations: [row] });
    apiMock.regenerateProductConversationName.mockResolvedValue({
      ...row, canonical_route: '/c/generated-name',
      canonical_root: { ...row.canonical_root, title: 'Generated Name', slug: 'generated-name' },
      presentation: { kind: 'state', display_name: 'Generated Name', presentation_mode: 'idle' },
      read_state: { state: 'read' },
      cached_pr: null,
      monitored: false,
      question_with_coordinator_since: null,
    });
    apiMock.renameProductConversation.mockResolvedValue({
      ...row,
      canonical_root: { ...row.canonical_root, title: 'Renamed Product Title' },
      presentation: { kind: 'state', display_name: 'Renamed Product Title', presentation_mode: 'idle' },
      read_state: { state: 'read' },
      cached_pr: null,
      monitored: false,
      question_with_coordinator_since: null,
    });
    const listRevisionListener = vi.fn();
    const unsubscribe = subscribeProductConversationListRevision(listRevisionListener);

    const { getByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-continued']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-continued" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-continued"]')).not.toBeNull());
    fireEvent.click(await waitFor(() => getByRole('button', { name: 'Actions for conversation Old Product' })));
    fireEvent.click(getByRole('button', { name: /Rename conversation Old Product/ }));
    const input = getByRole('textbox');
    expect(input).toHaveValue('Old Product');
    if (operation === 'manual') {
      fireEvent.change(input, { target: { value: 'Renamed Product Title' } });
      fireEvent.click(getByRole('button', { name: 'Rename' }));
      await waitFor(() => expect(apiMock.renameProductConversation).toHaveBeenCalledWith('pc-continued', 'Renamed Product Title'));
      expect(apiMock.regenerateProductConversationName).not.toHaveBeenCalled();
    } else {
      fireEvent.click(getByRole('button', { name: 'Generate with AI' }));
      await waitFor(() => expect(apiMock.regenerateProductConversationName).toHaveBeenCalledWith('pc-continued'));
      expect(apiMock.renameProductConversation).not.toHaveBeenCalled();
    }
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
    fireEvent.click(await waitFor(() => getByRole('button', { name: 'Actions for conversation Old Product' })));
    fireEvent.click(getByRole('button', { name: /Close conversation Old Product/ }));
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
    fireEvent.click(await waitFor(() => getByRole('button', { name: 'Actions for conversation Single Product' })));
    fireEvent.click(getByRole('button', { name: /Close conversation Single Product/ }));
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
    fireEvent.click(await waitFor(() => getByRole('button', { name: 'Actions for conversation Race Product' })));
    fireEvent.click(getByRole('button', { name: /Close conversation Race Product/ }));
    fireEvent.click(getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(apiMock.closeProductConversation).toHaveBeenCalledWith('pc-race'));
  });

  it('does not expose product conversation actions for history rows', async () => {
    apiMock.listProductConversations.mockResolvedValue({
      product_conversations: [makeProductConversation('pc-history', {
        lifecycle: { state: 'history' },
      })],
    });

    const { getByRole, queryByRole, container } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-history']}>
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-history" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );

    await waitFor(() => expect(container.querySelector('[data-product-conversation-id="pc-history"]')).not.toBeNull());
    fireEvent.click(getByRole('button', { name: /^Actions for conversation/ }));
    expect(queryByRole('button', { name: /Rename conversation/ })).toBeNull();
    expect(queryByRole('button', { name: /Close conversation/ })).toBeNull();
  });

  it('does not navigate back when a rename completes after leaving its route', async () => {
    const original = makeProductConversation('pc-rename', {
      canonical_root: { transcript_row_id: 'root-rename', slug: 'old-product', title: 'Old Product' },
    });
    const renamed = { ...original, canonical_route: '/c/new-product' };
    let finishRename!: (row: ProductConversationListRow) => void;
    apiMock.renameProductConversation.mockImplementationOnce(() => new Promise((resolve) => {
      finishRename = resolve;
    }));
    apiMock.listProductConversations.mockResolvedValue({ product_conversations: [original] });
    function NavigationProbe() {
      const location = useLocation();
      return <><Link to="/c/other?viewer=else#message-other">Leave conversation</Link>
        <output data-testid="current-location">{location.pathname}{location.search}{location.hash}</output></>;
    }
    const { getByRole, findByRole, getByTestId } = render(
      <MemoryRouter initialEntries={['/product-conversations/pc-rename']}>
        <NavigationProbe />
        <Sidebar collapsed={false} onToggle={vi.fn()} conversations={[]} archivedConversations={[]} activeSlug="pc-rename" onConversationCreated={vi.fn()} />
      </MemoryRouter>,
    );
    fireEvent.click(await findByRole('button', { name: 'Actions for conversation Old Product' }));
    fireEvent.click(getByRole('button', { name: /Rename conversation Old Product/ }));
    fireEvent.change(getByRole('textbox'), { target: { value: 'New Product' } });
    fireEvent.click(getByRole('button', { name: 'Rename' }));
    await waitFor(() => expect(apiMock.renameProductConversation).toHaveBeenCalled());
    fireEvent.click(getByRole('link', { name: 'Leave conversation' }));
    expect(getByTestId('current-location')).toHaveTextContent('/c/other?viewer=else#message-other');
    await act(async () => { finishRename(renamed); });
    expect(getByTestId('current-location')).toHaveTextContent('/c/other?viewer=else#message-other');
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

    fireEvent.click(await waitFor(() => getByRole('button', { name: 'Actions for conversation Old Product' })));
    fireEvent.click(await findByRole('button', { name: /Rename conversation Old Product/ }));
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
    fireEvent.click(await waitFor(() => getByRole('button', { name: 'Actions for conversation Old Product' })));
    fireEvent.click(getByRole('button', { name: /Rename conversation Old Product/ }));
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
    fireEvent.click(await waitFor(() => getByRole('button', { name: 'Actions for conversation Close Product' })));
    fireEvent.click(getByRole('button', { name: /Close conversation Close Product/ }));
    fireEvent.click(getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(apiMock.closeProductConversation).toHaveBeenCalledWith('pc-close-conflict'));
    await waitFor(() => expect(queryByRole('dialog', { name: 'Close Conversation' })).toBeNull());
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

    await act(async () => { await refreshProductConversationList(); });
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

  it('labels the global nav entry as Ember', async () => {
    const conversations = [makeConv('active-id', 'active-project-one')];

    const { getAllByLabelText, queryByLabelText } = render(
      <MemoryRouter initialEntries={['/global/coordinator-id']}>
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
      expect(getAllByLabelText('Open Ember').length).toBeGreaterThan(0);
    });
    expect(queryByLabelText('Global Recall')).toBeNull();
    expect(document.querySelector('.ember-entry.active')).toBeInTheDocument();
  });


});
