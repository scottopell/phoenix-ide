import { useState, useCallback, useEffect, useRef } from 'react';
import type { SseEventType, SseEventData, SseInitData, SseMessageData } from '../api';

const BACKOFF_BASE = 1000;      // 1 second
const BACKOFF_MAX = 30000;      // 30 seconds
const OFFLINE_THRESHOLD = 3;   // Show "offline" after N failures
const RECONNECTED_DISPLAY_MS = 2000;  // How long to show "reconnected" banner

export type ConnectionState = 
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'offline'
  | 'reconnected';  // Brief state after recovery

export interface ConnectionInfo {
  state: ConnectionState;
  attempt: number;
  nextRetryIn: number | null;  // Seconds until next retry (for countdown)
  lastSequenceId: number | null;
}

interface UseConnectionOptions {
  conversationId: string | undefined;
  onEvent: (eventType: SseEventType, data: SseEventData) => void;
}

function getBackoffDelay(attempt: number): number {
  return Math.min(BACKOFF_BASE * Math.pow(2, attempt - 1), BACKOFF_MAX);
}

/**
 * Hook for managing SSE connection lifecycle with reconnection handling.
 * Features:
 * - Exponential backoff (1s, 2s, 4s, ... max 30s)
 * - Sequence tracking for reconnection
 * - navigator.onLine integration
 * - Countdown timer for next retry
 */
export function useConnection({ conversationId, onEvent }: UseConnectionOptions): ConnectionInfo {
  const [state, setState] = useState<ConnectionState>('disconnected');
  const [attempt, setAttempt] = useState(0);
  const [nextRetryIn, setNextRetryIn] = useState<number | null>(null);
  const [lastSequenceId, setLastSequenceId] = useState<number | null>(null);

  const eventSourceRef = useRef<EventSource | null>(null);
  const retryTimeoutRef = useRef<number | null>(null);
  const countdownIntervalRef = useRef<number | null>(null);
  const lastSequenceIdRef = useRef<number | null>(null);
  const seenIdsRef = useRef<Set<number>>(new Set());
  const reconnectedTimeoutRef = useRef<number | null>(null);
  const onEventRef = useRef(onEvent);

  // Keep onEvent ref up to date
  useEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);

  // Load last sequence ID from localStorage on mount
  useEffect(() => {
    if (conversationId) {
      try {
        const stored = localStorage.getItem(`phoenix:lastSeq:${conversationId}`);
        if (stored) {
          const id = parseInt(stored, 10);
          if (!isNaN(id)) {
            lastSequenceIdRef.current = id;
            setLastSequenceId(id);
          }
        }
      } catch (error) {
        console.warn('Error reading lastSeq from localStorage:', error);
      }
    }
  }, [conversationId]);

  // Track sequence ID from messages
  const updateSequenceId = useCallback((seqId: number) => {
    lastSequenceIdRef.current = seqId;
    setLastSequenceId(seqId);
    if (conversationId) {
      try {
        localStorage.setItem(`phoenix:lastSeq:${conversationId}`, String(seqId));
      } catch (error) {
        console.warn('Error saving lastSeq to localStorage:', error);
      }
    }
  }, [conversationId]);

  // Clear timers helper
  const clearTimers = useCallback(() => {
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
    setNextRetryIn(null);
  }, []);

  // Close connection helper
  const closeConnection = useCallback(() => {
    if (eventSourceRef.current) {
      eventSourceRef.current.close();
      eventSourceRef.current = null;
    }
  }, []);

  // Schedule reconnection with backoff
  const scheduleReconnect = useCallback((attemptNum: number) => {
    clearTimers();
    
    const delay = getBackoffDelay(attemptNum);
    const delaySeconds = Math.ceil(delay / 1000);
    setNextRetryIn(delaySeconds);

    // Countdown timer
    let remaining = delaySeconds;
    countdownIntervalRef.current = window.setInterval(() => {
      remaining--;
      setNextRetryIn(remaining > 0 ? remaining : null);
      if (remaining <= 0 && countdownIntervalRef.current !== null) {
        clearInterval(countdownIntervalRef.current);
        countdownIntervalRef.current = null;
      }
    }, 1000);

    // Actual reconnect timeout - will be set up by the connect function
    return delay;
  }, [clearTimers]);

  // Connect to SSE stream
  const connect = useCallback(() => {
    if (!conversationId) return;

    closeConnection();
    clearTimers();
    
    const isReconnecting = attempt > 0;
    setState(isReconnecting ? 'reconnecting' : 'connecting');

    // Build URL with after parameter if we have a sequence ID
    let url = `/api/conversations/${conversationId}/stream`;
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

      // If we were reconnecting, show brief "reconnected" state
      if (isReconnecting) {
        setState('reconnected');
        reconnectedTimeoutRef.current = window.setTimeout(() => {
          setState('connected');
          reconnectedTimeoutRef.current = null;
        }, RECONNECTED_DISPLAY_MS);
      } else {
        setState('connected');
      }
      
      setAttempt(0);
      clearTimers();
      onEventRef.current('init', data);
    });

    es.addEventListener('message', (e) => {
      const data = JSON.parse((e as MessageEvent).data) as SseMessageData;
      const msg = data.message;
      
      if (msg) {
        // Deduplicate by sequence_id
        if (seenIdsRef.current.has(msg.sequence_id)) {
          return; // Already have this message
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
      if (es.readyState === EventSource.CLOSED || es.readyState === EventSource.CONNECTING) {
        closeConnection();
        
        const nextAttempt = attempt + 1;
        setAttempt(nextAttempt);
        
        // Update state based on attempt count
        if (nextAttempt >= OFFLINE_THRESHOLD) {
          setState('offline');
        } else {
          setState('reconnecting');
        }

        // Check if browser is online
        if (!navigator.onLine) {
          setState('offline');
          // Don't schedule reconnect, wait for online event
          return;
        }

        // Schedule reconnection
        const delay = scheduleReconnect(nextAttempt);
        retryTimeoutRef.current = window.setTimeout(() => {
          retryTimeoutRef.current = null;
          connect();
        }, delay);

        onEventRef.current('disconnected', {});
      }
    });

    es.addEventListener('open', () => {
      // Connection opened, waiting for init event
    });
  }, [conversationId, attempt, closeConnection, clearTimers, scheduleReconnect, updateSequenceId]);

  // Handle online/offline events
  useEffect(() => {
    const handleOnline = () => {
      // Browser came back online, try to reconnect immediately
      if (state === 'offline' || state === 'reconnecting') {
        clearTimers();
        connect();
      }
    };

    const handleOffline = () => {
      // Browser went offline
      clearTimers();
      setState('offline');
    };

    window.addEventListener('online', handleOnline);
    window.addEventListener('offline', handleOffline);

    return () => {
      window.removeEventListener('online', handleOnline);
      window.removeEventListener('offline', handleOffline);
    };
  }, [state, clearTimers, connect]);

  // Initial connection when conversationId changes
  useEffect(() => {
    if (conversationId) {
      // Reset state for new conversation
      setAttempt(0);
      seenIdsRef.current.clear();
      connect();
    } else {
      closeConnection();
      clearTimers();
      setState('disconnected');
    }

    return () => {
      closeConnection();
      clearTimers();
    };
  }, [conversationId]); // Intentionally not including connect to avoid loops

  return {
    state,
    attempt,
    nextRetryIn,
    lastSequenceId,
  };
}
