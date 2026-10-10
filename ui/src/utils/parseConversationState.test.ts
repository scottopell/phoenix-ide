import { describe, expect, it, vi } from 'vitest';
import { canChangeModelInState } from '../api';
import { parseConversationState, canCancelConversationState, isAgentWorking } from '../utils';

describe('parseConversationState recovery', () => {
  it('parses overload retry as busy and cancellable', () => {
    const state = parseConversationState({
      type: 'server_overload_retrying',
      retry: {
        target: 'ordinary',
        phase: { type: 'waiting', retry_at: '2026-01-01T00:00:30Z' },
        attempt: 3,
      },
    });

    expect(state).toEqual({
      type: 'server_overload_retrying',
      attempt: 3,
      maxAttempts: 5,
      retryAt: Date.parse('2026-01-01T00:00:30Z'),
      target: 'ordinary',
    });
    expect(isAgentWorking(state)).toBe(true);
    expect(canCancelConversationState(state)).toBe(true);
    expect(canChangeModelInState(state)).toBe(false);
  });

  it('parses the public continuation overload discriminator without private target payload', () => {
    expect(parseConversationState({
      type: 'server_overload_retrying',
      attempt: 2,
      max_attempts: 5,
      retry_at: null,
      target: 'continuation',
    })).toEqual({
      type: 'server_overload_retrying',
      attempt: 2,
      maxAttempts: 5,
      retryAt: null,
      target: 'continuation',
    });
  });

  it('preserves an overload recovery resume target without semantic substitution', () => {
    const state = parseConversationState({
      type: 'awaiting_recovery',
      message: 'refreshing credentials',
      recovery_kind: 'credential',
      resume: {
        type: 'server_overload_retry',
        retry: {
          target: {
            type: 'continuation',
            operation_id: 'continuation-7',
            rejected_tool_calls: [],
          },
          phase: { type: 'in_flight' },
          attempt: 3,
          started_at: '2026-01-01T00:00:00Z',
          deadline_at: '2026-01-01T00:02:00Z',
          logical_request_id: 'logical-request-7',
          model_id: 'resolved-model-7',
        },
      },
    });

    expect(state).toEqual({
      type: 'awaiting_recovery',
      message: 'refreshing credentials',
      recovery_kind: 'credential',
      resume: {
        type: 'server_overload_retry',
        retry: {
          target: {
            type: 'continuation',
            operation_id: 'continuation-7',
            rejected_tool_calls: [],
          },
          phase: { type: 'in_flight' },
          attempt: 3,
          started_at: '2026-01-01T00:00:00Z',
          deadline_at: '2026-01-01T00:02:00Z',
          logical_request_id: 'logical-request-7',
          model_id: 'resolved-model-7',
        },
      },
    });
  });

  it('rejects unknown recovery targets instead of substituting a conversation turn', () => {
    expect(parseConversationState({
      type: 'awaiting_recovery',
      message: 'recovering',
      recovery_kind: 'credential',
      resume: { type: 'future_operation' },
    })).toEqual({
      type: 'client_decode_error',
      message: 'Invalid recovery resume target',
    });
  });

  it.each([
    [{ type: 'awaiting_user_response', questions: [] }],
    [{ type: 'awaiting_user_response', questions: [], request_id: null }],
  ])('normalizes a legacy question request identity to absence', (raw) => {
    const parsed = parseConversationState(raw);
    expect(parsed).toEqual({
      type: 'awaiting_user_response',
      questions: [],
    });
    expect('request_id' in parsed).toBe(false);
  });

  it('preserves an identified question request identity', () => {
    expect(parseConversationState({
      type: 'awaiting_user_response',
      questions: [],
      request_id: 'request-q2',
    })).toEqual({
      type: 'awaiting_user_response',
      questions: [],
      request_id: 'request-q2',
    });
  });


  it('allows manual recovery of a persisted invalid-request error', () => {
    const state = parseConversationState({
      type: 'error',
      error_kind: 'invalid_request',
      message: 'The access_programs parameter is not enabled for this organization.',
    });
    expect(state).toMatchObject({
      type: 'error',
      error: { can_auto_retry: false, can_user_resume: true },
    });
  });

  it('retains typed continuation failure recovery', () => {
    expect(parseConversationState({
      type: 'recoverable_continuation_failure',
      failure: {
        message: 'Summary failed',
        error_kind: 'server_error',
        request: { operation_id: 'summary-op', attempt: 2, rejected_tool_calls: [] },
      },
    })).toEqual({
      type: 'recoverable_continuation_failure',
      message: 'Summary failed',
      error_kind: 'server_error',
      operation_id: 'summary-op',
      attempt: 2,
    });
  });

  it.each([
    { type: 'seeded_llm_requesting' },
    { type: 'handed_off' },
    { type: 'unrecognized_state' },
    { type: 'error' },
    { type: 'error', error_kind: '' },
    { type: 'error', error_kind: 'unrecognized_error' },
    { type: 'recoverable_continuation_failure' },
    { type: 'recoverable_continuation_failure', failure: [] },
    { type: 'recoverable_continuation_failure', failure: {} },
    { type: 'recoverable_continuation_failure', failure: { message: 'failed', error_kind: 'unrecognized_error', request: {} } },
  ])('does not infer recovery authority from unreadable state $type', (raw) => {
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      const state = parseConversationState(raw);
      expect(state).toEqual({ type: 'client_decode_error', message: expect.any(String) });
      expect(canChangeModelInState(state)).toBe(false);
      expect(canCancelConversationState(state)).toBe(false);
      expect(isAgentWorking(state)).toBe(false);
    } finally {
      warning.mockRestore();
    }
  });
});
