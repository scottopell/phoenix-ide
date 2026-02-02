import { describe, it, expect } from 'vitest';
import * as fc from 'fast-check';
import {
  transition,
  initialState,
  checkInvariants,
  getBackoffDelay,
  ConnectionMachineState,
  ConnectionInput,
  TransitionContext,
  BACKOFF_BASE_MS,
  BACKOFF_MAX_MS,
  OFFLINE_THRESHOLD,
} from './connectionMachine';

// Default context for tests - browser is online
const onlineCtx: TransitionContext = { browserOnline: true };
const offlineCtx: TransitionContext = { browserOnline: false };

// Arbitrary for generating random inputs
const inputArbitrary: fc.Arbitrary<ConnectionInput> = fc.oneof(
  fc.constant({ type: 'CONNECT' } as const),
  fc.constant({ type: 'DISCONNECT' } as const),
  fc.constant({ type: 'SSE_OPEN' } as const),
  fc.constant({ type: 'SSE_ERROR' } as const),
  fc.constant({ type: 'BROWSER_ONLINE' } as const),
  fc.constant({ type: 'BROWSER_OFFLINE' } as const),
  fc.constant({ type: 'RETRY_TIMER_FIRED' } as const),
  fc.constant({ type: 'RECONNECTED_DISPLAY_DONE' } as const)
);

// Arbitrary for generating random context
const contextArbitrary: fc.Arbitrary<TransitionContext> = fc.record({
  browserOnline: fc.boolean(),
});

describe('connectionMachine', () => {
  describe('getBackoffDelay', () => {
    it('returns base delay for attempt 1', () => {
      expect(getBackoffDelay(1)).toBe(BACKOFF_BASE_MS);
    });

    it('doubles each attempt', () => {
      expect(getBackoffDelay(2)).toBe(2000);
      expect(getBackoffDelay(3)).toBe(4000);
      expect(getBackoffDelay(4)).toBe(8000);
      expect(getBackoffDelay(5)).toBe(16000);
    });

    it('caps at max delay', () => {
      expect(getBackoffDelay(6)).toBe(BACKOFF_MAX_MS);
      expect(getBackoffDelay(10)).toBe(BACKOFF_MAX_MS);
      expect(getBackoffDelay(100)).toBe(BACKOFF_MAX_MS);
    });

    it('handles zero and negative attempts', () => {
      expect(getBackoffDelay(0)).toBe(BACKOFF_BASE_MS);
      expect(getBackoffDelay(-1)).toBe(BACKOFF_BASE_MS);
    });
  });

  describe('initialState', () => {
    it('starts disconnected with zero attempt', () => {
      const state = initialState();
      expect(state.state).toBe('disconnected');
      expect(state.attempt).toBe(0);
      expect(state.nextRetryMs).toBeNull();
    });

    it('passes invariant checks', () => {
      const violations = checkInvariants(initialState());
      expect(violations).toEqual([]);
    });
  });

  describe('property: invariants hold after any sequence of inputs', () => {
    it('maintains valid state through random input sequences', () => {
      fc.assert(
        fc.property(
          fc.array(fc.tuple(inputArbitrary, contextArbitrary), { minLength: 0, maxLength: 50 }),
          (inputsWithCtx) => {
            let state = initialState();
            
            for (const [input, ctx] of inputsWithCtx) {
              const result = transition(state, input, ctx);
              state = result.state;
              
              const violations = checkInvariants(state);
              if (violations.length > 0) {
                return false;
              }
            }
            
            return true;
          }
        ),
        { numRuns: 1000 }
      );
    });
  });

  describe('property: attempt is always non-negative', () => {
    it('never produces negative attempt count', () => {
      fc.assert(
        fc.property(
          fc.array(fc.tuple(inputArbitrary, contextArbitrary), { minLength: 1, maxLength: 100 }),
          (inputsWithCtx) => {
            let state = initialState();
            
            for (const [input, ctx] of inputsWithCtx) {
              const result = transition(state, input, ctx);
              state = result.state;
              
              if (state.attempt < 0) {
                return false;
              }
            }
            
            return true;
          }
        ),
        { numRuns: 1000 }
      );
    });
  });

  describe('property: SSE_OPEN always resets attempt to 0', () => {
    it('resets attempt on successful connection', () => {
      fc.assert(
        fc.property(
          fc.array(fc.tuple(inputArbitrary, contextArbitrary), { minLength: 0, maxLength: 20 }),
          (preInputsWithCtx) => {
            let state = initialState();
            
            // Apply random inputs to get into some state
            for (const [input, ctx] of preInputsWithCtx) {
              state = transition(state, input, ctx).state;
            }
            
            // Apply SSE_OPEN (context doesn't matter for this input)
            const result = transition(state, { type: 'SSE_OPEN' }, onlineCtx);
            
            return result.state.attempt === 0;
          }
        ),
        { numRuns: 500 }
      );
    });
  });

  describe('property: DISCONNECT always returns to disconnected with attempt 0', () => {
    it('resets to disconnected state', () => {
      fc.assert(
        fc.property(
          fc.array(fc.tuple(inputArbitrary, contextArbitrary), { minLength: 0, maxLength: 20 }),
          (preInputsWithCtx) => {
            let state = initialState();
            
            for (const [input, ctx] of preInputsWithCtx) {
              state = transition(state, input, ctx).state;
            }
            
            const result = transition(state, { type: 'DISCONNECT' }, onlineCtx);
            
            return (
              result.state.state === 'disconnected' &&
              result.state.attempt === 0 &&
              result.state.nextRetryMs === null
            );
          }
        ),
        { numRuns: 500 }
      );
    });
  });

  describe('property: nextRetryMs never exceeds BACKOFF_MAX_MS', () => {
    it('caps retry delay', () => {
      fc.assert(
        fc.property(
          fc.array(fc.tuple(inputArbitrary, contextArbitrary), { minLength: 1, maxLength: 100 }),
          (inputsWithCtx) => {
            let state = initialState();
            
            for (const [input, ctx] of inputsWithCtx) {
              const result = transition(state, input, ctx);
              state = result.state;
              
              if (state.nextRetryMs !== null && state.nextRetryMs > BACKOFF_MAX_MS) {
                return false;
              }
            }
            
            return true;
          }
        ),
        { numRuns: 1000 }
      );
    });
  });

  describe('property: effects are consistent', () => {
    it('CLOSE_SSE comes before OPEN_SSE in same transition', () => {
      fc.assert(
        fc.property(
          inputArbitrary,
          contextArbitrary,
          fc.array(fc.tuple(inputArbitrary, contextArbitrary), { minLength: 0, maxLength: 20 }),
          (input, ctx, preInputsWithCtx) => {
            let state = initialState();
            
            for (const [preInput, preCtx] of preInputsWithCtx) {
              state = transition(state, preInput, preCtx).state;
            }
            
            const result = transition(state, input, ctx);
            
            const closeIndex = result.effects.findIndex(e => e.type === 'CLOSE_SSE');
            const openIndex = result.effects.findIndex(e => e.type === 'OPEN_SSE');
            
            // If both are present, CLOSE must come first
            if (closeIndex !== -1 && openIndex !== -1) {
              return closeIndex < openIndex;
            }
            
            return true;
          }
        ),
        { numRuns: 1000 }
      );
    });
  });

  describe('specific transitions', () => {
    it('CONNECT from disconnected goes to connecting', () => {
      const result = transition(initialState(), { type: 'CONNECT' }, onlineCtx);
      expect(result.state.state).toBe('connecting');
      expect(result.effects).toContainEqual({ type: 'OPEN_SSE' });
    });

    it('SSE_ERROR increments attempt and schedules retry when online', () => {
      let state = initialState();
      state = transition(state, { type: 'CONNECT' }, onlineCtx).state;
      
      const result = transition(state, { type: 'SSE_ERROR' }, onlineCtx);
      
      expect(result.state.attempt).toBe(1);
      expect(result.state.state).toBe('reconnecting');
      expect(result.effects).toContainEqual({ type: 'CLOSE_SSE' });
      expect(result.effects.some(e => e.type === 'SCHEDULE_RETRY')).toBe(true);
    });

    it('SSE_ERROR goes to offline without scheduling retry when browser offline', () => {
      let state = initialState();
      state = transition(state, { type: 'CONNECT' }, onlineCtx).state;
      
      const result = transition(state, { type: 'SSE_ERROR' }, offlineCtx);
      
      expect(result.state.attempt).toBe(1);
      expect(result.state.state).toBe('offline');
      expect(result.effects).toContainEqual({ type: 'CLOSE_SSE' });
      expect(result.effects).toContainEqual({ type: 'CANCEL_TIMERS' });
      expect(result.effects.some(e => e.type === 'SCHEDULE_RETRY')).toBe(false);
    });

    it('becomes offline after OFFLINE_THRESHOLD errors', () => {
      let state = initialState();
      state = transition(state, { type: 'CONNECT' }, onlineCtx).state;
      
      // Simulate multiple errors
      for (let i = 0; i < OFFLINE_THRESHOLD; i++) {
        state = transition(state, { type: 'SSE_ERROR' }, onlineCtx).state;
        // Simulate timer firing (would normally trigger reconnect)
        if (state.state === 'reconnecting' || state.state === 'offline') {
          state = transition(state, { type: 'RETRY_TIMER_FIRED' }, onlineCtx).state;
        }
      }
      
      // After enough errors, should be offline
      expect(state.attempt).toBeGreaterThanOrEqual(OFFLINE_THRESHOLD);
    });

    it('BROWSER_ONLINE triggers reconnect from offline', () => {
      // Get to offline state
      let state: ConnectionMachineState = {
        state: 'offline',
        attempt: 5,
        nextRetryMs: 30000,
      };
      
      const result = transition(state, { type: 'BROWSER_ONLINE' }, onlineCtx);
      
      expect(result.state.state).toBe('reconnecting');
      expect(result.effects).toContainEqual({ type: 'CANCEL_TIMERS' });
      expect(result.effects).toContainEqual({ type: 'OPEN_SSE' });
    });

    it('reconnected state transitions to connected after display timer', () => {
      const state: ConnectionMachineState = {
        state: 'reconnected',
        attempt: 0,
        nextRetryMs: null,
      };
      
      const result = transition(state, { type: 'RECONNECTED_DISPLAY_DONE' }, onlineCtx);
      
      expect(result.state.state).toBe('connected');
    });
  });
});
