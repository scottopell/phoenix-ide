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
    [{ type: 'awaiting_commission_review_approval', brief: 'b', focus: null, allow_dirty_working_tree: false }, true],
    [{ type: 'awaiting_user_response', questions: [] }, false],
    [{ type: 'context_exhausted', summary: 's' }, false],
    [{ type: 'awaiting_recovery', message: 'm', recovery_kind: 'credential', resume: { type: 'conversation_turn' } }, true],
    [{ type: 'terminal' }, false],
    [{ type: 'handed_off', successor_conv_id: 'next' }, false],
    [{ type: 'seeded_llm_requesting', seed_message_id: 'seed', attempt: 1 }, true],
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
        allow_dirty_working_tree: true,
      },
    })).toEqual({
      type: 'awaiting_commission_review_approval',
      brief: 'Ready for review',
      focus: 'security',
      allow_dirty_working_tree: true,
    });
  });

  it('rejects invalid commission review payloads', () => {
    for (const raw of [
      { type: 'awaiting_commission_review_approval', request: { allow_dirty_working_tree: false } },
      { type: 'awaiting_commission_review_approval', request: { brief: 'Ready', allow_dirty_working_tree: 'false' } },
      { type: 'awaiting_commission_review_approval', request: { brief: 'Ready', focus: 42, allow_dirty_working_tree: false } },
    ]) {
      expect(parseConversationState(raw).type).toBe('error');
    }
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
    [{ type: 'awaiting_commission_review_approval', brief: 'b', focus: null, allow_dirty_working_tree: false }, false],
    [{ type: 'awaiting_user_response', questions: [] }, false],
    [{ type: 'context_exhausted', summary: 's' }, false],
    [{ type: 'awaiting_recovery', message: 'm', recovery_kind: 'credential', resume: { type: 'conversation_turn' } }, false],
    [{ type: 'terminal' }, false],
  ];

  it.each(cases)('%o -> %s', (state, expected) => {
    expect(canChangeModelInState(state)).toBe(expected);
  });
});
