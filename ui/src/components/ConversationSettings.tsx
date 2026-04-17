import { useId, useRef, useState, useEffect, useCallback } from 'react';
import { LlmStatusBanner } from './LlmStatusBanner';
import { SettingsFields } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import type { GitBranchEntry, ModelsResponse } from '../api';

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
  mode?: 'direct' | 'managed' | 'branch';
  /** Callback to change mode */
  setMode?: (m: 'direct' | 'managed' | 'branch') => void;
  /** Available git branches for the current directory */
  branches?: GitBranchEntry[];
  /** Currently checked-out branch */
  currentBranch?: string | null;
  /** User-selected base branch (null means use current) */
  baseBranch?: string | null;
  /** Callback to change base branch selection */
  setBaseBranch?: (b: string | null) => void;
  /** Remote default branch name (e.g. "main") */
  defaultBranch?: string | null;
  /** Current search query for remote branch search */
  branchSearch?: string;
  /** Callback to update branch search query */
  setBranchSearch?: (q: string) => void;
  /** Whether a remote branch search is in progress */
  branchSearchLoading?: boolean;
}

function branchLabel(b: GitBranchEntry, currentBranch?: string | null): string {
  let label = b.name;
  if (b.name === currentBranch) label += ' (current)';
  if (b.behind_remote && b.behind_remote > 0) label += ` \u2022 ${b.behind_remote} behind`;
  return label;
}

function branchTag(b: GitBranchEntry): { text: string; className: string } | null {
  if (b.local && !b.remote) return { text: 'local only', className: 'branch-tag branch-tag--local' };
  return null;
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
  defaultBranch,
  branchSearch = '',
  setBranchSearch,
  branchSearchLoading,
}: ConversationSettingsProps) {
  const radioGroupName = useId();
  const [comboOpen, setComboOpen] = useState(false);
  const comboRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Close dropdown on outside click.
  useEffect(() => {
    if (!comboOpen) return;
    const handler = (e: MouseEvent) => {
      if (comboRef.current && !comboRef.current.contains(e.target as Node)) {
        setComboOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [comboOpen]);

  const selectBranch = useCallback((name: string) => {
    setBaseBranch?.(name === currentBranch ? null : name);
    setBranchSearch?.('');
    setComboOpen(false);
  }, [currentBranch, setBaseBranch, setBranchSearch]);

  const selectedName = baseBranch ?? currentBranch ?? '';

  // Build display list: current branch first, then the rest in order received
  // (already sorted by recency from backend for local, or relevance for search).
  const displayBranches = branches ?? [];

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
                  Explore first, then propose a plan. Works on a new task branch.
                </span>
              </span>
            </label>
          )}
          {isGitDir && (
            <label
              className={`mode-option ${mode === 'branch' ? 'mode-option--active' : ''}`}
              onClick={() => setMode?.('branch')}
            >
              <input
                type="radio"
                name={radioGroupName}
                checked={mode === 'branch'}
                onChange={() => setMode?.('branch')}
              />
              <span className="mode-option-content">
                <strong>Branch <span className="beta-badge">BETA</span></strong>
                <span className="mode-option-desc">
                  Work directly on an existing branch. For PR fixes and iteration.
                </span>
              </span>
            </label>
          )}
        </div>
      )}

      {isGitDir && (mode === 'managed' || mode === 'branch') && (
        <div className="settings-field branch-selector" ref={comboRef}>
          <span className="settings-field-label">{mode === 'branch' ? 'Branch' : 'Base branch'}</span>
          <div className="branch-combobox">
            <input
              ref={inputRef}
              type="text"
              className="settings-input branch-combobox-input"
              placeholder={comboOpen ? 'Search branches...' : undefined}
              value={comboOpen ? branchSearch : selectedName}
              readOnly={!comboOpen}
              onFocus={() => setComboOpen(true)}
              onChange={(e) => setBranchSearch?.(e.target.value)}
            />
            {!comboOpen && (() => {
              const entry = displayBranches.find(b => b.name === selectedName);
              return entry?.behind_remote && entry.behind_remote > 0
                ? <span className="branch-combobox-badge">{entry.behind_remote} behind</span>
                : null;
            })()}
            {branchSearchLoading && <span className="branch-combobox-loading">...</span>}
            {comboOpen && (
              <div className="branch-combobox-dropdown">
                <div className="branch-combobox-hint">Fetches latest from origin when task starts</div>
                {defaultBranch && !branchSearch && (
                  <div
                    className={`branch-combobox-item branch-combobox-item--default ${selectedName === defaultBranch ? 'branch-combobox-item--selected' : ''}`}
                    onClick={() => selectBranch(defaultBranch)}
                  >
                    {defaultBranch} <span className="branch-tag">default</span>
                  </div>
                )}
                {displayBranches
                  .filter(b => branchSearch || b.name !== defaultBranch)
                  .map(b => {
                    const tag = branchTag(b);
                    return (
                      <div
                        key={b.name}
                        className={`branch-combobox-item ${selectedName === b.name ? 'branch-combobox-item--selected' : ''}`}
                        onClick={() => selectBranch(b.name)}
                      >
                        <span className="branch-combobox-item-name">{branchLabel(b, currentBranch)}</span>
                        {tag && <span className={tag.className}>{tag.text}</span>}
                      </div>
                    );
                  })}
                {displayBranches.length === 0 && branchSearch && !branchSearchLoading && (
                  <div className="branch-combobox-empty">No matching branches</div>
                )}
              </div>
            )}
          </div>
        </div>
      )}

      {(() => {
        const selected = displayBranches.find(b => b.name === selectedName);
        if (!selected?.conflict_slug || mode === 'direct') return null;
        return (
          <div className="branch-conflict-banner">
            This branch already has an active conversation.{' '}
            <a href={`/c/${selected.conflict_slug}`}>Continue there</a>{' '}
            or abandon it first.
          </div>
        );
      })()}
    </>
  );
}
