import { describe, expect, it } from 'vitest';
import type { ConversationState } from '../api';
import { activeToolUseIdFromState } from './toolActivity';

describe('active tool identity', () => {
  it.each(['tool_executing', 'cancelling_tool'] as const)(
    'accepts only nonempty string identities while %s', (type) => {
      for (const [id, expected] of [['active', 'active'], ['', undefined], [null, undefined], [42, undefined], [undefined, undefined]]) {
        const state = { type, current_tool: { id, name: 'bash', input: {} }, remaining_tools: [] } as ConversationState;
        expect(activeToolUseIdFromState(state)).toBe(expected);
      }
    },
  );
});
