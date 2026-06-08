// Unit tests for buildRenderUnits.
//
// Covers the spec rules in specs/messagelist-render-units/render_units.allium
// and the requirements REQ-MLRU-001 through REQ-MLRU-004, REQ-MLRU-010,
// REQ-MLRU-011, and the structural invariants.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import type { MockInstance } from 'vitest';
import {
  buildRenderUnits,
  SUB_AGENT_STATUS_KEY,
  type HistoricalUnit,
} from './renderUnits';
import type {
  ContentBlock,
  ConversationState,
  Message,
} from '../api';
import type { QueuedMessage } from '../hooks/useMessageQueue';

type AgentTurnUnit = Extract<HistoricalUnit, { kind: 'agent_turn' }>;

function assertAgentTurn(u: HistoricalUnit | undefined): AgentTurnUnit {
  if (!u) throw new Error('expected unit, got undefined');
  if (u.kind !== 'agent_turn') throw new Error(`expected agent_turn, got ${u.kind}`);
  return u;
}

// ----- Message factories -----

function userMsg(id: string, text = 'hi'): Message {
  return {
    message_id: id,
    sequence_id: 0,
    conversation_id: 'c1',
    message_type: 'user',
    content: { text },
    created_at: '',
  };
}

function agentMsg(id: string, blocks: ContentBlock[] = []): Message {
  return {
    message_id: id,
    sequence_id: 0,
    conversation_id: 'c1',
    message_type: 'agent',
    content: blocks,
    created_at: '',
  };
}

function toolMsg(
  id: string,
  toolUseId: string | undefined,
  result = 'ok',
): Message {
  return {
    message_id: id,
    sequence_id: 0,
    conversation_id: 'c1',
    message_type: 'tool',
    // The narrow type forbids omitting tool_use_id at compile time, but
    // production messages sometimes arrive without it (defensive log
    // path); we deliberately exercise that case.
    content: { tool_use_id: toolUseId as string, content: result },
    created_at: '',
  };
}

function systemMsg(id: string, text?: string): Message {
  return {
    message_id: id,
    sequence_id: 0,
    conversation_id: 'c1',
    message_type: 'system',
    content: (text !== undefined ? { text } : {}) as Message['content'],
    created_at: '',
  };
}

function skillMsg(id: string): Message {
  return {
    message_id: id,
    sequence_id: 0,
    conversation_id: 'c1',
    message_type: 'skill',
    content: { text: '' } as Message['content'],
    created_at: '',
  };
}

function queued(localId: string): QueuedMessage {
  return {
    localId,
    conversationId: 'conv-1',
    text: 'queued',
    images: [],
    timestamp: 0,
    status: 'pending',
  };
}

const IDLE: ConversationState = { type: 'idle' };

// ----- Test suite -----

describe('buildRenderUnits', () => {
  let debugSpy: MockInstance<typeof console.debug>;

  beforeEach(() => {
    debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});
  });

  afterEach(() => {
    debugSpy.mockRestore();
  });

  // -------------------------------------------------------------------
  // Empty / minimal
  // -------------------------------------------------------------------

  it('returns empty lists for empty inputs', () => {
    const out = buildRenderUnits({
      messages: [],
      pendingMessages: [],
      convState: IDLE,
      streamingHandle: null,
    });
    expect(out.historicalUnits).toEqual([]);
    expect(out.tailUnits).toEqual([]);
  });

  // -------------------------------------------------------------------
  // User / skill / system / agent emission
  // -------------------------------------------------------------------

  describe('user / skill / system / agent emission', () => {
    it('emits a user unit and breaks the agent run', () => {
      const u = userMsg('u1');
      const a = agentMsg('a1');
      const out = buildRenderUnits({
        messages: [u, a],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits[0]).toMatchObject({ kind: 'user', key: 'u1' });
      expect(out.historicalUnits[1]).toMatchObject({
        kind: 'agent_turn',
        key: 'a1',
        isFirstInTurn: true,
      });
    });

    it('emits a skill unit and breaks the agent run', () => {
      const s = skillMsg('s1');
      const a = agentMsg('a1');
      const out = buildRenderUnits({
        messages: [s, a],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits[0]).toMatchObject({ kind: 'skill', key: 's1' });
      expect(out.historicalUnits[1]).toMatchObject({
        kind: 'agent_turn',
        isFirstInTurn: true,
      });
    });

    it('emits a system unit only when content.text is non-empty', () => {
      const sysWithText = systemMsg('sys1', 'hello');
      const sysEmpty = systemMsg('sys2', '');
      const sysAbsent = systemMsg('sys3');
      const out = buildRenderUnits({
        messages: [sysWithText, sysEmpty, sysAbsent],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toHaveLength(1);
      expect(out.historicalUnits[0]).toMatchObject({
        kind: 'system',
        key: 'sys1',
      });
      expect(debugSpy).toHaveBeenCalledWith(
        '[renderUnits] skipped empty system',
        expect.objectContaining({ message_id: 'sys2', reason: 'empty_system' }),
      );
      expect(debugSpy).toHaveBeenCalledWith(
        '[renderUnits] skipped empty system',
        expect.objectContaining({ message_id: 'sys3', reason: 'empty_system' }),
      );
    });

    it('emits a single agent_turn with empty toolResultsByUseId when no tool messages follow', () => {
      const a = agentMsg('a1');
      const out = buildRenderUnits({
        messages: [a],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toHaveLength(1);
      const u = assertAgentTurn(out.historicalUnits[0]);
      expect(u.toolResultsByUseId.size).toBe(0);
      expect(u.isFirstInTurn).toBe(true);
    });
  });

  // -------------------------------------------------------------------
  // Tool-result ownership (REQ-MLRU-002)
  // -------------------------------------------------------------------

  describe('tool result ownership', () => {
    it('consumes trailing tool messages and pairs them by tool_use_id', () => {
      const a = agentMsg('a1');
      const t1 = toolMsg('t1', 'use-1', 'r1');
      const t2 = toolMsg('t2', 'use-2', 'r2');
      const t3 = toolMsg('t3', 'use-3', 'r3');
      const out = buildRenderUnits({
        messages: [a, t1, t2, t3],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toHaveLength(1);
      const u = assertAgentTurn(out.historicalUnits[0]);
      expect(u.toolResultsByUseId.size).toBe(3);
      expect(u.toolResultsByUseId.get('use-1')?.message_id).toBe('t1');
      expect(u.toolResultsByUseId.get('use-2')?.message_id).toBe('t2');
      expect(u.toolResultsByUseId.get('use-3')?.message_id).toBe('t3');
    });

    it('logs and skips tool messages without tool_use_id', () => {
      const a = agentMsg('a1');
      const tBad = toolMsg('tbad', undefined);
      const tGood = toolMsg('tgood', 'use-1');
      const out = buildRenderUnits({
        messages: [a, tBad, tGood],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const u = assertAgentTurn(out.historicalUnits[0]);
      expect(u.toolResultsByUseId.size).toBe(1);
      expect(u.toolResultsByUseId.has('use-1')).toBe(true);
      expect(debugSpy).toHaveBeenCalledWith(
        '[renderUnits] tool result missing tool_use_id',
        expect.objectContaining({
          message_id: 'tbad',
          reason: 'missing_tool_use_id',
        }),
      );
    });

    it('REQ-MLRU-012 prerequisite: 20 tool results stay attached to their agent_turn', () => {
      const u = userMsg('u1', 'do many');
      const a = agentMsg('a1');
      const tools: Message[] = [];
      for (let n = 0; n < 20; n++) {
        tools.push(toolMsg(`t${n}`, `use-${n}`));
      }
      const out = buildRenderUnits({
        messages: [u, a, ...tools],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      // 2 historical units: the user and the agent_turn. The 20 tools are
      // owned by the agent_turn's map, never standalone.
      expect(out.historicalUnits).toHaveLength(2);
      const at = assertAgentTurn(out.historicalUnits[1]);
      expect(at.toolResultsByUseId.size).toBe(20);
    });

    it('stops consuming at the first non-tool boundary', () => {
      const a1 = agentMsg('a1');
      const t1 = toolMsg('t1', 'use-1');
      const u = userMsg('u1');
      const a2 = agentMsg('a2');
      const t2 = toolMsg('t2', 'use-2');
      const out = buildRenderUnits({
        messages: [a1, t1, u, a2, t2],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toHaveLength(3);
      const first = assertAgentTurn(out.historicalUnits[0]);
      const second = assertAgentTurn(out.historicalUnits[2]);
      expect(first.toolResultsByUseId.has('use-1')).toBe(true);
      expect(first.toolResultsByUseId.has('use-2')).toBe(false);
      expect(second.toolResultsByUseId.has('use-2')).toBe(true);
    });
  });

  // -------------------------------------------------------------------
  // isFirstInTurn (REQ-MLRU-003)
  // -------------------------------------------------------------------

  describe('isFirstInTurn', () => {
    it('first agent after a user is first-in-turn', () => {
      const out = buildRenderUnits({
        messages: [userMsg('u1'), agentMsg('a1')],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const at = assertAgentTurn(out.historicalUnits[1]);
      expect(at.isFirstInTurn).toBe(true);
    });

    it('second consecutive agent is NOT first-in-turn', () => {
      const out = buildRenderUnits({
        messages: [userMsg('u1'), agentMsg('a1'), agentMsg('a2')],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const a1u = assertAgentTurn(out.historicalUnits[1]);
      const a2u = assertAgentTurn(out.historicalUnits[2]);
      expect(a1u.isFirstInTurn).toBe(true);
      expect(a2u.isFirstInTurn).toBe(false);
    });

    it('a user message between agents resets first-in-turn for the next agent', () => {
      const out = buildRenderUnits({
        messages: [
          userMsg('u1'),
          agentMsg('a1'),
          userMsg('u2'),
          agentMsg('a2'),
        ],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const a1u = assertAgentTurn(out.historicalUnits[1]);
      const a2u = assertAgentTurn(out.historicalUnits[3]);
      expect(a1u.isFirstInTurn).toBe(true);
      expect(a2u.isFirstInTurn).toBe(true);
    });

    it('a skill message between agents resets first-in-turn for the next agent', () => {
      const out = buildRenderUnits({
        messages: [userMsg('u1'), agentMsg('a1'), skillMsg('s1'), agentMsg('a2')],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const a2u = assertAgentTurn(out.historicalUnits[3]);
      expect(a2u.isFirstInTurn).toBe(true);
    });

    it('a system message between agents does NOT reset first-in-turn', () => {
      const out = buildRenderUnits({
        messages: [
          userMsg('u1'),
          agentMsg('a1'),
          systemMsg('s1', 'note'),
          agentMsg('a2'),
        ],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const a2u = assertAgentTurn(
        out.historicalUnits.find((u) => u.kind === 'agent_turn' && u.key === 'a2'),
      );
      expect(a2u.isFirstInTurn).toBe(false);
    });

    it('a tool message between agents (consumed by the first agent) leaves the next agent NOT first-in-turn', () => {
      const out = buildRenderUnits({
        messages: [
          userMsg('u1'),
          agentMsg('a1'),
          toolMsg('t1', 'use-1'),
          agentMsg('a2'),
        ],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const a2u = assertAgentTurn(
        out.historicalUnits.find((u) => u.kind === 'agent_turn' && u.key === 'a2'),
      );
      expect(a2u.isFirstInTurn).toBe(false);
    });
  });

  // -------------------------------------------------------------------
  // Capability-gap logging (REQ-MLRU-011)
  // -------------------------------------------------------------------

  describe('capability-gap logging', () => {
    it('logs and skips an orphan tool message at the start of the list', () => {
      const t = toolMsg('orphan', 'use-x');
      const out = buildRenderUnits({
        messages: [t],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toEqual([]);
      expect(debugSpy).toHaveBeenCalledWith(
        '[renderUnits] skipped orphan tool',
        expect.objectContaining({ message_id: 'orphan', reason: 'orphan_tool' }),
      );
    });

    it('logs and skips a tool message after a user (no agent to attach to)', () => {
      const u = userMsg('u1');
      const t = toolMsg('orphan', 'use-x');
      const out = buildRenderUnits({
        messages: [u, t],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toHaveLength(1);
      expect(out.historicalUnits[0]?.kind).toBe('user');
      expect(debugSpy).toHaveBeenCalledWith(
        '[renderUnits] skipped orphan tool',
        expect.objectContaining({ message_id: 'orphan', reason: 'orphan_tool' }),
      );
    });

    it('logs and skips an unknown message type', () => {
      const m: Message = {
        message_id: 'x1',
        sequence_id: 0,
        conversation_id: 'c1',
        // `continuation` and `error` are in the typed union but the
        // transform currently treats them as unknown (render path
        // unimplemented). Cast to bypass the literal-union narrowing.
        message_type: 'continuation' as Message['message_type'],
        content: {} as Message['content'],
        created_at: '',
      };
      const out = buildRenderUnits({
        messages: [m],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toEqual([]);
      expect(debugSpy).toHaveBeenCalledWith(
        '[renderUnits] skipped unknown type',
        expect.objectContaining({
          message_id: 'x1',
          message_type: 'continuation',
          reason: 'unknown_type',
        }),
      );
    });
  });

  // -------------------------------------------------------------------
  // Legacy `type` field fallback
  // -------------------------------------------------------------------

  it('uses the legacy `type` field when `message_type` is absent', () => {
    const m = {
      message_id: 'legacy1',
      sequence_id: 0,
      conversation_id: 'c1',
      type: 'user',
      content: { text: 'hi' },
      created_at: '',
    } as unknown as Message;
    const out = buildRenderUnits({
      messages: [m],
      pendingMessages: [],
      convState: IDLE,
      streamingHandle: null,
    });
    expect(out.historicalUnits).toHaveLength(1);
    expect(out.historicalUnits[0]).toMatchObject({ kind: 'user', key: 'legacy1' });
  });

  // -------------------------------------------------------------------
  // Tail units (REQ-MLRU-004, REQ-MLRU-010)
  // -------------------------------------------------------------------

  describe('tail units', () => {
    it('appends pending_user units at the tail of historicalUnits in input order', () => {
      // Pending user messages live in historicalUnits (REQ-MLRU-001) so
      // that pending → sent acknowledgement is an in-place keyed update
      // on a single render unit. The eventual user unit's key (the
      // server-echoed message_id) equals the localId by server
      // convention.
      const out = buildRenderUnits({
        messages: [],
        pendingMessages: [queued('q1'), queued('q2')],
        convState: IDLE,
        streamingHandle: null,
      });
      expect(out.historicalUnits).toHaveLength(2);
      expect(out.historicalUnits[0]).toMatchObject({ kind: 'pending_user', key: 'q1' });
      expect(out.historicalUnits[1]).toMatchObject({ kind: 'pending_user', key: 'q2' });
      expect(out.tailUnits.filter((u) => u.kind !== 'sub_agent_status' && u.kind !== 'streaming_agent')).toEqual([]);
    });

    it('appends a singleton sub_agent_status tail unit when awaiting_sub_agents', () => {
      const state: ConversationState = {
        type: 'awaiting_sub_agents',
        pending: [{ agent_id: 'a1', task: 'do' }],
        completed_results: [],
      };
      const out = buildRenderUnits({
        messages: [],
        pendingMessages: [],
        convState: state,
        streamingHandle: null,
      });
      expect(out.tailUnits).toEqual([
        {
          kind: 'sub_agent_status',
          key: SUB_AGENT_STATUS_KEY,
          state,
        },
      ]);
    });

    it('omits the sub_agent_status tail unit for any other convState', () => {
      const out = buildRenderUnits({
        messages: [],
        pendingMessages: [],
        convState: { type: 'idle' },
        streamingHandle: null,
      });
      expect(out.tailUnits.filter((u) => u.kind === 'sub_agent_status')).toEqual([]);
    });

    it('appends a streaming_agent tail unit (tag only) when streamingHandle is present', () => {
      const out = buildRenderUnits({
        messages: [],
        pendingMessages: [],
        convState: { type: 'llm_requesting', attempt: 1 },
        streamingHandle: { key: 'stream-c1-12345' },
      });
      expect(out.tailUnits).toEqual([
        { kind: 'streaming_agent', key: 'stream-c1-12345' },
      ]);
    });

    it('omits the streaming_agent tail unit when streamingHandle is null', () => {
      const out = buildRenderUnits({
        messages: [],
        pendingMessages: [],
        convState: { type: 'llm_requesting', attempt: 1 },
        streamingHandle: null,
      });
      expect(out.tailUnits.filter((u) => u.kind === 'streaming_agent')).toEqual([]);
    });

    it('emits pending_user at the tail of historicalUnits; sub_agent_status before streaming_agent in tailUnits', () => {
      const state: ConversationState = {
        type: 'awaiting_sub_agents',
        pending: [],
        completed_results: [],
      };
      const out = buildRenderUnits({
        messages: [],
        pendingMessages: [queued('q1'), queued('q2')],
        convState: state,
        streamingHandle: { key: 'stream-1' },
      });
      expect(out.historicalUnits.map((u) => u.kind)).toEqual([
        'pending_user',
        'pending_user',
      ]);
      expect(out.tailUnits.map((u) => u.kind)).toEqual([
        'sub_agent_status',
        'streaming_agent',
      ]);
    });
  });

  // -------------------------------------------------------------------
  // Structural invariants
  // -------------------------------------------------------------------

  describe('structural invariants', () => {
    it('never emits a historical unit with kind "tool"', () => {
      const out = buildRenderUnits({
        messages: [
          agentMsg('a1'),
          toolMsg('t1', 'use-1'),
          toolMsg('t2', 'use-2'),
          userMsg('u1'),
          agentMsg('a2'),
          toolMsg('t3', 'use-3'),
        ],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      for (const u of out.historicalUnits) {
        // The HistoricalUnit type has no 'tool' kind. The runtime check
        // guards against any future bug that fabricates one.
        const kind: string = u.kind;
        expect(kind).not.toBe('tool');
      }
    });

    it('historical unit keys are unique', () => {
      const out = buildRenderUnits({
        messages: [
          userMsg('a'),
          agentMsg('b'),
          systemMsg('c', 'note'),
          userMsg('d'),
        ],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const keys = out.historicalUnits.map((u) => u.key);
      expect(new Set(keys).size).toBe(keys.length);
    });

    it('every input tool message either lands in an agent_turn map or produces a debug log', () => {
      const a = agentMsg('a1');
      const tGood = toolMsg('tgood', 'use-1');
      const u = userMsg('u1');
      const orphan = toolMsg('orphan', 'use-2');
      const out = buildRenderUnits({
        messages: [a, tGood, u, orphan],
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const at = assertAgentTurn(out.historicalUnits[0]);
      expect(at.toolResultsByUseId.has('use-1')).toBe(true);
      expect(debugSpy).toHaveBeenCalledWith(
        '[renderUnits] skipped orphan tool',
        expect.objectContaining({ message_id: 'orphan' }),
      );
    });

    it('every emitted historical unit has a source message in the input', () => {
      const inputs: Message[] = [
        userMsg('u1'),
        agentMsg('a1'),
        toolMsg('t1', 'use-1'),
        skillMsg('s1'),
        agentMsg('a2'),
      ];
      const out = buildRenderUnits({
        messages: inputs,
        pendingMessages: [],
        convState: IDLE,
        streamingHandle: null,
      });
      const inputIds = new Set(inputs.map((m) => m.message_id));
      for (const unit of out.historicalUnits) {
        // The unit.key for user/skill/system/agent_turn IS the source
        // message_id by construction.
        expect(inputIds.has(unit.key)).toBe(true);
      }
    });
  });
});
