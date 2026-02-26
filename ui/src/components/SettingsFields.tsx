import type { ModelsResponse, ModelInfo } from '../api';
import { DirectoryPicker } from './DirectoryPicker';

export type DirStatus = 'checking' | 'exists' | 'will-create' | 'invalid';

export const DIR_STATUS_CONFIG = {
  checking: { icon: '...', class: 'status-checking', label: 'checking...' },
  exists: { icon: '\u2713', class: 'status-ok', label: 'exists' },
  'will-create': { icon: '+', class: 'status-create', label: 'will be created' },
  invalid: { icon: '\u2717', class: 'status-error', label: 'invalid path' },
} as const;

export function SettingsFields({
  cwd, setCwd, dirStatus, onDirStatusChange,
  selectedModel, setSelectedModel, models,
  showAllModels, setShowAllModels
}: {
  cwd: string;
  setCwd: (v: string) => void;
  dirStatus: DirStatus;
  onDirStatusChange: (status: DirStatus) => void;
  selectedModel: string | null;
  setSelectedModel: (v: string) => void;
  models: ModelsResponse | null;
  showAllModels: boolean;
  setShowAllModels: (v: boolean) => void;
}) {
  const dirStatusClass = DIR_STATUS_CONFIG[dirStatus].class;

  // Filter and group models
  const filteredModels = models?.models.filter(m => showAllModels || m.recommended) || [];
  const totalCount = models?.models.length || 0;
  const recommendedCount = models?.models.filter(m => m.recommended).length || 0;

  // Group by provider when showing all
  const groupedModels: Record<string, ModelInfo[]> = {};
  if (showAllModels) {
    filteredModels.forEach(m => {
      const providerGroup = groupedModels[m.provider];
      if (!providerGroup) {
        groupedModels[m.provider] = [m];
      } else {
        providerGroup.push(m);
      }
    });
  }

  return (
    <>
      <label className="settings-field">
        <span className="settings-field-label">
          Directory
          <span className={`field-status ${dirStatusClass}`}>
            {DIR_STATUS_CONFIG[dirStatus].label}
          </span>
        </span>
        <DirectoryPicker
          value={cwd}
          onChange={setCwd}
          onStatusChange={onDirStatusChange}
          className="settings-input"
        />
      </label>
      <label className="settings-field">
        <span className="settings-field-label">Model</span>
        <select
          className="settings-select"
          value={selectedModel || ''}
          onChange={(e) => setSelectedModel(e.target.value)}
          disabled={!models}
        >
          {!showAllModels ? (
            // Show only recommended models (ungrouped)
            filteredModels.map(m => (
              <option key={m.id} value={m.id}>
                {m.id}
              </option>
            ))
          ) : (
            // Show all models grouped by provider
            Object.entries(groupedModels)
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([provider, providerModels]) => (
                <optgroup key={provider} label={provider}>
                  {providerModels.map(m => (
                    <option key={m.id} value={m.id}>
                      {m.recommended ? '* ' : ''}{m.id}
                    </option>
                  ))}
                </optgroup>
              ))
          )}
        </select>
        <label className="model-filter-toggle">
          <input
            type="checkbox"
            checked={showAllModels}
            onChange={(e) => setShowAllModels(e.target.checked)}
          />
          <span>
            Show all models ({totalCount})
            {!showAllModels && ` · ${recommendedCount} recommended`}
          </span>
        </label>
      </label>
    </>
  );
}
