/**
 * Pure state machine for connection management.
 * Extracted for testability - can be property tested independently.
 */

export type ConnectionState =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'offline'
  | 'reconnected';

export type ConnectionInput =
  | { type: 'CONNECT' }                    // User/system wants to connect
  | { type: 'DISCONNECT' }                 // User/system wants to disconnect
  | { type: 'SSE_OPEN' }                   // EventSource opened (got init event)
  | { type: 'SSE_ERROR' }                  // EventSource error/closed
  | { type: 'BROWSER_ONLINE' }             // navigator.onLine became true
  | { type: 'BROWSER_OFFLINE' }            // navigator.onLine became false
  | { type: 'RETRY_TIMER_FIRED' }          // Backoff timer completed
  | { type: 'RECONNECTED_DISPLAY_DONE' };  // Brief "reconnected" display finished

export type ConnectionEffect =
  // Open EventSource connection. `epoch` is a per-machine monotonic
  // counter, freshly minted by the transition that emits this effect.
  // The hook captures this value in the EventSource handler closures so
  // every dispatched event can be tagged with the connection generation
  // that produced it; a stale handler firing into a different conversation
  // atom is then rejected by epoch mismatch (task 08683).
  | { type: 'OPEN_SSE'; epoch: number }
  | { type: 'CLOSE_SSE' }                  // Close EventSource connection
  | { type: 'SCHEDULE_RETRY'; delayMs: number }  // Schedule retry timer
  | { type: 'SCHEDULE_RECONNECTED_DISPLAY' }     // Schedule "reconnected" display timer
  | { type: 'CANCEL_TIMERS' };             // Cancel all pending timers

export interface ConnectionMachineState {
  state: ConnectionState;
  attempt: number;           // Current reconnection attempt (0 when connected)
  nextRetryMs: number | null; // For UI countdown display
  /** Monotonic per-machine counter; incremented on every transition that
   *  emits an `OPEN_SSE` effect. Used by the executor to stamp incoming
   *  SSE events so the conversation atom can drop events from a stale
   *  connection generation (cross-conversation contamination guard). */
  epoch: number;
}

export interface ConnectionTransitionResult {
  state: ConnectionMachineState;
  effects: ConnectionEffect[];
}

// Constants
export const BACKOFF_BASE_MS = 1000;
export const BACKOFF_MAX_MS = 30000;
export const OFFLINE_THRESHOLD = 3;
export const RECONNECTED_DISPLAY_MS = 2000;

/**
 * Calculate backoff delay for a given attempt number.
 * Attempt 1: 1s, 2: 2s, 3: 4s, 4: 8s, 5: 16s, 6+: 30s
 */
export function getBackoffDelay(attempt: number): number {
  if (attempt <= 0) return BACKOFF_BASE_MS;
  return Math.min(BACKOFF_BASE_MS * Math.pow(2, attempt - 1), BACKOFF_MAX_MS);
}

/**
 * Initial state for the connection machine.
 */
export function initialState(): ConnectionMachineState {
  return {
    state: 'disconnected',
    attempt: 0,
    nextRetryMs: null,
    epoch: 0,
  };
}

export interface TransitionContext {
  /** Whether the browser reports being online */
  browserOnline: boolean;
}

const defaultContext: TransitionContext = {
  browserOnline: true,
};

/**
 * Pure state transition function.
 * Given current state and input, returns new state and effects to execute.
 * 
 * @param current - Current state
 * @param input - Input event
 * @param ctx - Context with external state (e.g., browser online status)
 */
export function transition(
  current: ConnectionMachineState,
  input: ConnectionInput,
  ctx: TransitionContext = defaultContext
): ConnectionTransitionResult {
  const effects: ConnectionEffect[] = [];

  switch (input.type) {
    case 'CONNECT': {
      // Can only connect from disconnected or if we want to force reconnect
      if (current.state === 'disconnected') {
        const nextEpoch = current.epoch + 1;
        effects.push({ type: 'OPEN_SSE', epoch: nextEpoch });
        return {
          state: { state: 'connecting', attempt: 0, nextRetryMs: null, epoch: nextEpoch },
          effects,
        };
      }
      // Already connecting/connected, ignore
      return { state: current, effects };
    }

    case 'DISCONNECT': {
      effects.push({ type: 'CLOSE_SSE' });
      effects.push({ type: 'CANCEL_TIMERS' });
      return {
        state: { state: 'disconnected', attempt: 0, nextRetryMs: null, epoch: current.epoch },
        effects,
      };
    }

    case 'SSE_OPEN': {
      // Successfully connected
      effects.push({ type: 'CANCEL_TIMERS' });

      // If we were reconnecting, show brief "reconnected" state
      if (current.state === 'reconnecting' || current.state === 'offline') {
        effects.push({ type: 'SCHEDULE_RECONNECTED_DISPLAY' });
        return {
          state: { state: 'reconnected', attempt: 0, nextRetryMs: null, epoch: current.epoch },
          effects,
        };
      }

      return {
        state: { state: 'connected', attempt: 0, nextRetryMs: null, epoch: current.epoch },
        effects,
      };
    }

    case 'SSE_ERROR': {
      // Connection failed or lost
      effects.push({ type: 'CLOSE_SSE' });

      // If browser is offline, go to offline state without scheduling retry
      // (we'll retry when BROWSER_ONLINE fires)
      if (!ctx.browserOnline) {
        effects.push({ type: 'CANCEL_TIMERS' });
        return {
          state: { state: 'offline', attempt: current.attempt + 1, nextRetryMs: null, epoch: current.epoch },
          effects,
        };
      }

      const nextAttempt = current.attempt + 1;
      const delayMs = getBackoffDelay(nextAttempt);

      effects.push({ type: 'SCHEDULE_RETRY', delayMs });

      const nextState: ConnectionState = nextAttempt >= OFFLINE_THRESHOLD ? 'offline' : 'reconnecting';

      return {
        state: { state: nextState, attempt: nextAttempt, nextRetryMs: delayMs, epoch: current.epoch },
        effects,
      };
    }

    case 'BROWSER_ONLINE': {
      // Browser came back online
      if (current.state === 'offline' || current.state === 'reconnecting') {
        const nextEpoch = current.epoch + 1;
        effects.push({ type: 'CANCEL_TIMERS' });
        effects.push({ type: 'OPEN_SSE', epoch: nextEpoch });
        return {
          state: { ...current, state: 'reconnecting', nextRetryMs: null, epoch: nextEpoch },
          effects,
        };
      }
      return { state: current, effects };
    }

    case 'BROWSER_OFFLINE': {
      // Browser went offline
      if (current.state !== 'disconnected') {
        effects.push({ type: 'CLOSE_SSE' });
        effects.push({ type: 'CANCEL_TIMERS' });
        return {
          state: { state: 'offline', attempt: current.attempt, nextRetryMs: null, epoch: current.epoch },
          effects,
        };
      }
      return { state: current, effects };
    }

    case 'RETRY_TIMER_FIRED': {
      // Time to retry connection
      if (current.state === 'reconnecting' || current.state === 'offline') {
        const nextEpoch = current.epoch + 1;
        effects.push({ type: 'OPEN_SSE', epoch: nextEpoch });
        return {
          state: { ...current, nextRetryMs: null, epoch: nextEpoch },
          effects,
        };
      }
      return { state: current, effects };
    }

    case 'RECONNECTED_DISPLAY_DONE': {
      // Transition from "reconnected" to "connected"
      if (current.state === 'reconnected') {
        return {
          state: { state: 'connected', attempt: 0, nextRetryMs: null, epoch: current.epoch },
          effects,
        };
      }
      return { state: current, effects };
    }

    default: {
      // Exhaustive check - if this errors, a case is missing
      input satisfies never;
      return { state: current, effects: [] };
    }
  }
}

/**
 * Property invariants for testing:
 * 
 * 1. attempt >= 0 always
 * 2. nextRetryMs is null when state is 'connected' or 'disconnected' or 'reconnected'
 * 3. nextRetryMs <= BACKOFF_MAX_MS when not null
 * 4. state is 'offline' when attempt >= OFFLINE_THRESHOLD and not connected
 * 5. SSE_OPEN always results in attempt = 0
 * 6. DISCONNECT always results in state = 'disconnected', attempt = 0
 * 7. From 'disconnected', only CONNECT can change state
 * 8. Effects always include CLOSE_SSE before OPEN_SSE in same transition
 */
export function checkInvariants(state: ConnectionMachineState): string[] {
  const violations: string[] = [];
  
  if (state.attempt < 0) {
    violations.push(`attempt must be >= 0, got ${state.attempt}`);
  }
  
  if ((state.state === 'connected' || state.state === 'disconnected' || state.state === 'reconnected') 
      && state.nextRetryMs !== null) {
    violations.push(`nextRetryMs must be null when ${state.state}, got ${state.nextRetryMs}`);
  }
  
  if (state.nextRetryMs !== null && state.nextRetryMs > BACKOFF_MAX_MS) {
    violations.push(`nextRetryMs must be <= ${BACKOFF_MAX_MS}, got ${state.nextRetryMs}`);
  }
  
  return violations;
}
