import { useState, useCallback, useEffect, useRef, type Dispatch } from 'react';
import type { SseInitData, SseMessageData, SseStateChangeData } from '../api';
import type { SSEAction, InitPayload } from '../conversation/atom';
import type { Breadcrumb } from '../types';
import { parseConversationState } from '../utils';
import type { SseBreadcrumb } from '../api';
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

export type { ConnectionState } from './connectionMachine';

export interface ConnectionInfo {
  state: ConnectionState;
  attempt: number;
  nextRetryIn: number | null;
  retryNow: () => void;
}

interface UseConnectionOptions {
  conversationId: string | undefined;
  /** Current lastSequenceId from the conversation atom, used to build ?after= URL. */
  lastSequenceId: number;
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
  const conversation = raw.conversation;
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
      total: raw.model_context_window ?? 200_000,
    },
    lastSequenceId: raw.last_sequence_id ?? 0,
  };
}

/**
 * Hook for managing SSE connection lifecycle with reconnection handling.
 *
 * After refactor: this hook is a socket lifecycle manager only.
 * - It receives `dispatch` from the conversation atom and calls it with SSEActions.
 * - It receives `lastSequenceId` from the atom for reconnection URL construction.
 * - It does NOT own lastSequenceId or maintain a seenIds set.
 *   Deduplication is handled by the reducer's `lastSequenceId >= event.sequenceId` check.
 */
export function useConnection({
  conversationId,
  lastSequenceId,
  dispatch,
}: UseConnectionOptions): ConnectionInfo {
  const [machineState, setMachineState] = useState<ConnectionMachineState>(initialState);
  const [countdownSeconds, setCountdownSeconds] = useState<number | null>(null);

  // Refs for values that shouldn't trigger effect re-runs
  const eventSourceRef = useRef<EventSource | null>(null);
  const retryTimeoutRef = useRef<number | null>(null);
  const countdownIntervalRef = useRef<number | null>(null);
  const reconnectedTimeoutRef = useRef<number | null>(null);
  const lastSequenceIdRef = useRef<number>(lastSequenceId);
  const dispatchRef = useRef(dispatch);
  const conversationIdRef = useRef(conversationId);

  // Keep refs up to date
  useEffect(() => {
    lastSequenceIdRef.current = lastSequenceId;
  }, [lastSequenceId]);

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

          let url = `/api/conversations/${convId}/stream`;
          if (lastSequenceIdRef.current > 0) {
            url += `?after=${lastSequenceIdRef.current}`;
          }

          const es = new EventSource(url);
          eventSourceRef.current = es;

          es.addEventListener('init', (e) => {
            let raw: SseInitData;
            try {
              raw = JSON.parse((e as MessageEvent).data) as SseInitData;
            } catch {
              dispatchRef.current({
                type: 'sse_error',
                error: { type: 'ParseError', raw: (e as MessageEvent).data },
              });
              return;
            }

            dispatchMachineRef.current({ type: 'SSE_OPEN' });
            dispatchRef.current({
              type: 'sse_init',
              payload: transformInitData(raw),
            });
            dispatchRef.current({ type: 'connection_state', state: 'live' });
          });

          es.addEventListener('message', (e) => {
            let data: SseMessageData;
            try {
              data = JSON.parse((e as MessageEvent).data) as SseMessageData;
            } catch {
              dispatchRef.current({
                type: 'sse_error',
                error: { type: 'ParseError', raw: (e as MessageEvent).data },
              });
              return;
            }

            const msg = data.message;
            if (msg) {
              dispatchRef.current({
                type: 'sse_message',
                message: msg,
                sequenceId: msg.sequence_id,
              });
            }
          });

          es.addEventListener('state_change', (e) => {
            let data: SseStateChangeData;
            try {
              data = JSON.parse((e as MessageEvent).data) as SseStateChangeData;
            } catch {
              dispatchRef.current({
                type: 'sse_error',
                error: { type: 'ParseError', raw: (e as MessageEvent).data },
              });
              return;
            }

            dispatchRef.current({
              type: 'sse_state_change',
              phase: data.state,
              // sequenceId intentionally absent — backend doesn't provide it on state_change
            });
          });

          es.addEventListener('agent_done', () => {
            dispatchRef.current({ type: 'sse_agent_done' });
          });

          es.addEventListener('conversation_update', (e) => {
            try {
              const data = JSON.parse((e as MessageEvent).data) as { conversation?: Record<string, unknown> };
              if (data.conversation) {
                dispatchRef.current({
                  type: 'sse_conversation_update',
                  updates: data.conversation as Partial<import('../api').Conversation>,
                });
              }
            } catch {
              // Non-fatal — conversation metadata update can be retried on reconnect
            }
          });

          // Per-connection monotonic counter for sse_token dedup.
          // Reset on each new connection so the reducer's lastSequence check works correctly.
          let tokenSequence = 0;
          es.addEventListener('token', (e) => {
            let data: { text?: string; request_id?: string };
            try {
              data = JSON.parse((e as MessageEvent).data) as typeof data;
            } catch {
              // Token parse failures are non-fatal — ephemeral events, skip silently
              return;
            }
            if (data.text) {
              tokenSequence++;
              dispatchRef.current({
                type: 'sse_token',
                delta: data.text,
                sequence: tokenSequence,
              });
            }
          });

          es.addEventListener('error', () => {
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
