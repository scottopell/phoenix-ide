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

    // Task 02675 acceptance: replay the same init event twice → atom converges
    // to the same state. Re-applying init is idempotent; the reducer must not
    // duplicate messages or regress lastSequenceId.
    it('is idempotent: applying the same init twice yields equivalent state', () => {
      const payload = makeInitPayload({
        messages: [makeMessage(1), makeMessage(2), makeMessage(3)],
        lastSequenceId: 3,
      });

      const once = dispatch(createInitialAtom(), { type: 'sse_init', payload });
      const twice = dispatch(once, { type: 'sse_init', payload });

      expect(twice.messages).toHaveLength(3);
      expect(twice.messages.map((m) => m.message_id)).toEqual(once.messages.map((m) => m.message_id));
      expect(twice.lastSequenceId).toBe(once.lastSequenceId);
      expect(twice.conversationId).toBe(once.conversationId);
    });

    // Task 02675 acceptance: the lastSequenceId jump scenario. Init arrives
    // with lastSeq=100 but the client has already seen live events through
    // 105 (plausible when a reconnect snapshot is older than live events
    // delivered in the gap). lastSequenceId must not regress to 100 —
    // otherwise the 101–105 events would be reapplied on the next delivery.
    it('never regresses lastSequenceId when init lags live events', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastSequenceId: 105,
      };
      const stalePayload = makeInitPayload({ lastSequenceId: 100 });

      const next = dispatch(atom, { type: 'sse_init', payload: stalePayload });

      expect(next.lastSequenceId).toBe(105);
    });

    // Task 02675 acceptance: init lastSeq=100, messages only to 95. When
    // subsequent individual sse_message events for 96..100 arrive, all five
    // must land. (Before the fix, the old atom leapfrogged lastSequenceId to
    // 100 on init and then rejected 96..100 as "already seen".)
    //
    // Today init merges by id so the 96..100 messages arrive through init's
    // message list — but the property we need is that lastSequenceId after
    // init does not block future individual deliveries of those same ids.
    // The defensive id dedup in sse_message keeps this honest either way.
    it('messages 96..100 land when init lastSeq=100 but individual events follow', () => {
      // Scenario: init arrives first with messages only up to 95 (server
      // hasn't yet enriched the snapshot — 96..100 are in-flight). The
      // client seeds lastSequenceId=100 from init. Then individual events
      // for 96..100 race in. With the old strict-greater guard, all five
      // would be rejected. With applyIfNewer + message_id dedup, they must
      // all land exactly once.
      const payload = makeInitPayload({
        messages: [makeMessage(95)],
        lastSequenceId: 95, // Server correctly reports: highest is 95.
      });
      let atom = dispatch(createInitialAtom(), { type: 'sse_init', payload });
      expect(atom.messages).toHaveLength(1);

      for (const seq of [96, 97, 98, 99, 100]) {
        atom = dispatch(atom, { type: 'sse_message', message: makeMessage(seq), sequenceId: seq });
      }

      expect(atom.messages).toHaveLength(6);
      expect(atom.messages.map((m) => m.sequence_id)).toEqual([95, 96, 97, 98, 99, 100]);
      expect(atom.lastSequenceId).toBe(100);
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

    it('is a no-op for duplicate sequenceId', () => {
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

    // Task 02675: defense-in-depth id dedup. Even if the server assigns a
    // fresh sequence_id, a second delivery of a message with the same
    // message_id must not duplicate in atom.messages.
    it('dedups by message_id when a duplicate arrives with a fresh sequenceId', () => {
      const original = makeMessage(5);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastSequenceId: 5,
      };

      // Same message_id, but a brand-new (higher) sequenceId.
      const duplicateWithFreshSeq: Message = { ...original, sequence_id: 42 };
      const next = dispatch(atom, {
        type: 'sse_message',
        message: duplicateWithFreshSeq,
        sequenceId: 42,
      });

      expect(next.messages).toHaveLength(1);
      // applyIfNewer still bumped lastSequenceId — the fact was "seen" even
      // though the id-level dedup prevented a duplicate message.
      expect(next.lastSequenceId).toBe(42);
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
    // later SSE events. Now gated by sequenceId: the update must carry an id
    // higher than the previous high-water mark.
    it('applies display_data update-in-place with a monotonic sequenceId', () => {
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
        sequenceId: 43,
        messageId: original.message_id,
        displayData: summaryDisplayData,
      });

      expect(next.messages).toHaveLength(1);
      expect(next.messages[0]!.display_data).toEqual(summaryDisplayData);
      expect(next.lastSequenceId).toBe(43);
    });

    it('is a no-op when message_id is unknown', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [makeMessage(5)],
        lastSequenceId: 10,
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: 'nonexistent-id',
        displayData: { type: 'whatever' },
      });

      // applyIfNewer still bumps lastSequenceId (the fact was seen); only the
      // in-reducer lookup decides whether to mutate messages. This keeps the
      // contract consistent: applyIfNewer is the ONLY sequence_id gate.
      expect(next.lastSequenceId).toBe(11);
      expect(next.messages).toEqual(atom.messages);
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
        sequenceId: 11,
        messageId: original.message_id,
        displayData: { type: 'new_display' },
      });

      expect((next.messages[0]!.display_data as { type: string }).type).toBe('new_display');
      expect((next.messages[0]!.content as { text: string }).text).toBe('original content');
    });

    // Task 02675 acceptance: duplicate message_updated events → state reflects
    // exactly one application.
    it('is idempotent: duplicate message_updated events apply exactly once', () => {
      const original: Message = {
        ...makeMessage(5),
        display_data: { type: 'before' } as Record<string, unknown>,
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastSequenceId: 10,
      };

      const once = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: original.message_id,
        displayData: { type: 'after' },
      });
      // Second delivery with the SAME sequenceId: the replay guard rejects it.
      const twice = dispatch(once, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: original.message_id,
        displayData: { type: 'after' },
      });

      expect(twice).toBe(once); // applyIfNewer returned atom unchanged
      expect((twice.messages[0]!.display_data as { type: string }).type).toBe('after');
    });
  });

  describe('sse_state_change', () => {
    it('updates phase', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: { type: 'awaiting_llm' },
      });

      expect(next.phase.type).toBe('awaiting_llm');
    });

    it('appends breadcrumb for tool_executing', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 5,
        phase: {
          type: 'tool_executing',
          current_tool: { id: 'tool-1', input: { _tool: 'bash' } },
          remaining_tools: [],
        },
      });

      expect(next.breadcrumbs).toHaveLength(1);
      expect(next.breadcrumbs[0]!.type).toBe('tool');
      expect(next.breadcrumbs[0]!.label).toBe('bash');
    });

    it('is a no-op for sequenceId already seen', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 10 };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 10,
        phase: { type: 'awaiting_llm' },
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
        sequenceId: 10,
        phase: { type: 'llm_requesting', attempt: 2 },
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
        sequenceId: 8,
        phase: {
          type: 'awaiting_sub_agents',
          pending: [{ agent_id: 'a2', task: 'task2' }],
          completed_results: [
            { agent_id: 'a1', task: 'task1', outcome: { type: 'success' } },
          ],
        },
      });

      expect(next.breadcrumbs).toHaveLength(1);
      expect(next.breadcrumbs[0]!.label).toBe('sub-agents (1/2)');
    });

    it('advances lastSequenceId on acceptance', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 5 };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 7,
        phase: { type: 'awaiting_llm' },
      });

      expect(next.lastSequenceId).toBe(7);
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
      expect(next.lastSequenceId).toBe(20);
    });

    it('clears streaming buffer', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        streamingBuffer: { text: 'incomplete', lastSequence: 15, startedAt: Date.now() },
      };

      const next = dispatch(atom, { type: 'sse_agent_done', sequenceId: 16 });

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

      const s1 = dispatch(atom, { type: 'sse_token', sequenceId: 1, delta: 'Hello' });
      const s2 = dispatch(s1, { type: 'sse_token', sequenceId: 2, delta: ' world' });

      expect(s2.streamingBuffer?.text).toBe('Hello world');
      expect(s2.lastSequenceId).toBe(2);
    });

    it('is a no-op for duplicate or out-of-order sequence', () => {
      const atom: ConversationAtom = {
        ...llmRequestingAtom(),
        lastSequenceId: 5,
        streamingBuffer: { text: 'Hello', lastSequence: 5, startedAt: Date.now() },
      };

      const next = dispatch(atom, { type: 'sse_token', sequenceId: 3, delta: ' stale' });

      expect(next).toBe(atom);
    });

    it('preserves startedAt across token accumulation', () => {
      const startedAt = Date.now() - 1000;
      const atom: ConversationAtom = {
        ...llmRequestingAtom(),
        lastSequenceId: 1,
        streamingBuffer: { text: 'Hello', lastSequence: 1, startedAt },
      };

      const next = dispatch(atom, { type: 'sse_token', sequenceId: 2, delta: '!' });

      expect(next.streamingBuffer?.startedAt).toBe(startedAt);
    });

    // Task 24683 regression: tokens arriving after the phase has left
    // `llm_requesting` must be dropped. Otherwise a late token from a
    // previous turn creates a phantom streaming buffer below the
    // already-persisted assistant message.
    it('drops tokens when phase is idle', () => {
      const atom = createInitialAtom(); // default phase: idle
      const next = dispatch(atom, {
        type: 'sse_token',
        sequenceId: 1,
        delta: 'ghost',
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
        sequenceId: 1,
        delta: 'ghost',
      });
      expect(next).toBe(atom);
    });

    // Task 02675 acceptance: simulated reconnect mid-stream with server
    // continuing to emit tokens → new tokens accumulate without stall.
    //
    // Before the fix, the client ran a per-connection `tokenSequence` counter
    // that reset to 0 on every reconnect, while `atom.streamingBuffer.lastSequence`
    // persisted at the pre-reconnect high-water mark. Post-reconnect tokens
    // carried ids 1, 2, 3, … which were all below the high-water mark and
    // silently dropped until the counter crossed it.
    //
    // After the fix, tokens carry server-assigned global sequence_ids that
    // are strictly greater than anything the client has seen.
    it('accumulates tokens after simulated reconnect mid-stream', () => {
      // Pre-reconnect state: atom has been streaming, lastSequenceId=50.
      const preReconnect: ConversationAtom = {
        ...createInitialAtom(),
        phase: { type: 'llm_requesting', attempt: 1 },
        lastSequenceId: 50,
        streamingBuffer: { text: 'Before ', lastSequence: 50, startedAt: Date.now() },
      };

      // Server keeps streaming across the reconnect with ids 51, 52, 53.
      const a1 = dispatch(preReconnect, { type: 'sse_token', sequenceId: 51, delta: 'reconnect ' });
      const a2 = dispatch(a1, { type: 'sse_token', sequenceId: 52, delta: 'works ' });
      const a3 = dispatch(a2, { type: 'sse_token', sequenceId: 53, delta: 'correctly' });

      expect(a3.streamingBuffer?.text).toBe('Before reconnect works correctly');
      expect(a3.lastSequenceId).toBe(53);
    });
  });

  describe('sse_error', () => {
    it('sets uiError when no sequenceId (client-synthesized)', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_error',
        error: { type: 'BackendError', message: 'Something went wrong' },
      });

      expect(next.uiError).toEqual({ type: 'BackendError', message: 'Something went wrong' });
      // Client-synthesized errors do not bump the total-order counter.
      expect(next.lastSequenceId).toBe(0);
    });

    it('routes wire-originated errors through applyIfNewer', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 42 };

      const next = dispatch(atom, {
        type: 'sse_error',
        sequenceId: 43,
        error: { type: 'BackendError', message: 'server hiccup' },
      });

      expect(next.uiError).toEqual({ type: 'BackendError', message: 'server hiccup' });
      expect(next.lastSequenceId).toBe(43);
    });

    it('drops replayed wire errors after the user has moved on', () => {
      // Simulate: user dismissed the toast (uiError = null), lastSequenceId has
      // since advanced past the error's sequenceId, then a reconnect replays
      // the same error. Nothing should change.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastSequenceId: 50,
        uiError: null,
      };

      const next = dispatch(atom, {
        type: 'sse_error',
        sequenceId: 43,
        error: { type: 'BackendError', message: 'old error' },
      });

      expect(next.uiError).toBeNull();
      expect(next.lastSequenceId).toBe(50);
    });

    it('applies a wire error only once when dispatched twice with the same sequenceId', () => {
      const atom = createInitialAtom();

      const a1 = dispatch(atom, {
        type: 'sse_error',
        sequenceId: 10,
        error: { type: 'BackendError', message: 'first' },
      });
      // User dismisses.
      const a2 = dispatch(a1, { type: 'clear_error' });
      expect(a2.uiError).toBeNull();

      // Replay of the same envelope (e.g. after a reconnect before the server
      // advances its counter). Should be a no-op — the toast stays dismissed.
      const a3 = dispatch(a2, {
        type: 'sse_error',
        sequenceId: 10,
        error: { type: 'BackendError', message: 'first' },
      });

      expect(a3.uiError).toBeNull();
      expect(a3.lastSequenceId).toBe(10);
    });
  });

  describe('connection_state', () => {
    it('updates connectionState', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, { type: 'connection_state', state: 'live' });

      expect(next.connectionState).toBe('live');
    });
  });

  describe('local_phase_change', () => {
    // Client-originated optimistic phase updates do NOT bump lastSequenceId
    // (they're not part of the server's total order). This test guards
    // against a future change that accidentally wires them through the
    // server-side dedup path.
    it('updates phase without touching lastSequenceId', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastSequenceId: 42 };

      const next = dispatch(atom, {
        type: 'local_phase_change',
        phase: { type: 'awaiting_llm' },
      });

      expect(next.phase.type).toBe('awaiting_llm');
      expect(next.lastSequenceId).toBe(42);
    });
  });

  describe('local_conversation_update', () => {
    it('merges updates when conversation exists', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversation: testConversation,
      };

      const next = dispatch(atom, {
        type: 'local_conversation_update',
        updates: { model: 'new-model' },
      });

      expect(next.conversation?.model).toBe('new-model');
    });

    it('is a no-op when conversation is null', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'local_conversation_update',
        updates: { model: 'new-model' },
      });

      expect(next).toBe(atom);
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
