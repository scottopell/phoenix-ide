import { describe, expect, it, vi } from 'vitest';
import { canChangeModelInState } from '../api';
import { parseConversationState, canCancelConversationState, isAgentWorking } from '../utils';

describe('parseConversationState recovery', () => {
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

  it.each([
    { type: 'seeded_llm_requesting' },
    { type: 'handed_off' },
    { type: 'unrecognized_state' },
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
