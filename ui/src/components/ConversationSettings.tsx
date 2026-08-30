import { LlmStatusBanner } from './LlmStatusBanner';
import { SettingsFields } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import type { ModelEffort, ModelsResponse } from '../api';

interface ConversationSettingsProps {
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
    </>
  );
}
