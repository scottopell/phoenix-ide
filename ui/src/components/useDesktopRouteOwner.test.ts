import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiResponseError, type ProductConversationSnapshotView } from '../api';
import { notifyProductConversationSnapshotChanged } from '../notifications';
import { useDesktopRouteOwner } from './useDesktopRouteOwner';
const snapshot = (product: string, member: string) => ({ product_conversation_id: product, requested_transcript_row_id: member, latest_transcript_row_id: 'latest' }) as ProductConversationSnapshotView;
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
describe('desktop validated route ownership', () => {
  it.each(['?source_transcript=foreign', '?source_transcript=', '?source_transcript=old&source_transcript=other'])('never exposes a raw invalid selector %s', async (search) => {
    const get = vi.spyOn(api, 'getProductConversationSnapshot').mockImplementation(async id => snapshot(id === 'product' ? 'product' : 'foreign-product', id));
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', search));
    expect(result.current).toBeNull();
    await waitFor(() => expect(get).toHaveBeenCalled());
    await act(async () => {});
    expect(result.current).toBeNull();
  });
  it('keeps an exact historical /c member as the desktop owner', async () => {
    vi.spyOn(api, 'getProductConversationSnapshot').mockResolvedValue(snapshot('product', 'historical'));
    const { result } = renderHook(() => useDesktopRouteOwner('historical', 'historical', ''));
    await waitFor(() => expect(result.current).toBe('historical'));
  });
  it('uses latest ownership for the canonical product route', async () => {
    vi.spyOn(api, 'getProductConversationSnapshot').mockResolvedValue(snapshot('product', 'root'));
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', ''));
    await waitFor(() => expect(result.current).toBe('latest'));
  });
  it('follows the latest desktop owner after continuation without navigation', async () => {
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockResolvedValueOnce({ ...snapshot('product', 'root'), latest_transcript_row_id: 'continued' });
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', ''));
    await waitFor(() => expect(result.current).toBe('latest'));
    act(() => { window.dispatchEvent(new CustomEvent('phoenix:automatic-continuation-updated')); });
    await waitFor(() => expect(result.current).toBe('continued'));
    expect(get).toHaveBeenCalledTimes(2);
  });
  it('catches a second canonical notification while its first refresh is in flight', async () => {
    let finishRefresh!: (snapshot: ProductConversationSnapshotView) => void;
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockImplementationOnce(() => new Promise((resolve) => { finishRefresh = resolve; }))
      .mockResolvedValueOnce({ ...snapshot('product', 'root'), latest_transcript_row_id: 'continued-2' });
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', ''));
    await waitFor(() => expect(result.current).toBe('latest'));
    act(() => { notifyProductConversationSnapshotChanged('product'); });
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    act(() => { notifyProductConversationSnapshotChanged('product'); });
    await act(async () => {
      finishRefresh({ ...snapshot('product', 'root'), latest_transcript_row_id: 'continued-1' });
    });
    await waitFor(() => expect(result.current).toBe('continued-2'));
    expect(get).toHaveBeenCalledTimes(3);
  });
  it('catches a second automatic continuation while its first refresh is in flight', async () => {
    let finishRefresh!: (snapshot: ProductConversationSnapshotView) => void;
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockImplementationOnce(() => new Promise((resolve) => { finishRefresh = resolve; }))
      .mockResolvedValueOnce({ ...snapshot('product', 'root'), latest_transcript_row_id: 'continued-2' });
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', ''));
    await waitFor(() => expect(result.current).toBe('latest'));
    act(() => { window.dispatchEvent(new CustomEvent('phoenix:automatic-continuation-updated')); });
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    act(() => { window.dispatchEvent(new CustomEvent('phoenix:automatic-continuation-updated')); });
    await waitFor(() => expect(result.current).toBe('continued-2'));
    await act(async () => {
      finishRefresh({ ...snapshot('product', 'root'), latest_transcript_row_id: 'continued-1' });
    });
    expect(result.current).toBe('continued-2');
    expect(get).toHaveBeenCalledTimes(3);
  });
  it('catches mixed snapshot and automatic invalidations while a refresh is in flight', async () => {
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockImplementationOnce(() => new Promise(() => {}))
      .mockResolvedValueOnce({ ...snapshot('product', 'root'), latest_transcript_row_id: 'mixed-latest' });
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', ''));
    await waitFor(() => expect(result.current).toBe('latest'));
    act(() => { notifyProductConversationSnapshotChanged('product'); });
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    act(() => { window.dispatchEvent(new CustomEvent('phoenix:automatic-continuation-updated')); });
    await waitFor(() => expect(result.current).toBe('mixed-latest'));
    expect(get).toHaveBeenCalledTimes(3);
  });
  it('ignores snapshot invalidations for unrelated and previous routes while the new route resolves', async () => {
    let finishNewRoute!: (snapshot: ProductConversationSnapshotView) => void;
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('old-product', 'root'))
      .mockImplementationOnce(() => new Promise((resolve) => { finishNewRoute = resolve; }));
    const { result, rerender } = renderHook(
      ({ product }) => useDesktopRouteOwner(product, product, ''),
      { initialProps: { product: 'old-product' } },
    );
    await waitFor(() => expect(result.current).toBe('latest'));
    act(() => { notifyProductConversationSnapshotChanged('unrelated-product'); });
    expect(get).toHaveBeenCalledTimes(1);
    rerender({ product: 'new-product' });
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    act(() => { notifyProductConversationSnapshotChanged('old-product'); });
    expect(get).toHaveBeenCalledTimes(2);
    await act(async () => { finishNewRoute(snapshot('new-product', 'root')); });
    expect(result.current).toBe('latest');
    expect(get).toHaveBeenCalledTimes(2);
  });
  it('allows a slow successful initial owner request to settle without timer supersession', async () => {
    vi.useFakeTimers();
    const get = vi.spyOn(api, 'getProductConversationSnapshot').mockImplementation(() => (
      new Promise((resolve) => { window.setTimeout(() => resolve(snapshot('product', 'root')), 1_500); })
    ));
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', ''));
    await act(async () => { vi.advanceTimersByTime(1_000); });
    expect(get).toHaveBeenCalledTimes(1);
    await act(async () => { vi.advanceTimersByTime(500); });
    expect(result.current).toBe('latest');
    expect(get).toHaveBeenCalledTimes(1);
  });
  it('catches a canonical snapshot notification received while resolving a route alias', async () => {
    let finishInitial!: (snapshot: ProductConversationSnapshotView) => void;
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockImplementationOnce(() => new Promise((resolve) => { finishInitial = resolve; }))
      .mockResolvedValueOnce({ ...snapshot('canonical', 'canonical'), latest_transcript_row_id: 'continued' });
    const { result } = renderHook(() => useDesktopRouteOwner('alias', 'alias', ''));
    await act(async () => {
      finishInitial(snapshot('canonical', 'canonical'));
      await Promise.resolve();
      notifyProductConversationSnapshotChanged('canonical');
    });
    await waitFor(() => expect(result.current).toBe('continued'));
    expect(get).toHaveBeenCalledTimes(2);
  });
  it('keeps the last owner while a continuation refresh retries', async () => {
    vi.useFakeTimers();
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockRejectedValueOnce(new Error('temporary'))
      .mockResolvedValueOnce({ ...snapshot('product', 'root'), latest_transcript_row_id: 'continued' });
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', ''));
    await act(async () => {});
    expect(result.current).toBe('latest');
    act(() => { window.dispatchEvent(new CustomEvent('phoenix:automatic-continuation-updated')); });
    await act(async () => {});
    expect(result.current).toBe('latest');
    await act(async () => { vi.advanceTimersByTime(2_000); });
    await act(async () => {});
    expect(result.current).toBe('continued');
    expect(get).toHaveBeenCalledTimes(3);
  });
  it('keeps a validated pin while its continuation refresh retries', async () => {
    vi.useFakeTimers();
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockResolvedValueOnce(snapshot('product', 'old'))
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockRejectedValueOnce(new Error('temporary'))
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockResolvedValueOnce(snapshot('product', 'old'));
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', '?source_transcript=old'));
    await act(async () => {});
    expect(result.current).toBe('old');
    act(() => { window.dispatchEvent(new CustomEvent('phoenix:automatic-continuation-updated')); });
    await act(async () => {});
    expect(result.current).toBe('old');
    await act(async () => { vi.advanceTimersByTime(2_000); });
    await act(async () => {});
    expect(result.current).toBe('old');
    expect(get).toHaveBeenCalledTimes(6);
  });
  it('keeps a validated pin when unrelated viewer query changes while refresh is transiently unavailable', async () => {
    const get = vi.spyOn(api, 'getProductConversationSnapshot')
      .mockResolvedValueOnce(snapshot('product', 'root'))
      .mockResolvedValueOnce(snapshot('product', 'old'))
      .mockRejectedValueOnce(new Error('temporary'));
    const { result, rerender } = renderHook(
      ({ search }) => useDesktopRouteOwner('product', 'product', search),
      { initialProps: { search: '?source_transcript=old' } },
    );
    await act(async () => {});
    expect(result.current).toBe('old');
    rerender({ search: '?source_transcript=old&viewer=README.md' });
    await act(async () => {});
    expect(result.current).toBe('old');
    expect(get).toHaveBeenCalledTimes(3);
  });
  it('stops retrying an authoritative 404 including online events', async () => {
    vi.useFakeTimers();
    const get = vi.spyOn(api, 'getProductConversationSnapshot').mockRejectedValue(new ApiResponseError('missing', 404));
    renderHook(() => useDesktopRouteOwner('direct-row', 'direct-row', ''));
    await act(async () => {});
    await act(async () => { vi.advanceTimersByTime(5000); window.dispatchEvent(new Event('online')); });
    expect(get).toHaveBeenCalledTimes(1);
  });
  it('publishes a member only after matching product and exact requested identity', async () => {
    vi.spyOn(api, 'getProductConversationSnapshot').mockImplementation(async id => snapshot('product', id));
    const { result } = renderHook(() => useDesktopRouteOwner('product', 'product', '?source_transcript=old'));
    expect(result.current).toBeNull();
    await waitFor(() => expect(result.current).toBe('old'));
  });
});
