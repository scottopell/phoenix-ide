import { useState, useCallback, useEffect, useRef } from 'react';
import type { SseEventType, SseEventData, SseInitData, SseMessageData } from '../api';
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
  nextRetryIn: number | null;  // Seconds until next retry (for countdown)
  lastSequenceId: number | null;
  retryNow: () => void;
}

interface UseConnectionOptions {
  conversationId: string | undefined;
  onEvent: (eventType: SseEventType, data: SseEventData) => void;
}

/**
 * Hook for managing SSE connection lifecycle with reconnection handling.
 * Uses a pure state machine (connectionMachine.ts) for testable state transitions.
 */
export function useConnection({ conversationId, onEvent }: UseConnectionOptions): ConnectionInfo {
  const [machineState, setMachineState] = useState<ConnectionMachineState>(initialState);
  const [lastSequenceId, setLastSequenceId] = useState<number | null>(null);
  const [countdownSeconds, setCountdownSeconds] = useState<number | null>(null);

  // Refs for values that shouldn't trigger effect re-runs
  const eventSourceRef = useRef<EventSource | null>(null);
  const retryTimeoutRef = useRef<number | null>(null);
  const countdownIntervalRef = useRef<number | null>(null);
  const reconnectedTimeoutRef = useRef<number | null>(null);
  const lastSequenceIdRef = useRef<number | null>(null);
  const seenIdsRef = useRef<Set<number>>(new Set());
  const onEventRef = useRef(onEvent);
  const conversationIdRef = useRef(conversationId);

  // Keep onEvent ref up to date
  useEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);

  // Keep conversationId ref up to date
  useEffect(() => {
    conversationIdRef.current = conversationId;
  }, [conversationId]);

  // Track sequence ID from messages (for reconnection within same session)
  const updateSequenceId = useCallback((seqId: number) => {
    lastSequenceIdRef.current = seqId;
    setLastSequenceId(seqId);
  }, []);

  // Get current context for state machine
  const getContext = useCallback((): TransitionContext => ({
    browserOnline: typeof navigator !== 'undefined' ? navigator.onLine : true,
  }), []);

  // Dispatch function - we'll keep a ref to avoid effect re-runs
  const dispatchImpl = useCallback((input: ConnectionInput) => {
    const ctx = getContext();
    setMachineState((current) => {
      const result = transition(current, input, ctx);
      // Execute effects after state update
      // Using setTimeout to avoid state update during render
      if (result.effects.length > 0) {
        setTimeout(() => executeEffectsRef.current(result.effects), 0);
      }
      return result.state;
    });
  }, [getContext]);

  // Stable ref to dispatch - use this in effects that shouldn't re-run on callback changes
  const dispatchRef = useRef(dispatchImpl);
  useEffect(() => {
    dispatchRef.current = dispatchImpl;
  }, [dispatchImpl]);

  // Execute effects from state machine transitions
  const executeEffects = useCallback((effects: ConnectionEffect[]) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'OPEN_SSE': {
          const convId = conversationIdRef.current;
          if (!convId) break;

          // Close existing connection first
          if (eventSourceRef.current) {
            eventSourceRef.current.close();
            eventSourceRef.current = null;
          }

          // Build URL with after parameter if we have a sequence ID
          let url = `/api/conversations/${convId}/stream`;
          if (lastSequenceIdRef.current !== null) {
            url += `?after=${lastSequenceIdRef.current}`;
          }

          const es = new EventSource(url);
          eventSourceRef.current = es;

          es.addEventListener('init', (e) => {
            const data = JSON.parse((e as MessageEvent).data) as SseInitData;

            // Track sequence ID from init
            if (data.last_sequence_id !== undefined) {
              updateSequenceId(data.last_sequence_id);
            }

            // Track sequence IDs from messages to dedupe
            if (data.messages) {
              for (const msg of data.messages) {
                seenIdsRef.current.add(msg.sequence_id);
              }
            }

            // Signal successful connection to state machine
            dispatchRef.current({ type: 'SSE_OPEN' });
            onEventRef.current('init', data);
          });

          es.addEventListener('message', (e) => {
            const data = JSON.parse((e as MessageEvent).data) as SseMessageData;
            const msg = data.message;

            if (msg) {
              // Deduplicate by sequence_id
              if (seenIdsRef.current.has(msg.sequence_id)) {
                return;
              }
              seenIdsRef.current.add(msg.sequence_id);
              updateSequenceId(msg.sequence_id);
            }

            onEventRef.current('message', data);
          });

          es.addEventListener('state_change', (e) => {
            const data = JSON.parse((e as MessageEvent).data);
            onEventRef.current('state_change', data);
          });

          es.addEventListener('agent_done', () => {
            onEventRef.current('agent_done', {});
          });

          es.addEventListener('error', () => {
            dispatchRef.current({ type: 'SSE_ERROR' });
            onEventRef.current('disconnected', {});
          });
          break;
        }

        case 'CLOSE_SSE': {
          if (eventSourceRef.current) {
            eventSourceRef.current.close();
            eventSourceRef.current = null;
          }
          break;
        }

        case 'SCHEDULE_RETRY': {
          // Clear existing timers
          if (retryTimeoutRef.current !== null) {
            clearTimeout(retryTimeoutRef.current);
          }
          if (countdownIntervalRef.current !== null) {
            clearInterval(countdownIntervalRef.current);
          }

          // Start countdown
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

          // Schedule retry
          retryTimeoutRef.current = window.setTimeout(() => {
            retryTimeoutRef.current = null;
            dispatchRef.current({ type: 'RETRY_TIMER_FIRED' });
          }, effect.delayMs);
          break;
        }

        case 'SCHEDULE_RECONNECTED_DISPLAY': {
          if (reconnectedTimeoutRef.current !== null) {
            clearTimeout(reconnectedTimeoutRef.current);
          }
          reconnectedTimeoutRef.current = window.setTimeout(() => {
            reconnectedTimeoutRef.current = null;
            dispatchRef.current({ type: 'RECONNECTED_DISPLAY_DONE' });
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
  }, [updateSequenceId]);

  // Stable ref to executeEffects
  const executeEffectsRef = useRef(executeEffects);
  useEffect(() => {
    executeEffectsRef.current = executeEffects;
  }, [executeEffects]);

  // Handle online/offline events
  useEffect(() => {
    const handleOnline = () => dispatchRef.current({ type: 'BROWSER_ONLINE' });
    const handleOffline = () => dispatchRef.current({ type: 'BROWSER_OFFLINE' });

    window.addEventListener('online', handleOnline);
    window.addEventListener('offline', handleOffline);

    return () => {
      window.removeEventListener('online', handleOnline);
      window.removeEventListener('offline', handleOffline);
    };
  }, []); // No dependencies - handlers use refs

  // Handle visibility change - retry immediately when user tabs back
  useEffect(() => {
    const handleVisibility = () => {
      if (document.visibilityState === 'visible' && navigator.onLine) {
        dispatchRef.current({ type: 'BROWSER_ONLINE' });
      }
    };
    document.addEventListener('visibilitychange', handleVisibility);
    return () => document.removeEventListener('visibilitychange', handleVisibility);
  }, []);

  // Connect when conversationId changes
  useEffect(() => {
    if (conversationId) {
      // Reset for new conversation
      seenIdsRef.current.clear();
      dispatchRef.current({ type: 'CONNECT' });
    } else {
      dispatchRef.current({ type: 'DISCONNECT' });
    }

    return () => {
      dispatchRef.current({ type: 'DISCONNECT' });
    };
  }, [conversationId]); // Only depends on conversationId - dispatch accessed via ref

  // Expose manual retry (reuses BROWSER_ONLINE to cancel timers + open SSE)
  const retryNow = useCallback(() => {
    dispatchRef.current({ type: 'BROWSER_ONLINE' });
  }, []);

  return {
    state: machineState.state,
    attempt: machineState.attempt,
    nextRetryIn: countdownSeconds,
    lastSequenceId,
    retryNow,
  };
}
