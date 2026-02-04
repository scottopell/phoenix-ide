// StorageStatus.tsx
import { useState, useEffect } from 'react';
import { cacheDB } from '../cache';
import './StorageStatus.css';

interface StorageInfo {
  usageMB: number;
  quotaMB: number;
  percentage: number;
}

export function StorageStatus() {
  const [storageInfo, setStorageInfo] = useState<StorageInfo | null>(null);
  const [isClearing, setIsClearing] = useState(false);
  const [showDetails, setShowDetails] = useState(false);

  const checkStorage = async () => {
    try {
      const { usage, quota } = await cacheDB.getStorageInfo();
      const usageMB = usage / (1024 * 1024);
      const quotaMB = quota / (1024 * 1024);
      const percentage = quota > 0 ? (usage / quota) * 100 : 0;
      setStorageInfo({ usageMB, quotaMB, percentage });
    } catch (err) {
      console.error('Failed to get storage info:', err);
    }
  };

  useEffect(() => {
    checkStorage();
    // Check every minute
    const interval = setInterval(checkStorage, 60000);
    return () => clearInterval(interval);
  }, []);

  const handleClearOldData = async () => {
    setIsClearing(true);
    try {
      const purged = await cacheDB.purgeOldConversations(7); // 7 days
      alert(`Cleared ${purged} old conversations`);
      await checkStorage();
    } catch (err) {
      console.error('Failed to clear old data:', err);
      alert('Failed to clear old data');
    } finally {
      setIsClearing(false);
    }
  };

  if (!storageInfo) return null;

  const getStatusColor = () => {
    if (storageInfo.usageMB > 100) return 'red';
    if (storageInfo.usageMB > 75) return 'orange';
    return 'green';
  };

  return (
    <div className="storage-status">
      <button 
        className="storage-status-button"
        onClick={() => setShowDetails(!showDetails)}
        title="Storage usage"
      >
        <span className={`storage-indicator storage-indicator-${getStatusColor()}`}>💾</span>
        {storageInfo.usageMB.toFixed(1)}MB
      </button>

      {showDetails && (
        <div className="storage-details">
          <h3>Storage Usage</h3>
          <div className="storage-bar">
            <div 
              className="storage-bar-fill"
              style={{ 
                width: `${Math.min(storageInfo.percentage, 100)}%`,
                backgroundColor: getStatusColor() === 'red' ? '#ef4444' : getStatusColor() === 'orange' ? '#f59e0b' : '#10b981'
              }}
            />
          </div>
          <p className="storage-text">
            {storageInfo.usageMB.toFixed(1)}MB / {storageInfo.quotaMB.toFixed(0)}MB ({storageInfo.percentage.toFixed(1)}%)
          </p>
          {storageInfo.usageMB > 75 && (
            <div className="storage-warning">
              ⚠️ Storage usage is high. Consider clearing old conversations.
            </div>
          )}
          <button 
            className="btn btn-secondary"
            onClick={handleClearOldData}
            disabled={isClearing}
          >
            {isClearing ? 'Clearing...' : 'Clear Old Data (>7 days)'}
          </button>
        </div>
      )}
    </div>
  );
}