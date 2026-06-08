// Unit tests for buildConversationChapters — the chapter selector that
// classifies which already-built historical render units are skimmable
// "chapters" for the conversation nav strip.
//
// Coverage: prompt inclusion, significant-prose inclusion, short-prose
// exclusion, unitIndex correctness (chapters past non-chapter units keep the
// right index), pending-user handling, and label truncation. unitIndex
// correctness is also checked end-to-end via buildRenderUnits so the chapter
// indices are proven to line up with virtuoso's coordinate space.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import type { MockInstance } from 'vitest';
import {
  buildConversationChapters,
  truncateLabel,
} from './conversationChapters';
import { buildRenderUnits, type HistoricalUnit } from './renderUnits';
import { SIGNIFICANCE_THRESHOLD } from '../hooks/useDensity';
import type { ContentBlock, ConversationState, Message } from '../api';
import type { QueuedMessage } from '../hooks/useMessageQueue';

// A string long enough to clear the significance threshold.
const LONG_PROSE = 'A'.repeat(SIGNIFICANCE_THRESHOLD + 10);
const SHORT_PROSE = 'too short';

// ----- HistoricalUnit factories (operate directly on the selector input) ----

function userUnit(id: string, text: string, seq = 1): HistoricalUnit {
  return {
    kind: 'user',
    key: id,
    message: {
      message_id: id,
      sequence_id: seq,
      conversation_id: 'c1',
      message_type: 'user',
      content: { text },
      created_at: '',
    },
  };
}

function agentTurnUnit(id: string, blocks: ContentBlock[], seq = 2): HistoricalUnit {
  return {
    kind: 'agent_turn',
    key: id,
    agent: {
      message_id: id,
      sequence_id: seq,
      conversation_id: 'c1',
      message_type: 'agent',
      content: blocks,
      created_at: '',
    },
    toolResultsByUseId: new Map(),
    isFirstInTurn: true,
  };
}

function skillUnit(id: string): HistoricalUnit {
  return {
    kind: 'skill',
    key: id,
    message: {
      message_id: id,
      sequence_id: 0,
      conversation_id: 'c1',
      message_type: 'skill',
      content: { text: '' } as Message['content'],
      created_at: '',
    },
  };
}

function systemUnit(id: string): HistoricalUnit {
  return {
    kind: 'system',
    key: id,
    message: {
      message_id: id,
      sequence_id: 0,
      conversation_id: 'c1',
      message_type: 'system',
      content: { text: 'system notice' } as Message['content'],
      created_at: '',
    },
  };
}

function pendingUserUnit(localId: string, text: string): HistoricalUnit {
  const q: QueuedMessage = {
    localId,
    conversationId: 'conv-1',
    text,
    images: [],
    timestamp: 0,
    status: 'pending',
  };
  return { kind: 'pending_user', key: localId, message: q };
}

describe('truncateLabel', () => {
  it('collapses whitespace to a single line', () => {
    expect(truncateLabel('hello\n  world\t\tfoo')).toBe('hello world foo');
  });

  it('clips long text and appends an ellipsis', () => {
    const out = truncateLabel('x'.repeat(100), 10);
    expect(out.length).toBe(10);
    expect(out.endsWith('…')).toBe(true);
  });

  it('leaves short text untouched', () => {
    expect(truncateLabel('short')).toBe('short');
  });
});

describe('buildConversationChapters', () => {
  it('returns no chapters for empty input', () => {
    expect(buildConversationChapters([])).toEqual([]);
  });

  it('includes every non-empty user prompt as a prompt chapter', () => {
    const units = [userUnit('u1', 'first question', 5), userUnit('u2', 'second', 7)];
    const chapters = buildConversationChapters(units);
    expect(chapters).toEqual([
      { unitIndex: 0, kind: 'prompt', label: 'first question', sequenceId: 5 },
      { unitIndex: 1, kind: 'prompt', label: 'second', sequenceId: 7 },
    ]);
  });

  it('skips whitespace-only user prompts', () => {
    const units = [userUnit('u1', '   \n\t  ')];
    expect(buildConversationChapters(units)).toEqual([]);
  });

  it('includes an agent turn whose first text block clears the threshold', () => {
    const units = [
      agentTurnUnit('a1', [{ type: 'text', text: LONG_PROSE }], 9),
    ];
    const chapters = buildConversationChapters(units);
    expect(chapters).toHaveLength(1);
    expect(chapters[0]).toMatchObject({ unitIndex: 0, kind: 'prose', sequenceId: 9 });
  });

  it('excludes an agent turn whose only text is below the threshold', () => {
    const units = [agentTurnUnit('a1', [{ type: 'text', text: SHORT_PROSE }])];
    expect(buildConversationChapters(units)).toEqual([]);
  });

  it('excludes an agent turn with only tool_use blocks (no significant prose)', () => {
    const units = [
      agentTurnUnit('a1', [
        { type: 'tool_use', id: 't1', name: 'bash', input: {} },
      ]),
    ];
    expect(buildConversationChapters(units)).toEqual([]);
  });

  it('picks the first significant text block when a turn has short + long prose', () => {
    const units = [
      agentTurnUnit('a1', [
        { type: 'text', text: SHORT_PROSE },
        { type: 'text', text: LONG_PROSE },
      ]),
    ];
    const chapters = buildConversationChapters(units);
    expect(chapters).toHaveLength(1);
    expect(chapters[0]?.label).toBe(truncateLabel(LONG_PROSE));
  });

  it('keeps unitIndex aligned across non-chapter units (skill, system, short prose)', () => {
    // Index: 0 user, 1 skill, 2 system, 3 short-prose agent, 4 long-prose agent,
    //        5 user. Only 0, 4, 5 are chapters; their unitIndex must reflect
    //        their true array position, not their chapter ordinal.
    const units = [
      userUnit('u1', 'q1', 1), // 0 -> chapter
      skillUnit('s1'), // 1 -> not a chapter
      systemUnit('sys1'), // 2 -> not a chapter
      agentTurnUnit('a1', [{ type: 'text', text: SHORT_PROSE }]), // 3 -> not a chapter
      agentTurnUnit('a2', [{ type: 'text', text: LONG_PROSE }], 4), // 4 -> chapter
      userUnit('u2', 'q2', 6), // 5 -> chapter
    ];
    const chapters = buildConversationChapters(units);
    expect(chapters.map((c) => c.unitIndex)).toEqual([0, 4, 5]);
    expect(chapters.map((c) => c.kind)).toEqual(['prompt', 'prose', 'prompt']);
  });

  it('includes pending user messages with an undefined sequenceId', () => {
    const units = [pendingUserUnit('local-1', 'queued prompt')];
    const chapters = buildConversationChapters(units);
    expect(chapters).toEqual([
      { unitIndex: 0, kind: 'prompt', label: 'queued prompt', sequenceId: undefined },
    ]);
  });
});

describe('chapter unitIndex matches buildRenderUnits coordinate space', () => {
  let debugSpy: MockInstance<typeof console.debug>;
  beforeEach(() => {
    debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});
  });
  afterEach(() => {
    debugSpy.mockRestore();
  });

  function userMsg(id: string, text: string, seq: number): Message {
    return {
      message_id: id,
      sequence_id: seq,
      conversation_id: 'c1',
      message_type: 'user',
      content: { text },
      created_at: '',
    };
  }
  function agentMsg(id: string, blocks: ContentBlock[], seq: number): Message {
    return {
      message_id: id,
      sequence_id: seq,
      conversation_id: 'c1',
      message_type: 'agent',
      content: blocks,
      created_at: '',
    };
  }
  function toolMsg(id: string, toolUseId: string): Message {
    return {
      message_id: id,
      sequence_id: 0,
      conversation_id: 'c1',
      message_type: 'tool',
      content: { tool_use_id: toolUseId, content: 'ok' },
      created_at: '',
    };
  }

  it('chapter unitIndex indexes into the same historicalUnits virtuoso renders', () => {
    const IDLE: ConversationState = { type: 'idle' };
    // user, agent (long prose + a tool_use), tool-result (folds INTO the agent
    // turn, does NOT add a unit), user. So historicalUnits = [user, agent, user]
    // at indices 0, 1, 2 — the tool message is consumed by the agent turn.
    const messages: Message[] = [
      userMsg('u1', 'first', 1),
      agentMsg('a1', [
        { type: 'text', text: LONG_PROSE },
        { type: 'tool_use', id: 'tu1', name: 'bash', input: {} },
      ], 2),
      toolMsg('t1', 'tu1'),
      userMsg('u2', 'second', 4),
    ];
    const { historicalUnits } = buildRenderUnits({
      messages,
      pendingMessages: [],
      convState: IDLE,
      streamingHandle: null,
    });
    const chapters = buildConversationChapters(historicalUnits);
    expect(chapters.map((c) => c.unitIndex)).toEqual([0, 1, 2]);
    // Each unitIndex resolves to the expected unit kind in the same array.
    for (const c of chapters) {
      const unit = historicalUnits[c.unitIndex];
      expect(unit).toBeDefined();
      if (c.kind === 'prompt') expect(unit?.kind).toBe('user');
      if (c.kind === 'prose') expect(unit?.kind).toBe('agent_turn');
    }
  });
});
