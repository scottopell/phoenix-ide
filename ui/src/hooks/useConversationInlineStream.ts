import { useEffect, useReducer } from 'react';
import type { Message } from '../api';
import { api, type Conversation } from '../api';
import {
  SseAgentDoneDataSchema,
  SseErrorDataSchema,
  SseInitDataSchema,
  SseLlmAttemptDataSchema,
  SseLlmFirstByteDataSchema,
  SseMessageDataSchema,
  SseMessageUpdatedDataSchema,
  SseStateChangeDataSchema,
  SseTokenDataSchema,
  type SseInitData,
} from '../sseSchemas';
import { parseConversationState, isAgentWorking } from '../utils';
import { conversationReducer, createInitialAtom, type ConversationAtom, type InitPayload } from '../conversation/atom';
import * as v from 'valibot';

export type InlineStreamState =
  | { type: 'idle'; atom: ConversationAtom; error: null }
  | { type: 'connecting'; atom: ConversationAtom; error: null }
  | { type: 'ready'; atom: ConversationAtom; error: null }
  | { type: 'error'; atom: ConversationAtom; error: string };

type InlineStreamAction =
  | { type: 'reset' }
  | { type: 'connecting' }
  | { type: 'error'; error: string }
  | { type: 'atom'; atomAction: Parameters<typeof conversationReducer>[1] };

function initialState(): InlineStreamState {
  return { type: 'idle', atom: createInitialAtom(), error: null };
}

function reducer(state: InlineStreamState, action: InlineStreamAction): InlineStreamState {
  switch (action.type) {
    case 'reset': return initialState();
    case 'connecting': return { type: 'connecting', atom: state.atom, error: null };
    case 'error': return { type: 'error', atom: state.atom, error: action.error };
    case 'atom': {
      const atom = conversationReducer(state.atom, action.atomAction);
      return { type: 'ready', atom, error: null };
    }
    default: action satisfies never; return state;
  }
}

function transformInitData(raw: SseInitData): InitPayload {
  const conversation = raw.project_name != null
    ? { ...raw.conversation, project_name: raw.project_name }
    : raw.conversation;
  return {
    conversation,
    messages: raw.messages || [],
    phase: parseConversationState(conversation?.state),
    contextWindow: { used: raw.context_window_size ?? 0 },
    transcriptGeneration: raw.transcript_generation,
    lastAppliedEventSeq: raw.last_sequence_id ?? 0,
    pendingAnchorSequenceId: raw.pending_anchor_sequence_id,
    pendingEvents: raw.pending_events,
    pendingTruncated: raw.pending_truncated,
  };
}

function parseEventData(event: Event): unknown | null {
  try {
    return JSON.parse((event as MessageEvent).data);
  } catch {
    return null;
  }
}

// Bounded retry for a 404 on the initial snapshot. A freshly-spawned sub-agent
// can 404 transiently (the parent card renders before RuntimeManager inserts
// the child row); retry briefly to cover that race, then give up so a deleted
// sub-agent doesn't loop forever. ~10 × 500ms ≈ 5s.
const MAX_NOT_FOUND_RETRIES = 10;
const NOT_FOUND_RETRY_MS = 500;

function maxMessageSequence(messages: Message[]): number {
  let max = 0;
  for (const message of messages) {
    if (message.sequence_id > max) max = message.sequence_id;
  }
  return max;
}

function snapshotPayload(conversation: Conversation, messages: Message[], contextWindowSize: number): InitPayload {
  const lastAppliedEventSeq = maxMessageSequence(messages);
  return {
    conversation,
    messages,
    phase: conversation.state ? parseConversationState(conversation.state) : { type: 'idle' },
    contextWindow: { used: contextWindowSize },
    transcriptGeneration: conversation.transcript_generation ?? 1,
    lastAppliedEventSeq,
    pendingAnchorSequenceId: lastAppliedEventSeq,
    pendingEvents: [],
    pendingTruncated: false,
  };
}

function isNotFound(error: unknown): boolean {
  return error instanceof Error && /not found/i.test(error.message);
}

/**
 * Inline, read-only conversation stream for embedded child conversation views.
 *
 * Uses the same conversation reducer as the full page, including init pending
 * event replay, so sequence floors/token buffering/message updates follow one
 * protocol contract instead of a parallel sub-agent-specific implementation.
 *
 * `live` true opens the SSE stream (and self-closes once the sub-agent reaches
 * a terminal state); `false` is snapshot-only. `true` streams regardless of the
 * loaded phase so a just-spawned sub-agent that's still momentarily `Idle`
 * (created before its initial event) is followed once it starts working.
 */
export function useConversationInlineStream(conversationId: string, enabled: boolean, live: boolean): InlineStreamState {
  const [state, dispatch] = useReducer(reducer, undefined, initialState);

  useEffect(() => {
    if (!enabled) {
      dispatch({ type: 'reset' });
    }
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let source: EventSource | null = null;
    let notFoundRetries = 0;
    let retryTimer: number | null = null;

    const closeSource = () => {
      if (source) {
        source.close();
        source = null;
      }
    };

    const openLiveStream = () => {
      if (cancelled) return;
      source = new EventSource(`/api/conversations/${encodeURIComponent(conversationId)}/stream`);

      source.addEventListener('init', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseInitDataSchema, raw);
        if (!res.success) return;
        dispatch({ type: 'atom', atomAction: { type: 'sse_init', payload: transformInitData(res.output) } });
      });

      source.addEventListener('message', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseMessageDataSchema, raw);
        if (!res.success) return;
        dispatch({
          type: 'atom',
          atomAction: {
            type: 'sse_message',
            message: res.output.message,
            sequenceId: res.output.sequence_id,
          },
        });
      });

      source.addEventListener('message_updated', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseMessageUpdatedDataSchema, raw);
        if (!res.success) return;
        const data = res.output;
        dispatch({
          type: 'atom',
          atomAction: {
            type: 'sse_message_updated',
            sequenceId: data.sequence_id,
            messageId: data.message_id,
            ...(data.display_data != null && { displayData: data.display_data as Record<string, unknown> }),
            ...(data.content != null && { content: data.content as Message['content'] }),
            ...(data.duration_ms != null && { durationMs: data.duration_ms }),
          },
        });
      });

      source.addEventListener('state_change', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseStateChangeDataSchema, raw);
        if (!res.success) return;
        const phase = parseConversationState(res.output.state);
        dispatch({
          type: 'atom',
          atomAction: {
            type: 'sse_state_change',
            sequenceId: res.output.sequence_id,
            phase,
            // REQ-WPV-001: thread the server-authoritative entry time
            // (RFC3339 → ms) onto the atom.
            stateUpdatedAt: Date.parse(res.output.state_updated_at),
          },
        });
        // Sub-agent reached a terminal/idle state — stop streaming so we
        // don't hold the single live-stream slot (or an idle connection)
        // open after the agent has finished. The final phase is already on
        // the atom, so consumers see the completed state.
        if (!isAgentWorking(phase)) {
          closeSource();
        }
      });

      source.addEventListener('llm_first_byte', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseLlmFirstByteDataSchema, raw);
        if (!res.success) return;
        dispatch({
          type: 'atom',
          atomAction: {
            type: 'sse_llm_first_byte',
            sequenceId: res.output.sequence_id,
            requestId: res.output.request_id,
          },
        });
      });

      source.addEventListener('llm_attempt', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseLlmAttemptDataSchema, raw);
        if (!res.success) return;
        dispatch({
          type: 'atom',
          atomAction: {
            type: 'sse_llm_attempt',
            sequenceId: res.output.sequence_id,
            attempt: res.output.attempt,
            maxAttempts: res.output.max_attempts,
            reason: res.output.reason,
            backingOffMs: res.output.backing_off_ms,
            resetsAt: res.output.resets_at ? Date.parse(res.output.resets_at) : null,
          },
        });
      });

      source.addEventListener('token', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseTokenDataSchema, raw);
        if (!res.success) return;
        dispatch({
          type: 'atom',
          atomAction: {
            type: 'sse_token',
            sequenceId: res.output.sequence_id,
            delta: res.output.text,
            requestId: res.output.request_id,
          },
        });
      });

      source.addEventListener('agent_done', (event) => {
        const raw = parseEventData(event);
        if (raw === null) return;
        const res = v.safeParse(SseAgentDoneDataSchema, raw);
        if (!res.success) return;
        dispatch({ type: 'atom', atomAction: { type: 'sse_agent_done', sequenceId: res.output.sequence_id } });
      });

      source.addEventListener('error', (event) => {
        const messageEvent = event as MessageEvent;
        if (messageEvent.data) {
          const raw = parseEventData(event);
          if (raw === null) return;
          const res = v.safeParse(SseErrorDataSchema, raw);
          if (res.success) {
            dispatch({ type: 'error', error: res.output.message });
          }
          return;
        }
        if (source?.readyState === EventSource.CLOSED) {
          dispatch({ type: 'error', error: 'Sub-agent stream closed' });
          closeSource();
        }
      });
    };

    const loadSnapshot = () => {
      dispatch({ type: 'connecting' });
      api.getConversation(conversationId)
        .then((data) => {
          if (cancelled) return;
          dispatch({
            type: 'atom',
            atomAction: {
              type: 'sse_init',
              payload: snapshotPayload(data.conversation, data.messages, data.context_window_size ?? 0),
            },
          });
          if (live) openLiveStream();
        })
        .catch((err) => {
          if (cancelled) return;
          // Spawn race: the parent card can render a sub-agent before
          // RuntimeManager has inserted the freshly-spawned child row (the
          // spawn request is enqueued on an async channel before the parent
          // enters AwaitingSubAgents), so a 404 right after open is often
          // transient. Retry briefly when streaming live, but bound it so a
          // genuinely-missing / deleted sub-agent surfaces an error instead of
          // looping forever.
          if (live && isNotFound(err) && notFoundRetries < MAX_NOT_FOUND_RETRIES) {
            notFoundRetries += 1;
            retryTimer = window.setTimeout(loadSnapshot, NOT_FOUND_RETRY_MS);
            return;
          }
          dispatch({ type: 'error', error: err instanceof Error ? err.message : 'Failed to load sub-agent' });
        });
    };

    loadSnapshot();

    return () => {
      cancelled = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      closeSource();
    };
  }, [conversationId, enabled, live]);

  return state;
}
