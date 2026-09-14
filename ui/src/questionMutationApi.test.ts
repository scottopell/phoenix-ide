import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, QuestionMutationError } from './api';

afterEach(() => vi.unstubAllGlobals());

describe('request-bound question API', () => {
  it('binds both mutations to the original request and preserves answer text', async () => {
    const fetchMock = vi.fn().mockImplementation(async () => new Response(JSON.stringify({ success: true })));
    vi.stubGlobal('fetch', fetchMock);
    const answers = { Choice: 'custom\nanswer' };
    const annotations = { Choice: { notes: '  keep spacing  ' } };
    await api.respondToQuestion('conversation', 'original-request', answers, annotations);
    await api.dismissQuestion('conversation', 'original-request');
    expect(JSON.parse(fetchMock.mock.calls[0]![1].body)).toEqual({ tool_use_id: 'original-request', answers, annotations });
    expect(JSON.parse(fetchMock.mock.calls[1]![1].body)).toEqual({ tool_use_id: 'original-request' });
  });

  it.each([
    [400, 'question_request_invalid', true],
    [409, 'question_request_stale', true],
    [500, 'question_request_stale', false],
    [409, 'wrong_state', false],
    [500, undefined, false],
  ])('classifies only guaranteed no-mutation responses (%s, %s)', async (status, error_type, noMutation) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ error: 'failed', error_type }), { status })));
    const error = await api.respondToQuestion('conversation', 'original', { Choice: 'A' }).catch(e => e);
    expect(error instanceof QuestionMutationError).toBe(noMutation);
  });

  it('does not turn malformed success or transport failure into acceptance', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(new Response('{}')).mockRejectedValueOnce(new TypeError('network'));
    vi.stubGlobal('fetch', fetchMock);
    await expect(api.dismissQuestion('conversation', 'original')).rejects.not.toBeInstanceOf(QuestionMutationError);
    await expect(api.dismissQuestion('conversation', 'original')).rejects.not.toBeInstanceOf(QuestionMutationError);
  });
});
