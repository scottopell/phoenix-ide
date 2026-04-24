// Tests for the `api.continueConversation` client (REQ-BED-030, task 24696).
//
// The endpoint is idempotent on the backend: a second call returns the
// existing continuation with `already_existed: true`. The UI relies on
// that idempotence — callers dispatch `continueConversation` without
// client-side race resolution.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { api, ConflictError } from './api';

describe('api.continueConversation', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('POSTs /api/conversations/:id/continue and returns the parsed response', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        conversation_id: 'new-conv-id',
        slug: 'new-conv-slug',
        already_existed: false,
      }),
    } as unknown as Response);

    const res = await api.continueConversation('parent-id');

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations/parent-id/continue',
      { method: 'POST' },
    );
    expect(res).toEqual({
      conversation_id: 'new-conv-id',
      slug: 'new-conv-slug',
      already_existed: false,
    });
  });

  it('surfaces the already_existed flag when the parent had a continuation', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        conversation_id: 'existing-id',
        slug: 'existing-slug',
        already_existed: true,
      }),
    } as unknown as Response);

    const res = await api.continueConversation('parent-id');
    expect(res.already_existed).toBe(true);
    expect(res.slug).toBe('existing-slug');
  });

  it('throws ConflictError on 409 so the UI can dispatch on error_type', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 409,
      json: async () => ({
        error: 'Conversation is not in context-exhausted state (current: Idle); ...',
        error_type: 'parent_not_context_exhausted',
      }),
    } as unknown as Response);

    await expect(api.continueConversation('parent-id')).rejects.toBeInstanceOf(ConflictError);
  });

  it('throws a generic Error on 404 (parent not found)', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 404,
      json: async () => ({ error: 'Conversation not found: parent-id' }),
    } as unknown as Response);

    await expect(api.continueConversation('parent-id')).rejects.toThrow(
      /Conversation not found/,
    );
  });
});
