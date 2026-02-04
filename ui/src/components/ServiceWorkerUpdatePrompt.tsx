// ServiceWorkerUpdatePrompt.tsx
import { useEffect, useState } from 'react';
import './ServiceWorkerUpdatePrompt.css';

export function ServiceWorkerUpdatePrompt() {
  const [showUpdate, setShowUpdate] = useState(false);
  const [worker, setWorker] = useState<ServiceWorker | null>(null);

  useEffect(() => {
    const handleUpdate = (event: Event) => {
      const customEvent = event as CustomEvent;
      setWorker(customEvent.detail.worker);
      setShowUpdate(true);
    };

    window.addEventListener('sw-update-available', handleUpdate);
    return () => window.removeEventListener('sw-update-available', handleUpdate);
  }, []);

  const handleUpdate = () => {
    if (worker) {
      // Tell SW to skip waiting and activate
      worker.postMessage({ type: 'SKIP_WAITING' });
    }
    setShowUpdate(false);
  };

  const handleDismiss = () => {
    setShowUpdate(false);
  };

  if (!showUpdate) return null;

  return (
    <div className="sw-update-prompt">
      <div className="sw-update-content">
        <span className="sw-update-icon">🆕</span>
        <span className="sw-update-text">A new version is available!</span>
        <button className="sw-update-btn sw-update-btn-primary" onClick={handleUpdate}>
          Update
        </button>
        <button className="sw-update-btn sw-update-btn-secondary" onClick={handleDismiss}>
          Later
        </button>
      </div>
    </div>
  );
}