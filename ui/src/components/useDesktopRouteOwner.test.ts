import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiResponseError, type ProductConversationSnapshotView } from '../api';
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
