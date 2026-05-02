/**
 * SSE wire-format validation tests (task 02674).
 *
 * Exercises `parseEvent` end-to-end: a MessageEvent-shaped input is fed through
 * each schema and we verify that:
 *  - well-formed payloads produce an `ok: true` result with an inferred type
 *    that matches the schema
 *  - malformed payloads (missing required field, wrong type) are rejected,
 *    no reducer action is dispatched, and the sse_error channel receives a
 *    structured violation in prod mode
 *
 * Dev-mode (`import.meta.env.DEV === true`) is the default when vitest runs;
 * each test that exercises the failure path stubs it back to `false` so the
 * "throw on violation" branch doesn't nuke the vitest worker.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Dispatch } from 'react';
import { parseEvent } from './hooks/useConnection';
import type { SSEAction } from './conversation/atom';
import type { MessageType } from './generated/MessageType';
import { MESSAGE_TYPE_OPTIONS } from './sseSchemas';
import {
  SseInitDataSchema,
  SseMessageDataSchema,
  SseMessageUpdatedDataSchema,
  SseStateChangeDataSchema,
  SseTokenDataSchema,
  SseConversationUpdateDataSchema,
  SseAgentDoneDataSchema,
  SseConversationBecameTerminalDataSchema,
  SseErrorDataSchema,
  ChainQaTokenSchema,
  ChainQaCompletedSchema,
  ChainQaFailedSchema,
} from './sseSchemas';
import * as v from 'valibot';

function makeEvent(data: unknown): Event {
  // parseEvent casts Event -> MessageEvent and reads `.data`. happy-dom ships
  // a MessageEvent constructor, but we don't need the real one — a plain
  // object-with-a-data-property is equivalent for the code path we care
  // about, and avoids coupling tests to the DOM type.
  const payload = typeof data === 'string' ? data : JSON.stringify(data);
  return { data: payload } as unknown as Event;
}

function mockDispatch(): { dispatch: Dispatch<SSEAction>; actions: SSEAction[] } {
  const actions: SSEAction[] = [];
  const dispatch: Dispatch<SSEAction> = (action) => {
    actions.push(action);
  };
  return { dispatch, actions };
}

/** Run a schema-violation test without triggering the dev-mode throw. */
function inProdMode<T>(fn: () => T): T {
  // DEV is a readonly property on import.meta.env at the type level but a
  // plain mutable field in the vitest runtime; cast once through the
  // mutable-record type so we can flip it for this block.
  const env = import.meta.env as unknown as Record<string, unknown>;
  const original = env['DEV'];
  env['DEV'] = false;
  try {
    return fn();
  } finally {
    env['DEV'] = original;
  }
}

describe('parseEvent', () => {
  let consoleErrorSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    consoleErrorSpy.mockRestore();
  });

  describe('JSON parse failures', () => {
    it('dispatches ParseError in prod for non-JSON input', () => {
      const { dispatch, actions } = mockDispatch();
      inProdMode(() => {
        const res = parseEvent(
          SseMessageDataSchema,
          makeEvent('{not: json'),
          'message',
          dispatch,
        );
        expect(res.ok).toBe(false);
      });
      expect(actions).toHaveLength(1);
      expect(actions[0]).toEqual({
        type: 'sse_error',
        error: { type: 'ParseError', raw: '{not: json' },
      });
    });

    it('throws in dev for non-JSON input (loud contract drift)', () => {
      const { dispatch } = mockDispatch();
      expect(() =>
        parseEvent(SseMessageDataSchema, makeEvent('{bad'), 'message', dispatch),
      ).toThrow(/malformed JSON/);
    });
  });

  describe('init schema', () => {
    // Task 02677 tightened the init schema so that fields the Rust side
    // always sets (display_state, context_window_size, model_context_window,
    // breadcrumbs, commits_behind, commits_ahead, project_name) are required
    // here too. The generated TS type in `./generated/sse` is the source of
    // truth; the schema `satisfies v.GenericSchema<unknown, WireInitData>`
    // would fail to compile if these were still marked optional.
    const validInit = {
      sequence_id: 0,
      conversation: { id: 'conv-1' },
      messages: [],
      agent_working: false,
      last_sequence_id: 0,
      display_state: 'idle',
      context_window_size: 0,
      model_context_window: 200_000,
      breadcrumbs: [],
      commits_behind: 0,
      commits_ahead: 0,
      project_name: null,
    };

    it('accepts a minimal valid init payload', () => {
      const { dispatch, actions } = mockDispatch();
      const res = parseEvent(
        SseInitDataSchema,
        makeEvent(validInit),
        'init',
        dispatch,
      );
      expect(res.ok).toBe(true);
      if (res.ok) {
        expect(res.data.conversation.id).toBe('conv-1');
        expect(res.data.last_sequence_id).toBe(0);
        expect(res.data.sequence_id).toBe(0);
      }
      expect(actions).toHaveLength(0);
    });

    it('tolerates extra top-level wire fields (forward compat)', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseInitDataSchema,
        makeEvent({
          ...validInit,
          conversation: { id: 'conv-1', next_gen_field: 'hello' },
          some_new_server_feature: 123,
        }),
        'init',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('rejects init with missing last_sequence_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseInitDataSchema,
          makeEvent({
            sequence_id: 0,
            conversation: { id: 'conv-1' },
            messages: [],
            agent_working: false,
            // last_sequence_id missing
          }),
          'init',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
        expect(actions[0]!.type).toBe('sse_error');
      });
    });

    it('rejects init with missing sequence_id (task 02675 contract)', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseInitDataSchema,
          makeEvent({
            // sequence_id missing — task 02675 requires every event to carry one
            conversation: { id: 'conv-1' },
            messages: [],
            agent_working: false,
            last_sequence_id: 0,
          }),
          'init',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });

    it('rejects init with non-number last_sequence_id (the sequence-id corruption case)', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseInitDataSchema,
          makeEvent({
            sequence_id: 0,
            conversation: { id: 'conv-1' },
            messages: [],
            agent_working: false,
            last_sequence_id: '101', // string, the bug the task calls out
          }),
          'init',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });
  });

  describe('message schema', () => {
    const goodMsg = {
      message_id: 'msg-1',
      sequence_id: 5,
      conversation_id: 'conv-1',
      message_type: 'agent',
      content: { text: 'hello' },
      created_at: '2024-01-01T00:00:00Z',
    };

    it('accepts a valid message payload', () => {
      const { dispatch, actions } = mockDispatch();
      const res = parseEvent(
        SseMessageDataSchema,
        makeEvent({ sequence_id: 5, message: goodMsg }),
        'message',
        dispatch,
      );
      expect(res.ok).toBe(true);
      expect(actions).toHaveLength(0);
    });

    it('rejects a message whose sequence_id arrives as a string', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseMessageDataSchema,
          makeEvent({
            sequence_id: 5,
            message: { ...goodMsg, sequence_id: '5' },
          }),
          'message',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
        expect(actions[0]!.type).toBe('sse_error');
      });
    });

    it('rejects a message envelope missing the top-level sequence_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseMessageDataSchema,
          makeEvent({ message: goodMsg }), // envelope sequence_id missing
          'message',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });

    it('rejects a message with an unknown message_type', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseMessageDataSchema,
          makeEvent({
            sequence_id: 5,
            message: { ...goodMsg, message_type: 'wizard' },
          }),
          'message',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });

    it('rejects a message missing required message_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const rest: Record<string, unknown> = { ...goodMsg };
        delete rest['message_id'];
        const res = parseEvent(
          SseMessageDataSchema,
          makeEvent({ sequence_id: 5, message: rest }),
          'message',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });
  });

  describe('message_updated schema', () => {
    it('accepts display_data-only update with null content', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseMessageUpdatedDataSchema,
        makeEvent({
          sequence_id: 7,
          message_id: 'msg-1',
          display_data: { type: 'subagent_summary' },
          content: null,
        }),
        'message_updated',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('accepts update with duration_ms present', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseMessageUpdatedDataSchema,
        makeEvent({
          sequence_id: 8,
          message_id: 'msg-tool',
          display_data: null,
          content: null,
          duration_ms: 1234,
        }),
        'message_updated',
        dispatch,
      );
      expect(res.ok).toBe(true);
      if (res.ok) expect(res.data.duration_ms).toBe(1234);
    });

    it('accepts update with duration_ms absent (non-tool paths)', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseMessageUpdatedDataSchema,
        makeEvent({
          sequence_id: 9,
          message_id: 'msg-subagent',
          display_data: { type: 'subagent_summary' },
          content: null,
        }),
        'message_updated',
        dispatch,
      );
      expect(res.ok).toBe(true);
      if (res.ok) expect(res.data.duration_ms).toBeUndefined();
    });

    it('rejects update missing message_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseMessageUpdatedDataSchema,
          makeEvent({ sequence_id: 7, display_data: { type: 'x' } }),
          'message_updated',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });

    it('rejects update missing sequence_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseMessageUpdatedDataSchema,
          makeEvent({ message_id: 'msg-1', display_data: { type: 'x' } }),
          'message_updated',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });
  });

  describe('state_change schema', () => {
    it('accepts any opaque state payload (parseConversationState handles the union)', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseStateChangeDataSchema,
        makeEvent({
          sequence_id: 12,
          state: { type: 'awaiting_llm' },
          display_state: 'working',
        }),
        'state_change',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('rejects state_change missing the state envelope key', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseStateChangeDataSchema,
          makeEvent({ sequence_id: 12, display_state: 'working' }),
          'state_change',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });

    it('rejects state_change missing sequence_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseStateChangeDataSchema,
          makeEvent({ state: { type: 'awaiting_llm' } }),
          'state_change',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });
  });

  describe('token schema', () => {
    it('accepts a valid token payload', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseTokenDataSchema,
        makeEvent({ sequence_id: 3, text: 'Hello', request_id: 'req-1' }),
        'token',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('rejects a token whose text is not a string (was previously silently dropped)', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseTokenDataSchema,
          makeEvent({ sequence_id: 3, text: 42 }),
          'token',
          dispatch,
        );
        expect(res.ok).toBe(false);
        // Contrast with the old code path: this no longer silently returns.
        expect(actions).toHaveLength(1);
        expect(actions[0]!.type).toBe('sse_error');
      });
    });

    it('rejects a token missing sequence_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseTokenDataSchema,
          makeEvent({ text: 'hello' }),
          'token',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });
  });

  describe('conversation_update schema', () => {
    it('accepts a partial conversation object', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseConversationUpdateDataSchema,
        makeEvent({ sequence_id: 9, conversation: { commits_behind: 2 } }),
        'conversation_update',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('rejects a conversation_update where conversation is not an object', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseConversationUpdateDataSchema,
          makeEvent({ sequence_id: 9, conversation: 'scalar' }),
          'conversation_update',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });
  });

  describe('agent_done / conversation_became_terminal schemas', () => {
    it('accepts envelope with sequence_id for agent_done', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseAgentDoneDataSchema,
        makeEvent({ sequence_id: 15 }),
        'agent_done',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('rejects agent_done missing sequence_id', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseAgentDoneDataSchema,
          makeEvent({}),
          'agent_done',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });

    it('accepts envelope with sequence_id for conversation_became_terminal', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseConversationBecameTerminalDataSchema,
        makeEvent({ sequence_id: 42 }),
        'conversation_became_terminal',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('tolerates extra fields (forward compat)', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseConversationBecameTerminalDataSchema,
        makeEvent({ sequence_id: 42, future_field: 'ok' }),
        'conversation_became_terminal',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });
  });

  describe('error schema', () => {
    // Task 02677 tightened this schema: the Rust `SseWireEvent::Error`
    // variant always emits `sequence_id`, `message`, and `error` — see
    // `src/api/wire.rs`. The generated TS type requires all three, and
    // the schema's `satisfies` annotation enforces alignment.
    it('accepts a backend error payload with all required fields', () => {
      const { dispatch } = mockDispatch();
      const res = parseEvent(
        SseErrorDataSchema,
        makeEvent({
          sequence_id: 8,
          message: 'rate limited',
          error: { title: 'Rate limited', detail: 'retry', kind: 'retryable' },
        }),
        'error',
        dispatch,
      );
      expect(res.ok).toBe(true);
    });

    it('rejects a backend error payload without sequence_id', () => {
      // Contract check: errors now carry sequence_id like every other event
      // (task 02675 + 02677). Drops to sse_error in prod.
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseErrorDataSchema,
          makeEvent({ message: 'rate limited', error: {} }),
          'error',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });

    it('rejects an error payload missing message', () => {
      inProdMode(() => {
        const { dispatch, actions } = mockDispatch();
        const res = parseEvent(
          SseErrorDataSchema,
          makeEvent({ error: { kind: 'retryable' } }),
          'error',
          dispatch,
        );
        expect(res.ok).toBe(false);
        expect(actions).toHaveLength(1);
      });
    });
  });
});

describe('message_type picklist tripwire', () => {
  // 02677's `satisfies v.GenericSchema<unknown, WireMessageData>` catches a
  // schema that is narrower than the generated wire type at compile time, but
  // not a hand-authored picklist whose narrowness is the intent. If a new
  // `MessageType` variant lands on the Rust side and the picklist in
  // sseSchemas.ts isn't updated, the only symptom is a runtime parse
  // violation the first time a conversation carrying the new type reaches a
  // client.
  //
  // This test closes that gap by asserting the exported
  // `MESSAGE_TYPE_OPTIONS` list matches the generated `MessageType` union
  // exactly. The direction works because a type-level object keyed by
  // `MessageType` forces TSC to enumerate every variant: if a new variant is
  // added to the generated type, the `expected` map will fail to compile
  // until it's added here too. Conversely, if `MESSAGE_TYPE_OPTIONS` grows
  // extras that aren't in `MessageType`, the runtime comparison fails.

  it('MESSAGE_TYPE_OPTIONS matches the generated MessageType union', () => {
    // Compile-time exhaustiveness: this mapping must list every variant of
    // `MessageType`. Adding a new variant to the generated union without
    // extending this map is a TypeScript error here.
    const expected: Record<MessageType, true> = {
      user: true,
      agent: true,
      tool: true,
      system: true,
      skill: true,
      error: true,
      continuation: true,
    };

    const expectedSet = new Set(Object.keys(expected));
    const actualSet = new Set<string>(MESSAGE_TYPE_OPTIONS);

    expect(actualSet).toEqual(expectedSet);
  });
});

// ---------------------------------------------------------------------------
// Phoenix Chains v1: chain Q&A SSE schemas (REQ-CHN-004 / 005).
//
// These exercise the runtime validators added in Phase 4 — distinct from
// the conversation-scoped events because chain broadcasters carry a
// per-question demux discriminator (`chain_qa_id`) instead of the
// per-conversation `sequence_id`.
// ---------------------------------------------------------------------------

describe('chain Q&A SSE schemas', () => {
  it('accepts a well-formed chain_qa_token payload', () => {
    const result = v.safeParse(ChainQaTokenSchema, {
      chain_qa_id: 'qa-1',
      delta: 'hello',
    });
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.output.chain_qa_id).toBe('qa-1');
      expect(result.output.delta).toBe('hello');
    }
  });

  it('rejects chain_qa_token missing chain_qa_id', () => {
    const result = v.safeParse(ChainQaTokenSchema, { delta: 'hello' });
    expect(result.success).toBe(false);
  });

  it('accepts a well-formed chain_qa_completed payload', () => {
    const result = v.safeParse(ChainQaCompletedSchema, {
      chain_qa_id: 'qa-2',
      full_answer: 'the answer',
    });
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.output.full_answer).toBe('the answer');
    }
  });

  it('accepts chain_qa_failed with null partial_answer', () => {
    const result = v.safeParse(ChainQaFailedSchema, {
      chain_qa_id: 'qa-3',
      error: 'boom',
      partial_answer: null,
    });
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.output.partial_answer).toBeNull();
    }
  });

  it('accepts chain_qa_failed with a string partial_answer', () => {
    const result = v.safeParse(ChainQaFailedSchema, {
      chain_qa_id: 'qa-4',
      error: 'oops',
      partial_answer: 'partial',
    });
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.output.partial_answer).toBe('partial');
    }
  });

  it('rejects chain_qa_failed without partial_answer key', () => {
    // partial_answer is `Option<String>` Rust-side which serializes as
    // `null` when None (the wire is explicit, not absent). The schema
    // requires the key to be present so a missing key surfaces as a
    // validation error rather than silently coercing to null.
    const result = v.safeParse(ChainQaFailedSchema, {
      chain_qa_id: 'qa-5',
      error: 'no partial key',
    });
    expect(result.success).toBe(false);
  });
});
