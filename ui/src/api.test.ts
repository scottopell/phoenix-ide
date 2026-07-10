// Tests for the `api.continueConversation` client (REQ-BED-030, task 24696).
//
// The endpoint is idempotent on the backend: a second call returns the
// existing continuation with `already_existed: true`. The UI relies on
// that idempotence — callers dispatch `continueConversation` without
// client-side race resolution.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { api, canChangeModelInState, ConflictError, type ConversationState } from './api';
import { parseConversationState, canCancelConversationState } from './utils';

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

describe('conversation metadata client', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('GETs metadata by slug without fetching messages', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({ conversation: { id: 'conv-1' }, agent_working: false, presentation_mode: 'idle', context_window_size: 0 }),
    } as unknown as Response);

    await api.getConversationMetaBySlug('slug one');

    expect(fetchMock).toHaveBeenCalledWith('/api/conversations/by-slug/slug%20one/meta');
  });
});

describe('conversation message history clients', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('GETs latest messages with limit query', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        messages: [],
        tombstones: [],
        transcript_generation: null,
        server_message_tail: 42,
      }),
    } as unknown as Response);

    await api.getConversationMessagesLatest('conv/id', 25);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations/conv%2Fid/messages/latest?limit=25',
    );
  });

  it('GETs messages before a sequence using before_message_sequence', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        messages: [], tombstones: [], transcript_generation: null, server_message_tail: 9,
      }),
    } as unknown as Response);

    await api.getConversationMessagesBefore('conv-1', 77, 10);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations/conv-1/messages?before_message_sequence=77&limit=10',
    );
  });

  it('GETs messages after a sequence using after_message_sequence', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        messages: [], tombstones: [], transcript_generation: null, server_message_tail: 9,
      }),
    } as unknown as Response);

    await api.getConversationMessagesAfter('conv-1', 88, 11);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations/conv-1/messages?after_message_sequence=88&limit=11',
    );
  });

  it('GETs an exact range with explicit start/end_message_sequence params', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        messages: [], missing_sequences: [], tombstones: [], transcript_generation: 3, server_message_tail: 12,
      }),
    } as unknown as Response);

    await api.getConversationMessageRange('conv-1', 5, 12);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations/conv-1/messages/range?start_message_sequence=5&end_message_sequence=12',
    );
  });

  it('GETs messages around a pivot sequence with before/after query params', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        before: [], after: [], tombstones: [], transcript_generation: null, server_message_tail: 12,
      }),
    } as unknown as Response);

    await api.getConversationMessagesAround('conv-1', 17, 4, 6);

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations/conv-1/messages/around/17?before=4&after=6',
    );
  });
});

describe('api.regenerateConversationName', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('POSTs /api/conversations/:id/regenerate-name and returns the refreshed conversation', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        conversation: { id: 'conv-id', slug: 'generated-slug' },
      }),
    } as unknown as Response);

    const res = await api.regenerateConversationName('conv-id');

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations/conv-id/regenerate-name',
      { method: 'POST' },
    );
    expect(res.conversation.slug).toBe('generated-slug');
  });

  it('surfaces backend errors', async () => {
    const fetchMock = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    fetchMock.mockResolvedValueOnce({
      ok: false,
      status: 500,
      json: async () => ({ error: 'name regeneration failed' }),
    } as unknown as Response);

    await expect(api.regenerateConversationName('conv-id')).rejects.toThrow(
      /name regeneration failed/,
    );
  });
});

describe('canCancelConversationState', () => {
  const cases: ReadonlyArray<readonly [ConversationState, boolean]> = [
    [{ type: 'idle' }, false],
    [{ type: 'error', message: 'overloaded', error_kind: 'server_overloaded' }, false],
    [{ type: 'awaiting_llm' }, false],
    [{ type: 'llm_requesting', attempt: 1 }, true],
    [{ type: 'tool_executing', current_tool: { id: 't', name: 'bash', input: {} }, remaining_tools: [] }, true],
    [{ type: 'awaiting_sub_agents', pending: [], completed_results: [] }, true],
    [{ type: 'awaiting_continuation', attempt: 1 }, false],
    [{ type: 'cancelling' }, false],
    [{ type: 'cancelling_tool', current_tool: { id: 't', name: 'bash', input: {} } }, false],
    [{ type: 'cancelling_sub_agents', pending: [] }, false],
    [{ type: 'awaiting_task_approval', title: 't', priority: 'p1', plan: 'p' }, true],
    [{ type: 'awaiting_commission_review_approval', brief: 'b', focus: null, scope: undefined }, true],
    [{ type: 'awaiting_user_response', questions: [] }, false],
    [{ type: 'context_exhausted', summary: 's' }, false],
    [{ type: 'awaiting_recovery', message: 'm', recovery_kind: 'credential', resume: { type: 'conversation_turn' } }, true],
    [{ type: 'terminal' }, false],
    [{ type: 'handed_off', successor_conv_id: 'next' }, false],
    [{ type: 'seeded_llm_requesting', seed_message_id: 'seed', attempt: 1 }, true],
    [{ type: 'provisioning', prompt: 'hello' }, true],
    [{ type: 'creation_failed', message: 'boom', prompt: 'hello' }, false],
  ];

  it.each(cases)('%o -> %s', (state, expected) => {
    expect(canCancelConversationState(state)).toBe(expected);
  });
});

describe('parseConversationState commission review approval', () => {
  it('parses valid commission review approval state', () => {
    expect(parseConversationState({
      type: 'awaiting_commission_review_approval',
      request: {
        brief: 'Ready for review',
        focus: 'security',
      },
      scope: {
        kind: 'committed_branch_diff',
        repo_root: '/repo',
        base: 'refs/remotes/origin/main',
        head: 'task',
        dirty: false,
        changed_files: 0,
        insertions: 0,
        deletions: 0,
      },
    })).toEqual({
      type: 'awaiting_commission_review_approval',
      brief: 'Ready for review',
      focus: 'security',
      scope: {
        kind: 'committed_branch_diff',
        repo_root: '/repo',
        base: 'refs/remotes/origin/main',
        head: 'task',
        dirty: false,
        changed_files: 0,
        insertions: 0,
        deletions: 0,
      },
    });
  });

  it('rejects invalid commission review payloads', () => {
    for (const raw of [
      { type: 'awaiting_commission_review_approval', request: {} },
      { type: 'awaiting_commission_review_approval', request: { brief: 'Ready', focus: 42 } },
    ]) {
      expect(parseConversationState(raw).type).toBe('error');
    }
  });

  it('parses provisioning and creation_failed states', () => {
    expect(parseConversationState({ type: 'provisioning', prompt: 'ship it' })).toEqual({
      type: 'provisioning',
      prompt: 'ship it',
    });
    expect(parseConversationState({ type: 'creation_failed', message: 'boom', prompt: 'ship it' })).toEqual({
      type: 'creation_failed',
      message: 'boom',
      prompt: 'ship it',
    });
  });
});

describe('canChangeModelInState (task 02713)', () => {
  // One representative value per ConversationState variant. `cases`
  // is typed so adding a union member without classifying it here is
  // a tsc error (mirrors the Rust exhaustiveness guard).
  const cases: ReadonlyArray<readonly [ConversationState, boolean]> = [
    [{ type: 'idle' }, true],
    [{ type: 'error', message: 'overloaded', error_kind: 'server_overloaded' }, true],
    [{ type: 'awaiting_llm' }, false],
    [{ type: 'llm_requesting', attempt: 1 }, false],
    [{ type: 'tool_executing', current_tool: { id: 't', name: 'bash', input: {} }, remaining_tools: [] }, false],
    [{ type: 'awaiting_sub_agents', pending: [], completed_results: [] }, false],
    [{ type: 'awaiting_continuation', attempt: 1 }, false],
    [{ type: 'cancelling' }, false],
    [{ type: 'cancelling_tool', current_tool: { id: 't', name: 'bash', input: {} } }, false],
    [{ type: 'cancelling_sub_agents', pending: [] }, false],
    [{ type: 'awaiting_task_approval', title: 't', priority: 'p1', plan: 'p' }, false],
    [{ type: 'awaiting_commission_review_approval', brief: 'b', focus: null, scope: undefined }, false],
    [{ type: 'awaiting_user_response', questions: [] }, false],
    [{ type: 'context_exhausted', summary: 's' }, false],
    [{ type: 'awaiting_recovery', message: 'm', recovery_kind: 'credential', resume: { type: 'conversation_turn' } }, false],
    [{ type: 'provisioning', prompt: 'hello' }, false],
    [{ type: 'creation_failed', message: 'boom', prompt: 'hello' }, false],
    [{ type: 'terminal' }, false],
  ];

  it.each(cases)('%o -> %s', (state, expected) => {
    expect(canChangeModelInState(state)).toBe(expected);
  });
});
