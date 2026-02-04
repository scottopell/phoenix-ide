import type { PendingOperation } from '../cache';import { useState, useEffect, useCallback, useRef } from 'react';
import { 
  AppState, 
  AppEvent, 
  AppEffect, 
  initialAppState, 
  transition,
  shouldShowSyncStatus,
  getSyncProgress
} from '../machines/appMachine';
import { cacheDB } from '../cache';
import { syncQueue } from '../syncQueue';

export interface AppInfo {
  state: AppState;
  isOnline: boolean;
  isInitializing: boolean;
  isReady: boolean;
  hasError: boolean;
  showSyncStatus: boolean;
  syncProgress: number | null;
  syncMessage: string | null;
}

export function useAppMachine() {
  const [machineState, setMachineState] = useState<AppState>(initialAppState);
  const [pendingOpsCount, setPendingOpsCount] = useState(0);
  
  // Refs for stable callbacks
  const retryTimeoutRef = useRef<number | null>(null);
  
  // Get context for state machine
  const getContext = useCallback(() => ({
    pendingOpsCount
  }), [pendingOpsCount]);
  
  // Dispatch function
  const dispatch = useCallback((event: AppEvent) => {
    const context = getContext();
    setMachineState(current => {
      const result = transition(current, event, context);
      // Execute effects after state update
      if (result.effects.length > 0) {
        setTimeout(() => executeEffects(result.effects), 0);
      }
      return result.state;
    });
  }, [getContext]);
  
  // Execute effects
  const executeEffects = useCallback(async (effects: AppEffect[]) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'INIT_CACHE':
          try {
            await cacheDB.init();
            // Check for pending operations
            const pendingOps = await cacheDB.getPendingOps();
            setPendingOpsCount(pendingOps.length);
            dispatch({ type: 'INIT_SUCCESS' });
          } catch (error) {
            dispatch({ 
              type: 'INIT_ERROR', 
              error: error instanceof Error ? error.message : 'Failed to initialize cache' 
            });
          }
          break;
        
        case 'START_SYNC':
          try {
            const pendingOps = await cacheDB.getPendingOps();
            dispatch({ type: 'SYNC_STARTED', total: pendingOps.length });
            
            let completed = 0;
            for (const op of pendingOps) {
              try {
                await syncQueue.processOperation(op);
                await cacheDB.deletePendingOp(op.id);
                completed++;
                dispatch({ type: 'SYNC_PROGRESS', progress: completed });
              } catch (error) {
                // Individual operation failed, but continue with others
                console.error('Failed to sync operation:', op, error);
              }
            }
            
            setPendingOpsCount(await cacheDB.getPendingOps().then(ops => ops.length));
            dispatch({ type: 'SYNC_COMPLETED' });
          } catch (error) {
            dispatch({ 
              type: 'SYNC_ERROR', 
              message: error instanceof Error ? error.message : 'Sync failed' 
            });
          }
          break;
        
        case 'SCHEDULE_RETRY':
          if (retryTimeoutRef.current !== null) {
            clearTimeout(retryTimeoutRef.current);
          }
          retryTimeoutRef.current = window.setTimeout(() => {
            retryTimeoutRef.current = null;
            dispatch({ type: 'RETRY_SYNC' });
          }, effect.delayMs);
          break;
        
        case 'CANCEL_RETRY':
          if (retryTimeoutRef.current !== null) {
            clearTimeout(retryTimeoutRef.current);
            retryTimeoutRef.current = null;
          }
          break;
        
        case 'NOTIFY_USER':
          // TODO: Integrate with a toast/notification system
          console.log(`[${effect.level}] ${effect.message}`);
          break;
      }
    }
  }, [dispatch]);
  
  // Initialize on mount
  useEffect(() => {
    executeEffects([{ type: 'INIT_CACHE' }]);
  }, []);
  
  // Listen for online/offline events
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
  
  // Listen for storage quota warnings
  useEffect(() => {
    const checkStorage = async () => {
      const { usage, quota } = await cacheDB.getStorageInfo();
      const usageMB = usage / (1024 * 1024);
      const quotaMB = quota / (1024 * 1024);
      
      if (usageMB > 100) {
        console.warn(`Storage usage high: ${usageMB.toFixed(1)}MB / ${quotaMB.toFixed(1)}MB`);
        // Emit custom event for toast notification
        window.dispatchEvent(new CustomEvent('storage-warning', {
          detail: { usageMB, quotaMB }
        }));
      }
    };
    
    // Check every 5 minutes
    const interval = setInterval(checkStorage, 5 * 60 * 1000);
    checkStorage(); // Check immediately
    
    return () => clearInterval(interval);
  }, []);
  
  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (retryTimeoutRef.current !== null) {
        clearTimeout(retryTimeoutRef.current);
      }
    };
  }, []);
  
  // Public API
  const queueOperation = useCallback(async (op: Omit<PendingOperation, 'id'>) => {
    const id = await cacheDB.addPendingOp(op);
    setPendingOpsCount(count => count + 1);
    dispatch({ type: 'OPERATION_QUEUED', op: { ...op, id } as any });
    return id;
  }, [dispatch]);
  
  // Computed values
  const info: AppInfo = {
    state: machineState,
    isOnline: machineState.type === 'ready' && machineState.network === 'online',
    isInitializing: machineState.type === 'initializing',
    isReady: machineState.type === 'ready',
    hasError: machineState.type === 'error',
    showSyncStatus: shouldShowSyncStatus(machineState),
    syncProgress: getSyncProgress(machineState),
    syncMessage: machineState.type === 'ready' && machineState.sync.type === 'error' 
      ? machineState.sync.message 
      : null
  };
  
  return {
    ...info,
    queueOperation,
    pendingOpsCount
  };
}
