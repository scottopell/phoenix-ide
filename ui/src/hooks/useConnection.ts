import { useState, useCallback, useEffect, useRef, type Dispatch } from 'react';
import * as v from 'valibot';
import type { SseInitData, SseBreadcrumb } from '../api';
import type { SSEAction, InitPayload } from '../conversation/atom';
import type { Breadcrumb } from '../types';
import { parseConversationState } from '../utils';
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
} from '../sseSchemas';
import {
  ConnectionState,
  ConnectionMachineState,
  ConnectionInput,
  ConnectionEffect,
  TransitionContext,
  transition,
  initialState,
  RECONNECTED_DISPLAY_MS,
} from './connectionMachine';

/** Result of a schema-validated SSE parse. Callers branch on `ok` to either
 *  dispatch the action or propagate the already-handled failure. */
type ParseResult<T> = { ok: true; data: T } | { ok: false };

/**
 * Parse + validate an SSE event payload against its schema.
 *
 * On failure, consults `import.meta.env.DEV`:
 *   - dev: log structured detail and throw (caught by the React error boundary)
 *   - prod: dispatch `sse_error` with a readable violation message
 * In both cases the caller receives `{ ok: false }` and must not dispatch.
 */
export function parseEvent<TSchema extends v.BaseSchema<unknown, unknown, v.BaseIssue<unknown>>>(
  schema: TSchema,
  event: Event,
  eventType: string,
  dispatch: Dispatch<SSEAction>,
): ParseResult<v.InferOutput<TSchema>> {
  const raw = (event as MessageEvent).data;
  let json: unknown;
  try {
    json = JSON.parse(raw);
  } catch (jsonErr) {
    handleSchemaViolation(eventType, raw, 'json_parse_failed', jsonErr, dispatch);
    return { ok: false };
  }
  const result = v.safeParse(schema, json);
  if (!result.success) {
    handleSchemaViolation(eventType, raw, 'schema_violation', result.issues, dispatch);
    return { ok: false };
  }
  return { ok: true, data: result.output };
}

/** Centralized dev-loud / prod-dispatch failure handler for SSE validation. */
function handleSchemaViolation(
  eventType: string,
  raw: unknown,
  kind: 'json_parse_failed' | 'schema_violation',
  detail: unknown,
  dispatch: Dispatch<SSEAction>,
): void {
  const summary =
    kind === 'json_parse_failed'
      ? `SSE ${eventType}: malformed JSON on wire`
      : `SSE ${eventType}: payload failed schema validation`;

  if (import.meta.env.DEV) {
    // Loud. Contract drift in dev is a bug to fix, not a warning to skim.
    console.error(summary, { eventType, raw, detail });
    throw new Error(
      `${summary} — see console for raw payload and issue list.`,
    );
  }

  // Prod: surface via the sse_error channel so the UI shows a toast instead
  // of crashing. `raw` may be a long JSON string; the existing
  // `ParseError` / `BackendError` variants already carry the raw text.
  if (kind === 'json_parse_failed') {
    dispatch({
      type: 'sse_error',
      error: { type: 'ParseError', raw: typeof raw === 'string' ? raw : String(raw) },
    });
  } else {
    dispatch({
      type: 'sse_error',
      error: {
        type: 'BackendError',
        message: `${summary} (client-side schema rejected server payload)`,
      },
    });
  }
}

export type { ConnectionState } from './connectionMachine';

export interface ConnectionInfo {
  state: ConnectionState;
  attempt: number;
  nextRetryIn: number | null;
  retryNow: () => void;
}

interface UseConnectionOptions {
  conversationId: string | undefined;
  /** Dispatch SSE events directly to the conversation atom. */
  dispatch: Dispatch<SSEAction>;
}

function transformBreadcrumb(b: SseBreadcrumb): Breadcrumb {
  return {
    type: b.type,
    label: b.label,
    toolId: b.tool_id,
    sequenceId: b.sequence_id,
    preview: b.preview,
  };
}

function transformInitData(raw: SseInitData): InitPayload {
  // Merge top-level git delta + project info into conversation (backend sends at SSE init level)
  const overrides: Partial<typeof raw.conversation> = {};
  if (raw.commits_behind != null) overrides.commits_behind = raw.commits_behind;
  if (raw.commits_ahead != null) overrides.commits_ahead = raw.commits_ahead;
  if (raw.project_name != null) overrides.project_name = raw.project_name;
  const conversation = Object.keys(overrides).length > 0
    ? { ...raw.conversation, ...overrides }
    : raw.conversation;
  const messages = raw.messages || [];
  const phase = parseConversationState(conversation?.state);

  const breadcrumbs = (raw.breadcrumbs || []).map(transformBreadcrumb);
  const breadcrumbSequenceIds = new Set(
    breadcrumbs
      .filter((b): b is Breadcrumb & { sequenceId: number } => b.sequenceId !== undefined)
      .map((b) => b.sequenceId)
  );

  return {
    conversation,
    messages,
    phase,
    breadcrumbs,
    breadcrumbSequenceIds,
    contextWindow: {
      used: raw.context_window_size ?? 0,
    },
    lastSequenceId: raw.last_sequence_id ?? 0,
  };
}

/**
 * Hook for managing SSE connection lifecycle with reconnection handling.
 *
 * Socket lifecycle manager only. Receives `dispatch` from the conversation
 * atom and calls it with SSEActions. The server always returns the full
 * message list on /stream init — update-in-place mutations arrive via the
 * typed `message_updated` SSE event — so this hook carries no sequence-id
 * state of its own. Reducer-side dedup by `lastSequenceId >= event.sequenceId`
 * still applies inside the atom.
 */
export function useConnection({
  conversationId,
  dispatch,
}: UseConnectionOptions): ConnectionInfo {
  const [machineState, setMachineState] = useState<ConnectionMachineState>(initialState);
  const [countdownSeconds, setCountdownSeconds] = useState<number | null>(null);

  // Refs for values that shouldn't trigger effect re-runs
  const eventSourceRef = useRef<EventSource | null>(null);
  const retryTimeoutRef = useRef<number | null>(null);
  const countdownIntervalRef = useRef<number | null>(null);
  const reconnectedTimeoutRef = useRef<number | null>(null);
  const dispatchRef = useRef(dispatch);
  const conversationIdRef = useRef(conversationId);

  useEffect(() => {
    dispatchRef.current = dispatch;
  }, [dispatch]);

  useEffect(() => {
    conversationIdRef.current = conversationId;
  }, [conversationId]);

  const getContext = useCallback((): TransitionContext => ({
    browserOnline: typeof navigator !== 'undefined' ? navigator.onLine : true,
  }), []);

  const dispatchMachine = useCallback((input: ConnectionInput) => {
    const ctx = getContext();
    setMachineState((current) => {
      const result = transition(current, input, ctx);
      if (result.effects.length > 0) {
        setTimeout(() => executeEffectsRef.current(result.effects), 0);
      }
      return result.state;
    });
  }, [getContext]);

  const dispatchMachineRef = useRef(dispatchMachine);
  useEffect(() => {
    dispatchMachineRef.current = dispatchMachine;
  }, [dispatchMachine]);

  const executeEffects = useCallback((effects: ConnectionEffect[]) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'OPEN_SSE': {
          const convId = conversationIdRef.current;
          if (!convId) break;

          if (eventSourceRef.current) {
            eventSourceRef.current.close();
            eventSourceRef.current = null;
          }

          const url = `/api/conversations/${convId}/stream`;
          const es = new EventSource(url);
          eventSourceRef.current = es;

          es.addEventListener('init', (e) => {
            const res = parseEvent(SseInitDataSchema, e, 'init', dispatchRef.current);
            if (!res.ok) return;

            dispatchMachineRef.current({ type: 'SSE_OPEN' });
            dispatchRef.current({
              type: 'sse_init',
              payload: transformInitData(res.data),
            });
            dispatchRef.current({ type: 'connection_state', state: 'live' });
          });

          es.addEventListener('message', (e) => {
            const res = parseEvent(SseMessageDataSchema, e, 'message', dispatchRef.current);
            if (!res.ok) return;
            const msg = res.data.message;
            dispatchRef.current({
              type: 'sse_message',
              message: msg,
              sequenceId: msg.sequence_id,
            });
          });

          es.addEventListener('message_updated', (e) => {
            const res = parseEvent(
              SseMessageUpdatedDataSchema,
              e,
              'message_updated',
              dispatchRef.current,
            );
            if (!res.ok) return;
            const data = res.data;
            dispatchRef.current({
              type: 'sse_message_updated',
              sequenceId: data.sequence_id,
              messageId: data.message_id,
              ...(data.display_data != null && { displayData: data.display_data as Record<string, unknown> }),
              ...(data.content != null && { content: data.content as import('../api').Message['content'] }),
            });
          });

          es.addEventListener('state_change', (e) => {
            const res = parseEvent(
              SseStateChangeDataSchema,
              e,
              'state_change',
              dispatchRef.current,
            );
            if (!res.ok) return;
            // `data.state` is opaque at the SSE boundary; parseConversationState
            // performs its own discriminated-union validation.
            dispatchRef.current({
              type: 'sse_state_change',
              sequenceId: res.data.sequence_id,
              phase: parseConversationState(res.data.state),
            });
          });

          es.addEventListener('agent_done', (e) => {
            const res = parseEvent(
              SseAgentDoneDataSchema,
              e,
              'agent_done',
              dispatchRef.current,
            );
            if (!res.ok) return;
            dispatchRef.current({ type: 'sse_agent_done', sequenceId: res.data.sequence_id });
          });

          // Terminal subsystem lifecycle event — wired up fully in Task 5.
          // Still validated so a future server change that adds teardown
          // detail cannot slip past this no-op without a schema update.
          es.addEventListener('conversation_became_terminal', (e) => {
            parseEvent(
              SseConversationBecameTerminalDataSchema,
              e,
              'conversation_became_terminal',
              dispatchRef.current,
            );
            // no-op until terminal PTY teardown is implemented
          });

          es.addEventListener('conversation_update', (e) => {
            const res = parseEvent(
              SseConversationUpdateDataSchema,
              e,
              'conversation_update',
              dispatchRef.current,
            );
            if (!res.ok) return;
            dispatchRef.current({
              type: 'sse_conversation_update',
              sequenceId: res.data.sequence_id,
              updates: res.data.conversation as Partial<import('../api').Conversation>,
            });
          });

          // Task 02675: tokens share the server-side global sequence_id
          // counter, so the old per-connection `tokenSequence` closure is
          // gone. The reducer's `applyIfNewer` guard sees strictly
          // increasing ids across reconnects and never stalls.
          es.addEventListener('token', (e) => {
            const res = parseEvent(SseTokenDataSchema, e, 'token', dispatchRef.current);
            if (!res.ok) return;
            dispatchRef.current({
              type: 'sse_token',
              sequenceId: res.data.sequence_id,
              delta: res.data.text,
            });
          });

          es.addEventListener('error', (e) => {
            // Backend application errors arrive as SSE event type "error" WITH data.
            // Native EventSource connection errors fire with NO data — those are
            // not a schema concern and take the connection-error path below.
            const me = e as MessageEvent;
            if (me.data) {
              const res = parseEvent(
                SseErrorDataSchema,
                e,
                'error',
                dispatchRef.current,
              );
              if (!res.ok) return;
              dispatchRef.current({
                type: 'sse_error',
                error: { type: 'BackendError', message: res.data.message },
              });
              return; // Don't treat as connection error
            }
            dispatchMachineRef.current({ type: 'SSE_ERROR' });
            dispatchRef.current({ type: 'connection_state', state: 'reconnecting' });
          });
          break;
        }

        case 'CLOSE_SSE': {
          if (eventSourceRef.current) {
            eventSourceRef.current.close();
            eventSourceRef.current = null;
          }
          dispatchRef.current({ type: 'connection_state', state: 'connecting' });
          break;
        }

        case 'SCHEDULE_RETRY': {
          if (retryTimeoutRef.current !== null) {
            clearTimeout(retryTimeoutRef.current);
          }
          if (countdownIntervalRef.current !== null) {
            clearInterval(countdownIntervalRef.current);
          }

          const seconds = Math.ceil(effect.delayMs / 1000);
          setCountdownSeconds(seconds);

          let remaining = seconds;
          countdownIntervalRef.current = window.setInterval(() => {
            remaining--;
            setCountdownSeconds(remaining > 0 ? remaining : null);
            if (remaining <= 0 && countdownIntervalRef.current !== null) {
              clearInterval(countdownIntervalRef.current);
              countdownIntervalRef.current = null;
            }
          }, 1000);

          retryTimeoutRef.current = window.setTimeout(() => {
            retryTimeoutRef.current = null;
            dispatchMachineRef.current({ type: 'RETRY_TIMER_FIRED' });
          }, effect.delayMs);

          dispatchRef.current({ type: 'connection_state', state: 'reconnecting' });
          break;
        }

        case 'SCHEDULE_RECONNECTED_DISPLAY': {
          if (reconnectedTimeoutRef.current !== null) {
            clearTimeout(reconnectedTimeoutRef.current);
          }
          reconnectedTimeoutRef.current = window.setTimeout(() => {
            reconnectedTimeoutRef.current = null;
            dispatchMachineRef.current({ type: 'RECONNECTED_DISPLAY_DONE' });
          }, RECONNECTED_DISPLAY_MS);
          break;
        }

        case 'CANCEL_TIMERS': {
          if (retryTimeoutRef.current !== null) {
            clearTimeout(retryTimeoutRef.current);
            retryTimeoutRef.current = null;
          }
          if (countdownIntervalRef.current !== null) {
            clearInterval(countdownIntervalRef.current);
            countdownIntervalRef.current = null;
          }
          if (reconnectedTimeoutRef.current !== null) {
            clearTimeout(reconnectedTimeoutRef.current);
            reconnectedTimeoutRef.current = null;
          }
          setCountdownSeconds(null);
          break;
        }
      }
    }
  }, []);

  const executeEffectsRef = useRef(executeEffects);
  useEffect(() => {
    executeEffectsRef.current = executeEffects;
  }, [executeEffects]);

  useEffect(() => {
    const handleOnline = () => dispatchMachineRef.current({ type: 'BROWSER_ONLINE' });
    const handleOffline = () => dispatchMachineRef.current({ type: 'BROWSER_OFFLINE' });

    window.addEventListener('online', handleOnline);
    window.addEventListener('offline', handleOffline);

    return () => {
      window.removeEventListener('online', handleOnline);
      window.removeEventListener('offline', handleOffline);
    };
  }, []);

  useEffect(() => {
    const handleVisibility = () => {
      if (document.visibilityState === 'visible' && navigator.onLine) {
        dispatchMachineRef.current({ type: 'BROWSER_ONLINE' });
      }
    };
    document.addEventListener('visibilitychange', handleVisibility);
    return () => document.removeEventListener('visibilitychange', handleVisibility);
  }, []);

  useEffect(() => {
    if (conversationId) {
      dispatchMachineRef.current({ type: 'CONNECT' });
    } else {
      dispatchMachineRef.current({ type: 'DISCONNECT' });
    }

    return () => {
      dispatchMachineRef.current({ type: 'DISCONNECT' });
    };
  }, [conversationId]);

  const retryNow = useCallback(() => {
    dispatchMachineRef.current({ type: 'BROWSER_ONLINE' });
  }, []);

  return {
    state: machineState.state,
    attempt: machineState.attempt,
    nextRetryIn: countdownSeconds,
    retryNow,
  };
}
