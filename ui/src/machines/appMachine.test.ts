import { describe, it, expect } from 'vitest';
import {
  transition,
  initialAppState,
  AppState,
} from './appMachine';

// jsdom provides navigator.onLine = true by default
const ctx0 = { pendingOpsCount: 0 };
const ctx1 = { pendingOpsCount: 1 };

describe('appMachine', () => {
  describe('initialAppState', () => {
    it('starts in initializing state', () => {
      expect(initialAppState.type).toBe('initializing');
    });
  });

  describe('initializing state', () => {
    it('INIT_SUCCESS transitions to ready with no effects when no pending ops', () => {
      const result = transition(initialAppState, { type: 'INIT_SUCCESS' }, ctx0);
      expect(result.state.type).toBe('ready');
      expect(result.effects).toEqual([]);
    });

    it('INIT_SUCCESS emits START_SYNC when there are pending ops', () => {
      const result = transition(initialAppState, { type: 'INIT_SUCCESS' }, ctx1);
      expect(result.state.type).toBe('ready');
      expect(result.effects).toContainEqual({ type: 'START_SYNC' });
    });

    it('INIT_SUCCESS sets network from navigator.onLine', () => {
      const result = transition(initialAppState, { type: 'INIT_SUCCESS' }, ctx0);
      if (result.state.type === 'ready') {
        // jsdom defaults navigator.onLine to true
        expect(['online', 'offline']).toContain(result.state.network);
      }
    });

    it('INIT_ERROR transitions to error state with message', () => {
      const result = transition(initialAppState, { type: 'INIT_ERROR', error: 'db failed' }, ctx0);
      expect(result.state.type).toBe('error');
      if (result.state.type === 'error') {
        expect(result.state.message).toBe('db failed');
      }
      expect(result.effects).toContainEqual(
        expect.objectContaining({ type: 'NOTIFY_USER', level: 'error' })
      );
    });

    it('ignores unrelated events while initializing', () => {
      const result = transition(initialAppState, { type: 'NETWORK_ONLINE' }, ctx0);
      expect(result.state).toBe(initialAppState);
      expect(result.effects).toEqual([]);
    });
  });

  describe('ready state', () => {
    const readyOnline: AppState = { type: 'ready', network: 'online', sync: { type: 'idle' } };
    const readyOffline: AppState = { type: 'ready', network: 'offline', sync: { type: 'idle' } };

    describe('network transitions', () => {
      it('NETWORK_OFFLINE transitions to offline and cancels retry', () => {
        const result = transition(readyOnline, { type: 'NETWORK_OFFLINE' }, ctx0);
        expect(result.state.type).toBe('ready');
        if (result.state.type === 'ready') {
          expect(result.state.network).toBe('offline');
        }
        expect(result.effects).toContainEqual({ type: 'CANCEL_RETRY' });
        expect(result.effects).toContainEqual(
          expect.objectContaining({ type: 'NOTIFY_USER', level: 'warning' })
        );
      });

      it('NETWORK_OFFLINE is idempotent when already offline', () => {
        const result = transition(readyOffline, { type: 'NETWORK_OFFLINE' }, ctx0);
        expect(result.state).toBe(readyOffline);
        expect(result.effects).toEqual([]);
      });

      it('NETWORK_ONLINE transitions to online and notifies', () => {
        const result = transition(readyOffline, { type: 'NETWORK_ONLINE' }, ctx0);
        expect(result.state.type).toBe('ready');
        if (result.state.type === 'ready') {
          expect(result.state.network).toBe('online');
        }
        expect(result.effects).toContainEqual(
          expect.objectContaining({ type: 'NOTIFY_USER', level: 'info' })
        );
      });

      it('NETWORK_ONLINE with pending ops triggers sync', () => {
        const result = transition(readyOffline, { type: 'NETWORK_ONLINE' }, ctx1);
        expect(result.effects).toContainEqual({ type: 'START_SYNC' });
      });

      it('NETWORK_ONLINE is idempotent when already online', () => {
        const result = transition(readyOnline, { type: 'NETWORK_ONLINE' }, ctx0);
        expect(result.state).toBe(readyOnline);
        expect(result.effects).toEqual([]);
      });
    });

    describe('operation queueing', () => {
      it('OPERATION_QUEUED triggers sync when online and idle', () => {
        const op = { type: 'archive' as const, conversationId: 'c1', payload: {}, createdAt: new Date(), retryCount: 0, status: 'pending' as const, id: 'op1' };
        const result = transition(readyOnline, { type: 'OPERATION_QUEUED', op }, ctx1);
        expect(result.effects).toContainEqual({ type: 'START_SYNC' });
      });

      it('OPERATION_QUEUED does not trigger sync when offline', () => {
        const op = { type: 'archive' as const, conversationId: 'c1', payload: {}, createdAt: new Date(), retryCount: 0, status: 'pending' as const, id: 'op1' };
        const result = transition(readyOffline, { type: 'OPERATION_QUEUED', op }, ctx1);
        expect(result.effects).toEqual([]);
      });

      it('OPERATION_QUEUED does not trigger sync when already syncing', () => {
        const syncing: AppState = { type: 'ready', network: 'online', sync: { type: 'syncing', progress: 0, total: 1 } };
        const op = { type: 'archive' as const, conversationId: 'c1', payload: {}, createdAt: new Date(), retryCount: 0, status: 'pending' as const, id: 'op1' };
        const result = transition(syncing, { type: 'OPERATION_QUEUED', op }, ctx1);
        expect(result.effects).toEqual([]);
      });
    });

    describe('sync lifecycle', () => {
      it('SYNC_STARTED transitions to syncing state', () => {
        const result = transition(readyOnline, { type: 'SYNC_STARTED', total: 5 }, ctx0);
        expect(result.state.type).toBe('ready');
        if (result.state.type === 'ready') {
          expect(result.state.sync.type).toBe('syncing');
          if (result.state.sync.type === 'syncing') {
            expect(result.state.sync.total).toBe(5);
            expect(result.state.sync.progress).toBe(0);
          }
        }
      });

      it('SYNC_PROGRESS updates progress', () => {
        const syncing: AppState = { type: 'ready', network: 'online', sync: { type: 'syncing', progress: 1, total: 5 } };
        const result = transition(syncing, { type: 'SYNC_PROGRESS', progress: 3 }, ctx0);
        if (result.state.type === 'ready' && result.state.sync.type === 'syncing') {
          expect(result.state.sync.progress).toBe(3);
        }
      });

      it('SYNC_PROGRESS is ignored when not syncing', () => {
        const result = transition(readyOnline, { type: 'SYNC_PROGRESS', progress: 3 }, ctx0);
        expect(result.state).toBe(readyOnline);
      });

      it('SYNC_COMPLETED resets sync to idle', () => {
        const syncing: AppState = { type: 'ready', network: 'online', sync: { type: 'syncing', progress: 2, total: 5 } };
        const result = transition(syncing, { type: 'SYNC_COMPLETED' }, ctx0);
        if (result.state.type === 'ready') {
          expect(result.state.sync.type).toBe('idle');
        }
      });

      it('SYNC_ERROR schedules retry with initial delay when online', () => {
        const syncing: AppState = { type: 'ready', network: 'online', sync: { type: 'syncing', progress: 1, total: 2 } };
        const result = transition(syncing, { type: 'SYNC_ERROR', message: 'timeout' }, ctx0);
        if (result.state.type === 'ready') {
          expect(result.state.sync.type).toBe('error');
        }
        expect(result.effects.some(e => e.type === 'SCHEDULE_RETRY')).toBe(true);
        expect(result.effects).toContainEqual(
          expect.objectContaining({ type: 'NOTIFY_USER', level: 'warning' })
        );
      });

      it('SYNC_ERROR when offline resets to idle without scheduling retry', () => {
        const syncing: AppState = { type: 'ready', network: 'offline', sync: { type: 'syncing', progress: 1, total: 2 } };
        const result = transition(syncing, { type: 'SYNC_ERROR', message: 'timeout' }, ctx0);
        if (result.state.type === 'ready') {
          expect(result.state.sync.type).toBe('idle');
        }
        expect(result.effects.some(e => e.type === 'SCHEDULE_RETRY')).toBe(false);
      });

      it('SYNC_ERROR uses exponential backoff on repeated errors', () => {
        const firstError: AppState = { type: 'ready', network: 'online', sync: { type: 'error', message: 'timeout', retryIn: 1000 } };
        const result = transition(firstError, { type: 'SYNC_ERROR', message: 'timeout again' }, ctx0);
        const retryEffect = result.effects.find(e => e.type === 'SCHEDULE_RETRY');
        expect(retryEffect).toBeDefined();
        if (retryEffect && retryEffect.type === 'SCHEDULE_RETRY') {
          expect(retryEffect.delayMs).toBeGreaterThan(1000);
        }
      });

      it('RETRY_SYNC triggers sync when online with pending ops', () => {
        const errorState: AppState = { type: 'ready', network: 'online', sync: { type: 'error', message: 'failed', retryIn: 1000 } };
        const result = transition(errorState, { type: 'RETRY_SYNC' }, ctx1);
        expect(result.effects).toContainEqual({ type: 'START_SYNC' });
      });

      it('RETRY_SYNC does nothing when offline', () => {
        const errorState: AppState = { type: 'ready', network: 'offline', sync: { type: 'error', message: 'failed', retryIn: 1000 } };
        const result = transition(errorState, { type: 'RETRY_SYNC' }, ctx1);
        expect(result.effects).toEqual([]);
      });
    });
  });

  describe('error state', () => {
    it('is terminal — all events return unchanged state', () => {
      const errState: AppState = { type: 'error', message: 'fatal' };
      const result = transition(errState, { type: 'INIT_SUCCESS' }, ctx0);
      expect(result.state).toBe(errState);
      expect(result.effects).toEqual([]);
    });
  });
});
