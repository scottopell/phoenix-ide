import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api } from '../api';
import type { ProductConversationListRow } from '../api';
import {
  __testing,
  refreshProductConversationList,
  updateProductConversationRows,
  useProductConversationList,
} from './productConversationList';

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
  };
}

describe('useProductConversationList', () => {
  afterEach(() => vi.restoreAllMocks());

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
