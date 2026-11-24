import { LlmStatusBanner } from './LlmStatusBanner';
import { SettingsFields } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import type { LlmLanguageCatalogEntry, ModelEffort, ModelsResponse } from '../api';

interface ConversationSettingsProps {
  languages: LlmLanguageCatalogEntry[];
  selectedLanguage: string;
  setSelectedLanguage: (language: string) => void;
  languageError: string | null;
  languageDisabled: boolean;
  cwd: string;
  setCwd: (v: string) => void;
  dirStatus: DirStatus;
  onDirStatusChange: (status: DirStatus) => void;
  onGitStatusChange?: (isGit: boolean | null) => void;
  selectedModel: string | null;
  setSelectedModel: (v: string) => void;
  selectedEffort: ModelEffort | null;
  setSelectedEffort: (v: ModelEffort | null) => void;
  models: ModelsResponse | null;
  showAllModels: boolean;
  setShowAllModels: (v: boolean) => void;
  error?: string | null;
  recentPaths?: readonly string[];
  rootFreshness?: 'fresh' | 'stale_cached' | 'unresolved' | null;
}

export function ConversationSettings({
  languages, selectedLanguage, setSelectedLanguage, languageError, languageDisabled,
  cwd,
  setCwd,
  onDirStatusChange,
  onGitStatusChange,
  selectedModel,
  setSelectedModel,
  selectedEffort,
  setSelectedEffort,
  models,
  showAllModels,
  setShowAllModels,
  error,
  recentPaths = [],
  rootFreshness = null,
}: ConversationSettingsProps) {
  if (models && !models.llm_configured) {
    return <LlmStatusBanner models={models} />;
  }

  return (
    <>
      <LlmStatusBanner models={models} />
      {error && <div className="new-conv-error">{error}</div>}
      {rootFreshness === 'stale_cached' && (
        <div className="new-conversation-status">Using cached default branch while the remote is unavailable.</div>
      )}
      <SettingsFields
        cwd={cwd}
        setCwd={setCwd}
        onDirStatusChange={onDirStatusChange}
        {...(onGitStatusChange ? { onGitStatusChange } : {})}
        selectedModel={selectedModel}
        setSelectedModel={setSelectedModel}
        selectedEffort={selectedEffort}
        setSelectedEffort={setSelectedEffort}
        models={models}
        showAllModels={showAllModels}
        setShowAllModels={setShowAllModels}
        recentPaths={recentPaths}
      />
      <label className="settings-field">
        <span className="settings-field-label">Language</span>
        <select className="settings-select" aria-label="Language" value={selectedLanguage} onChange={(event) => setSelectedLanguage(event.target.value)}
          disabled={languageDisabled || languages.length === 0}>
          {!languages.some((language) => language.id === selectedLanguage) && (
            <option value={selectedLanguage}>{selectedLanguage === 'phoenix-native' ? 'Phoenix' : selectedLanguage}</option>
          )}
          {languages.map((language) => (
            <option key={language.id} value={language.id}>{language.label}</option>
          ))}
        </select>
        <span className="settings-section__hint">
          {languages.find((language) => language.id === selectedLanguage)?.description}
        </span>
      </label>
      {languageError && <div className="settings-section__error">{languageError}</div>}
    </>
  );
}
