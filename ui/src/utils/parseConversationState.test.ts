import { describe, expect, it, vi } from 'vitest';
import { parseConversationState } from '../utils';

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
      expect(parseConversationState(raw)).toMatchObject({
        type: 'error',
        error: { can_auto_retry: false, can_user_resume: false },
      });
    } finally {
      warning.mockRestore();
    }
  });
});
