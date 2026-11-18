import { describe, expect, it } from 'vitest';
import type { ConversationState, FinalizedToolRoundResult, Message } from '../api';
import { parseConversationState } from '../utils';
import { getToolActivity, progressForToolRound, projectToolRound, toolStartedAt } from './toolActivity';

const call = (id: string) => ({ id, name: 'bash', input: {} });
const owner: Message = {
  message_id: 'round-a', conversation_id: 'conv', sequence_id: 1, message_type: 'agent',
  content: [], created_at: '2026-11-17T00:00:00Z', event_presentation: null,
  display_data: { tool_starts: { first: 100, second: 200, third: 300, fourth: 400 } },
};
const completed: FinalizedToolRoundResult = { tool_use_id: 'fourth', outcome: { type: 'error', output: 'failed sibling' }, duration_ms: 75 };
const state: ConversationState = {
  type: 'tool_executing', current_tool: call('first'), remaining_tools: ['second', 'third', 'fifth'].map(call),
  assistant_message: { message_id: owner.message_id }, completed_results: [completed],
};

describe('foreground batch activity', () => {
  it('keeps actual starts separate from queued calls and retires out-of-order completions', () => {
    const activity = getToolActivity(parseConversationState(state));
    expect(toolStartedAt(owner, 'first', activity)).toBe(100);
    expect(toolStartedAt(owner, 'second', activity)).toBe(200);
    expect(toolStartedAt(owner, 'third', activity)).toBe(300);
    expect(toolStartedAt(owner, 'fifth', activity)).toBeUndefined();
    expect(toolStartedAt(owner, 'fourth', activity)).toBeUndefined();
    const results = projectToolRound(owner, new Map(), activity);
    expect(results.get('fourth')?.content).toMatchObject({ tool_use_id: 'fourth', content: 'failed sibling', is_error: true });
    expect(results.get('fourth')?.display_data).toMatchObject({ duration_ms: 75 });
    expect(results.has('first')).toBe(false);
  });

  it('scopes starts and accepted results to the assistant round even when provider IDs recur', () => {
    const previous = { ...owner, message_id: 'previous-round' };
    const activity = getToolActivity(state);
    expect(toolStartedAt(previous, 'first', activity)).toBeUndefined();
    expect(projectToolRound(previous, new Map(), activity).size).toBe(0);
    expect(progressForToolRound(previous, { first: { progress: {} as never } }, activity)).toEqual({});
  });

  it('retains the same unresolved membership during cancellation, including launched tail siblings', () => {
    const activity = getToolActivity(parseConversationState({
      type: 'cancelling_tool', tool_use_id: 'first', skipped_tools: [call('second'), call('fifth')],
      completed_results: [completed], assistant_message: { message_id: owner.message_id },
    }));
    expect(toolStartedAt(owner, 'second', activity)).toBe(200);
    expect(toolStartedAt(owner, 'fifth', activity)).toBeUndefined();
    expect(projectToolRound(owner, new Map(), activity).has('fourth')).toBe(true);
  });

  it('does not flatten pending self-management intents into finalized results', () => {
    const activity = getToolActivity({ ...state, completed_results: [{ pending_self_goal: {} }, { pending_goal_report: {} }] });
    expect(projectToolRound(owner, new Map(), activity).size).toBe(0);
  });
});
