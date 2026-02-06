import { useSettings } from '../hooks';

interface SettingsModalProps {
  onClose: () => void;
}

export function SettingsModal({ onClose }: SettingsModalProps) {
  const { settings, updateSettings } = useSettings();

  return (
    <div id="modal-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal">
        <h3>Settings</h3>
        
        <div className="settings-list">
          <label className="setting-item">
            <input
              type="checkbox"
              checked={settings.showLayoutOverlay}
              onChange={(e) => updateSettings({ showLayoutOverlay: e.target.checked })}
            />
            <span>Show Layout Debug Overlay</span>
          </label>
          <label className="setting-item">
            <input
              type="checkbox"
              checked={settings.showPerformanceDashboard}
              onChange={(e) => updateSettings({ showPerformanceDashboard: e.target.checked })}
            />
            <span>Show Performance Dashboard</span>
          </label>
        </div>
        
        <div className="modal-actions">
          <button className="btn-secondary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
