import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { NewConversationPage } from './NewConversationPage';
import { recoveryDiscoveryDelay } from './recoveryPolling';
import { ConversationProvider } from '../conversation';

const { apiMock } = vi.hoisted(() => ({ apiMock: {
  listModels: vi.fn().mockResolvedValue({ models: [{ id: 'claude-3-5-sonnet' }], default: 'claude-3-5-sonnet', llm_configured: true }),
  getEnv: vi.fn().mockResolvedValue({ home_dir: '/home/user' }),
  listRecentManagementRootSuggestions: vi.fn().mockResolvedValue({ suggestions: [] }),
  getProjects: vi.fn().mockResolvedValue([]),
  validateCwd: vi.fn().mockResolvedValue({ valid: true }),
  listDirectory: vi.fn().mockResolvedValue({ entries: [] }),
  listConversations: vi.fn().mockResolvedValue([]),
  listArchivedConversations: vi.fn().mockResolvedValue([]),
  listProductConversationCreations: vi.fn().mockResolvedValue({ product_creations: [] }),
  cancelProductConversationCreation: vi.fn().mockResolvedValue(undefined),
  retryProductConversationDelivery: vi.fn().mockResolvedValue(undefined),
  deleteProductConversationCreation: vi.fn().mockResolvedValue(undefined),
} }));

vi.mock('../api', () => ({
  api: apiMock,
  ExpansionError: class ExpansionError extends Error {},
}));

vi.mock('../cache', () => ({
  cacheDB: {
    getAllConversations: vi.fn().mockResolvedValue([]),
    syncConversations: vi.fn().mockResolvedValue(undefined),
    putConversation: vi.fn().mockResolvedValue(undefined),
  },
}));

vi.mock('../hooks/useMediaQuery', () => ({ useIsDesktop: () => false }));
vi.mock('../hooks', async () => {
  const actual = await vi.importActual('../hooks');
  return {
    ...actual,
    useInlineReferences: () => ({
      dropdown: null,
      onValueChange: vi.fn(),
      onKeyDown: vi.fn(() => false),
      onSelectionChange: vi.fn(),
      reset: vi.fn(),
      expansionError: null,
      setExpansionError: vi.fn(),
      skillArgumentHint: null,
    }),
  };
});

describe('NewConversationPage', () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem('phoenix-last-cwd', '/home/user/projects');
    localStorage.setItem('phoenix-last-model', 'claude-3-5-sonnet');
    Object.values(apiMock).forEach((fn) => {
      if ('mockClear' in fn) fn.mockClear();
    });
    apiMock.listProductConversationCreations.mockResolvedValue({ product_creations: [] });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  function renderPage() {
    return render(
      <MemoryRouter>
        <ConversationProvider>
          <NewConversationPage />
        </ConversationProvider>
      </MemoryRouter>,
    );
  }

  it('does not show checking status on initial render when cwd is saved', () => {
    const { container } = renderPage();
    expect(container.querySelectorAll('.status-checking').length).toBe(0);
  });

  it('renders recovery actions and wires retry/cancel/delete', async () => {
    apiMock.listProductConversationCreations
      .mockResolvedValueOnce({
        product_creations: [
          { request_id: 'req-pending', status: 'accepted', cwd: '/repo/pending', objective: 'pending objective', model: 'claude', effort: null, images: [], llm_language: 'English', updated_at: '2025-01-01T00:00:00Z', last_error: null, allowed_actions: ['cancel', 'delete'] },
          { request_id: 'req-delivery', status: 'delivery_failed', cwd: '/repo/delivery', objective: 'delivery objective', model: 'claude', effort: 'high', images: [], llm_language: 'English', updated_at: '2025-01-01T00:00:00Z', last_error: null, allowed_actions: ['retry_delivery'] },
          { request_id: 'req-failed', status: 'failed', cwd: '/repo/failed', objective: 'failed objective', model: 'claude', effort: null, images: [], llm_language: 'English', updated_at: '2025-01-01T00:00:00Z', last_error: 'boom', allowed_actions: ['delete', 'start_over'] },
        ],
      })
      .mockResolvedValue({ product_creations: [] });

    renderPage();

    expect((await screen.findAllByText('Recent starts')).length).toBeGreaterThan(0);
    fireEvent.click(screen.getAllByRole('button', { name: 'Cancel' }).at(0)!);
    await waitFor(() => expect(apiMock.cancelProductConversationCreation).toHaveBeenCalledWith('req-pending'));

    apiMock.listProductConversationCreations.mockResolvedValueOnce({ product_creations: [] });
    renderPage();
  });

  it('shows a failed recovery action inline and refreshes recovery rows', async () => {
    const row = {
      request_id: 'req-failed-action', status: 'delivery_failed', cwd: '/repo/delivery',
      objective: 'delivery objective', model: 'claude', effort: null, images: [],
      llm_language: 'English', updated_at: '2025-01-01T00:00:00Z', last_error: 'delivery failed',
      allowed_actions: ['retry_delivery'], published_product_conversation_id: 'product-1',
    };
    apiMock.listProductConversationCreations.mockResolvedValue({ product_creations: [row] });
    apiMock.retryProductConversationDelivery.mockRejectedValueOnce(new Error('retry endpoint unavailable'));

    renderPage();
    fireEvent.click((await screen.findAllByRole('button', { name: 'Retry' })).at(0)!);

    expect((await screen.findAllByRole('alert')).some((alert) =>
      alert.textContent === 'retry endpoint unavailable')).toBe(true);
    await waitFor(() => expect(apiMock.listProductConversationCreations.mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it('retains active recovery rows when a poll fails transiently', async () => {
    apiMock.listProductConversationCreations
      .mockResolvedValueOnce({
        product_creations: [{
          request_id: 'req-active', status: 'claimed', cwd: '/repo/a', objective: 'still running',
          model: null, effort: null, images: [], updated_at: '2026-01-01T00:00:00Z', last_error: null,
          allowed_actions: ['cancel'],
        }],
      })
      .mockRejectedValueOnce(new Error('transient'));

    renderPage();
    expect((await screen.findAllByText('still running')).length).toBeGreaterThan(0);

    await waitFor(() => expect(apiMock.listProductConversationCreations.mock.calls.length).toBeGreaterThan(1), { timeout: 3000 });
    expect(screen.getAllByText('still running').length).toBeGreaterThan(0);
  });

  it('keeps loaded pages visible after a full refresh and labels cancelling', async () => {
    apiMock.listProductConversationCreations
      .mockResolvedValueOnce({ product_creations: [{ request_id: 'one', status: 'cancelling', cwd: '/one', objective: 'first page', model: null, effort: null, images: [], llm_language: 'English', updated_at: '2026-01-01T00:00:00Z', last_error: null, allowed_actions: ['delete'], published_product_conversation_id: null }], next_cursor: "cursor-50" })
      .mockResolvedValueOnce({ product_creations: [{ request_id: 'two', status: 'failed', cwd: '/two', objective: 'second page', model: null, effort: null, images: [], llm_language: 'English', updated_at: '2025-01-01T00:00:00Z', last_error: null, allowed_actions: ['delete', 'start_over'], published_product_conversation_id: null }], next_cursor: null })
      .mockResolvedValueOnce({ product_creations: [{ request_id: 'one', status: 'cancelling', cwd: '/one', objective: 'first page', model: null, effort: null, images: [], llm_language: 'English', updated_at: '2026-01-01T00:00:00Z', last_error: null, allowed_actions: ['delete'], published_product_conversation_id: null }], next_cursor: "cursor-50" })
      .mockResolvedValueOnce({ product_creations: [{ request_id: 'two', status: 'failed', cwd: '/two', objective: 'second page', model: null, effort: null, images: [], llm_language: 'English', updated_at: '2025-01-01T00:00:00Z', last_error: null, allowed_actions: ['delete', 'start_over'], published_product_conversation_id: null }], next_cursor: null });

    renderPage();
    expect((await screen.findAllByText('Cancelling')).length).toBeGreaterThan(0);
    fireEvent.click(screen.getAllByRole('button', { name: 'Load more' }).at(0)!);
    expect((await screen.findAllByText('second page')).length).toBeGreaterThan(0);
    fireEvent.click(screen.getAllByRole('button', { name: 'Delete' }).at(0)!);
    await waitFor(() => expect(apiMock.listProductConversationCreations).toHaveBeenCalledWith("cursor-50"));
    expect(screen.getAllByText('first page').length).toBeGreaterThan(0);
    expect(screen.getAllByText('second page').length).toBeGreaterThan(0);
  });

  it('does not let background polling supersede a slow load-more request', async () => {
    const realSetTimeout = window.setTimeout.bind(window);
    let poll: TimerHandler | undefined;
    const setTimeout = vi.spyOn(window, 'setTimeout').mockImplementation((handler, delay, ...args) => {
      if (Number(delay) >= 2000) {
        poll = handler;
        return {} as ReturnType<typeof setTimeout>;
      }
      return realSetTimeout(handler, delay, ...args) as unknown as ReturnType<typeof setTimeout>;
    });
    let resolveSecondPage: ((value: { product_creations: Array<Record<string, unknown>>; next_cursor: null }) => void) | undefined;
    const secondPage = new Promise<{ product_creations: Array<Record<string, unknown>>; next_cursor: null }>((resolve) => {
      resolveSecondPage = resolve;
    });
    apiMock.listProductConversationCreations
      .mockResolvedValueOnce({
        product_creations: [{ request_id: 'one', status: 'failed', cwd: '/one', objective: 'first page', model: null, effort: null, images: [], llm_language: 'English', updated_at: '2026-01-01T00:00:00Z', last_error: null, allowed_actions: ['delete', 'start_over'], published_product_conversation_id: null }],
        next_cursor: 'cursor-50',
      })
      .mockReturnValueOnce(secondPage)
      .mockResolvedValue({ product_creations: [], next_cursor: null });

    renderPage();
    fireEvent.click((await screen.findAllByRole('button', { name: 'Load more' })).at(0)!);
    await waitFor(() => expect(apiMock.listProductConversationCreations).toHaveBeenCalledTimes(2));
    expect(typeof poll).toBe('function');
    if (typeof poll === 'function') poll();
    expect(apiMock.listProductConversationCreations).toHaveBeenCalledTimes(2);

    resolveSecondPage?.({
      product_creations: [{ request_id: 'two', status: 'failed', cwd: '/two', objective: 'slow second page', model: null, effort: null, images: [], llm_language: 'English', updated_at: '2025-01-01T00:00:00Z', last_error: null, allowed_actions: ['delete', 'start_over'], published_product_conversation_id: null }],
      next_cursor: null,
    });
    expect((await screen.findAllByText('slow second page')).length).toBeGreaterThan(0);
    setTimeout.mockRestore();
  });

  it('backs off discovery after projecting only terminal recovery rows', async () => {
    const realSetTimeout = window.setTimeout.bind(window);
    const delays: number[] = [];
    const callbacks: TimerHandler[] = [];
    const setTimeout = vi.spyOn(window, 'setTimeout').mockImplementation((handler, delay, ...args) => {
      if (Number(delay) >= 2000) {
        callbacks.push(handler);
        delays.push(Number(delay));
        return {} as ReturnType<typeof setTimeout>;
      }
      return realSetTimeout(handler, delay, ...args) as unknown as ReturnType<typeof setTimeout>;
    });
    apiMock.listProductConversationCreations.mockResolvedValue({
      product_creations: [{ request_id: 'done', status: 'failed', cwd: '/done', objective: 'terminal payload', model: null, effort: null, images: [{ media_type: 'image/png', data: 'large' }], llm_language: 'English', updated_at: '2026-01-01T00:00:00Z', last_error: 'done', allowed_actions: ['delete', 'start_over'], published_product_conversation_id: null }],
      next_cursor: null,
    });

    renderPage();
    expect((await screen.findAllByText('terminal payload')).length).toBeGreaterThan(0);
    const discoveryIndex = delays.indexOf(2000);
    expect(discoveryIndex).toBeGreaterThanOrEqual(0);
    const initialFiveSecondTimers = delays.filter((delay) => delay === 5000).length;
    const firstDiscovery = callbacks[discoveryIndex];
    if (typeof firstDiscovery === 'function') firstDiscovery();
    await waitFor(() => expect(apiMock.listProductConversationCreations).toHaveBeenCalledTimes(2));
    expect(delays.filter((delay) => delay === 2000)).toHaveLength(1);
    expect(delays.filter((delay) => delay === 5000)).toHaveLength(initialFiveSecondTimers + 1);

    setTimeout.mockRestore();
  });

  it('continues recovery discovery at the capped backoff after extended outages', () => {
    expect([0, 1, 2, 3, 4, 5].map((attempt) => recoveryDiscoveryDelay(attempt, false)))
      .toEqual([2000, 5000, 10000, 30000, 30000, 30000]);
    expect(recoveryDiscoveryDelay(20, true)).toBe(2000);
  });

  it('backs off terminal cleanup conflicts instead of polling continuously', async () => {
    const realSetTimeout = window.setTimeout.bind(window);
    const delays: number[] = [];
    const callbacks: TimerHandler[] = [];
    const setTimeout = vi.spyOn(window, 'setTimeout').mockImplementation((handler, delay, ...args) => {
      if (Number(delay) >= 2000) {
        callbacks.push(handler);
        delays.push(Number(delay));
        return {} as ReturnType<typeof setTimeout>;
      }
      return realSetTimeout(handler, delay, ...args) as unknown as ReturnType<typeof setTimeout>;
    });
    apiMock.listProductConversationCreations.mockResolvedValue({
      product_creations: [{ request_id: 'conflict', status: 'cleanup_ambiguous', cwd: '/conflict', objective: 'terminal conflict', model: null, effort: null, images: [], llm_language: 'English', updated_at: '2026-01-01T00:00:00Z', last_error: 'occupied', allowed_actions: ['delete'], published_product_conversation_id: null }],
      next_cursor: null,
    });

    renderPage();
    expect((await screen.findAllByText('terminal conflict')).length).toBeGreaterThan(0);
    const discoveryIndex = delays.indexOf(2000);
    expect(discoveryIndex).toBeGreaterThanOrEqual(0);
    const initialFiveSecondTimers = delays.filter((delay) => delay === 5000).length;
    const firstDiscovery = callbacks[discoveryIndex];
    if (typeof firstDiscovery === 'function') firstDiscovery();
    await waitFor(() => expect(apiMock.listProductConversationCreations).toHaveBeenCalledTimes(2));
    expect(delays.filter((delay) => delay === 2000)).toHaveLength(1);
    expect(delays.filter((delay) => delay === 5000)).toHaveLength(initialFiveSecondTimers + 1);

    setTimeout.mockRestore();
  });

  it('start over prefills the composer from a recovery row', async () => {
    apiMock.listProductConversationCreations.mockResolvedValueOnce({
      product_creations: [
        { request_id: 'req-failed', status: 'failed', cwd: '/repo/failed', objective: 'redo this', model: 'claude-3-5-sonnet', effort: 'high', updated_at: '2025-01-01T00:00:00Z', last_error: 'boom', allowed_actions: ['delete', 'start_over'] },
      ],
    });

    renderPage();
    fireEvent.click((await screen.findAllByRole('button', { name: 'Start over' })).at(0)!);
    await waitFor(() => expect(screen.getAllByDisplayValue('redo this').length).toBeGreaterThan(0));
  });
});
