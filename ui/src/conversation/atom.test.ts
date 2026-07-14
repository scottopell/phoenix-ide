import { describe, it, expect } from 'vitest';
import {
  conversationReducer,
  createInitialAtom,
  type ConversationAtom,
  type SSEAction,
  type InitPayload,
} from './atom';
import type { Conversation, Message } from '../api';
import { derivePendingMessages, type QueuedMessage } from '../hooks/useMessageQueue';

// Minimal test fixtures
const testConversation: Conversation = {
  id: 'conv-1',
  slug: 'test-slug',
  model: 'claude-3-5-sonnet',
  cwd: '/home/user/project',
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
  message_count: 0,
  transcript_generation: 1,
  browser_session_active: false,
  terminal_uses_tmux: false,
  work_scope_key: 'conversation:conv-1',
};

function makeMessage(
  sequenceId: number,
  messageTypeOrOverrides: 'user' | 'agent' | Partial<Message> = 'agent',
): Message {
  const overrides = typeof messageTypeOrOverrides === 'string'
    ? { message_type: messageTypeOrOverrides }
    : messageTypeOrOverrides;
  return {
    message_id: `msg-${sequenceId}`,
    sequence_id: sequenceId,
    conversation_id: 'conv-1',
    message_type: 'agent',
    content: { text: `message ${sequenceId}` } as Message['content'],
    created_at: '2024-01-01T00:00:00Z',
    ...overrides,
  };
}

function makeInitPayload(overrides: Partial<InitPayload> = {}): InitPayload {
  const lastAppliedEventSeq = overrides.lastAppliedEventSeq ?? 5;
  return {
    conversation: testConversation,
    messages: [],
    phase: { type: 'idle' },
    contextWindow: { used: 1000 },
    transcriptGeneration: 1,
    lastAppliedEventSeq,
    // Default to "no pending replay": anchor matches the tip and the ring
    // is empty. Tests exercising ReplayRing behaviour override these.
    pendingAnchorSequenceId: lastAppliedEventSeq,
    pendingEvents: [],
    pendingTruncated: false,
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
        lastAppliedEventSeq: 5,
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.conversationId).toBe('conv-1');
      expect(next.messages).toHaveLength(2);
      expect(next.lastAppliedEventSeq).toBe(5);
      expect(next.transcriptCoverage).toBe('complete');
      expect(next.streamingBuffer).toBeNull();
    });

    it('merges delta messages on reconnect (lastAppliedEventSeq > 0)', () => {
      const existing = makeMessage(3);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 3,
        messages: [existing],
      };
      const newMsg = makeMessage(4);
      const payload = makeInitPayload({ messages: [newMsg], lastAppliedEventSeq: 4 });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(2);
      expect(next.messages[0]!.sequence_id).toBe(3);
      expect(next.messages[1]!.sequence_id).toBe(4);
    });

    it('replaces messages on fresh connect (lastAppliedEventSeq = 0)', () => {
      const payload = makeInitPayload({ messages: [makeMessage(1), makeMessage(2)] });
      const atom = createInitialAtom();

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(2);
    });

    it('merges a generation-matched suffix into a REST tail on first SSE connect', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [makeMessage(40), makeMessage(50)],
        transcriptGeneration: 3,
      };
      const payload = makeInitPayload({
        messages: [makeMessage(51)],
        transcriptGeneration: 3,
        messageSnapshot: 'suffix',
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages.map((message) => message.sequence_id)).toEqual([40, 50, 51]);
    });

    it('replaces a suffix snapshot when the local transcript generation is unknown', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [makeMessage(40), makeMessage(50)],
        transcriptGeneration: null,
      };
      const payload = makeInitPayload({
        messages: [makeMessage(51)],
        transcriptGeneration: 3,
        messageSnapshot: 'suffix',
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages.map((message) => message.sequence_id)).toEqual([51]);
      expect(next.transcriptGeneration).toBe(3);
      expect(next.transcriptCoverage).toBe('tail');
    });

    it('replaces a suffix snapshot on reconnect when local transcript generation is unknown', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 7,
        messages: [makeMessage(40), makeMessage(50)],
        transcriptGeneration: null,
      };
      const payload = makeInitPayload({
        messages: [makeMessage(51)],
        transcriptGeneration: 3,
        messageSnapshot: 'suffix',
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages.map((message) => message.sequence_id)).toEqual([51]);
      expect(next.transcriptGeneration).toBe(3);
      expect(next.transcriptCoverage).toBe('tail');
    });

    it('marks suffix init as tail after empty local state even when generation matches', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        transcriptGeneration: 3,
        transcriptCoverage: 'complete',
      };
      const payload = makeInitPayload({
        messages: [makeMessage(51)],
        transcriptGeneration: 3,
        messageSnapshot: 'suffix',
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.transcriptCoverage).toBe('tail');
    });

    it('marks full init as complete', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [makeMessage(40)],
        transcriptGeneration: 3,
        transcriptCoverage: 'tail',
      };
      const payload = makeInitPayload({
        messages: [makeMessage(1), makeMessage(2)],
        transcriptGeneration: 3,
        messageSnapshot: 'full',
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.transcriptCoverage).toBe('complete');
    });

    it('preserves known complete coverage on reconnect suffix init', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 7,
        messages: [makeMessage(1), makeMessage(2)],
        transcriptGeneration: 3,
        transcriptCoverage: 'complete',
      };
      const payload = makeInitPayload({
        messages: [makeMessage(3)],
        transcriptGeneration: 3,
        messageSnapshot: 'suffix',
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages.map((message) => message.sequence_id)).toEqual([1, 2, 3]);
      expect(next.transcriptCoverage).toBe('complete');
    });

    it('replaces a REST tail when the SSE transcript generation changed', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [makeMessage(40), makeMessage(50)],
        transcriptGeneration: 2,
      };
      const payload = makeInitPayload({
        messages: [makeMessage(1)],
        transcriptGeneration: 3,
        messageSnapshot: 'full',
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages.map((message) => message.sequence_id)).toEqual([1]);
      expect(next.transcriptCoverage).toBe('complete');
    });

    it('clears streaming buffer on init', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        streamingBuffer: { text: 'partial', lastSequence: 3, startedAt: Date.now(), requestId: 'test-req-id' },
      };

      const next = dispatch(atom, { type: 'sse_init', payload: makeInitPayload() });

      expect(next.streamingBuffer).toBeNull();
    });

    // Task 24683 defensive dedup: even if the server unexpectedly re-sends
    // messages the client already has, the client must not display them
    // twice. `sse_message` already dedups by message_id and sequence_id;
    // this proves `sse_init`'s merge path matches that discipline.
    it('drops overlapping messages by sequence_id on reconnect merge', () => {
      const existing = makeMessage(3);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 3,
        messages: [existing],
      };
      // Server sends [3, 4] even though client already has 3 (off-by-one
      // or server bug). The client must keep exactly one copy of 3.
      const payload = makeInitPayload({
        messages: [makeMessage(3), makeMessage(4)],
        lastAppliedEventSeq: 4,
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
        lastAppliedEventSeq: 3,
        messages: [existing],
      };
      const incoming: Message = { ...makeMessage(4), message_id: 'stable-id' };
      const payload = makeInitPayload({
        messages: [incoming],
        lastAppliedEventSeq: 4,
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
        lastAppliedEventSeq: 5,
      };
      const freshMsg: Message = {
        ...staleMsg,
        display_data: { type: 'subagent_summary', results: [] } as Record<string, unknown>,
      };
      const payload = makeInitPayload({
        messages: [freshMsg],
        lastAppliedEventSeq: 5,
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages).toHaveLength(1);
      expect((next.messages[0]!.display_data as { type: string }).type).toBe('subagent_summary');
    });

    // Task 02675 acceptance: replay the same init event twice → atom converges
    // to the same state. Re-applying init is idempotent; the reducer must not
    // duplicate messages or regress lastAppliedEventSeq.
    it('is idempotent: applying the same init twice yields equivalent state', () => {
      const payload = makeInitPayload({
        messages: [makeMessage(1), makeMessage(2), makeMessage(3)],
        lastAppliedEventSeq: 3,
      });

      const once = dispatch(createInitialAtom(), { type: 'sse_init', payload });
      const twice = dispatch(once, { type: 'sse_init', payload });

      expect(twice.messages).toHaveLength(3);
      expect(twice.messages.map((m) => m.message_id)).toEqual(once.messages.map((m) => m.message_id));
      expect(twice.lastAppliedEventSeq).toBe(once.lastAppliedEventSeq);
      expect(twice.conversationId).toBe(once.conversationId);
    });

    // Task 02675 acceptance: the lastAppliedEventSeq jump scenario. Init arrives
    // with lastSeq=100 but the client has already seen live events through
    // 105 (plausible when a reconnect snapshot is older than live events
    // delivered in the gap). lastAppliedEventSeq must not regress to 100 —
    // otherwise the 101–105 events would be reapplied on the next delivery.
    it('never regresses lastAppliedEventSeq when init lags live events', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 105,
      };
      const stalePayload = makeInitPayload({ lastAppliedEventSeq: 100 });

      const next = dispatch(atom, { type: 'sse_init', payload: stalePayload });

      expect(next.lastAppliedEventSeq).toBe(105);
    });

    // Task 02675 acceptance: init lastSeq=100, messages only to 95. When
    // subsequent individual sse_message events for 96..100 arrive, all five
    // must land. (Before the fix, the old atom leapfrogged lastAppliedEventSeq to
    // 100 on init and then rejected 96..100 as "already seen".)
    //
    // Today init merges by id so the 96..100 messages arrive through init's
    // message list — but the property we need is that lastAppliedEventSeq after
    // init does not block future individual deliveries of those same ids.
    // The defensive id dedup in sse_message keeps this honest either way.
    it('messages 96..100 land when init lastSeq=100 but individual events follow', () => {
      // Scenario: init arrives first with messages only up to 95 (server
      // hasn't yet enriched the snapshot — 96..100 are in-flight). The
      // client seeds lastAppliedEventSeq=100 from init. Then individual events
      // for 96..100 race in. With the old strict-greater guard, all five
      // would be rejected. With applyIfNewer + message_id dedup, they must
      // all land exactly once.
      const payload = makeInitPayload({
        messages: [makeMessage(95)],
        lastAppliedEventSeq: 95, // Server correctly reports: highest is 95.
      });
      let atom = dispatch(createInitialAtom(), { type: 'sse_init', payload });
      expect(atom.messages).toHaveLength(1);

      for (const seq of [96, 97, 98, 99, 100]) {
        atom = dispatch(atom, { type: 'sse_message', message: makeMessage(seq), sequenceId: seq });
      }

      expect(atom.messages).toHaveLength(6);
      expect(atom.messages.map((m) => m.sequence_id)).toEqual([95, 96, 97, 98, 99, 100]);
      expect(atom.lastAppliedEventSeq).toBe(100);
    });
  });

  describe('pending user-message reconciliation', () => {
    const queued: QueuedMessage = {
      localId: 'local-user-1',
      conversationId: 'conv-1',
      text: 'hello',
      images: [],
      timestamp: 1,
      status: 'pending',
    };

    it('keeps an omitted accepted local message pending when init advances past it without delivery evidence', () => {
      const payload = makeInitPayload({
        messages: [],
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 12,
        pendingAnchorSequenceId: 12,
        pendingEvents: [],
      });

      const atom = dispatch(createInitialAtom(), { type: 'sse_init', payload });
      const pending = derivePendingMessages([queued], atom.messages.map((m) => m.message_id));

      expect(atom.lastAppliedEventSeq).toBe(12);
      expect(pending.map((m) => m.localId)).toEqual(['local-user-1']);
    });

    it('removes the local pending bubble when authoritative history contains the same message_id', () => {
      const reflected: Message = {
        ...makeMessage(12, 'user'),
        message_id: 'local-user-1',
        content: { text: 'hello' } as Message['content'],
      };
      const payload = makeInitPayload({
        messages: [reflected],
        lastAppliedEventSeq: 12,
        pendingAnchorSequenceId: 12,
      });

      const atom = dispatch(createInitialAtom(), { type: 'sse_init', payload });
      const pending = derivePendingMessages([queued], atom.messages.map((m) => m.message_id));

      expect(atom.messages.map((m) => m.message_id)).toEqual(['local-user-1']);
      expect(pending).toEqual([]);
    });
  });

  // Phase 3 of the SSE ReplayRing rollout (`tasks/62002`). Pending events
  // delivered on the `init` snapshot are applied through the reducer's
  // per-event rules so a mid-turn reconnect restores in-flight streaming
  // text, current-tool state, and eager assistant Messages. See
  // `specs/conversation_atom/conversation_atom.allium` SseInitFreshConnect
  // and SseInitReconnectMerge.
  describe('sse_init pending replay', () => {
    // Pending replay tokens belong to the in-flight attempt, so they carry
    // the same request_id as the preserved streamingBuffer fixtures
    // ('test-req-id'). A reconnect replays the *same* attempt's tokens; a
    // genuinely new attempt (after a mid-stream retry) would carry a different
    // request_id and is covered by the buffer-keying tests below.
    function tokenEntry(seq: number, text: string): unknown {
      return { type: 'token', sequence_id: seq, text, request_id: 'test-req-id' };
    }
    function stateChangeEntry(seq: number, state: Record<string, unknown>): unknown {
      return {
        type: 'state_change',
        sequence_id: seq,
        state,
        presentation_mode: 'default',
        // REQ-WPV-001: required on the wire; tests use a stable fixture
        // value so the elapsed-time math is deterministic.
        state_updated_at: '2026-05-28T00:00:00.000Z',
      };
    }
    function messageEntry(seq: number, msg: Message): unknown {
      return { type: 'message', sequence_id: seq, message: msg };
    }
    it('fresh connect with pending tokens rebuilds streamingBuffer', () => {
      const payload = makeInitPayload({
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 7,
        pendingAnchorSequenceId: 5,
        pendingEvents: [
          tokenEntry(6, 'Hel'),
          tokenEntry(7, 'lo '),
        ],
      });

      const next = dispatch(createInitialAtom(), { type: 'sse_init', payload });

      expect(next.streamingBuffer).not.toBeNull();
      expect(next.streamingBuffer!.text).toBe('Hello ');
      expect(next.streamingBuffer!.lastSequence).toBe(7);
      expect(next.lastAppliedEventSeq).toBe(7);
    });

    // Load-bearing test for the SseInitFreshConnect rule: fresh-connect must
    // seed lastAppliedEventSeq from `pendingAnchorSequenceId`, NOT from
    // `lastAppliedEventSeq`. If it seeded from `lastAppliedEventSeq` (the server's
    // tip), every pending entry would be dropped as a replay because each
    // entry's seq ≤ tip. With the anchor as the floor, applyIfNewer(5, 6)
    // → 6 > 5 → accept.
    it('fresh-connect anchor: token at seq=anchor+1 is accepted, not dropped', () => {
      const payload = makeInitPayload({
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 6,
        pendingAnchorSequenceId: 5,
        pendingEvents: [tokenEntry(6, 'X')],
      });

      const next = dispatch(createInitialAtom(), { type: 'sse_init', payload });

      expect(next.streamingBuffer?.text).toBe('X');
      expect(next.lastAppliedEventSeq).toBe(6);
    });

    it('fresh connect with pending state_change updates phase to latest pending state', () => {
      const payload = makeInitPayload({
        phase: { type: 'awaiting_llm' },
        lastAppliedEventSeq: 7,
        pendingAnchorSequenceId: 5,
        pendingEvents: [
          stateChangeEntry(6, { type: 'llm_requesting', attempt: 1 }),
          stateChangeEntry(7, {
            type: 'tool_executing',
            current_tool: { id: 't1', name: 'bash', input: {} },
            remaining_tools: [],
          }),
        ],
      });

      const next = dispatch(createInitialAtom(), { type: 'sse_init', payload });

      expect(next.phase.type).toBe('tool_executing');
      expect(next.lastAppliedEventSeq).toBe(7);
    });

    it('reconnect buffers a pending eager Message when an earlier event is missing', () => {
      const existing = makeMessage(3, 'user');
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 5,
        messages: [existing],
        conversationId: 'conv-1',
      };
      const eagerMsg: Message = {
        message_id: 'eager-1',
        sequence_id: 7,
        conversation_id: 'conv-1',
        message_type: 'agent',
        content: [
          { type: 'tool_use', id: 'tool-1', name: 'bash', input: { command: 'ls' } },
        ] as Message['content'],
        created_at: '2024-01-01T00:00:00Z',
      };
      const payload = makeInitPayload({
        messages: [existing], // DB snapshot — eager not yet persisted
        lastAppliedEventSeq: 7,
        pendingAnchorSequenceId: 5,
        pendingEvents: [messageEntry(7, eagerMsg)],
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages.map((m) => m.message_id)).toEqual(['msg-3']);
      expect(next.lastAppliedEventSeq).toBe(5);
      expect(next.eventGap).toEqual({ expectedNextEventSeq: 6, firstBufferedEventSeq: 7 });
      expect(next.bufferedEventEnvelopes[7]).toMatchObject({ type: 'sse_message' });
    });
    it('pending patch survives event buffering/drain until the message arrives', () => {
      const seeded: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 10,
      };

      const pending = dispatch(seeded, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: 'late-msg',
        displayData: { type: 'patched-from-ring' },
      });
      expect(pending.lastAppliedEventSeq).toBe(11);
      expect(pending.pendingMessagePatches['late-msg']).toEqual({
        lastAppliedPatchEventSeq: 0,
        patches: [{ eventSeq: 11, displayData: { type: 'patched-from-ring' } }],
      });

      const buffered = dispatch(pending, {
        type: 'sse_state_change',
        sequenceId: 13,
        phase: { type: 'awaiting_llm' },
        stateUpdatedAt: 13,
      });
      expect(buffered.lastAppliedEventSeq).toBe(11);
      expect(buffered.eventGap).toEqual({ expectedNextEventSeq: 12, firstBufferedEventSeq: 13 });
      expect(buffered.pendingMessagePatches['late-msg']).toEqual({
        lastAppliedPatchEventSeq: 0,
        patches: [{ eventSeq: 11, displayData: { type: 'patched-from-ring' } }],
      });

      const drained = dispatch(buffered, {
        type: 'sse_state_change',
        sequenceId: 12,
        phase: { type: 'idle' },
        stateUpdatedAt: 12,
      });
      expect(drained.lastAppliedEventSeq).toBe(13);
      expect(drained.phase.type).toBe('awaiting_llm');
      expect(drained.pendingMessagePatches['late-msg']).toEqual({
        lastAppliedPatchEventSeq: 0,
        patches: [{ eventSeq: 11, displayData: { type: 'patched-from-ring' } }],
      });

      const delivered = dispatch(drained, {
        type: 'sse_message',
        sequenceId: 14,
        message: { ...makeMessage(2), message_id: 'late-msg', display_data: { existing: true } as Record<string, unknown> },
      });

      expect(delivered.lastAppliedEventSeq).toBe(14);
      expect(delivered.messages[0]!.message_id).toBe('late-msg');
      expect(delivered.messages[0]!.display_data).toEqual({ existing: true, type: 'patched-from-ring' });
      expect(delivered.pendingMessagePatches['late-msg']).toEqual({
        lastAppliedPatchEventSeq: 11,
        patches: [],
      });
    });

    it('pendingTruncated=true with empty pendingEvents yields DB-only render and advances lastAppliedEventSeq', () => {
      const payload = makeInitPayload({
        messages: [makeMessage(50)],
        lastAppliedEventSeq: 100,
        pendingAnchorSequenceId: 50,
        pendingEvents: [],
        pendingTruncated: true,
      });

      const next = dispatch(createInitialAtom(), { type: 'sse_init', payload });

      expect(next.lastAppliedEventSeq).toBe(100);
      expect(next.messages).toHaveLength(1);
      expect(next.messages[0]!.sequence_id).toBe(50);
      expect(next.uiError).toBeNull();
      expect(next.streamingBuffer).toBeNull();
      expect(next.eventGap).toBeNull();
    });

    // Reconnect floor stays at the atom's live tip. Pending entries whose
    // seq is at or below that floor are dropped by the per-event
    // applyIfNewer guard — exactly the dedup contract the spec relies on.
    it('reconnect drops pending entries with seq <= atom.lastAppliedEventSeq as replays', () => {
      const existing = makeMessage(10);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 10,
        messages: [existing],
        conversationId: 'conv-1',
      };
      const dupMsg: Message = {
        ...makeMessage(8),
        message_id: 'replay-id',
      };
      const payload = makeInitPayload({
        messages: [existing],
        lastAppliedEventSeq: 10,
        pendingAnchorSequenceId: 5,
        pendingEvents: [messageEntry(8, dupMsg)],
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.messages.map((m) => m.message_id)).toEqual(['msg-10']);
      expect(next.lastAppliedEventSeq).toBe(10);
    });

    // Malformed pending entries should not crash the init — the whole
    // value of the loose `unknown[]` wire shape is per-entry recoverability.
    it('skips malformed pending entries without crashing', () => {
      const payload = makeInitPayload({
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 7,
        pendingAnchorSequenceId: 5,
        pendingEvents: [
          { type: 'token', sequence_id: 'not-a-number', text: 'oops', request_id: 'r' },
          tokenEntry(7, 'ok'),
        ],
      });

      const next = dispatch(createInitialAtom(), { type: 'sse_init', payload });

      expect(next.streamingBuffer).toBeNull();
      expect(next.lastAppliedEventSeq).toBe(5);
      expect(next.eventGap).toEqual({ expectedNextEventSeq: 6, firstBufferedEventSeq: 7 });
    });

    // In-page reconnect after a network blip: the atom has already
    // accepted live tokens, so atom.lastAppliedEventSeq is at the live tip.
    // The init's pending tokens carry the same seqs and are dropped by
    // applyIfNewer as replays. If phase 1 cleared streamingBuffer, the
    // cleared buffer would have no way to rebuild (replays can't fill
    // it) and the user would see a blank window until the next live
    // token arrives. Phase 1 must preserve the buffer when the snapshot
    // phase is still llm_requesting. Codex P1 from PR #79.
    it('reconnect preserves streamingBuffer when snapshot phase is llm_requesting', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 7,
        phase: { type: 'llm_requesting', attempt: 1 },
        streamingBuffer: { text: 'Hello ', lastSequence: 7, startedAt: 1000, requestId: 'test-req-id' },
        conversationId: 'conv-1',
      };
      const payload = makeInitPayload({
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 7,
        pendingAnchorSequenceId: 5,
        pendingEvents: [
          tokenEntry(6, 'Hel'),
          tokenEntry(7, 'lo '),
        ],
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.streamingBuffer).not.toBeNull();
      expect(next.streamingBuffer!.text).toBe('Hello ');
      expect(next.streamingBuffer!.lastSequence).toBe(7);
      expect(next.lastAppliedEventSeq).toBe(7);
    });

    // Companion to the above: if the gap is real (server emitted tokens
    // 8, 9 while the atom was offline), pending tokens above the floor
    // extend the preserved buffer.
    it('reconnect extends preserved streamingBuffer with above-floor pending tokens', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 7,
        phase: { type: 'llm_requesting', attempt: 1 },
        streamingBuffer: { text: 'Hello ', lastSequence: 7, startedAt: 1000, requestId: 'test-req-id' },
        conversationId: 'conv-1',
      };
      const payload = makeInitPayload({
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 9,
        pendingAnchorSequenceId: 5,
        pendingEvents: [
          tokenEntry(6, 'Hel'),  // replay, dropped
          tokenEntry(7, 'lo '),  // replay, dropped
          tokenEntry(8, 'wor'),  // new, extends
          tokenEntry(9, 'ld'),   // new, extends
        ],
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.streamingBuffer!.text).toBe('Hello world');
      expect(next.streamingBuffer!.lastSequence).toBe(9);
      expect(next.lastAppliedEventSeq).toBe(9);
    });

    // Codex P2 from PR #79: when pendingTruncated=true the ring
    // overflowed, so the server is intentionally NOT sending the tokens
    // between anchor and tip. The safety belt advances lastAppliedEventSeq
    // past the gap. Preserving the buffer would leave a stale prefix
    // that future live tokens append onto, producing a gapped/corrupted
    // message. Truncated must force a clear regardless of phase.
    it('reconnect clears streamingBuffer when pendingTruncated even if phase is llm_requesting', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 7,
        phase: { type: 'llm_requesting', attempt: 1 },
        streamingBuffer: { text: 'Hello ', lastSequence: 7, startedAt: 1000, requestId: 'test-req-id' },
        conversationId: 'conv-1',
      };
      const payload = makeInitPayload({
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 50,
        pendingAnchorSequenceId: 7,
        pendingEvents: [],
        pendingTruncated: true,
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.streamingBuffer).toBeNull();
      expect(next.lastAppliedEventSeq).toBe(50);
      expect(next.eventGap).toBeNull();
    });

    // Turn ended while disconnected: snapshot phase is no longer
    // llm_requesting, so the buffer must clear (the next response will
    // start fresh; preserving stale text would confuse the UI).
    it('reconnect clears streamingBuffer when snapshot phase is not llm_requesting', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 7,
        phase: { type: 'llm_requesting', attempt: 1 },
        streamingBuffer: { text: 'Hello ', lastSequence: 7, startedAt: 1000, requestId: 'test-req-id' },
        conversationId: 'conv-1',
      };
      const payload = makeInitPayload({
        phase: { type: 'idle' },
        lastAppliedEventSeq: 10,
        pendingAnchorSequenceId: 7,
        pendingEvents: [],
      });

      const next = dispatch(atom, { type: 'sse_init', payload });

      expect(next.streamingBuffer).toBeNull();
      expect(next.phase.type).toBe('idle');
      expect(next.lastAppliedEventSeq).toBe(7);
      expect(next.eventGap).toBeNull();
    });
  });

  describe('sse_message', () => {
    it('appends new message and advances the event cursor', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastAppliedEventSeq: 9 };
      const msg = makeMessage(10);

      const next = dispatch(atom, { type: 'sse_message', message: msg, sequenceId: 10 });

      expect(next.messages).toHaveLength(1);
      expect(next.lastAppliedEventSeq).toBe(10);
    });

    it('consumes duplicate known message creates without replacing message content', () => {
      const existing = makeMessage(10, { content: [{ type: 'text', text: 'persisted' }] });
      const atom: ConversationAtom = { ...createInitialAtom(), messages: [existing], lastAppliedEventSeq: 9 };
      const next = dispatch(atom, {
        type: 'sse_message',
        sequenceId: 10,
        message: { ...existing, content: [{ type: 'text', text: 'replayed duplicate' }] },
      });

      expect(next.messages).toEqual([existing]);
      expect(next.lastAppliedEventSeq).toBe(10);
    });

    it('drops a message at or below the applied event cursor', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 10,
      };

      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(10),
        sequenceId: 10,
      });

      expect(next).toBe(atom);
    });

    it('buffers an ahead-of-cursor message and drains it when REST reaches the preceding floor', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastAppliedEventSeq: 100 };

      const buffered = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(501),
        sequenceId: 501,
      });

      expect(buffered.messages).toEqual([]);
      expect(buffered.lastAppliedEventSeq).toBe(100);
      expect(buffered.bufferedEventEnvelopes[501]).toMatchObject({ type: 'sse_message' });

      const drained = dispatch(buffered, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(500)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        eventCursorFloor: 500,
        snapshotStartedAtEventSeq: 100,
      });

      expect(drained.messages.map((message) => message.sequence_id)).toEqual([500, 501]);
      expect(drained.lastAppliedEventSeq).toBe(501);
      expect(drained.bufferedEventEnvelopes).toEqual({});
    });

    // Task 02675: defense-in-depth id dedup. Even if the server assigns a
    // fresh sequence_id, a second delivery of a message with the same
    // message_id must not duplicate in atom.messages.
    it('dedups by message_id when a duplicate arrives with a fresh sequenceId', () => {
      const original = makeMessage(5);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastAppliedEventSeq: 5,
      };

      // Same message_id, but only the next contiguous event is admissible.
      const duplicateWithFreshSeq: Message = { ...original, sequence_id: 6 };
      const next = dispatch(atom, {
        type: 'sse_message',
        message: duplicateWithFreshSeq,
        sequenceId: 6,
      });

      expect(next.messages).toHaveLength(1);
      expect(next.lastAppliedEventSeq).toBe(6);
    });

    it('clears streamingBuffer atomically on message arrival', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 8,
        streamingBuffer: { text: 'partial text', lastSequence: 8, startedAt: Date.now(), requestId: 'test-req-id' },
      };

      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(9),
        sequenceId: 9,
      });

      expect(next.streamingBuffer).toBeNull();
      expect(next.messages).toHaveLength(1);
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
        lastAppliedEventSeq: 42,
      };
      const summaryDisplayData: Record<string, unknown> = {
        type: 'subagent_summary',
        results: [{ agent_id: 'a1', task: 't', outcome: { type: 'success', result: 'done' } }],
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 43,
        messageId: original.message_id,
        transcriptGeneration: 3,
        displayData: summaryDisplayData,
      });

      expect(next.messages).toHaveLength(1);
      expect(next.messages[0]!.display_data).toEqual(summaryDisplayData);
      expect(next.lastAppliedEventSeq).toBe(43);
      expect(next.transcriptGeneration).toBe(3);
    });

    it('stores a pending patch when message_id is unknown', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [makeMessage(5)],
        lastAppliedEventSeq: 10,
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: 'nonexistent-id',
        transcriptGeneration: 7,
        displayData: { type: 'whatever' },
      });

      // The event was applied to pending reducer state, so the event floor still
      // advances even though no live message row existed yet.
      expect(next.lastAppliedEventSeq).toBe(11);
      expect(next.messages).toEqual(atom.messages);
      expect(next.pendingMessagePatches['nonexistent-id']).toEqual({
        lastAppliedPatchEventSeq: 0,
        patches: [{ eventSeq: 11, displayData: { type: 'whatever' } }],
      });
      expect(next.transcriptGeneration).toBe(7);
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
        lastAppliedEventSeq: 10,
      };

      // Update only display_data, not content
      const next = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: original.message_id,
        transcriptGeneration: 8,
        displayData: { type: 'new_display' },
      });

      expect((next.messages[0]!.display_data as { type: string }).type).toBe('new_display');
      expect((next.messages[0]!.content as { text: string }).text).toBe('original content');
      expect(next.transcriptGeneration).toBe(8);
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
        lastAppliedEventSeq: 10,
      };

      const once = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: original.message_id,
        transcriptGeneration: 9,
        displayData: { type: 'after' },
      });
      // Second delivery with the SAME sequenceId: the replay guard rejects it.
      const twice = dispatch(once, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: original.message_id,
        transcriptGeneration: 9,
        displayData: { type: 'after' },
      });

      expect(twice).toBe(once); // applyIfNewer returned atom unchanged
      expect((twice.messages[0]!.display_data as { type: string }).type).toBe('after');
    });
    it('is idempotent: duplicate message_updated events apply exactly once', () => {
      const original: Message = {
        ...makeMessage(5),
        display_data: { type: 'before' } as Record<string, unknown>,
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastAppliedEventSeq: 10,
      };

      const once = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: original.message_id,
        transcriptGeneration: 9,
        displayData: { type: 'after' },
      });
      // Second delivery with the SAME sequenceId: the replay guard rejects it.
      const twice = dispatch(once, {
        type: 'sse_message_updated',
        sequenceId: 11,
        messageId: original.message_id,
        transcriptGeneration: 9,
        displayData: { type: 'after' },
      });

      expect(twice).toBe(once); // applyIfNewer returned atom unchanged
      expect((twice.messages[0]!.display_data as { type: string }).type).toBe('after');
    });

    it('applies pending patch after a later create without advancing message availability early', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 20,
      };
      let patched = atom;
      for (const seq of [21, 22, 23, 24]) {
        patched = dispatch(patched, {
          type: 'sse_state_change',
          sequenceId: seq,
          phase: { type: 'idle' },
          stateUpdatedAt: seq,
        });
      }

      patched = dispatch(patched, {
        type: 'sse_message_updated',
        sequenceId: 25,
        messageId: 'late-msg',
        transcriptGeneration: 11,
        displayData: { type: 'deferred' },
        durationMs: 321,
      });

      expect(patched.lastAppliedEventSeq).toBe(25);
      expect(patched.messageRanges).toEqual([]);
      expect(patched.contiguousMessageHighWater).toBe(0);
      expect(patched.pendingMessagePatches['late-msg']).toEqual({
        lastAppliedPatchEventSeq: 0,
        patches: [{ eventSeq: 25, displayData: { type: 'deferred' }, durationMs: 321 }],
      });

      const created = dispatch(patched, {
        type: 'sse_message',
        sequenceId: 26,
        message: {
          ...makeMessage(7),
          message_id: 'late-msg',
          display_data: { existing: 'yes' } as Record<string, unknown>,
        },
      });

      expect(created.messages).toHaveLength(1);
      expect(created.messages[0]!.display_data).toEqual({ existing: 'yes', type: 'deferred', duration_ms: 321 });
      expect(created.messageRanges).toEqual([{ start: 7, end: 7 }]);
      expect(created.contiguousMessageHighWater).toBe(7);
      expect(created.pendingMessagePatches['late-msg']).toEqual({
        lastAppliedPatchEventSeq: 25,
        patches: [],
      });
      expect(patched.transcriptGeneration).toBe(11);
    });

    it('stale pending/live patches are no-ops once a newer patch has already applied', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 30,
      };
      let withPending = atom;
      for (const seq of [31, 32, 33, 34]) {
        withPending = dispatch(withPending, {
          type: 'sse_state_change',
          sequenceId: seq,
          phase: { type: 'idle' },
          stateUpdatedAt: seq,
        });
      }

      withPending = dispatch(withPending, {
        type: 'sse_message_updated',
        sequenceId: 35,
        messageId: 'msg-stale',
        transcriptGeneration: 12,
        displayData: { type: 'newer' },
      });
      const created = dispatch(withPending, {
        type: 'sse_message',
        sequenceId: 36,
        message: {
          ...makeMessage(8),
          message_id: 'msg-stale',
          display_data: { base: true } as Record<string, unknown>,
        },
      });

      expect(created.messages[0]!.display_data).toEqual({ base: true, type: 'newer' });
      expect(created.pendingMessagePatches['msg-stale']).toEqual({
        lastAppliedPatchEventSeq: 35,
        patches: [],
      });

      const replayedCreate = dispatch(created, {
        type: 'sse_message',
        sequenceId: 37,
        message: {
          ...makeMessage(8),
          message_id: 'msg-stale',
          display_data: { base: true } as Record<string, unknown>,
        },
      });

      expect(replayedCreate.messages[0]!.display_data).toEqual({ base: true, type: 'newer' });
      expect(replayedCreate.pendingMessagePatches['msg-stale']).toEqual({
        lastAppliedPatchEventSeq: 35,
        patches: [],
      });
      expect(withPending.transcriptGeneration).toBe(12);
    });

    it('merges durationMs into display_data, preserving existing keys', () => {
      const original: Message = {
        ...makeMessage(5),
        message_type: 'tool',
        display_data: { bash: [{ tool_use_id: 'abc', display: 'ls' }] } as Record<string, unknown>,
        content: { tool_use_id: 'abc', content: 'file.txt', is_error: false } as Message['content'],
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastAppliedEventSeq: 20,
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 21,
        messageId: original.message_id,
        transcriptGeneration: 13,
        durationMs: 4567,
      });

      const dd = next.messages[0]!.display_data as Record<string, unknown>;
      // duration_ms was injected
      expect(dd['duration_ms']).toBe(4567);
      // existing keys survive
      expect(dd['bash']).toEqual([{ tool_use_id: 'abc', display: 'ls' }]);
      expect(next.lastAppliedEventSeq).toBe(21);
      expect(next.transcriptGeneration).toBe(13);
    });

    it('durationMs update is gated by sequenceId (replay guard)', () => {
      const original: Message = {
        ...makeMessage(5),
        message_type: 'tool',
        display_data: null,
        content: { tool_use_id: 'abc', content: 'ok', is_error: false } as Message['content'],
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastAppliedEventSeq: 30,
      };

      // Stale sequenceId — should be rejected
      const next = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 29,
        messageId: original.message_id,
        transcriptGeneration: 14,
        durationMs: 9999,
      });

      expect(next).toBe(atom); // applyIfNewer returned unchanged
      expect(next.messages[0]!.display_data).toBeNull();
      expect(next.transcriptGeneration).toBe(atom.transcriptGeneration);
    });

    it('preserves existing display-data markers for ephemeral updates without transcript generation', () => {
      const original: Message = {
        ...makeMessage(5),
        display_data: { marker: 'keep' } as Record<string, unknown>,
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        messages: [original],
        lastAppliedEventSeq: 30,
        transcriptGeneration: 14,
      };

      const next = dispatch(atom, {
        type: 'sse_message_updated',
        sequenceId: 31,
        messageId: original.message_id,
        displayData: { marker: 'keep', ephemeral: true },
      });

      expect(next.messages[0]!.display_data).toEqual({ marker: 'keep', ephemeral: true });
      expect(next.transcriptGeneration).toBe(14);
    });
  });

  describe('sse_state_change', () => {
    it('updates phase', () => {
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 1,
        phase: { type: 'awaiting_llm' },
        stateUpdatedAt: 0,
      });

      expect(next.phase.type).toBe('awaiting_llm');
    });

    it('applies live awaiting_continuation state changes', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        phase: { type: 'idle' },
        lastAppliedEventSeq: 12,
      };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 13,
        phase: { type: 'awaiting_continuation', attempt: 1 },
        stateUpdatedAt: 1_700_000_000_000,
      });

      expect(next.phase).toEqual({ type: 'awaiting_continuation', attempt: 1 });
      expect(next.phaseStateUpdatedAt).toBe(1_700_000_000_000);
      expect(next.lastAppliedEventSeq).toBe(13);
    });

    it('is a no-op for sequenceId already seen', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastAppliedEventSeq: 10 };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 10,
        phase: { type: 'awaiting_llm' },
        stateUpdatedAt: 0,
      });

      expect(next).toBe(atom);
    });

    it('advances lastAppliedEventSeq on acceptance', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastAppliedEventSeq: 5 };

      const next = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 6,
        phase: { type: 'awaiting_llm' },
        stateUpdatedAt: 0,
      });

      expect(next.lastAppliedEventSeq).toBe(6);
    });

    it('buffers out-of-order state changes until the missing event arrives', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastAppliedEventSeq: 100 };

      const buffered = dispatch(atom, {
        type: 'sse_state_change',
        sequenceId: 102,
        phase: { type: 'awaiting_llm' },
        stateUpdatedAt: 0,
      });

      expect(buffered.lastAppliedEventSeq).toBe(100);
      expect(buffered.eventGap).toEqual({ expectedNextEventSeq: 101, firstBufferedEventSeq: 102 });

      const drained = dispatch(buffered, {
        type: 'sse_state_change',
        sequenceId: 101,
        phase: { type: 'awaiting_llm' },
        stateUpdatedAt: 1,
      });

      expect(drained.lastAppliedEventSeq).toBe(102);
      expect(drained.eventGap).toBeNull();
      expect(drained.phase.type).toBe('awaiting_llm');
    });
  });

  describe('sse_browser_session_state', () => {
    // REQ-BT-018: server-authoritative live-session edge. The reducer
    // mutates conversation.browser_session_active as the single source of
    // truth — no parallel sticky bool, no message-content scanning.
    it('updates conversation.browser_session_active from false to true', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversation: { ...testConversation, browser_session_active: false },
      };

      const atomWithFloor: ConversationAtom = { ...atom, lastAppliedEventSeq: 11 };
      const next = dispatch(atomWithFloor, {
        type: 'sse_browser_session_state',
        sequenceId: 12,
        active: true,
      });

      expect(next.conversation?.browser_session_active).toBe(true);
      expect(next.lastAppliedEventSeq).toBe(12);
    });

    it('updates conversation.browser_session_active from true to false', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversation: { ...testConversation, browser_session_active: true },
      };

      const atomWithFloor: ConversationAtom = { ...atom, lastAppliedEventSeq: 13 };
      const next = dispatch(atomWithFloor, {
        type: 'sse_browser_session_state',
        sequenceId: 14,
        active: false,
      });

      expect(next.conversation?.browser_session_active).toBe(false);
      expect(next.lastAppliedEventSeq).toBe(14);
    });

    it('does not patch a non-existent conversation', () => {
      // Init hasn't landed; conversation is null. The reducer must not
      // synthesise a conversation row from nothing.
      const atom = createInitialAtom();

      const next = dispatch(atom, {
        type: 'sse_browser_session_state',
        sequenceId: 3,
        active: true,
      });

      expect(next.conversation).toBeNull();
    });

    it('rejects a stale sequenceId', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 30,
        conversation: { ...testConversation, browser_session_active: false },
      };

      const next = dispatch(atom, {
        type: 'sse_browser_session_state',
        sequenceId: 29,
        active: true,
      });

      expect(next).toBe(atom);
      expect(next.conversation?.browser_session_active).toBe(false);
    });
  });

  describe('sse_work_scope_update', () => {
    // REQ-WSUI-007 / REQ-WSUI-010: full-snapshot inventory push. The reducer
    // replaces `workScope` wholesale (no delta merge) and respects the same
    // applyIfNewer total order as every other wire event.
    const inventory = (scopeKey: string, bashIds: string[]) => ({
      scope_key: scopeKey,
      bash: bashIds.map((id) => ({
        handle_id: id,
        cmd: `cmd ${id}`,
        state: 'running' as const,
        started_at: '2024-01-01T00:00:00Z',
        output_bytes: 0,
      })),
      tmux: null,
      browser: null,
    });

    it('seeds workScope from null on first push', () => {
      const atom = createInitialAtom();
      expect(atom.workScope).toBeNull();

      const seeded = { ...atom, lastAppliedEventSeq: 6 };
      const next = dispatch(seeded, {
        type: 'sse_work_scope_update',
        sequenceId: 7,
        inventory: inventory('conversation:conv-1', ['b-1']),
      });

      expect(next.workScope?.bash.map((h) => h.handle_id)).toEqual(['b-1']);
      expect(next.lastAppliedEventSeq).toBe(7);
    });

    it('replaces (not merges) the previous inventory wholesale', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 7,
        workScope: inventory('conversation:conv-1', ['b-1', 'b-2']),
      };

      const next = dispatch(atom, {
        type: 'sse_work_scope_update',
        sequenceId: 8,
        // The new snapshot drops b-1/b-2 and carries only b-3: a merge would
        // resurrect the stale handles; a replace must not.
        inventory: inventory('conversation:conv-1', ['b-3']),
      });

      expect(next.workScope?.bash.map((h) => h.handle_id)).toEqual(['b-3']);
      expect(next.lastAppliedEventSeq).toBe(8);
    });

    it('rejects a stale sequenceId (applyIfNewer)', () => {
      const held = inventory('conversation:conv-1', ['b-1']);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 30,
        workScope: held,
      };

      const next = dispatch(atom, {
        type: 'sse_work_scope_update',
        sequenceId: 29,
        inventory: inventory('conversation:conv-1', ['b-9']),
      });

      expect(next).toBe(atom);
      expect(next.workScope).toBe(held);
    });
  });

  describe('sse_agent_done', () => {
    it('resets phase to idle', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 19,
        phase: { type: 'awaiting_llm' },
      };

      const next = dispatch(atom, { type: 'sse_agent_done', sequenceId: 20 });

      expect(next.phase.type).toBe('idle');
      expect(next.lastAppliedEventSeq).toBe(20);
    });

    it('clears streaming buffer', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 15,
        streamingBuffer: { text: 'incomplete', lastSequence: 15, startedAt: Date.now(), requestId: 'test-req-id' },
      };

      const next = dispatch(atom, { type: 'sse_agent_done', sequenceId: 16 });

      expect(next.streamingBuffer).toBeNull();
    });

    it('is a no-op if sequenceId already seen', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 25,
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

      const s1 = dispatch(atom, { type: 'sse_token', sequenceId: 1, delta: 'Hello', requestId: 'test-req-id' });
      const s2 = dispatch(s1, { type: 'sse_token', sequenceId: 2, delta: ' world', requestId: 'test-req-id' });

      expect(s2.streamingBuffer?.text).toBe('Hello world');
      expect(s2.lastAppliedEventSeq).toBe(2);
    });

    it('is a no-op for duplicate or out-of-order sequence', () => {
      const atom: ConversationAtom = {
        ...llmRequestingAtom(),
        lastAppliedEventSeq: 5,
        streamingBuffer: { text: 'Hello', lastSequence: 5, startedAt: Date.now(), requestId: 'test-req-id' },
      };

      const next = dispatch(atom, { type: 'sse_token', sequenceId: 3, delta: ' stale', requestId: 'test-req-id' });

      expect(next).toBe(atom);
    });

    it('preserves startedAt across token accumulation', () => {
      const startedAt = Date.now() - 1000;
      const atom: ConversationAtom = {
        ...llmRequestingAtom(),
        lastAppliedEventSeq: 1,
        streamingBuffer: { text: 'Hello', lastSequence: 1, startedAt, requestId: 'test-req-id' },
      };

      const next = dispatch(atom, { type: 'sse_token', sequenceId: 2, delta: '!', requestId: 'test-req-id' });

      expect(next.streamingBuffer?.startedAt).toBe(startedAt);
    });

    // A retry after a mid-stream failure (network / server_error /
    // invalid_response) opens a fresh LLM dispatch with a new request_id. The
    // new attempt's tokens must start a clean buffer, not concatenate onto the
    // failed attempt's partial text.
    it('resets the streaming buffer when a new request_id arrives (retry)', () => {
      const startedAt = Date.now() - 5000;
      const atom: ConversationAtom = {
        ...llmRequestingAtom(),
        lastAppliedEventSeq: 4,
        streamingBuffer: {
          text: 'partial from attempt 1',
          lastSequence: 4,
          startedAt,
          requestId: 'attempt-1',
        },
      };

      const next = dispatch(atom, {
        type: 'sse_token',
        sequenceId: 5,
        delta: 'fresh',
        requestId: 'attempt-2',
      });

      expect(next.streamingBuffer?.text).toBe('fresh');
      expect(next.streamingBuffer?.requestId).toBe('attempt-2');
      expect(next.streamingBuffer?.startedAt).not.toBe(startedAt);
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
        requestId: 'test-req-id',
      });
      expect(next.streamingBuffer).toBeNull();
      expect(next.lastAppliedEventSeq).toBe(1);
    });

    it('drops tokens when phase is tool_executing', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        phase: {
          type: 'tool_executing',
          current_tool: { id: 'tool-1', name: 'bash', input: { _tool: 'bash' } },
          remaining_tools: [],
        },
      };
      const next = dispatch(atom, {
        type: 'sse_token',
        sequenceId: 1,
        delta: 'ghost',
        requestId: 'test-req-id',
      });
      expect(next.streamingBuffer).toBeNull();
      expect(next.lastAppliedEventSeq).toBe(1);
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
      // Pre-reconnect state: atom has been streaming, lastAppliedEventSeq=50.
      const preReconnect: ConversationAtom = {
        ...createInitialAtom(),
        phase: { type: 'llm_requesting', attempt: 1 },
        lastAppliedEventSeq: 50,
        streamingBuffer: { text: 'Before ', lastSequence: 50, startedAt: Date.now(), requestId: 'test-req-id' },
      };

      // Server keeps streaming across the reconnect with ids 51, 52, 53.
      const a1 = dispatch(preReconnect, { type: 'sse_token', sequenceId: 51, delta: 'reconnect ', requestId: 'test-req-id' });
      const a2 = dispatch(a1, { type: 'sse_token', sequenceId: 52, delta: 'works ', requestId: 'test-req-id' });
      const a3 = dispatch(a2, { type: 'sse_token', sequenceId: 53, delta: 'correctly', requestId: 'test-req-id' });

      expect(a3.streamingBuffer?.text).toBe('Before reconnect works correctly');
      expect(a3.lastAppliedEventSeq).toBe(53);
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
      expect(next.lastAppliedEventSeq).toBe(0);
    });

    it('routes wire-originated errors through applyIfNewer', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastAppliedEventSeq: 42 };

      const next = dispatch(atom, {
        type: 'sse_error',
        sequenceId: 43,
        error: { type: 'BackendError', message: 'server hiccup' },
      });

      expect(next.uiError).toEqual({ type: 'BackendError', message: 'server hiccup' });
      expect(next.lastAppliedEventSeq).toBe(43);
    });

    it('drops replayed wire errors after the user has moved on', () => {
      // Simulate: user dismissed the toast (uiError = null), lastAppliedEventSeq has
      // since advanced past the error's sequenceId, then a reconnect replays
      // the same error. Nothing should change.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        lastAppliedEventSeq: 50,
        uiError: null,
      };

      const next = dispatch(atom, {
        type: 'sse_error',
        sequenceId: 43,
        error: { type: 'BackendError', message: 'old error' },
      });

      expect(next.uiError).toBeNull();
      expect(next.lastAppliedEventSeq).toBe(50);
    });

    it('applies a wire error only once when dispatched twice with the same sequenceId', () => {
      const atom = { ...createInitialAtom(), lastAppliedEventSeq: 9 };

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
      expect(a3.lastAppliedEventSeq).toBe(10);
    });
  });

  describe('connection epoch (task 08683)', () => {
    it('createInitialAtom() starts with connectionEpoch=null', () => {
      expect(createInitialAtom().connectionEpoch).toBeNull();
    });

    it('connection_opened lifts connectionEpoch from null', () => {
      const atom = createInitialAtom();
      const next = dispatch(atom, { type: 'connection_opened', epoch: 1 });
      expect(next.connectionEpoch).toBe(1);
    });

    it('connection_opened advances epoch monotonically', () => {
      const a1 = dispatch(createInitialAtom(), { type: 'connection_opened', epoch: 1 });
      const a2 = dispatch(a1, { type: 'connection_opened', epoch: 5 });
      expect(a2.connectionEpoch).toBe(5);
    });

    it('connection_opened drops a regression (stale OPEN_SSE closure)', () => {
      // Within a single hook lifetime, the machine epoch is monotonic. A
      // stale `OPEN_SSE` executor closure firing connection_opened with an
      // older epoch must not regress the atom — that would re-accept events
      // the new generation has already superseded.
      const a1 = dispatch(createInitialAtom(), { type: 'connection_opened', epoch: 5 });
      const a2 = dispatch(a1, { type: 'connection_opened', epoch: 3 });
      expect(a2).toBe(a1);
      expect(a2.connectionEpoch).toBe(5);
    });

    it('connection_opened drops an equal epoch as a no-op', () => {
      const a1 = dispatch(createInitialAtom(), { type: 'connection_opened', epoch: 4 });
      const a2 = dispatch(a1, { type: 'connection_opened', epoch: 4 });
      expect(a2).toBe(a1);
    });

    it('connection_reset nulls connectionEpoch so the next remount can lift again', () => {
      // Hook remount scenario: the atom retains epoch 5 from a prior visit.
      // The new machine starts at epoch 0; without reset, monotonic guard
      // would reject every connection_opened from the new generation.
      const a1 = dispatch(createInitialAtom(), { type: 'connection_opened', epoch: 5 });
      const a2 = dispatch(a1, { type: 'connection_reset' });
      expect(a2.connectionEpoch).toBeNull();
      expect(a2.connectionState).toBe('connecting');
      const a3 = dispatch(a2, { type: 'connection_opened', epoch: 1 });
      expect(a3.connectionEpoch).toBe(1);
    });

    it('connection_state updates the visible lifecycle indicator', () => {
      const a1 = dispatch(createInitialAtom(), { type: 'connection_opened', epoch: 1 });
      expect(a1.connectionState).toBe('connecting');
      const a2 = dispatch(a1, { type: 'connection_state', state: 'live', epoch: 1 });
      expect(a2.connectionState).toBe('live');
    });

    it('rejects stamped actions until connection_opened establishes their epoch', () => {
      const atom = createInitialAtom();
      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(1),
        sequenceId: 1,
        epoch: 1,
      });
      expect(next).toBe(atom);
    });

    it('rejects stamped action when epoch does not match (cross-conv contamination)', () => {
      // Atom is on connection generation 7; an event arrives stamped with
      // generation 5. This is exactly the scenario where a stale
      // EventSource for a different conversation slug fires through a
      // dispatchRef that has already been re-bound to this atom's slug.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        connectionEpoch: 7,
      };
      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(100),
        sequenceId: 100,
        epoch: 5,
      });
      // Atom unchanged: same reference, no state mutation.
      expect(next).toBe(atom);
      expect(next.messages).toHaveLength(0);
    });

    it('accepts stamped action when epoch matches', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        connectionEpoch: 3,
      };
      const next = dispatch(atom, {
        type: 'sse_message',
        message: makeMessage(1),
        sequenceId: 1,
        epoch: 3,
      });
      expect(next.messages).toHaveLength(1);
    });

    it('rejects a stale-epoch sse_message even if its sequence_id is fresh', () => {
      // Defense in depth: the sequence-id guard would happily accept this
      // message (lastAppliedEventSeq=0 < 100). The epoch guard sits *before*
      // the reducer cases and rejects on identity, not freshness.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        connectionEpoch: 7,
      };
      const message: Message = {
        message_id: 'msg-stale',
        sequence_id: 100,
        conversation_id: 'conv-1',
        message_type: 'agent',
        content: { text: 'from a stale connection' } as Message['content'],
        created_at: '2024-01-01T00:00:00Z',
      };
      const next = dispatch(atom, {
        type: 'sse_message',
        message,
        sequenceId: 100,
        epoch: 5,
      });
      expect(next).toBe(atom);
      expect(next.messages).toHaveLength(0);
      expect(next.lastAppliedEventSeq).toBe(0);
    });

    it('client-originated actions are gated by expectedConversationId, not epoch', () => {
      // local_phase_change carries no epoch — the epoch guard ignores it.
      // The guard against cross-conversation contamination is structural:
      // the dispatch site captures conversationId, the reducer drops the
      // action when the atom is no longer that conversation.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        connectionEpoch: 7,
      };
      const next = dispatch(atom, {
        type: 'local_phase_change',
        phase: { type: 'awaiting_llm' },
        expectedConversationId: 'conv-1',
      });
      expect(next.phase.type).toBe('awaiting_llm');
    });

    it('drops a stale-epoch sse_error even when client-synthesized (no sequenceId)', () => {
      // Cross-conversation contamination edge: a stale handler closure on
      // conversation A's EventSource fires schema-violation handling AFTER
      // navigation to B. handleSchemaViolation routes through the
      // epoch-stamped dispatch, so the synthesized sse_error carries A's
      // epoch. B's atom (different epoch) must reject it — otherwise an
      // error toast for A pops up while the user is reading B.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        connectionEpoch: 7,
      };
      const next = dispatch(atom, {
        type: 'sse_error',
        epoch: 3, // stale: from A's connection generation
        error: {
          type: 'BackendError',
          message: 'parse error from a stale connection',
        },
      });
      expect(next).toBe(atom);
      expect(next.uiError).toBeNull();
    });

    it('drops a stale-epoch sse_error that does carry a sequenceId', () => {
      // Same as above but the wire emitted a real backend error with a
      // sequenceId. The epoch guard runs BEFORE applyIfNewer; the
      // sequenceId is fresh but the epoch isn't, so the error is dropped
      // before it would otherwise pop a toast on the wrong atom.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        connectionEpoch: 7,
      };
      const next = dispatch(atom, {
        type: 'sse_error',
        epoch: 3,
        sequenceId: 100,
        error: { type: 'BackendError', message: 'wrong atom' },
      });
      expect(next).toBe(atom);
      expect(next.uiError).toBeNull();
      expect(next.lastAppliedEventSeq).toBe(0);
    });
  });

  describe('local_phase_change', () => {
    // Client-originated optimistic phase updates do NOT bump lastAppliedEventSeq
    // (they're not part of the server's total order). This test guards
    // against a future change that accidentally wires them through the
    // server-side dedup path.
    it('updates phase without touching lastAppliedEventSeq', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        lastAppliedEventSeq: 42,
      };

      const next = dispatch(atom, {
        type: 'local_phase_change',
        phase: { type: 'awaiting_llm' },
        expectedConversationId: 'conv-1',
      });

      expect(next.phase.type).toBe('awaiting_llm');
      expect(next.lastAppliedEventSeq).toBe(42);
    });

    it('supports optimistic awaiting_continuation after manual trigger acceptance', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        lastAppliedEventSeq: 42,
      };

      const next = dispatch(atom, {
        type: 'local_phase_change',
        phase: { type: 'awaiting_continuation', attempt: 1 },
        expectedConversationId: 'conv-1',
      });

      expect(next.phase).toEqual({ type: 'awaiting_continuation', attempt: 1 });
      expect(next.lastAppliedEventSeq).toBe(42);
      expect(next.phaseStateUpdatedAt).toBeNull();
    });

    it('drops when expectedConversationId does not match (post-navigation resolve)', () => {
      // `await api.sendMessage` resolved after the user navigated A→B.
      // Dispatch was bound to A's atom but the atom is now showing B.
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-B',
      };
      const next = dispatch(atom, {
        type: 'local_phase_change',
        phase: { type: 'awaiting_llm' },
        expectedConversationId: 'conv-A',
      });
      expect(next).toBe(atom);
    });
  });

  describe('local_conversation_update', () => {
    it('merges updates when conversation exists', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
      };

      const next = dispatch(atom, {
        type: 'local_conversation_update',
        updates: { model: 'new-model' },
        expectedConversationId: 'conv-1',
      });

      expect(next.conversation?.model).toBe('new-model');
    });

    it('is a no-op when conversation is null', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), conversationId: 'conv-1' };

      const next = dispatch(atom, {
        type: 'local_conversation_update',
        updates: { model: 'new-model' },
        expectedConversationId: 'conv-1',
      });

      expect(next).toBe(atom);
    });

    it('drops when expectedConversationId does not match', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-B',
        conversation: testConversation,
      };
      const next = dispatch(atom, {
        type: 'local_conversation_update',
        updates: { model: 'new-model' },
        expectedConversationId: 'conv-A',
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
        transcriptGeneration: 1,
      });

      expect(next.conversationId).toBe('conv-1');
      expect(next.messages).toHaveLength(1);
      expect(next.contextWindow.used).toBe(500);
    });

    it('can seed the event cursor from a cached transcript tail', () => {
      const next = dispatch(createInitialAtom(), {
        type: 'set_initial_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(42)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 1,
        eventCursorFloor: 42,
      });

      expect(next.lastAppliedEventSeq).toBe(42);
      expect(next.messages.map((m) => m.sequence_id)).toEqual([42]);
    });

    it('merge_conversation_data updates messages even after SSE cursor exists', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        lastAppliedEventSeq: 10,
      };
      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1), makeMessage(2)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 1,
        snapshotStartedAtEventSeq: 10,
      });

      expect(next.lastAppliedEventSeq).toBe(10);
      expect(next.messages.map((m) => m.sequence_id)).toEqual([1, 2]);
    });

    it('merge_conversation_data drains buffered events when the cursor floor reaches them', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        phase: { type: 'idle' },
        lastAppliedEventSeq: 1,
        bufferedEventEnvelopes: {
          3: { type: 'sse_agent_done', sequenceId: 3 },
        },
        eventGap: { expectedNextEventSeq: 2, firstBufferedEventSeq: 3 },
      };
      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(2)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        eventCursorFloor: 2,
        snapshotStartedAtEventSeq: 1,
      });

      expect(next.lastAppliedEventSeq).toBe(3);
      expect(next.bufferedEventEnvelopes).toEqual({});
      expect(next.eventGap).toBeNull();
    });

    it('merge_conversation_data preserves live phase after SSE has started', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        phase: { type: 'llm_requesting', attempt: 1 },
        phaseLastAppliedEventSeq: 9,
        lastAppliedEventSeq: 10,
      };
      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(2)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        eventCursorFloor: 2,
        snapshotStartedAtEventSeq: 1,
      });

      expect(next.phase).toEqual({ type: 'llm_requesting', attempt: 1 });
    });

    it('merge_conversation_data accepts authoritative phase when only a cached cursor was seeded', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        phase: { type: 'idle' },
        lastAppliedEventSeq: 10,
      };
      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(2)],
        phase: { type: 'llm_requesting', attempt: 1 },
        contextWindow: { used: 0 },
        eventCursorFloor: 10,
        snapshotStartedAtEventSeq: 10,
      });

      expect(next.phase).toEqual({ type: 'llm_requesting', attempt: 1 });
    });

    it('merge_conversation_data preserves phase from stream init over stale REST metadata', () => {
      const atom = dispatch(createInitialAtom(), {
        type: 'sse_init',
        payload: makeInitPayload({
          phase: { type: 'llm_requesting', attempt: 1 },
          lastAppliedEventSeq: 12,
          pendingAnchorSequenceId: 12,
        }),
      });

      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        eventCursorFloor: 12,
        snapshotStartedAtEventSeq: 11,
      });

      expect(next.phase).toEqual({ type: 'llm_requesting', attempt: 1 });
    });

    it('merge_conversation_data preserves live-patched messages over stale REST rows', () => {
      const liveMessage = makeMessage(1, { display_data: { duration_ms: 123 } });
      const staleRestMessage = makeMessage(1);
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [liveMessage],
        pendingMessagePatches: {
          [liveMessage.message_id]: { lastAppliedPatchEventSeq: 7, patches: [] },
        },
      };
      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [staleRestMessage],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        snapshotStartedAtEventSeq: 6,
      });

      expect(next.messages[0]).toEqual(liveMessage);
    });

    it('merge_conversation_data accepts REST rows when their patch predates the request', () => {
      const liveMessage = makeMessage(1, { display_data: { duration_ms: 123 } });
      const restMessage = makeMessage(1, { display_data: { duration_ms: 456 } });
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [liveMessage],
        pendingMessagePatches: {
          [liveMessage.message_id]: { lastAppliedPatchEventSeq: 7, patches: [] },
        },
      };

      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [restMessage],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        snapshotStartedAtEventSeq: 8,
      });

      expect(next.messages[0]).toEqual(restMessage);
      expect(next.pendingMessagePatches).toEqual({});
    });

    it('merge_conversation_data preserves live conversation metadata over older REST rows', () => {
      const atomAfterLiveUpdate = dispatch(
        {
          ...createInitialAtom(),
          conversationId: 'conv-1',
          conversation: testConversation,
          messages: [makeMessage(1)],
          lastAppliedEventSeq: 6,
        },
        {
          type: 'sse_conversation_update',
          sequenceId: 7,
          updates: { cwd: '/newer/live/cwd' },
        },
      );

      const next = dispatch(atomAfterLiveUpdate, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: { ...testConversation, cwd: '/older/rest/cwd' },
        messages: [makeMessage(1)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        eventCursorFloor: 6,
        snapshotStartedAtEventSeq: 6,
      });

      expect(next.conversation?.cwd).toBe('/newer/live/cwd');
    });

    it('merge_conversation_data preserves existing tail coverage when transcriptCoverage is omitted', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        transcriptGeneration: 7,
        transcriptCoverage: 'tail',
      };

      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: { ...testConversation, transcript_generation: 8 },
        messages: [makeMessage(1), makeMessage(2)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 8,
        snapshotStartedAtEventSeq: 0,
      });

      expect(next.transcriptGeneration).toBe(8);
      expect(next.transcriptCoverage).toBe('tail');
    });

    it('merge_conversation_data preserves existing complete coverage when transcriptCoverage is omitted', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        transcriptGeneration: 7,
        transcriptCoverage: 'complete',
      };

      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: { ...testConversation, transcript_generation: 8 },
        messages: [makeMessage(1), makeMessage(2)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 8,
        snapshotStartedAtEventSeq: 0,
      });

      expect(next.transcriptGeneration).toBe(8);
      expect(next.transcriptCoverage).toBe('complete');
    });

    it('merge_conversation_data lets explicit transcriptCoverage override existing coverage', () => {
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        transcriptGeneration: 7,
        transcriptCoverage: 'complete',
      };

      const next = dispatch(atom, {
        type: 'merge_conversation_data',
        conversationId: 'conv-1',
        conversation: { ...testConversation, transcript_generation: 8 },
        messages: [makeMessage(1), makeMessage(2)],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 8,
        transcriptCoverage: 'tail',
        snapshotStartedAtEventSeq: 0,
      });

      expect(next.transcriptGeneration).toBe(8);
      expect(next.transcriptCoverage).toBe('tail');
    });

    it('set_initial_data can reset a stale cached conversation to the fetched slug owner', () => {
      const replacementConversation: Conversation = {
        ...testConversation,
        id: 'conv-2',
        slug: 'test-slug',
      };
      const atom: ConversationAtom = {
        ...createInitialAtom(),
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [makeMessage(1)],
        lastAppliedEventSeq: 10,
        bufferedEventEnvelopes: { 11: { type: 'sse_agent_done', sequenceId: 11 } },
        pendingMessagePatches: {
          'msg-1': { lastAppliedPatchEventSeq: 9, patches: [] },
        },
        streamingBuffer: { text: 'stale', lastSequence: 9, startedAt: 1, requestId: 'old' },
        phase: { type: 'llm_requesting', attempt: 1 },
        phaseLastAppliedEventSeq: 8,
        conversationLastAppliedEventSeq: 7,
        connectionEpoch: 4,
        connectionState: 'live',
        systemPrompt: 'old prompt',
      };

      const next = dispatch(atom, {
        type: 'set_initial_data',
        conversationId: 'conv-2',
        conversation: replacementConversation,
        messages: [makeMessage(1, { conversation_id: 'conv-2' })],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        eventCursorFloor: 1,
        reset: true,
      });

      expect(next.conversationId).toBe('conv-2');
      expect(next.conversation).toEqual(replacementConversation);
      expect(next.lastAppliedEventSeq).toBe(1);
      expect(next.bufferedEventEnvelopes).toEqual({});
      expect(next.pendingMessagePatches).toEqual({});
      expect(next.streamingBuffer).toBeNull();
      expect(next.phaseLastAppliedEventSeq).toBe(0);
      expect(next.conversationLastAppliedEventSeq).toBe(0);
      expect(next.connectionEpoch).toBeNull();
      expect(next.connectionState).toBe('connecting');
      expect(next.systemPrompt).toBeNull();
    });

    it('is a no-op if SSE data already present', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), lastAppliedEventSeq: 5 };

      const next = dispatch(atom, {
        type: 'set_initial_data',
        conversationId: 'conv-1',
        conversation: testConversation,
        messages: [],
        phase: { type: 'idle' },
        contextWindow: { used: 0 },
        transcriptGeneration: 1,
      });

      expect(next).toBe(atom);
    });
  });

  describe('set_system_prompt', () => {
    it('stores system prompt when expectedConversationId matches', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), conversationId: 'conv-1' };

      const next = dispatch(atom, {
        type: 'set_system_prompt',
        systemPrompt: 'You are helpful.',
        expectedConversationId: 'conv-1',
      });

      expect(next.systemPrompt).toBe('You are helpful.');
    });

    it('drops when expectedConversationId does not match', () => {
      const atom: ConversationAtom = { ...createInitialAtom(), conversationId: 'conv-B' };
      const next = dispatch(atom, {
        type: 'set_system_prompt',
        systemPrompt: 'late-resolving promise from conv-A',
        expectedConversationId: 'conv-A',
      });
      expect(next).toBe(atom);
    });
  });
});
