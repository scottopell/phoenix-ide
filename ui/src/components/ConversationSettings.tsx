import { useId, useRef, useState, useEffect, useCallback } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { LlmStatusBanner } from './LlmStatusBanner';
import { SettingsFields } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import type { GitBranchEntry, ModelsResponse, TaskEntry } from '../api';
import type { ConversationIntent, StartingPoint } from '../hooks/useCreateConversation';

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
  /** Selected user intent */
  intent?: ConversationIntent;
  /** Callback to change user intent */
  setIntent?: (m: ConversationIntent) => void;
  startingPoint?: StartingPoint | null;
  setStartingPoint?: (p: StartingPoint | null) => void;
  tasks?: TaskEntry[];
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

const REMARK_PLUGINS = [remarkGfm];

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
  intent = 'direct',
  setIntent,
  startingPoint,
  setStartingPoint,
  tasks = [],
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
  const [taskPickerOpen, setTaskPickerOpen] = useState(false);
  const [branchPickerOpen, setBranchPickerOpen] = useState(false);
  const [taskDetail, setTaskDetail] = useState<{ path: string; content: string } | null>(null);
  const [taskDetailLoading, setTaskDetailLoading] = useState(false);
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
    setStartingPoint?.({ kind: 'branch', name });
    setBranchSearch?.('');
    setComboOpen(false);
  }, [currentBranch, setBaseBranch, setBranchSearch, setStartingPoint]);

  const selectCheckoutBranch = useCallback((name: string) => {
    setStartingPoint?.({ kind: 'checkoutBranch', name });
    setBranchSearch?.('');
    setComboOpen(false);
  }, [setBranchSearch, setStartingPoint]);

  const selectTask = useCallback((task: TaskEntry) => {
    setStartingPoint?.({ kind: 'task', task });
    setBranchSearch?.('');
    setComboOpen(false);
  }, [setBranchSearch, setStartingPoint]);

  const selectedName = startingPoint?.kind === 'branch' || startingPoint?.kind === 'checkoutBranch'
    ? startingPoint.name
    : (baseBranch ?? currentBranch ?? defaultBranch ?? '');
  const selectedTask = startingPoint?.kind === 'task' ? startingPoint.task : null;

  useEffect(() => {
    if (!selectedTask) {
      setTaskDetail(null);
      setTaskDetailLoading(false);
      return;
    }
    let cancelled = false;
    setTaskDetailLoading(true);
    fetch(`/api/files/read?path=${encodeURIComponent(selectedTask.path)}`)
      .then(async resp => {
        if (!resp.ok) throw new Error('Failed to read task');
        return resp.json() as Promise<{ content: string }>;
      })
      .then(data => {
        if (!cancelled) setTaskDetail({ path: selectedTask.path, content: data.content });
      })
      .catch(() => {
        if (!cancelled) setTaskDetail({ path: selectedTask.path, content: 'Could not load task details.' });
      })
      .finally(() => {
        if (!cancelled) setTaskDetailLoading(false);
      });
    return () => { cancelled = true; };
  }, [selectedTask]);
  const activeTasks = tasks.filter(t => !['done', 'wont-do'].includes(t.status));
  const importantTasks = activeTasks
    .toSorted((a, b) => {
      const priorityRank = (p: string) => Number(p.replace(/^p/, '')) || 9;
      return priorityRank(a.priority) - priorityRank(b.priority) || a.id.localeCompare(b.id);
    })
    .slice(0, 8);

  // Build display list: current branch first, then the rest in order received
  // (already sorted by recency from backend for local, or relevance for search).
  const displayBranches = branches ?? [];

  // When no LLM is configured, the only useful action is signing in. Hide
  // every downstream field (model picker, directory, mode, branches) so the
  // user isn't presented with a half-functional form full of empty selects
  // and disabled controls. The banner itself owns the sign-in CTA.
  if (models && !models.llm_configured) {
    return <LlmStatusBanner models={models} />;
  }

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
        <div className="new-conv-workflows">
          <label
            className={`workflow-card ${intent === 'direct' ? 'workflow-card--active' : ''}`}
            onClick={() => setIntent?.('direct')}
          >
            <input
              type="radio"
              name={radioGroupName}
              checked={intent === 'direct'}
              onChange={() => setIntent?.('direct')}
            />
            <span className="workflow-card-content">
              <strong>Direct</strong>
              <span>Familiar chat mode. The agent works directly in this folder.</span>
            </span>
          </label>
          {isGitDir && (
            <label
              className={`workflow-card ${intent === 'fromExistingWork' ? 'workflow-card--active' : ''}`}
              onClick={() => setIntent?.('fromExistingWork')}
            >
              <input
                type="radio"
                name={radioGroupName}
                checked={intent === 'fromExistingWork'}
                onChange={() => setIntent?.('fromExistingWork')}
              />
              <span className="workflow-card-content">
              <strong>Worktree-based</strong>
              <span>Use a separate git worktree. Default: start from latest {defaultBranch ?? 'default branch'}.</span>

              </span>
            </label>
          )}
        </div>
      )}

      {isGitDir && intent === 'fromExistingWork' && (
        <div className="git-workflow-panel" ref={comboRef}>
          <button
            type="button"
            className={`git-workflow-option ${(!startingPoint || startingPoint.kind === 'branch') ? 'git-workflow-option--active' : ''}`}
            onClick={() => {
              selectBranch(defaultBranch ?? currentBranch ?? selectedName);
              setTaskPickerOpen(false);
              setBranchPickerOpen(false);
            }}
          >
            <span className="git-workflow-title">Start fresh from default branch</span>
            <span className="git-workflow-desc">New worktree from latest {(defaultBranch ?? selectedName) || 'default branch'}.</span>
          </button>

          <button
            type="button"
            className={`git-workflow-option ${selectedTask ? 'git-workflow-option--active' : ''}`}
            onClick={() => {
              setTaskPickerOpen(open => !open);
              setBranchPickerOpen(false);
            }}
          >
            <span className="git-workflow-title">Pick a task</span>
            <span className="git-workflow-desc">
              {selectedTask ? `${selectedTask.priority} ${selectedTask.id}: ${selectedTask.slug}` : `${importantTasks.length} active tasks`}
            </span>
          </button>
          {taskPickerOpen && (
            <div className="task-start-list">
              {importantTasks.length === 0 && <div className="task-start-empty">No active tasks found.</div>}
              {importantTasks.map(t => (
                <button
                  key={t.path}
                  type="button"
                  className={`task-start-item ${selectedTask?.path === t.path ? 'task-start-item--active' : ''}`}
                  onClick={() => selectTask(t)}
                >
                  <span className={`task-start-priority task-start-priority--${t.priority}`}>{t.priority}</span>
                  <span className="task-start-main">
                    <span className="task-start-title">{t.id} · {t.slug}</span>
                    <span className="task-start-meta">{t.status}{t.conversation_slug ? ' · active conversation' : ''}</span>
                  </span>
                </button>
              ))}
            </div>
          )}
          {selectedTask && (
            <div className="task-start-detail">
              <div className="task-start-detail-title">
                <span>{selectedTask.id} · {selectedTask.slug}</span>
                <span className="task-start-detail-meta">{selectedTask.priority} · {selectedTask.status}</span>
              </div>
              <div className="task-start-detail-markdown">
                {taskDetailLoading
                  ? 'Loading task details...'
                  : <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>{taskDetail?.content ?? ''}</ReactMarkdown>}
              </div>
            </div>
          )}

          <button
            type="button"
            className={`git-workflow-option ${startingPoint?.kind === 'checkoutBranch' ? 'git-workflow-option--active' : ''}`}
            onClick={() => {
              const branch = startingPoint?.kind === 'checkoutBranch'
                ? startingPoint.name
                : (currentBranch ?? defaultBranch ?? selectedName);
              if (branch) selectCheckoutBranch(branch);
              setBranchPickerOpen(open => !open);
              setTaskPickerOpen(false);
            }}
          >
            <span className="git-workflow-title">Work in branch</span>
            <span className="git-workflow-desc">New worktree with an existing branch checked out.</span>
          </button>
          {branchPickerOpen && (
            <div className="branch-combobox">
              <span className="settings-field-label">Branch to work in</span>
              <input
                ref={inputRef}
                type="text"
                className="settings-input branch-combobox-input"
                placeholder={comboOpen ? 'Type to filter branches...' : undefined}
                value={comboOpen ? branchSearch : (startingPoint?.kind === 'checkoutBranch' ? selectedName : '')}
                readOnly={!comboOpen}
                onFocus={() => setComboOpen(true)}
                onChange={(e) => setBranchSearch?.(e.target.value)}
              />
              {branchSearchLoading && <span className="branch-combobox-loading">...</span>}
              {comboOpen && (
                <div className="branch-combobox-dropdown">
                  {displayBranches.map(b => {
                    const tag = branchTag(b);
                    return (
                      <div
                        key={`checkout:${b.name}`}
                        className={`branch-combobox-item ${startingPoint?.kind === 'checkoutBranch' && selectedName === b.name ? 'branch-combobox-item--selected' : ''}`}
                        onClick={() => selectCheckoutBranch(b.name)}
                      >
                        <span className="branch-combobox-item-name">{branchLabel(b, currentBranch)}</span>
                        {b.conflict_slug && <span className="branch-tag branch-tag--conflict">active</span>}
                        {tag && <span className={tag.className}>{tag.text}</span>}
                      </div>
                    );
                  })}
                  {displayBranches.length === 0 && !branchSearchLoading && (
                    <div className="branch-combobox-empty">No branches found</div>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {(() => {
        const selectedBranch = displayBranches.find(b => b.name === selectedName);
        const conflictSlug = selectedTask?.conversation_slug ?? selectedBranch?.conflict_slug;
        if (!conflictSlug || intent === 'direct') return null;
        return (
          <div className="branch-conflict-banner">
            This starting point already has an active conversation.{' '}
            <a href={`/c/${conflictSlug}`}>Continue there</a>{' '}
            or abandon it first.
          </div>
        );
      })()}
    </>
  );
}
