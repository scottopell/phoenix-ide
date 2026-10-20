import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';

const references = ['.', '..', '雪', '%E9%9B%AA', '%2F', '/', 'a/b?c#d', 'a\\b', 'a+b', '%', '%ZZ', ' leading ', '7d121a3f-17f8-4da2-b6c0-114934f9a6ab'];
const reads = [
  ['coordinator route', (id: string) => api.resolveCoordinatorRoute(id), '/api/global/coordinator/reads/route'],
  ['transcript', (id: string) => api.getConversation(id), '/api/conversation-reads/snapshot'],
  ['product snapshot', (id: string) => api.getProductConversationSnapshot(id), '/api/product-conversation-reads/snapshot'],
  ['product history', (id: string) => api.getProductConversationHistorySnapshot(id), '/api/product-conversation-reads/snapshot?boundary_representation=original_messages'],
  ['coordinator history', (id: string) => api.getCoordinatorHistory(id), '/api/global/coordinator/reads/history'],
  ['latest messages', (id: string) => api.getConversationMessagesLatest(id, 25), '/api/conversation-reads/messages/latest?limit=25'],
  ['older messages', (id: string) => api.getConversationMessagesBefore(id, 77, 10), '/api/conversation-reads/messages?before_message_sequence=77&limit=10'],
  ['newer messages', (id: string) => api.getConversationMessagesAfter(id, 88, 11), '/api/conversation-reads/messages?after_message_sequence=88&limit=11'],
  ['exact range', (id: string) => api.getConversationMessageRange(id, 5, 12), '/api/conversation-reads/messages/range?start_message_sequence=5&end_message_sequence=12'],
  ['around message', (id: string) => api.getConversationMessagesAround(id, 17, 4, 6), '/api/conversation-reads/messages/around/17?before=4&after=6'],
] as const;

describe('opaque identity HTTP read transport', () => {
  afterEach(() => vi.unstubAllGlobals());

  it.each(['.', '..'])('demonstrates why encoding a %s path component is insufficient', id => {
    const url = new URL(`/api/conversations/${encodeURIComponent(id)}`, 'https://phoenix.test');
    expect(new Request(url).url).not.toBe(`https://phoenix.test/api/conversations/${id}`);
  });

  it.each(reads)('%s survives real URL and Request normalization for every reference', async (_name, read, endpoint) => {
    for (const reference of references) {
      const send = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
        const request = new Request(new URL(String(input), 'https://phoenix.test'), init);
        const expected = new URL(endpoint, 'https://phoenix.test');
        expect(new URL(request.url).pathname).toBe(expected.pathname);
        expect(new URL(request.url).searchParams.toString()).toBe(expected.searchParams.toString());
        expect(request.method).toBe('POST');
        expect(request.headers.get('content-type')).toBe('application/json');
        expect(await request.json()).toEqual({ reference });
        return new Response(JSON.stringify({ reference, latest_opening: { status: 'none' } }));
      });
      vi.stubGlobal('fetch', send);
      expect(await read(reference)).toMatchObject({ reference });
      expect(send).toHaveBeenCalledTimes(1);
    }
  });
});
