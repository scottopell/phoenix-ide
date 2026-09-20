import { describe, expect, it, vi } from 'vitest';
import { canChangeModelInState } from '../api';
import { parseConversationState, canCancelConversationState, isAgentWorking } from '../utils';

describe('parseConversationState recovery', () => {
  it('parses overload retry as busy and cancellable', () => {
    const state = parseConversationState({
      type: 'server_overload_retrying',
      retry: {
        target: { type: 'ordinary' },
        phase: { type: 'waiting', retry_at: '2026-01-01T00:00:30Z' },
        attempt: 3,
      },
    });

    expect(state).toEqual({ type: 'server_overload_retrying', attempt: 3 });
    expect(isAgentWorking(state)).toBe(true);
    expect(canCancelConversationState(state)).toBe(true);
    expect(canChangeModelInState(state)).toBe(false);
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
