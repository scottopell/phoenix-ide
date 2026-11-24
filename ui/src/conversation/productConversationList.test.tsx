import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import type { ProductConversationListRow } from '../api';
import { notifyProductConversationListMayHaveChanged } from '../notifications';
import {
  __testing,
  refreshProductConversationList,
  updateProductConversationRows,
  useProductConversationList,
} from './productConversationList';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
function row(id: string): ProductConversationListRow {
  return {
    product_conversation_id: id,
    canonical_route: `/c/${id}`,
    canonical_root: { transcript_row_id: `root-${id}`, slug: id, title: id },
    lifecycle: { state: 'open', close_action: { availability: 'available' } },
    latest_transcript_row_id: `latest-${id}`,
    updated_at: '2026-11-12T00:00:00Z',
    presentation: { kind: 'state', display_name: id, presentation_mode: 'idle' },
    read_state: { state: 'read' },
    cached_pr: null,
    monitored: false,
    question_with_coordinator_since: null,
  };
}

describe('useProductConversationList', () => {
  beforeEach(() => __testing.reset());
  afterEach(() => {
    __testing.reset();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('loads, caches, and keeps the last good rows when a refresh fails', async () => {
    const list = vi.spyOn(api, 'listProductConversations')
      .mockResolvedValueOnce({ product_conversations: [row('a')], coordinator: null });
    const { result } = renderHook(() => useProductConversationList());
    await waitFor(() => expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['a']));
    expect(result.current.fresh).toBe(true);
    expect(localStorage.getItem(__testing.cacheKey)).toContain('"a"');

    list.mockRejectedValueOnce(new Error('offline'));
    await act(() => refreshProductConversationList());
    expect(result.current.error).toBe('offline');
    expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['a']);
  });

  it('hydrates from the cache before the server answers', async () => {
    localStorage.setItem(__testing.cacheKey, JSON.stringify({ product_conversations: [row('cached')], coordinator: null }));
    __testing.resetCachedSnapshotForTests();
    vi.spyOn(api, 'listProductConversations').mockReturnValue(new Promise(() => {}));
    const { result } = renderHook(() => useProductConversationList());
    expect(result.current.fromCache).toBe(true);
    expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['cached']);
  });

  it('keeps an uncached initial list not fresh until the empty server response succeeds', async () => {
    const request = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    vi.spyOn(api, 'listProductConversations').mockReturnValue(request.promise);

    const { result } = renderHook(() => useProductConversationList());
    expect(result.current.rows).toEqual([]);
    expect(result.current.fresh).toBe(false);
    expect(result.current.fromCache).toBe(false);

    await act(async () => request.resolve({ product_conversations: [], coordinator: null }));
    expect(result.current.rows).toEqual([]);
    expect(result.current.fresh).toBe(true);
    expect(result.current.fromCache).toBe(false);
  });

  it('clears an initial failure for retry while retaining its unverified provenance until success', async () => {
    const retry = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const list = vi.spyOn(api, 'listProductConversations')
      .mockRejectedValueOnce(new Error('offline'))
      .mockReturnValueOnce(retry.promise);
    const { result } = renderHook(() => useProductConversationList());

    await waitFor(() => expect(result.current.error).toBe('offline'));
    expect(result.current.fresh).toBe(false);
    expect(result.current.fromCache).toBe(false);

    let retryPromise!: Promise<void>;
    act(() => { retryPromise = refreshProductConversationList(); });
    expect(result.current.error).toBeNull();
    expect(result.current.fresh).toBe(false);
    expect(result.current.fromCache).toBe(false);
    expect(list).toHaveBeenCalledTimes(2);

    await act(async () => {
      retry.resolve({ product_conversations: [], coordinator: null });
      await retryPromise;
    });
    expect(result.current.error).toBeNull();
    expect(result.current.fresh).toBe(true);
    expect(result.current.rows).toEqual([]);
  });

  it('coalesces passive revision notifications during a slow fetch into one latest follow-up', async () => {
    vi.useFakeTimers();
    const first = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const followUp = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const list = vi.spyOn(api, 'listProductConversations')
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(followUp.promise);
    const { result } = renderHook(() => useProductConversationList());
    expect(list).toHaveBeenCalledTimes(1);

    const signal = list.mock.calls[0]![0] as AbortSignal;
    for (let revision = 0; revision < 3; revision += 1) {
      act(() => { notifyProductConversationListMayHaveChanged(); });
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(list).toHaveBeenCalledTimes(1);
      expect(signal.aborted).toBe(false);
    }

    const latestRename = row('renamed-latest');
    await act(async () => first.resolve({ product_conversations: [row('slow-old')], coordinator: null }));
    expect(list).toHaveBeenCalledTimes(2);
    expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['slow-old']);

    await act(async () => followUp.resolve({ product_conversations: [latestRename], coordinator: null }));
    expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['renamed-latest']);
    expect(list).toHaveBeenCalledTimes(2);
  });

  it('runs a queued invalidation after initial failure without publishing false empty success', async () => {
    vi.useFakeTimers();
    const first = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const followUp = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const list = vi.spyOn(api, 'listProductConversations')
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(followUp.promise);
    const { result } = renderHook(() => useProductConversationList());
    act(() => { notifyProductConversationListMayHaveChanged(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    await act(async () => first.reject(new Error('offline')));
    expect(list).toHaveBeenCalledTimes(2);
    expect(result.current.fresh).toBe(false);
    expect(result.current.fromCache).toBe(false);
    expect(result.current.error).toBe('offline');
    await act(async () => followUp.resolve({ product_conversations: [row('recovered')], coordinator: null }));
    expect(result.current.fresh).toBe(true);
    expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['recovered']);
  });

  it('keeps timeout failures visible during repeated passive retries until a request succeeds', async () => {
    vi.useFakeTimers();
    const recovery = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const hungRequest = (signal?: AbortSignal) => new Promise<never>((_resolve, reject) => {
      signal?.addEventListener('abort', () => reject(new Error('list timed out')), { once: true });
    });
    const list = vi.spyOn(api, 'listProductConversations')
      .mockImplementationOnce(hungRequest)
      .mockImplementationOnce(hungRequest)
      .mockReturnValueOnce(recovery.promise);
    const { result } = renderHook(() => useProductConversationList());
    const invalidations = window.setInterval(notifyProductConversationListMayHaveChanged, 5_000);
    await act(async () => { await vi.advanceTimersByTimeAsync(15_000); });
    expect(list).toHaveBeenCalledTimes(2);
    expect(result.current.error).toBe('list timed out');
    expect(result.current.fresh).toBe(false);
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    window.clearInterval(invalidations);
    await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
    expect(list).toHaveBeenCalledTimes(3);
    expect(result.current.error).toBe('list timed out');
    expect(result.current.fresh).toBe(false);
    await act(async () => recovery.resolve({ product_conversations: [], coordinator: null }));
    expect(result.current.error).toBeNull();
    expect(result.current.fresh).toBe(true);
    expect(result.current.rows).toEqual([]);
  });

  it.each(['rename', 'delete'] as const)('retains acknowledged %s rows and cache when a pre-mutation response arrives and reconciliation fails', async (mutation) => {
    vi.useFakeTimers();
    const original = row('original');
    const renamed: ProductConversationListRow = {
      ...original,
      canonical_route: '/c/renamed',
      canonical_root: { ...original.canonical_root, slug: 'renamed', title: 'Renamed' },
    };
    const expected = mutation === 'rename' ? [renamed] : [];
    const initial = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const stale = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const list = vi.spyOn(api, 'listProductConversations')
      .mockReturnValueOnce(initial.promise)
      .mockReturnValueOnce(stale.promise)
      .mockRejectedValueOnce(new Error('reconciliation offline'));
    const { result } = renderHook(() => useProductConversationList());
    await act(async () => initial.resolve({ product_conversations: [original], coordinator: null }));
    expect(result.current.rows).toEqual([original]);
    let pending!: Promise<void>;
    act(() => { pending = refreshProductConversationList(); });
    const signal = list.mock.calls[1]![0] as AbortSignal;
    act(() => {
      updateProductConversationRows(() => expected);
      notifyProductConversationListMayHaveChanged();
    });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(result.current.rows).toEqual(expected);
    expect(signal.aborted).toBe(false);
    await act(async () => {
      stale.resolve({ product_conversations: [original], coordinator: null });
      await pending;
    });
    expect(list).toHaveBeenCalledTimes(3);
    expect(result.current.error).toBe('reconciliation offline');
    expect(result.current.rows).toEqual(expected);
    expect(JSON.parse(localStorage.getItem(__testing.cacheKey)!).product_conversations).toEqual(expected);
  });

  it('keeps stale superseded request failures and finalizers from replacing a newer success', async () => {
    const stale = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const current = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const list = vi.spyOn(api, 'listProductConversations')
      .mockReturnValueOnce(stale.promise)
      .mockReturnValueOnce(current.promise);
    const { result } = renderHook(() => useProductConversationList());
    expect(list).toHaveBeenCalledTimes(1);

    let newerPromise!: Promise<void>;
    act(() => { newerPromise = refreshProductConversationList(); });
    expect(list).toHaveBeenCalledTimes(2);
    await act(async () => {
      current.resolve({ product_conversations: [row('newer')], coordinator: null });
      await newerPromise;
    });
    expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['newer']);
    expect(result.current.error).toBeNull();

    await act(async () => stale.reject(new Error('stale failure')));
    expect(result.current.rows.map((r) => r.product_conversation_id)).toEqual(['newer']);
    expect(result.current.error).toBeNull();
    expect(list).toHaveBeenCalledTimes(2);
  });

  it('shares one initial request across concurrent list consumers', async () => {
    const request = deferred<{ product_conversations: ProductConversationListRow[]; coordinator: null }>();
    const list = vi.spyOn(api, 'listProductConversations').mockReturnValue(request.promise);
    const first = renderHook(() => useProductConversationList());
    const second = renderHook(() => useProductConversationList());
    expect(list).toHaveBeenCalledTimes(1);

    first.unmount();
    const remounted = renderHook(() => useProductConversationList());
    expect(list).toHaveBeenCalledTimes(1);
    await act(async () => request.resolve({ product_conversations: [row('shared')], coordinator: null }));
    expect(second.result.current.rows.map((r) => r.product_conversation_id)).toEqual(['shared']);
    expect(remounted.result.current.rows.map((r) => r.product_conversation_id)).toEqual(['shared']);
  });

  it('applies optimistic updates to every consumer', async () => {
    vi.spyOn(api, 'listProductConversations').mockResolvedValue({ product_conversations: [row('a'), row('b')], coordinator: null });
    const first = renderHook(() => useProductConversationList());
    const second = renderHook(() => useProductConversationList());
    await waitFor(() => expect(first.result.current.rows).toHaveLength(2));
    act(() => updateProductConversationRows((rows) => rows.filter((r) => r.product_conversation_id !== 'a')));
    expect(first.result.current.rows.map((r) => r.product_conversation_id)).toEqual(['b']);
    expect(second.result.current.rows.map((r) => r.product_conversation_id)).toEqual(['b']);
  });
});
