import { useId } from 'react';
import { LlmStatusBanner } from './LlmStatusBanner';
import { SettingsFields } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import type { ModelsResponse } from '../api';

interface ConversationSettingsProps {
  cwd: string;
  setCwd: (v: string) => void;
  dirStatus: DirStatus;
  onDirStatusChange: (status: DirStatus) => void;
  onGitStatusChange?: (isGit: boolean) => void;
  selectedModel: string | null;
  setSelectedModel: (v: string) => void;
  models: ModelsResponse | null;
  showAllModels: boolean;
  setShowAllModels: (v: boolean) => void;
  /** Recent project directories for quick selection */
  recentDirs?: string[];
  /** Is the selected directory a git repo? (for mode preview) */
  isGitDir?: boolean | null;
  /** Error message to display */
  error?: string | null;
  /** Selected conversation mode */
  mode?: 'direct' | 'managed';
  /** Callback to change mode */
  setMode?: (m: 'direct' | 'managed') => void;
  /** Available git branches for the current directory */
  branches?: string[];
  /** Currently checked-out branch */
  currentBranch?: string | null;
  /** User-selected base branch (null means use current) */
  baseBranch?: string | null;
  /** Callback to change base branch selection */
  setBaseBranch?: (b: string | null) => void;
}

export function ConversationSettings({
  cwd,
  setCwd,
  dirStatus,
  onDirStatusChange,
  onGitStatusChange,
  selectedModel,
  setSelectedModel,
  models,
  showAllModels,
  setShowAllModels,
  recentDirs,
  isGitDir,
  error,
  mode = 'direct',
  setMode,
  branches,
  currentBranch,
  baseBranch,
  setBaseBranch,
}: ConversationSettingsProps) {
  const radioGroupName = useId();
  return (
    <>
      <LlmStatusBanner models={models} />
      {error && <div className="new-conv-error">{error}</div>}

      {recentDirs && recentDirs.length > 0 && (
        <div className="new-conv-recent">
          {recentDirs.map(dir => {
            const label = dir.split('/').filter(Boolean).pop() || dir;
            const isSelected = cwd.trim() === dir;
            return (
              <button
                key={dir}
                className={`new-conv-recent-chip ${isSelected ? 'active' : ''}`}
                onClick={() => setCwd(dir)}
                title={dir}
              >
                {label}
              </button>
            );
          })}
        </div>
      )}

      <SettingsFields
        cwd={cwd}
        setCwd={setCwd}
        dirStatus={dirStatus}
        onDirStatusChange={onDirStatusChange}
        {...(onGitStatusChange ? { onGitStatusChange } : {})}
        selectedModel={selectedModel}
        setSelectedModel={setSelectedModel}
        models={models}
        showAllModels={showAllModels}
        setShowAllModels={setShowAllModels}
      />

      {dirStatus === 'exists' && isGitDir !== null && isGitDir !== undefined && (
        <div className="new-conv-mode-selector">
          <label
            className={`mode-option ${mode === 'direct' ? 'mode-option--active' : ''}`}
            onClick={() => setMode?.('direct')}
          >
            <input
              type="radio"
              name={radioGroupName}
              checked={mode === 'direct'}
              onChange={() => setMode?.('direct')}
            />
            <span className="mode-option-content">
              <strong>Direct</strong>
              <span className="mode-option-desc">
                Full tool access. Changes happen on your current branch.
              </span>
            </span>
          </label>
          {isGitDir && (
            <label
              className={`mode-option ${mode === 'managed' ? 'mode-option--active' : ''}`}
              onClick={() => setMode?.('managed')}
            >
              <input
                type="radio"
                name={radioGroupName}
                checked={mode === 'managed'}
                onChange={() => setMode?.('managed')}
              />
              <span className="mode-option-content">
                <strong>Managed <span className="beta-badge">BETA</span></strong>
                <span className="mode-option-desc">
                  Read-only exploration first. Proposes a task plan for your approval, then works on an isolated worktree.
                </span>
              </span>
            </label>
          )}
        </div>
      )}

      {isGitDir && mode === 'managed' && branches && branches.length > 0 && (
        <label className="settings-field branch-selector">
          <span className="settings-field-label">Branch</span>
          <select
            className="settings-select"
            value={baseBranch ?? currentBranch ?? ''}
            onChange={(e) => {
              const val = e.target.value;
              setBaseBranch?.(val === currentBranch ? null : val);
            }}
          >
            {currentBranch && (
              <option value={currentBranch}>
                {currentBranch} (current)
              </option>
            )}
            {branches
              .filter(b => b !== currentBranch)
              .toSorted((a, b) => a.localeCompare(b))
              .map(b => (
                <option key={b} value={b}>{b}</option>
              ))}
          </select>
        </label>
      )}
    </>
  );
}
