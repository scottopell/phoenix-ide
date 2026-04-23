import { describe, it, expect } from 'vitest';
import {
  conversationReducer,
  createInitialAtom,
  breadcrumbFromPhase,
  type ConversationAtom,
  type SSEAction,
  type InitPayload,
} from './atom';
import type { Conversation, Message } from '../api';

// Minimal test fixtures
const testConversation: Conversation = {
  id: 'conv-1',
  slug: 'test-slug',
  model: 'claude-3-5-sonnet',
  cwd: '/home/user/project',
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  message_count: 0,
};

function makeMessage(sequenceId: number, messageType: 'user' | 'agent' = 'agent'): Message {
  return {
    message_id: `msg-${sequenceId}`,
    sequence_id: sequenceId,
    conversation_id: 'conv-1',
    message_type: messageType,
    content: { text: `message ${sequenceId}` } as Message['content'],
    created_at: '2024-01-01T00:00:00Z',
  };
}

function makeInitPayload(overrides: Partial<InitPayload> = {}): InitPayload {
  return {
    conversation: testConversation,
    messages: [],
    phase: { type: 'idle' },
    breadcrumbs: [],
    breadcrumbSequenceIds: new Set(),
    contextWindow: { used: 1000 },
    lastSequenceId: 5,
    ...overrides,
  };
}

function dispatch(atom: ConversationAtom, action: SSEAction): ConversationAtom {
  return conversationReducer(atom, action);
}

describe('conversationReducer', () => {
  describe('sse_init', () => {
    it('replaces all state authoritatively', () => {
      const atom = createInitialAtom();
      const payload = makeInitPayload({
        messages: [makeMessage(1), makeMessage(2)],
        lastSequenceId: 5,
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.conversationId).toBe('conv-1');
      expect(next.messages).toHaveLength(2);
      expect(next.lastSequenceId).toBe(5);
      expect(next.streamingBuffer).toBeNull();
    });

    it('merges delta messages on reconnect (lastSequenceId > 0)', () => {
      const existing = makeMessage(3);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastSequenceId: 3,
        messages: [existing],
      };
      const newMsg = makeMessage(4);
      const payload = makeInitPayload({ messages: [newMsg], lastSequenceId: 4 });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(2);
      expect(next.messages[0]!.sequence_id).toBe(3);
      expect(next.messages[1]!.sequence_id).toBe(4);
    });

    it('replaces messages on fresh connect (lastSequenceId = 0)', () => {
      const payload = makeInitPayload({ messages: [makeMessage(1), makeMessage(2)] });
      const atom = createInitialAtom();

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(2);
    });

    it('clears streaming buffer on init', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        streamingBuffer: { text: 'partial', lastSequence: 3, startedAt: Date.now() },
      };

      const next = dispatch(atom, { type: 'sse_init', payload: makeInitPayload() });

      expect(next.streamingBuffer).toBeNull();
    });

    it('replaces breadcrumbs entirely from server payload', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        breadcrumbs: [{ type: 'user', label: 'Old breadcrumb' }],
      };
      const payload = makeInitPayload({
        breadcrumbs: [{ type: 'llm', label: 'LLM', sequenceId: 2 }],
        breadcrumbSequenceIds: new Set([2]),
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.breadcrumbs).toHaveLength(1);
      expect(next.breadcrumbs[0]!.type).toBe('llm');
    });

    // Task 24683 defensive dedup: even if the server unexpectedly re-sends
    // messages the client already has, the client must not display them
    // twice. `sse_message` already dedups by message_id and sequence_id;
    // this proves `sse_init`'s merge path matches that discipline.
    it('drops overlapping messages by sequence_id on reconnect merge', () => {
      const existing = makeMessage(3);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastSequenceId: 3,
        messages: [existing],
      };
      // Server sends [3, 4] even though client already has 3 (off-by-one
      // or server bug). The client must keep exactly one copy of 3.
      const payload = makeInitPayload({
        messages: [makeMessage(3), makeMessage(4)],
        lastSequenceId: 4,
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(2);
      expect(next.messages.map((m) => m.sequence_id)).toEqual([3, 4]);
    });

    it('drops overlapping messages by message_id on reconnect merge', () => {
      // Same story but the server reassigned sequence_id (hypothetical).
      // message_id is the stable identifier; the incoming version replaces the existing.
      const existing: Message = { ...makeMessage(3), message_id: 'stable-id' };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastSequenceId: 3,
        messages: [existing],
      };
      const incoming: Message = { ...makeMessage(4), message_id: 'stable-id' };
      const payload = makeInitPayload({
        messages: [incoming],
        lastSequenceId: 4,
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(1);
      expect(next.messages[0]!.message_id).toBe('stable-id');
    });

    // Reconnect gap: client disconnects, sub-agent run completes (display_data mutated
    // in DB), client reconnects. The full message list from the server must overwrite the
    // stale display_data the client already had — not silently skip it as a duplicate.
    it('replaces existing message in-place on full-resync when display_data changed', () => {
      const staleMsg: Message = {
        ...makeMessage(5),
        display_data: { type: 'spawning' } as Record<string, unknown>,
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [staleMsg],
        lastSequenceId: 5,
      };
      const freshMsg: Message = {
        ...staleMsg,
        display_data: { type: 'subagent_summary', results: [] } as Record<string, unknown>,
      };
      const payload = makeInitPayload({
        messages: [freshMsg],
        lastSequenceId: 5,
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(1);
      expect((next.messages[0]!.display_data as { type: string }).type).toBe('subagent_summary');
    });
  });

  describe('sse_message', () => {
    it('appends new message and advances lastSequenceId', () => {
      const atom = createInitialAtom();
      const msg = makeMessage(10);

      const next = dispatch(atom, { type: 'sse_message', message: msg, sequenceId: 10 });

      expect(next.messages).toHaveLength(1);
      expect(next.lastSequenceId).toBe(10);
    });

    it('is a no-op for duplicate sequenceId (not in messages)', () => {
      // Duplicate seq id with no existing message_id match — pure dedup case.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastSequenceId: 10,
      };

      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(10),
        sequenceId: 10,
      });

      expect(next).toBe(atom); // Same reference — no update
    });

    it('is a no-op for sequenceId below lastSequenceId', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 20 };

      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(15),
        sequenceId: 15,
      });

      expect(next).toBe(atom);
    });

    it('clears streamingBuffer atomically on message arrival', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        streamingBuffer: { text: 'partial text', lastSequence: 8, startedAt: Date.now() },
      };

      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(9),
        sequenceId: 9,
      });

      expect(next.streamingBuffer).toBeNull();
      expect(next.messages).toHaveLength(1);
    });

    // Regression: sse_message with an existing message_id but sequenceId below
    // lastSequenceId must be a NO-OP. The old runtime message_id lookup is gone —
    // mutations arrive via sse_message_updated instead.
    it('is a no-op when message_id already exists and sequenceId is below lastSequenceId', () => {
      const original: Message = {
        ...makeMessage(5, 'agent'),
        message_type: 'tool',
        content: {
          tool_use_id: 'toolu-spawn',
          content: 'Spawning 3 sub-agents...',
          is_error: false,
        } as Message['content'],
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastSequenceId: 42,
      };
      const rebroadcast: Message = { ...original, display_data: { type: 'subagent_summary' } };

      const next = dispatch(atom, {
        type: 'sse_message',
        message: rebroadcast,
        sequenceId: 5,
      });

      expect(next).toBe(atom);
    });

    it('updates resultSummary on matching tool breadcrumb when tool result arrives', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        breadcrumbs: [
          { type: 'user', label: 'User' },
          { type: 'tool', label: 'bash', toolId: 'toolu-abc', sequenceId: 5 },
        ],
      };
      const toolResultMsg: Message = {
        message_id: 'msg-10',
        sequence_id: 10,
        conversation_id: 'conv-1',
        message_type: 'tool',
        content: { tool_use_id: 'toolu-abc', content: 'hello world\nmore output', is_error: false } as Message['content'],
        created_at: '2024-01-01T00:00:00Z',
      };

      const next = dispatch(atom, { type: 'sse_message', message: toolResultMsg, sequenceId: 10 });

      const toolCrumb = next.breadcrumbs.find((b) => b.toolId === 'toolu-abc');
      expect(toolCrumb?.resultSummary).toBe('hello world');
    });

    it('sets error resultSummary when tool result is an error', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        breadcrumbs: [
          { type: 'tool', label: 'bash', toolId: 'toolu-xyz', sequenceId: 5 },
        ],
      };
      const toolResultMsg: Message = {
        message_id: 'msg-11',
        sequence_id: 11,
        conversation_id: 'conv-1',
        message_type: 'tool',
        content: {
          tool_use_id: 'toolu-xyz',
          content: '[command failed: exit code 1]\nsome output',
          is_error: true,
        } as Message['content'],
        created_at: '2024-01-01T00:00:00Z',
      };

      const next = dispatch(atom, { type: 'sse_message', message: toolResultMsg, sequenceId: 11 });

      const toolCrumb = next.breadcrumbs.find((b) => b.toolId === 'toolu-xyz');
      expect(toolCrumb?.resultSummary).toBe('error: [command failed: exit code 1]');
    });

    it('does not modify breadcrumbs when tool_use_id has no matching breadcrumb', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        breadcrumbs: [{ type: 'tool', label: 'bash', toolId: 'toolu-other', sequenceId: 5 }],
      };
      const toolResultMsg: Message = {
        message_id: 'msg-12',
        sequence_id: 12,
        conversation_id: 'conv-1',
        message_type: 'tool',
        content: { tool_use_id: 'toolu-nomatch', content: 'output' } as Message['content'],
        created_at: '2024-01-01T00:00:00Z',
      };

      const next = dispatch(atom, { type: 'sse_message', message: toolResultMsg, sequenceId: 12 });

      expect(next.breadcrumbs[0]?.resultSummary).toBeUndefined();
    });

    it('resets breadcrumbs on user message', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        breadcrumbs: [{ type: 'llm', label: 'LLM' }, { type: 'tool', label: 'bash' }],
      };
      const userMsg = makeMessage(10, 'user');

      const next = dispatch(atom, {
        type: 'sse_message',
        message: userMsg,
        sequenceId: 10,
      });

      expect(next.breadcrumbs).toHaveLength(1);
      expect(next.breadcrumbs[0]!.type).toBe('user');
    });
  });

  describe('sse_message_updated', () => {
    // Regression: spawn_agents tool_result gets display_data refreshed AFTER many
    // later SSE events have advanced lastSequenceId. The backend broadcasts
    // MessageUpdated (not Message) so the monotonic cursor guard never applies.
    it('applies display_data update-in-place even when lastSequenceId is high', () => {
      const original: Message = {
        ...makeMessage(5, 'agent'),
        message_type: 'tool',
        content: {
          tool_use_id: 'toolu-spawn',
          content: 'Spawning 3 sub-agents...',
          is_error: false,
        } as Message['content'],
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastSequenceId: 42,
      };
      const summaryDisplayData: Record<string, unknown> = {
        type: 'subagent_summary',
        results: [{ agent_id: 'a1', task: 't', outcome: { type: 'success', result: 'done' } }],
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        messageId: original.message_id,
        displayData: summaryDisplayData,
      });

      expect(next.messages).toHaveLength(1);
      expect(next.messages[0]!.display_data).toEqual(summaryDisplayData);
      expect(next.lastSequenceId).toBe(42);
    });

    it('does not touch lastSequenceId or streamingBuffer', () => {
      const original = makeMessage(5);
      const buffer = { text: 'streaming...', lastSequence: 40, startedAt: Date.now() };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastSequenceId: 42,
        streamingBuffer: buffer,
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        messageId: original.message_id,
        displayData: { type: 'updated' },
      });

      expect(next.streamingBuffer).toBe(buffer);
      expect(next.lastSequenceId).toBe(42);
    });

    it('is a no-op when message_id is unknown', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [makeMessage(5)],
        lastSequenceId: 10,
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        messageId: 'nonexistent-id',
        displayData: { type: 'whatever' },
      });

      expect(next).toBe(atom);
    });

    it('merges content and display_data independently', () => {
      const original: Message = {
        ...makeMessage(5),
        display_data: { type: 'original' } as Record<string, unknown>,
        content: { text: 'original content' } as Message['content'],
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastSequenceId: 10,
      };

      // Update only display_data, not content
      const next = dispatch(atom, {
        type: 'sse_message_updated',
        messageId: original.message_id,
        displayData: { type: 'new_display' },
      });

      expect((next.messages[0]!.display_data as { type: string }).type).toBe('new_display');
      expect((next.messages[0]!.content as { text: string }).text).toBe('original content');
    });
  });

  describe('sse_state_change', () => {
    it('updates phase', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_state_change',
        phase: { type: 'awaiting_llm' },
      });

      expect(next.phase.type).toBe('awaiting_llm');
    });

    it('appends breadcrumb for tool_executing', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_state_change',
        phase: {
          type: 'tool_executing',
          current_tool: { id: 'tool-1', input: { _tool: 'bash' } },
          remaining_tools: [],
        },
        sequenceId: 5,
      });

      expect(next.breadcrumbs).toHaveLength(1);
      expect(next.breadcrumbs[0]!.type).toBe('tool');
      expect(next.breadcrumbs[0]!.label).toBe('bash');
    });

    it('is a no-op for sequenceId already seen', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 10 };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        phase: { type: 'awaiting_llm' },
        sequenceId: 10,
      });

      expect(next).toBe(atom);
    });

    it('replaces LLM breadcrumb on retry', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        breadcrumbs: [{ type: 'llm', label: 'LLM', sequenceId: 5 }],
      };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        phase: { type: 'llm_requesting', attempt: 2 },
        sequenceId: 10,
      });

      expect(next.breadcrumbs).toHaveLength(1);
      expect(next.breadcrumbs[0]!.label).toBe('LLM (retry 2)');
    });

    it('replaces subagents breadcrumb on count update', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        breadcrumbs: [{ type: 'subagents', label: 'sub-agents (0/2)', sequenceId: 3 }],
      };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        phase: {
          type: 'awaiting_sub_agents',
          pending: [{ agent_id: 'a2', task: 'task2' }],
          completed_results: [
            { agent_id: 'a1', task: 'task1', outcome: { type: 'success' } },
          ],
        },
        sequenceId: 8,
      });

      expect(next.breadcrumbs).toHaveLength(1);
      expect(next.breadcrumbs[0]!.label).toBe('sub-agents (1/2)');
    });

    it('does not update lastSequenceId when sequenceId is absent', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 5 };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        phase: { type: 'awaiting_llm' },
        // No sequenceId
      });

      expect(next.lastSequenceId).toBe(5); // Unchanged
    });
  });

  describe('sse_agent_done', () => {
    it('resets phase to idle', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        phase: { type: 'awaiting_llm' },
      };

      const next = dispatch(atom, { type: 'sse_agent_done', sequenceId: 20 });

      expect(next.phase.type).toBe('idle');
    });

    it('clears streaming buffer', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        streamingBuffer: { text: 'incomplete', lastSequence: 15, startedAt: Date.now() },
      };

      const next = dispatch(atom, { type: 'sse_agent_done' });

      expect(next.streamingBuffer).toBeNull();
    });

    it('is a no-op if sequenceId already seen', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastSequenceId: 25,
        phase: { type: 'awaiting_llm' },
      };

      const next = dispatch(atom, { type: 'sse_agent_done', sequenceId: 25 });

      expect(next).toBe(atom);
    });
  });

  describe('sse_token', () => {
    // Tokens are only accepted while phase === 'llm_requesting' (task 24683).
    // All tests in this block set that phase first to mirror how the real
    // SSE stream looks: tokens only arrive during an in-flight LLM call.
    const llmRequestingAtom = (): ConversationAtom => ({
      ...createInitialAtom(),
      phase: { type: 'llm_requesting', attempt: 1 },
    });

    it('accumulates tokens in streaming buffer', () => {
      const atom = llmRequestingAtom();

      const s1 = dispatch(atom, { type: 'sse_token', delta: 'Hello', sequence: 1 });
      const s2 = dispatch(s1, { type: 'sse_token', delta: ' world', sequence: 2 });

      expect(s2.streamingBuffer?.text).toBe('Hello world');
    });

    it('is a no-op for duplicate or out-of-order sequence', () => {
      const atom: ConversationAtom = {
        ...llmRequestingAtom(),
        streamingBuffer: { text: 'Hello', lastSequence: 5, startedAt: Date.now() },
      };

      const next = dispatch(atom, { type: 'sse_token', delta: ' stale', sequence: 3 });

      expect(next).toBe(atom);
    });

    it('preserves startedAt across token accumulation', () => {
      const startedAt = Date.now() - 1000;
      const atom: ConversationAtom = {
        ...llmRequestingAtom(),
        streamingBuffer: { text: 'Hello', lastSequence: 1, startedAt },
      };

      const next = dispatch(atom, { type: 'sse_token', delta: '!', sequence: 2 });

      expect(next.streamingBuffer?.startedAt).toBe(startedAt);
    });

    // Task 24683 regression: tokens arriving after the phase has left
    // `llm_requesting` must be dropped. Otherwise a late token from a
    // previous turn creates a phantom streaming buffer below the
    // already-persisted assistant message — the client-facing half of the
    // "message repeats itself" bug.
    it('drops tokens when phase is idle', () => {
      const atom = createInitialAtom(); // default phase: idle
      const next = dispatch(atom, {
        type: 'sse_token',
        delta: 'ghost',
        sequence: 1,
      });
      expect(next).toBe(atom);
      expect(next.streamingBuffer).toBeNull();
    });

    it('drops tokens when phase is tool_executing', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        phase: {
          type: 'tool_executing',
          current_tool: { id: 'tool-1', input: { _tool: 'bash' } },
          remaining_tools: [],
        },
      };
      const next = dispatch(atom, {
        type: 'sse_token',
        delta: 'ghost',
        sequence: 1,
      });
      expect(next).toBe(atom);
    });
  });

  describe('sse_error', () => {
    it('sets uiError', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_error',
        error: { type: 'BackendError', message: 'Something went wrong' },
      });

      expect(next.uiError).toEqual({ type: 'BackendError', message: 'Something went wrong' });
    });
  });

  describe('connection_state', () => {
    it('updates connectionState', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, { type: 'connection_state', state: 'live' });

      expect(next.connectionState).toBe('live');
    });
  });

  describe('set_initial_data', () => {
    it('sets initial data when atom is fresh', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'set_initial_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(3)],
        phase: { type: 'idle' },
        contextWindow: { used: 500 },
      });

      expect(next.conversationId).toBe('conv-1');
      expect(next.messages).toHaveLength(1);
      expect(next.contextWindow.used).toBe(500);
    });

    it('is a no-op if SSE data already present', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 5 };

      const next = dispatch(atom, {
        type: 'set_initial_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
      });

      expect(next).toBe(atom);
    });
  });

  describe('set_system_prompt', () => {
    it('stores system prompt in atom', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, { type: 'set_system_prompt', systemPrompt: 'You are helpful.' });

      expect(next.systemPrompt).toBe('You are helpful.');
    });
  });
});

describe('breadcrumbFromPhase', () => {
  it('returns null for non-breadcrumb phases', () => {
    expect(breadcrumbFromPhase({ type: 'idle' }, 1)).toBeNull();
    expect(breadcrumbFromPhase({ type: 'awaiting_llm' }, 1)).toBeNull();
    expect(breadcrumbFromPhase({ type: 'error', message: 'err' }, 1)).toBeNull();
  });

  it('returns tool breadcrumb with queue depth', () => {
    const crumb = breadcrumbFromPhase(
      {
        type: 'tool_executing',
        current_tool: { id: 't1', input: { _tool: 'bash' } },
        remaining_tools: [{ id: 't2', input: {} }, { id: 't3', input: {} }],
      },
      5
    );

    expect(crumb?.type).toBe('tool');
    expect(crumb?.label).toBe('bash (+2)');
    expect(crumb?.toolId).toBe('t1');
  });

  it('returns LLM breadcrumb with retry number', () => {
    const crumb = breadcrumbFromPhase({ type: 'llm_requesting', attempt: 3 }, 7);

    expect(crumb?.type).toBe('llm');
    expect(crumb?.label).toBe('LLM (retry 3)');
  });

  it('returns subagents breadcrumb with progress', () => {
    const crumb = breadcrumbFromPhase(
      {
        type: 'awaiting_sub_agents',
        pending: [{ agent_id: 'a2', task: 't2' }],
        completed_results: [{ agent_id: 'a1', task: 't1', outcome: { type: 'success' } }],
      },
      10
    );

    expect(crumb?.type).toBe('subagents');
    expect(crumb?.label).toBe('sub-agents (1/2)');
  });
});
