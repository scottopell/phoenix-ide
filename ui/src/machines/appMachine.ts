// App-level state machine for managing global state

import type { PendingOperation } from '../cache';

export type NetworkState = 'online' | 'offline' | 'unknown';

export type SyncStatus = 
  | { type: 'idle' }
  | { type: 'syncing'; progress: number; total: number }
  | { type: 'error'; message: string; retryIn: number };

export type AppState =
  | { type: 'initializing' }
  | { type: 'ready'; network: NetworkState; sync: SyncStatus }
  | { type: 'error'; message: string };

export type AppEvent =
  | { type: 'INIT_SUCCESS' }
  | { type: 'INIT_ERROR'; error: string }
  | { type: 'NETWORK_ONLINE' }
  | { type: 'NETWORK_OFFLINE' }
  | { type: 'SYNC_STARTED'; total: number }
  | { type: 'SYNC_PROGRESS'; progress: number }
  | { type: 'SYNC_COMPLETED' }
  | { type: 'SYNC_ERROR'; message: string }
  | { type: 'RETRY_SYNC' }
  | { type: 'OPERATION_QUEUED'; op: PendingOperation };

export type AppEffect =
  | { type: 'INIT_CACHE' }
  | { type: 'START_SYNC' }
  | { type: 'SCHEDULE_RETRY'; delayMs: number }
  | { type: 'CANCEL_RETRY' }
  | { type: 'NOTIFY_USER'; message: string; level: 'info' | 'warning' | 'error' };

export interface TransitionResult {
  state: AppState;
  effects: AppEffect[];
}

const RETRY_DELAYS = [1000, 2000, 5000, 10000, 30000]; // Exponential backoff

export function transition(
  state: AppState,
  event: AppEvent,
  context: { pendingOpsCount: number }
): TransitionResult {
  switch (state.type) {
    case 'initializing':
      switch (event.type) {
        case 'INIT_SUCCESS':
          return {
            state: {
              type: 'ready',
              network: navigator.onLine ? 'online' : 'offline',
              sync: { type: 'idle' }
            },
            effects: context.pendingOpsCount > 0 ? [{ type: 'START_SYNC' }] : []
          };
        
        case 'INIT_ERROR':
          return {
            state: { type: 'error', message: event.error },
            effects: [{ 
              type: 'NOTIFY_USER', 
              message: `Failed to initialize: ${event.error}`,
              level: 'error' 
            }]
          };
        
        default:
          return { state, effects: [] };
      }
    
    case 'ready':
      switch (event.type) {
        case 'NETWORK_ONLINE':
          if (state.network === 'online') {
            return { state, effects: [] };
          }
          return {
            state: { ...state, network: 'online' },
            effects: context.pendingOpsCount > 0 
              ? [
                  { type: 'START_SYNC' },
                  { type: 'NOTIFY_USER', message: 'Back online', level: 'info' }
                ]
              : [{ type: 'NOTIFY_USER', message: 'Back online', level: 'info' }]
          };
        
        case 'NETWORK_OFFLINE':
          if (state.network === 'offline') {
            return { state, effects: [] };
          }
          return {
            state: { 
              ...state, 
              network: 'offline',
              sync: { type: 'idle' }
            },
            effects: [
              { type: 'CANCEL_RETRY' },
              { type: 'NOTIFY_USER', message: 'Offline - changes will sync when connection returns', level: 'warning' }
            ]
          };
        
        case 'OPERATION_QUEUED':
          if (state.network === 'online' && state.sync.type === 'idle') {
            return {
              state,
              effects: [{ type: 'START_SYNC' }]
            };
          }
          return { state, effects: [] };
        
        case 'SYNC_STARTED':
          return {
            state: {
              ...state,
              sync: { type: 'syncing', progress: 0, total: event.total }
            },
            effects: []
          };
        
        case 'SYNC_PROGRESS':
          if (state.sync.type !== 'syncing') {
            return { state, effects: [] };
          }
          return {
            state: {
              ...state,
              sync: { ...state.sync, progress: event.progress }
            },
            effects: []
          };
        
        case 'SYNC_COMPLETED':
          return {
            state: {
              ...state,
              sync: { type: 'idle' }
            },
            effects: []
          };
        
        case 'SYNC_ERROR': {
          if (state.network === 'offline') {
            // Don't retry if offline
            return {
              state: {
                ...state,
                sync: { type: 'idle' }
              },
              effects: []
            };
          }
          
          // Calculate retry delay
          const currentRetry = state.sync.type === 'error' ? state.sync.retryIn : 0;
          const retryIndex = RETRY_DELAYS.findIndex(d => d > currentRetry);
          const nextDelay = RETRY_DELAYS[retryIndex === -1 ? RETRY_DELAYS.length - 1 : retryIndex];
          
          return {
            state: {
              ...state,
              sync: { 
                type: 'error', 
                message: event.message,
                retryIn: nextDelay 
              }
            },
            effects: [
              { type: 'SCHEDULE_RETRY', delayMs: nextDelay },
              { 
                type: 'NOTIFY_USER', 
                message: `Sync failed: ${event.message}. Retrying in ${Math.round(nextDelay / 1000)}s`,
                level: 'warning' 
              }
            ]
          };
        }
        
        case 'RETRY_SYNC':
          if (state.network === 'online' && context.pendingOpsCount > 0) {
            return {
              state,
              effects: [{ type: 'START_SYNC' }]
            };
          }
          return { state, effects: [] };
        
        default:
          return { state, effects: [] };
      }
    
    case 'error':
      // Terminal state - would need app reload to recover
      return { state, effects: [] };
  }
}

export const initialAppState: AppState = { type: 'initializing' };

// Helper to check if we should show sync status
export function shouldShowSyncStatus(state: AppState): boolean {
  if (state.type !== 'ready') return false;
  return state.sync.type !== 'idle';
}

// Helper to get sync progress percentage
export function getSyncProgress(state: AppState): number | null {
  if (state.type !== 'ready' || state.sync.type !== 'syncing') return null;
  if (state.sync.total === 0) return 0;
  return Math.round((state.sync.progress / state.sync.total) * 100);
}
