import { useState, useEffect, useCallback, useRef } from 'react';
import { cacheDB, PendingOperation } from '../cache';
import { syncQueue } from '../syncQueue';
import {
  transition,
  initialAppState,
  AppState,
  AppEvent,
  AppEffect,
} from '../machines/appMachine';

export function useAppMachine() {
  const [appState, setAppState] = useState<AppState>(initialAppState);
  const pendingOpsCountRef = useRef(0);
  const [pendingOpsCount, setPendingOpsCount] = useState(0);
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const executeEffectsRef = useRef<(effects: AppEffect[]) => void>(() => {});

  const dispatch = useCallback((event: AppEvent) => {
    setAppState(current => {
      const { state, effects } = transition(current, event, { pendingOpsCount: pendingOpsCountRef.current });
      setTimeout(() => executeEffectsRef.current(effects), 0);
      return state;
    });
  }, []);

  const executeEffects = useCallback(async (effects: AppEffect[]) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'INIT_CACHE': {
          // Machine doesn't emit INIT_CACHE, handled defensively
          try {
            await cacheDB.init();
            const pendingOps = await cacheDB.getPendingOps();
            pendingOpsCountRef.current = pendingOps.length;
            setPendingOpsCount(pendingOps.length);
            dispatch({ type: 'INIT_SUCCESS' });
          } catch (error) {
            const message = error instanceof Error ? error.message : 'Failed to initialize local storage';
            dispatch({ type: 'INIT_ERROR', error: message });
          }
          break;
        }
        case 'START_SYNC': {
          try {
            const pendingOps = await cacheDB.getPendingOps();
            if (pendingOps.length === 0) {
              dispatch({ type: 'SYNC_COMPLETED' });
              break;
            }
            dispatch({ type: 'SYNC_STARTED', total: pendingOps.length });
            let completed = 0;
            for (const op of pendingOps) {
              try {
                await syncQueue.processOperation(op);
                await cacheDB.deletePendingOp(op.id);
                completed++;
                pendingOpsCountRef.current = Math.max(0, pendingOpsCountRef.current - 1);
                setPendingOpsCount(prev => Math.max(0, prev - 1));
                dispatch({ type: 'SYNC_PROGRESS', progress: completed });
              } catch (error) {
                console.error('Failed to sync operation:', op, error);
              }
            }
            dispatch({ type: 'SYNC_COMPLETED' });
          } catch (error) {
            const message = error instanceof Error ? error.message : 'Sync failed';
            dispatch({ type: 'SYNC_ERROR', message });
          }
          break;
        }
        case 'SCHEDULE_RETRY': {
          if (retryTimerRef.current !== null) {
            clearTimeout(retryTimerRef.current);
          }
          retryTimerRef.current = setTimeout(() => {
            retryTimerRef.current = null;
            dispatch({ type: 'RETRY_SYNC' });
          }, effect.delayMs);
          break;
        }
        case 'CANCEL_RETRY': {
          if (retryTimerRef.current !== null) {
            clearTimeout(retryTimerRef.current);
            retryTimerRef.current = null;
          }
          break;
        }
        case 'NOTIFY_USER': {
          if (effect.level === 'error') {
            console.error(`[AppMachine] ${effect.message}`);
          } else if (effect.level === 'warning') {
            console.warn(`[AppMachine] ${effect.message}`);
          }
          break;
        }
      }
    }
  }, [dispatch]);

  executeEffectsRef.current = executeEffects;

  // Initialize cache on mount, dispatch result to machine
  useEffect(() => {
    const init = async () => {
      try {
        await cacheDB.init();
        const pendingOps = await cacheDB.getPendingOps();
        pendingOpsCountRef.current = pendingOps.length;
        setPendingOpsCount(pendingOps.length);
        dispatch({ type: 'INIT_SUCCESS' });
      } catch (error) {
        const message = error instanceof Error ? error.message : 'Failed to initialize local storage';
        dispatch({ type: 'INIT_ERROR', error: message });
      }
    };
    init();

    return () => {
      if (retryTimerRef.current !== null) {
        clearTimeout(retryTimerRef.current);
      }
    };
  }, [dispatch]);

  // Track online/offline status
  useEffect(() => {
    const handleOnline = () => dispatch({ type: 'NETWORK_ONLINE' });
    const handleOffline = () => dispatch({ type: 'NETWORK_OFFLINE' });

    window.addEventListener('online', handleOnline);
    window.addEventListener('offline', handleOffline);

    return () => {
      window.removeEventListener('online', handleOnline);
      window.removeEventListener('offline', handleOffline);
    };
  }, [dispatch]);

  const queueOperation = useCallback(async (op: Omit<PendingOperation, 'id'>) => {
    const id = await cacheDB.addPendingOp(op);
    pendingOpsCountRef.current += 1;
    setPendingOpsCount(prev => prev + 1);
    dispatch({ type: 'OPERATION_QUEUED', op: { ...op, id } as PendingOperation });
    return id;
  }, [dispatch]);

  return {
    isReady: appState.type === 'ready',
    initError: appState.type === 'error' ? appState.message : null,
    isOnline: appState.type === 'ready' ? appState.network === 'online' : navigator.onLine,
    pendingOpsCount,
    isSyncing: appState.type === 'ready' && appState.sync.type === 'syncing',
    queueOperation,
  };
}
