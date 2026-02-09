// StorageStatus.tsx - Simple footer showing basic stats
import { useState, useEffect } from 'react';
import { cacheDB } from '../cache';
import './StorageStatus.css';

export function StorageStatus({ conversationCount }: { conversationCount: number }) {
  const [cachedMB, setCachedMB] = useState<number | null>(null);

  useEffect(() => {
    const checkStorage = async () => {
      try {
        const { usage } = await cacheDB.getStorageInfo();
        setCachedMB(usage / (1024 * 1024));
      } catch (err) {
        // Storage API not available
      }
    };
    checkStorage();
  }, []);

  return (
    <footer className="storage-footer">
      <span>{conversationCount} conversations</span>
      {cachedMB !== null && (
        <>
          <span className="storage-footer-sep">·</span>
          <span>{cachedMB.toFixed(1)}MB cached</span>
        </>
      )}
    </footer>
  );
}
