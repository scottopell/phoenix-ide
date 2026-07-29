import type { EffortCapabilities, ModelEffort, ModelsResponse, ModelInfo } from '../api';
import { DirectoryPicker } from './DirectoryPicker';

const EFFORT_LABELS: Record<ModelEffort, string> = {
  none: 'None',
  minimal: 'Minimal',
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  xhigh: 'X-High',
  max: 'Max',
};

function effortOptionLabel(level: ModelEffort): string {
  return EFFORT_LABELS[level];
}

function defaultEffortLabel(capabilities: EffortCapabilities | undefined): string {
  if (!capabilities || capabilities.support !== 'supported') return 'Model default';
  const nativeDefault = capabilities.native_default;
  if (nativeDefault && typeof nativeDefault === 'object' && 'known' in nativeDefault) {
    return `Model default (${effortOptionLabel(nativeDefault.known)})`;
  }
  return 'Model default';
}
export type DirStatus = 'checking' | 'exists' | 'will-create' | 'invalid';

export const DIR_STATUS_CONFIG = {
  checking: { icon: '...', class: 'status-checking', label: 'checking...' },
  exists: { icon: '\u2713', class: 'status-ok', label: 'exists' },
  'will-create': { icon: '+', class: 'status-create', label: 'will be created' },
  invalid: { icon: '\u2717', class: 'status-error', label: 'invalid path' },
} as const;

export function SettingsFields({
  cwd, setCwd, onDirStatusChange, onGitStatusChange,
  selectedModel, setSelectedModel, selectedEffort, setSelectedEffort, models,
  showAllModels, setShowAllModels
}: {
  cwd: string;
  setCwd: (v: string) => void;
  onDirStatusChange: (status: DirStatus) => void;
  onGitStatusChange?: (isGit: boolean | null) => void;
  selectedModel: string | null;
  setSelectedModel: (v: string) => void;
  selectedEffort: ModelEffort | null;
  setSelectedEffort: (v: ModelEffort | null) => void;
  models: ModelsResponse | null;
  showAllModels: boolean;
  setShowAllModels: (v: boolean) => void;
}) {
  // Filter and group models
  const filteredModels = models?.models.filter(m => showAllModels || m.recommended) || [];
  const totalCount = models?.models.length || 0;
  const recommendedCount = models?.models.filter(m => m.recommended).length || 0;
  const selectedModelInfo = models?.models.find((m) => m.id === selectedModel) ?? null;
  const effortCapabilities = selectedModelInfo?.effort_capabilities;

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
        <span className="settings-field-label">Directory</span>
        <DirectoryPicker
          value={cwd}
          onChange={setCwd}
          onStatusChange={onDirStatusChange}
          onGitStatusChange={onGitStatusChange}
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
              .toSorted(([a], [b]) => a.localeCompare(b))
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
      {effortCapabilities?.support === 'supported' && (
        <label className="settings-field">
          <span className="settings-field-label">Effort</span>
          <select
            className="settings-select"
            value={selectedEffort ?? ''}
            onChange={(e) => setSelectedEffort((e.target.value || null) as ModelEffort | null)}
            disabled={!models}
          >
            <option value="">{defaultEffortLabel(effortCapabilities)}</option>
            {effortCapabilities?.support === 'supported' && effortCapabilities.levels.map((level) => (
              <option key={level} value={level}>
                {effortOptionLabel(level)}
              </option>
            ))}
          </select>
        </label>
      )}
    </>
  );
}
