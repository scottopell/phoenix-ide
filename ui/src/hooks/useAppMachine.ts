import { useState, useEffect, useCallback, useRef } from 'react';
import { cacheDB, PendingOperation } from '../cache';
import { syncQueue } from '../syncQueue';

/**
 * Simplified app state management.
 * - Initializes IndexedDB on mount
 * - Tracks online/offline status
 * - Handles pending operation sync when online
 */
export function useAppMachine() {
  const [isReady, setIsReady] = useState(false);
  const [initError, setInitError] = useState<string | null>(null);
  const [isOnline, setIsOnline] = useState(navigator.onLine);
  const [pendingOpsCount, setPendingOpsCount] = useState(0);
  const [isSyncing, setIsSyncing] = useState(false);
  
  const syncingRef = useRef(false);

  // Initialize IndexedDB on mount
  useEffect(() => {
    const init = async () => {
      try {
        await cacheDB.init();
        const pendingOps = await cacheDB.getPendingOps();
        setPendingOpsCount(pendingOps.length);
        setIsReady(true);
      } catch (error) {
        console.error('Failed to initialize IndexedDB:', error);
        setInitError(error instanceof Error ? error.message : 'Failed to initialize local storage');
      }
    };
    init();
  }, []);

  // Track online/offline status
  useEffect(() => {
    const handleOnline = () => setIsOnline(true);
    const handleOffline = () => setIsOnline(false);
    
    window.addEventListener('online', handleOnline);
    window.addEventListener('offline', handleOffline);
    
    return () => {
      window.removeEventListener('online', handleOnline);
      window.removeEventListener('offline', handleOffline);
    };
  }, []);

  // Sync pending operations when online
  useEffect(() => {
    if (!isReady || !isOnline || pendingOpsCount === 0 || syncingRef.current) return;

    const syncPendingOps = async () => {
      if (syncingRef.current) return;
      syncingRef.current = true;
      setIsSyncing(true);

      try {
        const pendingOps = await cacheDB.getPendingOps();
        for (const op of pendingOps) {
          try {
            await syncQueue.processOperation(op);
            await cacheDB.deletePendingOp(op.id);
            setPendingOpsCount(count => Math.max(0, count - 1));
          } catch (error) {
            console.error('Failed to sync operation:', op, error);
            // Continue with next operation
          }
        }
      } catch (error) {
        console.error('Sync failed:', error);
      } finally {
        syncingRef.current = false;
        setIsSyncing(false);
      }
    };

    syncPendingOps();
  }, [isReady, isOnline, pendingOpsCount]);

  // Queue an operation for offline sync
  const queueOperation = useCallback(async (op: Omit<PendingOperation, 'id'>) => {
    const id = await cacheDB.addPendingOp(op);
    setPendingOpsCount(count => count + 1);
    return id;
  }, []);

  return {
    isReady,
    initError,
    isOnline,
    pendingOpsCount,
    isSyncing,
    queueOperation
  };
}
