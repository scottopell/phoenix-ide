import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CreateProductConversationRequest, CreateProductConversationResponse, ProductConversationCreationObservation } from '../api';
import { ApiResponseError } from '../api';
import { useCreateConversation } from './useCreateConversation';

const { apiMock, generateUUIDMock, creationStoreMock, creationStoreState } = vi.hoisted(() => ({
  apiMock: {
    getEnv: vi.fn().mockResolvedValue({ home_dir: '/home/user' }),
    listRecentManagementRootSuggestions: vi.fn().mockResolvedValue({ suggestions: [] }),
    createProductConversation: vi.fn<(request: CreateProductConversationRequest) => Promise<CreateProductConversationResponse>>(),
    getProductConversationCreation: vi.fn<(requestId: string, signal?: AbortSignal) => Promise<ProductConversationCreationObservation>>(),
  },
  generateUUIDMock: vi.fn(() => 'request-1'),
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
}));
vi.mock('../modelsPoller', () => ({ subscribeModels: () => () => {} }));
vi.mock('../utils/uuid', () => ({ generateUUID: generateUUIDMock }));
vi.mock('../productCreationStore', async () => ({
  ...await vi.importActual<typeof import('../productCreationStore')>('../productCreationStore'),
  ...creationStoreMock,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const stageFields = {
  product_conversation_id: 'product-1',
  provisioning_stage: 'materialize_worktree',
  stage_updated_at: '2026-01-01T00:00:00Z',
} as const;
const pending: ProductConversationCreationObservation = {
  ...stageFields,
  kind: 'pending', request_id: 'request-1', status: 'retry_scheduled',
  retry_at: '2026-01-01T00:00:02Z',
  allowed_actions: ['cancel', 'delete'],
};
const failed: ProductConversationCreationObservation = {
  ...stageFields,
  kind: 'failed', request_id: 'request-1', status: 'delivery_failed',
  error: 'Delivery failed', allowed_actions: ['retry_delivery', 'delete'],
};
const cancelled: ProductConversationCreationObservation = {
  ...stageFields,
  kind: 'cancelled', request_id: 'request-1',
  error: null, allowed_actions: ['start_over', 'delete'],
};
const published: ProductConversationCreationObservation = {
  kind: 'published', request_id: 'request-1', product_conversation_id: 'product-1',
  transcript_row_id: 'transcript-1', canonical_route: '/c/canonical-product-1',
};
function storedCreation(): Record<string, unknown> | null {
  const activeRequestId = creationStoreState.activeRequestId;
  const stored = activeRequestId ? creationStoreState.records.get(activeRequestId) : null;
  return stored && typeof stored === 'object' ? stored as Record<string, unknown> : null;
}

function seedStoredCreation(requestId: string, value: unknown): void {
  creationStoreState.activeRequestId = requestId;
  creationStoreState.records.set(requestId, value);
}

function mountCreation() {
  const navigate = vi.fn();
  return { ...renderHook(() => useCreateConversation(navigate)), navigate };
}

async function prepareCreation(hook: ReturnType<typeof mountCreation>) {
  await act(async () => {
    await apiMock.getEnv.mock.results.at(-1)?.value;
    await apiMock.listRecentManagementRootSuggestions.mock.results.at(-1)?.value;
    hook.result.current.setDirStatus('exists');
    hook.result.current.setDraft('Create the original objective');
  });
}

describe('exact-request creation observation', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    localStorage.setItem('phoenix-last-cwd', '/repo');
    localStorage.setItem('phoenix-last-model', 'model-1');
    apiMock.createProductConversation.mockReset().mockResolvedValue({ request_id: 'request-1', product_conversation_id: 'product-1' });
    apiMock.getProductConversationCreation.mockReset();
    generateUUIDMock.mockReset().mockReturnValue('request-1');
    creationStoreState.activeRequestId = null;
    creationStoreState.records.clear();
    creationStoreMock.activeProductCreationRequestId.mockReset().mockImplementation(() => creationStoreState.activeRequestId);
    creationStoreMock.readProductCreationRequest.mockReset().mockImplementation(async (requestId: string) =>
      creationStoreState.records.get(requestId) ?? null);
    creationStoreMock.storeProductCreationRequest.mockReset().mockImplementation(async (stored: { kind: string; request?: { request_id: string }; requestId?: string }) => {
      const storedRequestId = stored.kind === 'uncertain' ? stored.request!.request_id : stored.requestId!;
      creationStoreState.records.set(storedRequestId, stored);
      creationStoreState.activeRequestId = storedRequestId;
    });
    creationStoreMock.clearProductCreationRequest.mockReset().mockImplementation(async (requestId: string) => {
      creationStoreState.records.delete(requestId);
      if (creationStoreState.activeRequestId === requestId) creationStoreState.activeRequestId = null;
    });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('settles submit on acceptance, observes pending/retry serially, and navigates only on exact publication', async () => {
    const publication = deferred<ProductConversationCreationObservation>();
    apiMock.getProductConversationCreation.mockResolvedValueOnce(pending).mockReturnValueOnce(publication.promise);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });

    expect(hook.navigate).not.toHaveBeenCalled();
    expect(hook.result.current.creationObservation).toEqual(pending);
    expect(hook.result.current.draft).toBe('Create the original objective');
    expect(hook.result.current.creating).toBe(true);
    expect(hook.result.current.canSend).toBe(false);
    expect(storedCreation()).toEqual({ version: 3, kind: 'accepted', requestId: 'request-1', productConversationId: 'product-1' });

    act(() => {
      hook.result.current.setCwd('/another-repo');
      hook.result.current.setDraft('Changed after acceptance');
    });
    await act(async () => { await hook.result.current.handleSend(); });
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(1);
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);

    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(10000); });
    expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(2);
    expect(hook.navigate).not.toHaveBeenCalled();

    await act(async () => { publication.resolve(published); await publication.promise; });
    expect(hook.navigate).toHaveBeenCalledExactlyOnceWith('/c/canonical-product-1');
    expect(hook.result.current.draft).toBe('');
    expect(storedCreation()).toBeNull();
    expect(apiMock.getProductConversationCreation.mock.calls.every(([id]) => id === 'request-1')).toBe(true);
  });

  it.each([failed, cancelled])('surfaces $kind without navigation, replay, or allocating another identity', async (terminal) => {
    apiMock.getProductConversationCreation.mockResolvedValue(terminal);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });

    expect(hook.result.current.creating).toBe(false);
    expect(hook.result.current.canSend).toBe(false);
    expect(hook.result.current.error).toBe(terminal.error ?? 'Conversation creation cancelled.');
    expect(hook.result.current.creationObservation).toEqual(terminal);
    expect(hook.result.current.draft).toBe('Create the original objective');
    await act(async () => {
      await hook.result.current.handleSend();
      await vi.advanceTimersByTimeAsync(10000);
    });
    expect(hook.navigate).not.toHaveBeenCalled();
    expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(1);
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(1);
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
    expect(storedCreation()).not.toBeNull();
  });

  it('resumes the persisted exact request after reload without another POST', async () => {
    const firstRead = deferred<ProductConversationCreationObservation>();
    const resumedRead = deferred<ProductConversationCreationObservation>();
    apiMock.getProductConversationCreation.mockReturnValueOnce(firstRead.promise).mockReturnValueOnce(resumedRead.promise);
    const original = mountCreation();
    await prepareCreation(original);
    await act(async () => { await original.result.current.handleSend(); });
    const originalSignal = apiMock.getProductConversationCreation.mock.calls[0]?.[1];
    original.unmount();
    expect(originalSignal?.aborted).toBe(true);

    const resumed = mountCreation();
    await act(async () => {
      firstRead.resolve(published);
      await firstRead.promise;
    });
    expect(original.navigate).not.toHaveBeenCalled();
    expect(resumed.navigate).not.toHaveBeenCalled();
    expect(resumed.result.current.canSend).toBe(false);
    await act(async () => { resumedRead.resolve(published); await resumedRead.promise; });
    expect(resumed.navigate).toHaveBeenCalledExactlyOnceWith('/c/canonical-product-1');
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(1);
    expect(apiMock.getProductConversationCreation).toHaveBeenNthCalledWith(2, 'request-1', expect.any(AbortSignal));
    expect(storedCreation()).toBeNull();
  });

  it('persists the exact uncertain intent before POST, then binds accepted identity on 202', async () => {
    const acceptance = deferred<CreateProductConversationResponse>();
    let storedAtPost: unknown;
    apiMock.createProductConversation.mockImplementationOnce(() => {
      storedAtPost = storedCreation();
      return acceptance.promise;
    });
    apiMock.getProductConversationCreation.mockResolvedValue(pending);
    const hook = mountCreation();
    await prepareCreation(hook);
    let submit!: Promise<void>;
    await act(async () => {
      submit = hook.result.current.handleSend();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(storedAtPost).toEqual({
      version: 3, kind: 'uncertain', request: apiMock.createProductConversation.mock.calls[0]![0],
    });
    expect(storedCreation()).toEqual({
      version: 3, kind: 'uncertain', request: apiMock.createProductConversation.mock.calls[0]![0],
    });
    expect(creationStoreState.activeRequestId).toBe('request-1');
    expect(apiMock.getProductConversationCreation).not.toHaveBeenCalled();
    await act(async () => {
      acceptance.resolve({ request_id: 'request-1', product_conversation_id: 'product-1' });
      await submit;
    });
    expect(storedCreation()).toEqual({
      version: 3, kind: 'accepted', requestId: 'request-1', productConversationId: 'product-1',
    });
  });

  it('recovers the complete uncertain intent after unmount before acceptance, ignoring late old callbacks', async () => {
    const acceptance = deferred<CreateProductConversationResponse>();
    apiMock.createProductConversation.mockReturnValueOnce(acceptance.promise);
    const original = mountCreation();
    await prepareCreation(original);
    let submit!: Promise<void>;
    await act(async () => {
      submit = original.result.current.handleSend();
      await Promise.resolve();
      await Promise.resolve();
    });
    const storedRequest = apiMock.createProductConversation.mock.calls[0]![0];
    original.unmount();
    expect(storedCreation()).toEqual({ version: 3, kind: 'uncertain', request: storedRequest });
    apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Not yet found', 404)).mockResolvedValue(pending);
    const resumed = mountCreation();
    await act(async () => {
      acceptance.resolve({ request_id: 'request-1', product_conversation_id: 'product-1' });
      await submit;
    });
    expect(original.navigate).not.toHaveBeenCalled();
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(2);
    expect(apiMock.createProductConversation).toHaveBeenNthCalledWith(2, storedRequest);
    expect(resumed.result.current.creationObservation).toEqual(pending);
    expect(resumed.result.current.canSend).toBe(false);
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
  });

  it('replays a reloaded uncertain request with images, effort and pinned language instead of current composer settings', async () => {
    const storedRequest: CreateProductConversationRequest = {
      request_id: 'request-1', cwd: '/original', objective: 'Original objective', model: 'original-model',
      effort: 'high', llm_language: 'Japanese', images: [{ data: 'aW1hZ2U=', media_type: 'image/png' }],
    };
    seedStoredCreation('request-1', { version: 3, kind: 'uncertain', request: storedRequest });
    apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Not yet found', 404)).mockResolvedValue(pending);
    const hook = mountCreation();
    await act(async () => {});
    expect(apiMock.createProductConversation).toHaveBeenCalledExactlyOnceWith(storedRequest);
    expect(hook.result.current.creationObservation).toEqual(pending);
    expect(generateUUIDMock).not.toHaveBeenCalled();
  });

  it.each([500, 503])('keeps HTTP %s after commit ambiguous and observes publication without a new POST', async (status) => {
    const committed = deferred<ProductConversationCreationObservation>();
    apiMock.createProductConversation.mockRejectedValueOnce(new ApiResponseError('Response failed after commit', status));
    apiMock.getProductConversationCreation.mockReturnValueOnce(committed.promise);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.result.current.canSend).toBe(false);
    expect(hook.result.current.creating).toBe(true);
    expect(storedCreation()?.['kind']).toBe('uncertain');
    await act(async () => { committed.resolve(published); await committed.promise; });
    expect(hook.navigate).toHaveBeenCalledExactlyOnceWith('/c/canonical-product-1');
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(1);
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
    expect(storedCreation()).toBeNull();
  });

  it('keeps a replay 500 ambiguous, observing the same request until its committed row is found', async () => {
    apiMock.createProductConversation.mockRejectedValueOnce(new TypeError('Lost response'))
      .mockRejectedValueOnce(new ApiResponseError('Replay response failed', 500));
    apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Not yet found', 404))
      .mockResolvedValue(pending);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(2);
    expect(apiMock.createProductConversation.mock.calls[1]![0]).toEqual(apiMock.createProductConversation.mock.calls[0]![0]);
    expect(hook.result.current.canSend).toBe(false);
    expect(storedCreation()?.['kind']).toBe('uncertain');
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(hook.result.current.creationObservation).toEqual(pending);
    expect(storedCreation()?.['kind']).toBe('accepted');
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
  });

  it.each([408, 425, 429])('keeps a transient replay %i bound to the original request', async (status) => {
    apiMock.createProductConversation.mockRejectedValueOnce(new TypeError('Lost response'))
      .mockRejectedValueOnce(new ApiResponseError('Replay remains ambiguous', status));
    apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Not yet found', 404))
      .mockResolvedValue(published);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(2);
    expect(apiMock.createProductConversation.mock.calls[1]![0]).toEqual(apiMock.createProductConversation.mock.calls[0]![0]);
    expect(hook.result.current.canSend).toBe(false);
    expect(storedCreation()?.['kind']).toBe('uncertain');
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(hook.navigate).toHaveBeenCalledExactlyOnceWith('/c/canonical-product-1');
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
  });

  it('retries exact release after a recovery-storage delete abort', async () => {
    seedStoredCreation('request-1', {
      version: 3, kind: 'accepted', requestId: 'request-1', productConversationId: 'product-1',
    });
    creationStoreMock.clearProductCreationRequest.mockRejectedValueOnce(new Error('IndexedDB abort'));
    apiMock.getProductConversationCreation.mockRejectedValue(new ApiResponseError('Removed', 404));
    const hook = mountCreation();
    await act(async () => {});
    expect(hook.result.current.creating).toBe(true);
    expect(hook.result.current.canSend).toBe(false);
    expect(hook.result.current.error).toContain('exact request remains active');
    expect(storedCreation()).not.toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(storedCreation()).toBeNull();
    expect(hook.result.current.creating).toBe(false);
    expect(hook.result.current.canSend).toBe(true);
    expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(2);
  });

  it.each(['initial', 'replay'])('releases and rotates a conflicting %s POST identity instead of looping on 409', async (phase) => {
    generateUUIDMock.mockReturnValueOnce('request-1').mockReturnValue('request-2');
    if (phase === 'replay') {
      apiMock.createProductConversation.mockRejectedValueOnce(new TypeError('Lost response'));
      apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Not yet found', 404));
    }
    apiMock.createProductConversation.mockRejectedValueOnce(new ApiResponseError('Intent conflict', 409))
      .mockResolvedValue({ request_id: 'request-2', product_conversation_id: 'product-2' });
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.result.current.canSend).toBe(true);
    expect(hook.result.current.error).toContain('new request ID');
    expect(storedCreation()).toBeNull();
    const callsBefore = apiMock.createProductConversation.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(10000); });
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(callsBefore);
    apiMock.getProductConversationCreation.mockResolvedValue({ ...pending, request_id: 'request-2', product_conversation_id: 'product-2' });
    await act(async () => { await hook.result.current.handleSend(); });
    expect(apiMock.createProductConversation.mock.calls.at(-1)![0]).toEqual({
      ...apiMock.createProductConversation.mock.calls[0]![0], request_id: 'request-2',
    });
    expect(hook.result.current.creationObservation?.request_id).toBe('request-2');
    expect(generateUUIDMock).toHaveBeenCalledTimes(2);
  });

  it('does not POST when uncertain intent cannot be persisted for reload recovery', async () => {
    const hook = mountCreation();
    await prepareCreation(hook);
    creationStoreMock.storeProductCreationRequest.mockRejectedValueOnce(new Error('Recovery storage unavailable'));
    await act(async () => { await hook.result.current.handleSend(); });
    expect(apiMock.createProductConversation).not.toHaveBeenCalled();
    expect(hook.result.current.canSend).toBe(true);
    expect(hook.result.current.error).toContain('Recovery storage unavailable');
  });

  it('persists an attachment larger than localStorage before submitting it', async () => {
    const hook = mountCreation();
    await prepareCreation(hook);
    const image = { data: 'a'.repeat(5_500_000), media_type: 'image/png' };
    act(() => hook.result.current.startOverFromCreation('unused-request', {
      cwd: '/repo', objective: 'Create with image', model: 'model-1', effort: null,
      images: [image], llm_language: null,
    }));
    await act(async () => {});
    await act(async () => { await hook.result.current.handleSend(); });
    expect(creationStoreMock.storeProductCreationRequest).toHaveBeenCalledWith(expect.objectContaining({
      kind: 'uncertain', request: expect.objectContaining({ images: [image] }),
    }));
    expect(apiMock.createProductConversation).toHaveBeenCalledOnce();
  });

  it('clears only its exact request when another flow owns the recovery pointer', async () => {
    const publication = deferred<ProductConversationCreationObservation>();
    apiMock.getProductConversationCreation.mockReturnValueOnce(publication.promise);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    seedStoredCreation('request-2', {
      version: 3, kind: 'accepted', requestId: 'request-2', productConversationId: 'product-2',
    });
    await act(async () => { publication.resolve(published); await publication.promise; });
    expect(creationStoreMock.clearProductCreationRequest).toHaveBeenCalledWith('request-1');
    expect(creationStoreState.activeRequestId).toBe('request-2');
    expect(storedCreation()?.['productConversationId']).toBe('product-2');
  });

  it('retries observation errors without re-submitting or overlapping reads', async () => {
    apiMock.getProductConversationCreation.mockRejectedValueOnce(new Error('network unavailable')).mockResolvedValueOnce(published);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.result.current.error).toContain('checking again');
    expect(hook.navigate).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(hook.navigate).toHaveBeenCalledExactlyOnceWith('/c/canonical-product-1');
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(1);
  });

  it('observes a delivery retry on the same request without a creation replay', async () => {
    apiMock.getProductConversationCreation
      .mockResolvedValueOnce(failed)
      .mockResolvedValueOnce(published);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.navigate).not.toHaveBeenCalled();
    await act(async () => { hook.result.current.resumeCreationObservation('request-1'); });
    expect(hook.navigate).toHaveBeenCalledExactlyOnceWith('/c/canonical-product-1');
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(1);
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
  });

  it.each([
    { scope: 'legacy intent', requestId: 'legacy-request' },
    { requestId: 'legacy-request', productConversationId: 'legacy-product' },
    { version: 0, requestId: 'legacy-request', productConversationId: 'legacy-product' },
    { version: 1, requestId: 'legacy-request', productConversationId: null },
    { version: 1, requestId: '', productConversationId: 'legacy-product' },
    { version: 1, requestId: 'legacy-request', productConversationId: '' },
    { version: 3, kind: 'accepted', requestId: '', productConversationId: 'product' },
    { version: 3, kind: 'uncertain', request: { request_id: 'request-1' } },
    { version: 3, kind: 'uncertain', request: { request_id: 'request-1', cwd: '/repo', objective: 'objective', model: 'model', images: [{}] } },
  ])('rejects and removes unversioned or invalid pending storage: %j', async (stored) => {
    seedStoredCreation('request-1', stored);
    creationStoreMock.readProductCreationRequest.mockResolvedValueOnce(null);
    const hook = mountCreation();
    await prepareCreation(hook);
    expect(storedCreation()).toBeNull();
    expect(hook.result.current.creating).toBe(false);
    expect(hook.result.current.canSend).toBe(true);
    expect(apiMock.getProductConversationCreation).not.toHaveBeenCalled();
  });

  it('removes malformed pending storage', async () => {
    seedStoredCreation('request-1', '{not JSON');
    creationStoreMock.readProductCreationRequest.mockResolvedValueOnce(null);
    const hook = mountCreation();
    await prepareCreation(hook);
    expect(storedCreation()).toBeNull();
    expect(apiMock.getProductConversationCreation).not.toHaveBeenCalled();
  });

  it.each([400, 403, 422])('releases HTTP %s as a definite rejection and permits same-ID resubmission', async (status) => {
    apiMock.createProductConversation.mockRejectedValueOnce(new ApiResponseError('Admission failed', status));
    apiMock.getProductConversationCreation.mockResolvedValue(pending);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.result.current.error).toBe('Admission failed');
    expect(hook.result.current.canSend).toBe(true);
    expect(hook.result.current.creating).toBe(false);
    expect(storedCreation()).toBeNull();
    expect(apiMock.getProductConversationCreation).not.toHaveBeenCalled();
    await act(async () => { await hook.result.current.handleSend(); });
    expect(apiMock.createProductConversation.mock.calls[1]?.[0]).toEqual(apiMock.createProductConversation.mock.calls[0]?.[0]);
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
  });

  it('replays only the immutable original POST when an uncertain exact GET races acceptance and returns 404', async () => {
    const read = deferred<ProductConversationCreationObservation>();
    const replay = deferred<CreateProductConversationResponse>();
    apiMock.createProductConversation.mockRejectedValueOnce(new TypeError('Failed to fetch')).mockReturnValueOnce(replay.promise);
    apiMock.getProductConversationCreation.mockReturnValueOnce(read.promise).mockResolvedValue(pending);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    const originalRequest = apiMock.createProductConversation.mock.calls[0]![0];
    expect(storedCreation()).toEqual({ version: 3, kind: 'uncertain', request: originalRequest });
    act(() => {
      hook.result.current.setDraft('Changed draft must not be replayed');
      hook.result.current.setCwd('/changed');
    });
    await act(async () => { read.reject(new ApiResponseError('Not found', 404)); await read.promise.catch(() => {}); });
    expect(apiMock.createProductConversation).toHaveBeenNthCalledWith(2, originalRequest);
    expect(hook.result.current.creating).toBe(true);
    expect(hook.result.current.canSend).toBe(false);
    await act(async () => {
      await hook.result.current.handleSend();
      await vi.advanceTimersByTimeAsync(10000);
    });
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(2);
    expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(1);
    await act(async () => { replay.resolve({ request_id: 'request-1', product_conversation_id: 'product-1' }); await replay.promise; });
    expect(storedCreation()).toEqual({
      version: 3, kind: 'accepted', requestId: 'request-1', productConversationId: 'product-1',
    });
    expect(hook.result.current.creationObservation).toEqual(pending);
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
  });

  it('releases accepted identity and storage on authoritative exact deletion 404', async () => {
    apiMock.getProductConversationCreation.mockRejectedValueOnce(new ApiResponseError('Not found', 404));
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.result.current.error).toContain('removed from the server');
    expect(hook.result.current.canSend).toBe(true);
    expect(hook.result.current.creating).toBe(false);
    expect(storedCreation()).toBeNull();
    expect(hook.result.current.creationObservation).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(10000); });
    expect(apiMock.getProductConversationCreation).toHaveBeenCalledTimes(1);
    expect(apiMock.createProductConversation).toHaveBeenCalledTimes(1);
    expect(hook.navigate).not.toHaveBeenCalled();
  });

  it('binds the product ID after uncertain acceptance is confirmed and fences later results', async () => {
    apiMock.createProductConversation.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    apiMock.getProductConversationCreation.mockResolvedValueOnce(pending)
      .mockResolvedValueOnce({ ...published, product_conversation_id: 'wrong-product' })
      .mockRejectedValueOnce(new ApiResponseError('Not found', 404));
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(storedCreation()).toEqual({
      version: 3, kind: 'accepted', requestId: 'request-1', productConversationId: 'product-1',
    });
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(hook.result.current.error).toContain('did not match');
    expect(hook.navigate).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(hook.result.current.canSend).toBe(true);
    expect(hook.result.current.error).toContain('removed from the server');
  });

  it.each([pending, failed, cancelled, published])('fences a mismatched product ID on $kind', async (observation) => {
    apiMock.getProductConversationCreation.mockResolvedValue({ ...observation, product_conversation_id: 'wrong-product' });
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.result.current.creationObservation).toBeNull();
    expect(hook.result.current.creating).toBe(true);
    expect(hook.result.current.error).toContain('did not match');
    expect(hook.navigate).not.toHaveBeenCalled();
    expect(storedCreation()?.['productConversationId']).toBe('product-1');
  });

  it('does not let start over for another row clear the active request or mutate its intent', async () => {
    apiMock.getProductConversationCreation.mockResolvedValue(pending);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    const storage = storedCreation();
    act(() => hook.result.current.startOverFromCreation('unrelated-request', {
      cwd: '/other', objective: 'Other objective', model: 'other-model', effort: null, images: [], llm_language: null,
    }));
    expect(storedCreation()).toBe(storage);
    expect(hook.result.current.creating).toBe(true);
    expect(hook.result.current.draft).toBe('Create the original objective');
    expect(hook.result.current.cwd).toBe('/repo');
    expect(generateUUIDMock).toHaveBeenCalledTimes(1);
  });

  it('allows start over for the exact terminal row', async () => {
    apiMock.getProductConversationCreation.mockResolvedValue(cancelled);
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    act(() => hook.result.current.startOverFromCreation('request-1', {
      cwd: '/other', objective: 'New objective', model: 'model-1', effort: null, images: [], llm_language: null,
    }));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(storedCreation()).toBeNull();
    expect(hook.result.current.creationObservation).toBeNull();
    expect(hook.result.current.draft).toBe('New objective');
    expect(hook.result.current.canSend).toBe(true);
    expect(generateUUIDMock).toHaveBeenCalledTimes(2);
  });

  it.each(['failed', 'cancelled'] as const)('allows explicitly forgetting an actionless %s terminal result and starting anew', async (kind) => {
    const terminal: ProductConversationCreationObservation = kind === 'failed'
      ? { ...failed, allowed_actions: [] } : { ...cancelled, allowed_actions: [] };
    apiMock.getProductConversationCreation.mockResolvedValue(terminal);
    generateUUIDMock.mockReturnValueOnce('request-1').mockReturnValue('request-2');
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.result.current.canSend).toBe(false);
    act(() => hook.result.current.forgetCreationObservation('request-1'));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(hook.result.current.canSend).toBe(true);
    expect(storedCreation()).toBeNull();
    expect(hook.result.current.creationObservation).toBeNull();
    expect(hook.result.current.error).toContain('server record has not been deleted');
    apiMock.createProductConversation.mockResolvedValue({ request_id: 'request-2', product_conversation_id: 'product-2' });
    apiMock.getProductConversationCreation.mockResolvedValue({ ...pending, request_id: 'request-2', product_conversation_id: 'product-2' });
    await act(async () => { await hook.result.current.handleSend(); });
    expect(apiMock.createProductConversation.mock.calls.at(-1)![0].request_id).toBe('request-2');
  });

  it('does not navigate on a status response for another request', async () => {
    apiMock.getProductConversationCreation.mockResolvedValue({ ...published, request_id: 'unrelated-request' });
    const hook = mountCreation();
    await prepareCreation(hook);
    await act(async () => { await hook.result.current.handleSend(); });
    expect(hook.navigate).not.toHaveBeenCalled();
    expect(hook.result.current.error).toContain('did not match');
    expect(hook.result.current.creating).toBe(true);
  });
});
