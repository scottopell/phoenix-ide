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
  SseConversationHardDeletedDataSchema,
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
 *
 * Task 08683: callers that need their synthesized `sse_error` stamped with
 * a connection epoch wrap `dispatch` themselves (see `epochStampedDispatch`
 * in this file). `parseEvent` itself stays epoch-agnostic so non-conversation
 * consumers (e.g. SharePage) can keep using it unchanged.
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

/**
 * Wrap a dispatch so every action it forwards is stamped with the given
 * connection epoch (task 08683). Used inside `useConnection` to ensure that
 * a stale handler closure — firing into a dispatchRef that already points
 * at a different conversation's atom — produces actions that the new atom
 * will reject as out-of-generation.
 *
 * Cheap: just spreads the action and adds `epoch`. The reducer ignores
 * `epoch` on actions that don't carry it in their type; for actions whose
 * type permits `epoch`, it participates in the stale-epoch guard.
 */
function epochStampedDispatch(
  dispatch: Dispatch<SSEAction>,
  epoch: number,
): Dispatch<SSEAction> {
  return (action) => dispatch({ ...action, epoch } as SSEAction);
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
  // Task 08683: read-side mirror of machineState so dispatchMachine can
  // compute the next state synchronously without a functional updater.
  // The previous functional-updater pattern was forced to schedule effects
  // via `setTimeout(0)` because StrictMode invokes updaters twice and would
  // otherwise fire effects twice (e.g. duplicate retry timers). Reading
  // current state from a ref avoids the updater entirely — effects run
  // exactly once per dispatchMachine call.
  const machineStateRef = useRef(machineState);
  machineStateRef.current = machineState;

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
    const current = machineStateRef.current;
    const result = transition(current, input, ctx);
    if (result.state !== current) {
      machineStateRef.current = result.state;
      setMachineState(result.state);
    }
    if (result.effects.length > 0) {
      executeEffectsRef.current(result.effects);
    }
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

          // Task 08683: stamp every dispatched action with the connection's
          // epoch so the atom can drop events from a stale generation.
          // `epoch` is captured in closure here; the handlers below each
          // see the epoch that was current when *this* OPEN_SSE ran, even
          // if a later OPEN_SSE has minted a fresher epoch in the meantime.
          const epoch = effect.epoch;
          const stampedDispatch = epochStampedDispatch(dispatchRef.current, epoch);

          // Lift the atom's `connectionEpoch` to the new generation before
          // any wire event lands. Without this, the very first stamped
          // action (e.g. `connection_state: 'live'` from the init handler)
          // would still pass `isStaleEpoch` (atom.connectionEpoch === null)
          // but every subsequent stamped action from a *different* slug's
          // stale connection would also pass — which is exactly the
          // contamination scenario this task closes.
          dispatchRef.current({ type: 'connection_opened', epoch });

          const url = `/api/conversations/${convId}/stream`;
          const es = new EventSource(url);
          eventSourceRef.current = es;

          es.addEventListener('init', (e) => {
            const res = parseEvent(SseInitDataSchema, e, 'init', stampedDispatch);
            if (!res.ok) return;

            dispatchMachineRef.current({ type: 'SSE_OPEN' });
            stampedDispatch({
              type: 'sse_init',
              payload: transformInitData(res.data),
            });
            stampedDispatch({ type: 'connection_state', state: 'live' });
          });

          es.addEventListener('message', (e) => {
            const res = parseEvent(SseMessageDataSchema, e, 'message', stampedDispatch);
            if (!res.ok) return;
            const msg = res.data.message;
            stampedDispatch({
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
              stampedDispatch,
            );
            if (!res.ok) return;
            const data = res.data;
            stampedDispatch({
              type: 'sse_message_updated',
              sequenceId: data.sequence_id,
              messageId: data.message_id,
              ...(data.display_data != null && { displayData: data.display_data as Record<string, unknown> }),
              ...(data.content != null && { content: data.content as import('../api').Message['content'] }),
              ...(data.duration_ms != null && { durationMs: data.duration_ms }),
            });
          });

          es.addEventListener('state_change', (e) => {
            const res = parseEvent(
              SseStateChangeDataSchema,
              e,
              'state_change',
              stampedDispatch,
            );
            if (!res.ok) return;
            // `data.state` is opaque at the SSE boundary; parseConversationState
            // performs its own discriminated-union validation.
            stampedDispatch({
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
              stampedDispatch,
            );
            if (!res.ok) return;
            stampedDispatch({ type: 'sse_agent_done', sequenceId: res.data.sequence_id });
          });

          // Terminal subsystem lifecycle event — wired up fully in Task 5.
          // Still validated so a future server change that adds teardown
          // detail cannot slip past this no-op without a schema update.
          es.addEventListener('conversation_became_terminal', (e) => {
            parseEvent(
              SseConversationBecameTerminalDataSchema,
              e,
              'conversation_became_terminal',
              stampedDispatch,
            );
            // no-op until terminal PTY teardown is implemented
          });

          es.addEventListener('conversation_update', (e) => {
            const res = parseEvent(
              SseConversationUpdateDataSchema,
              e,
              'conversation_update',
              stampedDispatch,
            );
            if (!res.ok) return;
            stampedDispatch({
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
            const res = parseEvent(SseTokenDataSchema, e, 'token', stampedDispatch);
            if (!res.ok) return;
            stampedDispatch({
              type: 'sse_token',
              sequenceId: res.data.sequence_id,
              delta: res.data.text,
            });
          });

          // REQ-BED-032 step 6: hard-delete cascade emits this on the
          // per-conversation channel after the row is gone. Notify the
          // sidebar (cross-tab) by dispatching a window event so the
          // DesktopLayout can refresh its conversation list immediately
          // — without waiting for the 5s polling tick.
          es.addEventListener('conversation_hard_deleted', (e) => {
            const res = parseEvent(
              SseConversationHardDeletedDataSchema,
              e,
              'conversation_hard_deleted',
              stampedDispatch,
            );
            if (!res.ok) return;
            window.dispatchEvent(
              new CustomEvent('phoenix:conversation-hard-deleted', {
                detail: { conversationId: res.data.conversation_id },
              }),
            );
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
                stampedDispatch,
              );
              if (!res.ok) return;
              stampedDispatch({
                type: 'sse_error',
                sequenceId: res.data.sequence_id,
                error: { type: 'BackendError', message: res.data.message },
              });
              return; // Don't treat as connection error
            }
            dispatchMachineRef.current({ type: 'SSE_ERROR' });
            stampedDispatch({ type: 'connection_state', state: 'reconnecting' });
          });
          break;
        }

        case 'CLOSE_SSE': {
          if (eventSourceRef.current) {
            eventSourceRef.current.close();
            eventSourceRef.current = null;
          }
          // Stamp with the current machine epoch (the connection generation
          // that just closed). After a slug change this epoch will not
          // match the freshly-navigated atom's `connectionEpoch`, so the
          // 'connecting' state from the old conversation's close cannot
          // leak into the new conversation's atom.
          dispatchRef.current({
            type: 'connection_state',
            state: 'connecting',
            epoch: machineStateRef.current.epoch,
          });
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

          dispatchRef.current({
            type: 'connection_state',
            state: 'reconnecting',
            epoch: machineStateRef.current.epoch,
          });
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