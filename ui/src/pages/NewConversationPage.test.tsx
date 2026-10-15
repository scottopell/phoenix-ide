import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { NewConversationPage } from './NewConversationPage';
import { recoveryDiscoveryDelay } from './recoveryPolling';
import { ConversationProvider } from '../conversation';
import type { ProductConversationCreationObservation } from '../api';
import { ApiResponseError } from '../api';

const { apiMock, creationStoreMock, creationStoreState } = vi.hoisted(() => ({
  apiMock: {
    listModels: vi.fn().mockResolvedValue({ models: [{ id: 'claude-3-5-sonnet' }], default: 'claude-3-5-sonnet', llm_configured: true }),
    getEnv: vi.fn().mockResolvedValue({ home_dir: '/home/user' }),
    listRecentManagementRootSuggestions: vi.fn().mockResolvedValue({ suggestions: [] }),
    getProjects: vi.fn().mockResolvedValue([]),
    validateCwd: vi.fn().mockResolvedValue({ valid: true }),
    listDirectory: vi.fn().mockResolvedValue({ entries: [] }),
    listConversations: vi.fn().mockResolvedValue([]),
    listArchivedConversations: vi.fn().mockResolvedValue([]),
    listProductConversationCreations: vi.fn().mockResolvedValue({ product_creations: [] }),
    getProductConversationCreation: vi.fn<(requestId: string, signal?: AbortSignal) => Promise<ProductConversationCreationObservation>>(),
    cancelProductConversationCreation: vi.fn().mockResolvedValue(undefined),
    retryProductConversationDelivery: vi.fn().mockResolvedValue(undefined),
    deleteProductConversationCreation: vi.fn().mockResolvedValue(undefined),
  },
  creationStoreMock: {
    activeProductCreationRequestId: vi.fn<() => string | null>(),
    readProductCreationRequest: vi.fn(),
    storeProductCreationRequest: vi.fn(),
    clearProductCreationRequest: vi.fn(),
  },
  creationStoreState: {
    activeRequestId: null as string | null,
    records: new Map<string, unknown>(),
  },
}));

vi.mock('../api', async () => ({
  ...await vi.importActual<typeof import('../api')>('../api'),
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

vi.mock('../productCreationStore', async () => ({
  ...await vi.importActual<typeof import('../productCreationStore')>('../productCreationStore'),
  ...creationStoreMock,
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

function seedAcceptedCreation(requestId: string, productConversationId: string): void {
  const stored = { version: 3, kind: 'accepted', requestId, productConversationId };
  creationStoreState.records.set(requestId, stored);
  creationStoreState.activeRequestId = requestId;
}

function storedCreation(): unknown {
  const requestId = creationStoreState.activeRequestId;
  return requestId ? creationStoreState.records.get(requestId) ?? null : null;
}

describe('NewConversationPage', () => {
  beforeEach(() => {
    vi.spyOn(console, 'error');
    localStorage.clear();
    localStorage.setItem('phoenix-last-cwd', '/home/user/projects');
    localStorage.setItem('phoenix-last-model', 'claude-3-5-sonnet');
    Object.values(apiMock).forEach((fn) => {
      if ('mockClear' in fn) fn.mockClear();
    });
    apiMock.listProductConversationCreations.mockResolvedValue({ product_creations: [] });
    creationStoreState.activeRequestId = null;
    creationStoreState.records.clear();
    creationStoreMock.activeProductCreationRequestId.mockReset().mockImplementation(() => creationStoreState.activeRequestId);
    creationStoreMock.readProductCreationRequest.mockReset().mockImplementation(async (requestId: string) =>
      creationStoreState.records.get(requestId) ?? null);
    creationStoreMock.storeProductCreationRequest.mockReset().mockImplementation(async (stored: { kind: string; request?: { request_id: string }; requestId?: string }) => {
      const requestId = stored.kind === 'uncertain' ? stored.request!.request_id : stored.requestId!;
      creationStoreState.records.set(requestId, stored);
      creationStoreState.activeRequestId = requestId;
    });
    creationStoreMock.clearProductCreationRequest.mockReset().mockImplementation(async (requestId: string) => {
      creationStoreState.records.delete(requestId);
      if (creationStoreState.activeRequestId === requestId) creationStoreState.activeRequestId = null;
    });
  });

  afterEach(() => {
    cleanup();
    const actWarnings = vi.mocked(console.error).mock.calls.filter(([message]) =>
      String(message).includes('not wrapped in act'));
    expect(actWarnings).toEqual([]);
    vi.restoreAllMocks();
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

  async function settleInitialPageWork() {
    await waitFor(() => {
      expect(apiMock.getEnv).toHaveBeenCalled();
      expect(apiMock.listRecentManagementRootSuggestions).toHaveBeenCalled();
      expect(apiMock.listDirectory).toHaveBeenCalled();
      expect(apiMock.validateCwd).toHaveBeenCalledWith('/home/user/projects');
      expect(apiMock.listProductConversationCreations).toHaveBeenCalled();
    });

    const latestResult = (mock: ReturnType<typeof vi.fn>) => mock.mock.results.at(-1)?.value;
    await act(async () => {
      await Promise.all([
        latestResult(apiMock.getEnv),
        latestResult(apiMock.listRecentManagementRootSuggestions),
        latestResult(apiMock.listDirectory),
        latestResult(apiMock.validateCwd),
        latestResult(apiMock.listProductConversationCreations),
      ]);
    });
    await waitFor(() => {
      expect(document.querySelectorAll('.status-ok').length).toBeGreaterThan(0);
      expect(Array.from(document.querySelectorAll<HTMLSelectElement>('.settings-select'))
        .every((select) => !select.disabled)).toBe(true);
    });
  }

  it('does not show checking status on initial render when cwd is saved', async () => {
    const { container } = renderPage();
    expect(container.querySelectorAll('.status-checking').length).toBe(0);
    await settleInitialPageWork();
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
    await settleInitialPageWork();

    expect(screen.getAllByText('Recent starts').length).toBeGreaterThan(0);
    fireEvent.click(screen.getAllByRole('button', { name: 'Cancel' }).at(0)!);
    await waitFor(() => expect(apiMock.cancelProductConversationCreation).toHaveBeenCalledWith('req-pending'));
    await waitFor(() => expect(screen.queryByText('pending objective')).not.toBeInTheDocument());
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

  it('resumes exact-request observation and pauses list discovery polling until it settles', async () => {
    vi.useFakeTimers();
    let resolveObservation!: (value: ProductConversationCreationObservation) => void;
    const observation = new Promise<ProductConversationCreationObservation>((resolve) => { resolveObservation = resolve; });
    apiMock.getProductConversationCreation.mockReturnValueOnce(observation);
    seedAcceptedCreation('resumed-request', 'resumed-product');
    const page = renderPage();
    try {
      await act(async () => {
        await Promise.all([
          apiMock.getEnv.mock.results.at(-1)?.value,
          apiMock.listProductConversationCreations.mock.results.at(-1)?.value,
        ]);
      });
      expect(apiMock.getProductConversationCreation).toHaveBeenCalledWith('resumed-request', expect.any(AbortSignal));
      expect(screen.getAllByText('Waiting for conversation publication…').length).toBeGreaterThan(0);
      const listReads = apiMock.listProductConversationCreations.mock.calls.length;
      await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
      expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(1);
      expect(apiMock.listProductConversationCreations).toHaveBeenCalledTimes(listReads);
      await act(async () => {
        resolveObservation({
          kind: 'failed', request_id: 'resumed-request', product_conversation_id: 'resumed-product', status: 'failed',
          provisioning_stage: 'resolve_starting_pin', stage_updated_at: '2026-01-01T00:00:00Z',
          error: 'Repository pinning failed', allowed_actions: ['delete', 'start_over'],
        });
        await observation;
      });
      expect(screen.getAllByText('Repository pinning failed').length).toBeGreaterThan(0);
    } finally {
      page.unmount();
      vi.useRealTimers();
    }
  });

  it.each(['failed', 'cancelled'] as const)('uses exact %s actions for retry/delete even when discovery is unavailable', async (kind) => {
    seedAcceptedCreation('exact-request', 'exact-product');
    const fields = {
      request_id: 'exact-request', product_conversation_id: 'exact-product',
      provisioning_stage: 'bootstrap_initial_turn', stage_updated_at: '2026-01-01T00:00:00Z',
      error: 'Exact terminal result',
    } as const;
    const observation: ProductConversationCreationObservation = kind === 'failed'
      ? { ...fields, kind, status: 'delivery_failed', allowed_actions: ['retry_delivery', 'delete'] }
      : { ...fields, kind, allowed_actions: ['delete'] };
    apiMock.getProductConversationCreation.mockResolvedValue(observation);
    apiMock.listProductConversationCreations.mockRejectedValue(new Error('Discovery unavailable'));
    renderPage();
    await screen.findAllByLabelText('Current creation result');
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Start over' })).not.toBeInTheDocument();
    if (kind === 'failed') {
      fireEvent.click(screen.getAllByRole('button', { name: 'Retry' })[0]!);
      await waitFor(() => expect(apiMock.retryProductConversationDelivery).toHaveBeenCalledExactlyOnceWith('exact-request'));
      await waitFor(() => expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(screen.getAllByRole('button', { name: 'Delete' })[0]).toBeEnabled());
    }
    apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Not found', 404));
    fireEvent.click(screen.getAllByRole('button', { name: 'Delete' })[0]!);
    await waitFor(() => expect(apiMock.deleteProductConversationCreation).toHaveBeenCalledExactlyOnceWith('exact-request'));
    await waitFor(() => expect(screen.queryByLabelText('Current creation result')).not.toBeInTheDocument());
    expect(storedCreation()).toBeNull();
  });

  it.each(['retry_delivery', 'cancel', 'delete'] as const)('resumes exact observation after a lost %s action response', async (action) => {
    seedAcceptedCreation('exact-request', 'exact-product');
    const fields = {
      request_id: 'exact-request', product_conversation_id: 'exact-product',
      provisioning_stage: 'bootstrap_initial_turn', stage_updated_at: '2026-01-01T00:00:00Z',
    } as const;
    const initial: ProductConversationCreationObservation = action === 'cancel'
      ? { ...fields, kind: 'pending', status: 'accepted', retry_at: null, allowed_actions: ['cancel'] }
      : { ...fields, kind: 'failed', status: 'delivery_failed', error: 'Delivery failed', allowed_actions: [action] };
    apiMock.getProductConversationCreation.mockResolvedValueOnce(initial);
    if (action === 'delete') {
      apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Deleted', 404));
    } else if (action === 'cancel') {
      apiMock.getProductConversationCreation.mockResolvedValueOnce({ ...fields, kind: 'cancelled', error: null, allowed_actions: ['delete'] });
    } else {
      apiMock.getProductConversationCreation.mockResolvedValueOnce({ ...fields, kind: 'pending', status: 'delivery_pending', retry_at: null, allowed_actions: ['delete'] });
    }
    const actionApi = action === 'retry_delivery' ? apiMock.retryProductConversationDelivery
      : action === 'cancel' ? apiMock.cancelProductConversationCreation : apiMock.deleteProductConversationCreation;
    actionApi.mockRejectedValueOnce(new TypeError('Action response lost'));
    apiMock.listProductConversationCreations.mockRejectedValue(new Error('Discovery unavailable'));
    renderPage();
    const label = action === 'retry_delivery' ? 'Retry' : action === 'cancel' ? 'Cancel' : 'Delete';
    fireEvent.click((await screen.findAllByRole('button', { name: label }))[0]!);
    await waitFor(() => expect(actionApi).toHaveBeenCalledExactlyOnceWith('exact-request'));
    await waitFor(() => expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(2));
    expect(apiMock.getProductConversationCreation.mock.calls.every(([id]) => id === 'exact-request')).toBe(true);
    if (action === 'delete') {
      await waitFor(() => expect(screen.queryByLabelText('Current creation result')).not.toBeInTheDocument());
      expect(storedCreation()).toBeNull();
      expect(screen.getAllByText('This conversation creation was removed from the server. You can start a new conversation.').length).toBeGreaterThan(0);
    } else {
      expect((await screen.findAllByText(action === 'cancel' ? 'Cancelled' : 'Finishing')).length).toBeGreaterThan(0);
      expect(storedCreation()).not.toBeNull();
    }
  });

  it('visibly disables unrelated Start over while exact creation is active', async () => {
    seedAcceptedCreation('active-request', 'active-product');
    apiMock.getProductConversationCreation.mockResolvedValue({
      kind: 'pending', request_id: 'active-request', product_conversation_id: 'active-product', status: 'claimed',
      provisioning_stage: 'resolve_repository', stage_updated_at: '2026-01-01T00:00:00Z', retry_at: null, allowed_actions: ['cancel'],
    });
    apiMock.listProductConversationCreations.mockResolvedValue({ product_creations: [{
      request_id: 'other-request', status: 'failed', cwd: '/other', objective: 'Other objective',
      model: 'claude-3-5-sonnet', effort: null, images: [], llm_language: 'English',
      updated_at: '2026-01-01T00:00:00Z', last_error: null, allowed_actions: ['start_over'],
    }] });
    renderPage();
    const buttons = await screen.findAllByRole('button', { name: 'Start over' });
    expect(buttons.every((button) => (button as HTMLButtonElement).disabled)).toBe(true);
    expect(buttons[0]).toHaveAttribute('title', 'Resolve the current creation before starting over from another attempt');
    fireEvent.click(buttons[0]!);
    expect(storedCreation()).toEqual(expect.objectContaining({ requestId: 'active-request' }));
    expect(screen.queryByDisplayValue('Other objective')).not.toBeInTheDocument();
  });

  it.each(['failed', 'cancelled'] as const)('offers a local release for an actionless exact %s result', async (kind) => {
    seedAcceptedCreation('exact-request', 'exact-product');
    localStorage.setItem('phoenix-new-conversation-draft', 'Retained objective');
    const fields = {
      request_id: 'exact-request', product_conversation_id: 'exact-product',
      provisioning_stage: 'resolve_repository', stage_updated_at: '2026-01-01T00:00:00Z', error: 'Terminal record', allowed_actions: [],
    } as const;
    apiMock.getProductConversationCreation.mockResolvedValue(kind === 'failed'
      ? { ...fields, kind, status: 'failed', allowed_actions: [] }
      : { ...fields, kind, allowed_actions: [] });
    renderPage();
    const forget = (await screen.findAllByRole('button', { name: 'Forget locally / New conversation' }))[0]!;
    expect(screen.getAllByRole('button', { name: 'Send' }).every((button) => (button as HTMLButtonElement).disabled)).toBe(true);
    fireEvent.click(forget);
    await waitFor(() => expect(screen.queryByLabelText('Current creation result')).not.toBeInTheDocument());
    expect(storedCreation()).toBeNull();
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeEnabled());
    expect(apiMock.deleteProductConversationCreation).not.toHaveBeenCalled();
    expect(apiMock.cancelProductConversationCreation).not.toHaveBeenCalled();
  });

  it('offers local release without deleting a cleanup-ambiguous server record', async () => {
    seedAcceptedCreation('exact-request', 'exact-product');
    apiMock.getProductConversationCreation.mockResolvedValue({
      kind: 'failed', request_id: 'exact-request', product_conversation_id: 'exact-product',
      status: 'cleanup_ambiguous', provisioning_stage: 'resolve_repository',
      stage_updated_at: '2026-01-01T00:00:00Z', error: 'Ownership remains ambiguous',
      allowed_actions: ['delete'],
    });
    renderPage();
    expect((await screen.findAllByRole('button', { name: 'Delete' })).length).toBeGreaterThan(0);
    const forget = (await screen.findAllByRole('button', { name: 'Forget locally / New conversation' }))[0]!;
    fireEvent.click(forget);
    await waitFor(() => expect(screen.queryByLabelText('Current creation result')).not.toBeInTheDocument());
    expect(storedCreation()).toBeNull();
    expect(apiMock.deleteProductConversationCreation).not.toHaveBeenCalled();
  });

  it('does not offer stale discovery actions instead of the exact terminal allowed_actions', async () => {
    seedAcceptedCreation('exact-request', 'exact-product');
    apiMock.getProductConversationCreation.mockResolvedValue({
      kind: 'cancelled', request_id: 'exact-request', product_conversation_id: 'exact-product',
      provisioning_stage: 'resolve_repository', stage_updated_at: '2026-01-01T00:00:00Z',
      error: null, allowed_actions: ['delete'],
    });
    apiMock.listProductConversationCreations.mockResolvedValue({ product_creations: [{
      request_id: 'exact-request', status: 'accepted', cwd: '/repo', objective: 'stale row',
      model: 'claude-3-5-sonnet', effort: null, images: [], llm_language: 'English',
      updated_at: '2026-01-01T00:00:00Z', last_error: null, allowed_actions: ['cancel', 'start_over'],
    }] });
    renderPage();
    await screen.findAllByLabelText('Current creation result');
    expect(screen.getAllByRole('button', { name: 'Delete' })).toHaveLength(2);
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Start over' })).not.toBeInTheDocument();
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
