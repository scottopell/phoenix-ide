import { useId, useRef, useState, useEffect, useCallback } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { LlmStatusBanner } from './LlmStatusBanner';
import { SettingsFields } from './SettingsFields';
import type { DirStatus } from './SettingsFields';
import type { GitBranchEntry, ModelsResponse, TaskEntry } from '../api';
import type { NewConversationWorkflow } from '../hooks/useCreateConversation';

interface ConversationSettingsProps {
  cwd: string;
  setCwd: (v: string) => void;
  dirStatus: DirStatus;
  onDirStatusChange: (status: DirStatus) => void;
  onGitStatusChange?: (isGit: boolean | null) => void;
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
  workflow?: NewConversationWorkflow;
  setWorkflow?: (workflow: NewConversationWorkflow) => void;
  tasks?: TaskEntry[];
  taskAvailabilityLoading?: boolean;
  taskAvailable?: boolean | null;
  tasksLoading?: boolean;
  tasksLoaded?: boolean;
  loadProjectTasks?: () => void;
  /** Available git branches for the current directory */
  branches?: GitBranchEntry[];
  /** Currently checked-out branch */
  currentBranch?: string | null;
  gitMetadataLoading?: boolean;
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
  workflow = { kind: 'direct' },
  setWorkflow,
  tasks = [],
  taskAvailabilityLoading = false,
  taskAvailable = null,
  tasksLoading = false,
  tasksLoaded = false,
  loadProjectTasks,
  branches,
  currentBranch,
  gitMetadataLoading,
  branchSearch = '',
  setBranchSearch,
  branchSearchLoading,
}: ConversationSettingsProps) {
  const radioGroupName = useId();
  const [comboOpen, setComboOpen] = useState(false);
  const [taskPickerOpen, setTaskPickerOpen] = useState(false);
  const [taskDetail, setTaskDetail] = useState<{ path: string; content: string } | null>(null);
  const [taskDetailLoading, setTaskDetailLoading] = useState(false);
  const [taskPage, setTaskPage] = useState(0);
  const [taskSearch, setTaskSearch] = useState('');
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

  const selectedName = workflow.kind === 'planFromBranch'
    ? (workflow.baseBranch ?? '')
    : workflow.kind === 'planFromTask'
      ? (workflow.baseBranch ?? '')
      : workflow.kind === 'continueBranch'
        ? (workflow.branch ?? '')
        : '';
  const selectedTask = workflow.kind === 'planFromTask' ? workflow.task : null;

  const selectContinueBranch = useCallback((name: string) => {
    setWorkflow?.({ kind: 'continueBranch', branch: name });
    setBranchSearch?.('');
    setComboOpen(false);
  }, [setBranchSearch, setWorkflow]);

  const selectTask = useCallback((task: TaskEntry) => {
    setWorkflow?.({ kind: 'planFromTask', task, baseBranch: task.source_ref ?? null });
    setBranchSearch?.('');
    setComboOpen(false);
  }, [setBranchSearch, setWorkflow]);

  const chooseWorkflow = useCallback((next: NewConversationWorkflow) => {
    setWorkflow?.(next);
    setTaskPickerOpen(false);
    setComboOpen(false);
    setBranchSearch?.('');
  }, [setBranchSearch, setWorkflow]);

  useEffect(() => {
    if (!selectedTask) {
      setTaskDetail(null);
      setTaskDetailLoading(false);
      return;
    }
    let cancelled = false;
    setTaskDetailLoading(true);
    if (selectedTask.content !== undefined) {
      setTaskDetail({ path: selectedTask.path, content: selectedTask.content });
      setTaskDetailLoading(false);
      return () => { cancelled = true; };
    }
    fetch(`/api/files/read?path=${encodeURIComponent(selectedTask.path)}&cwd=${encodeURIComponent(cwd)}`)
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
  }, [selectedTask, cwd]);
  const activeTasks = tasks.filter(t => !['done', 'wont-do'].includes(t.status));
  const taskWorkflowVisible = taskAvailable !== false || workflow.kind === 'planFromTask';
  const taskWorkflowEnabled = taskAvailable === true && !taskAvailabilityLoading;
  const gitAlternatesClass = taskWorkflowVisible
    ? 'new-conv-workflow-alternates new-conv-workflow-alternates--three'
    : 'new-conv-workflow-alternates new-conv-workflow-alternates--two';
  const sortedActiveTasks = activeTasks.toSorted((a, b) => {
    const priorityRank = (p: string) => {
      const rank = Number(p.replace(/^p/, ''));
      return Number.isNaN(rank) ? 9 : rank;
    };
    return priorityRank(a.priority) - priorityRank(b.priority) || a.id.localeCompare(b.id);
  });
  const normalizedTaskSearch = taskSearch.trim().toLowerCase();
  const filteredTasks = normalizedTaskSearch
    ? sortedActiveTasks.filter(t => `${t.id} ${t.slug} ${t.priority} ${t.status}`.toLowerCase().includes(normalizedTaskSearch))
    : sortedActiveTasks;
  const taskPageSize = 8;
  const taskPageCount = Math.max(1, Math.ceil(filteredTasks.length / taskPageSize));
  const clampedTaskPage = Math.min(taskPage, taskPageCount - 1);
  const pagedTasks = filteredTasks.slice(
    clampedTaskPage * taskPageSize,
    clampedTaskPage * taskPageSize + taskPageSize,
  );
  useEffect(() => {
    setTaskPage(0);
  }, [tasks, taskSearch]);

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
        onDirStatusChange={onDirStatusChange}
        {...(onGitStatusChange ? { onGitStatusChange } : {})}
        selectedModel={selectedModel}
        setSelectedModel={setSelectedModel}
        models={models}
        showAllModels={showAllModels}
        setShowAllModels={setShowAllModels}
      />

      {dirStatus === 'exists' && isGitDir === true && (
        <div className="new-conv-workflow-layout" ref={comboRef}>
          <div className="new-conv-workflow-heading">
            <span className="settings-field-label">Workflow</span>
            <span className="new-conv-workflow-hint">Choose how Phoenix should use this Git repository.</span>
          </div>
          <div className="new-conv-workflows">
            <label
              className={`workflow-card workflow-card--hero ${workflow.kind === 'planFromBranch' ? 'workflow-card--active' : ''}`}
              onClick={() => chooseWorkflow({ kind: 'planFromBranch', baseBranch: workflow.kind === 'planFromBranch' ? workflow.baseBranch : null })}
            >
              <input
                className="workflow-card-radio"
                type="radio"
                name={radioGroupName}
                checked={workflow.kind === 'planFromBranch'}
                onChange={() => chooseWorkflow({ kind: 'planFromBranch', baseBranch: workflow.kind === 'planFromBranch' ? workflow.baseBranch : null })}
              />
              <span className="workflow-card-content">
                <strong>Chat in a fresh worktree</strong>
                <span>Recommended for Git repos. Start from the default branch in an isolated worktree.</span>
              </span>
            </label>
          </div>
          <div className={gitAlternatesClass}>
            {taskWorkflowVisible && (
              <label
                className={`workflow-card ${!taskWorkflowEnabled ? 'workflow-card--disabled' : ''} ${workflow.kind === 'planFromTask' ? 'workflow-card--active' : ''}`}
                onClick={() => {
                  if (!taskWorkflowEnabled) return;
                  const task = workflow.kind === 'planFromTask' ? workflow.task : null;
                  chooseWorkflow({ kind: 'planFromTask', task, baseBranch: task?.source_ref ?? null });
                  loadProjectTasks?.();
                }}
              >
                <input
                  className="workflow-card-radio"
                  type="radio"
                  name={radioGroupName}
                  checked={workflow.kind === 'planFromTask'}
                  disabled={!taskWorkflowEnabled}
                  onChange={() => {
                    if (!taskWorkflowEnabled) return;
                    const task = workflow.kind === 'planFromTask' ? workflow.task : null;
                  chooseWorkflow({ kind: 'planFromTask', task, baseBranch: task?.source_ref ?? null });
                    loadProjectTasks?.();
                  }}
                />
                <span className="workflow-card-content">
                  <strong>Start from a task</strong>
                  <span>
                    {taskAvailabilityLoading || taskAvailable === null
                      ? 'Loading tasks...'
                      : taskAvailable === true
                        ? 'Pick a task file and approve the plan before Work mode.'
                        : 'No repo tasks detected.'}
                  </span>
                </span>
              </label>
            )}

            <label
              className={`workflow-card ${workflow.kind === 'continueBranch' ? 'workflow-card--active' : ''}`}
              onClick={() => chooseWorkflow({ kind: 'continueBranch', branch: workflow.kind === 'continueBranch' ? workflow.branch : null })}
            >
              <input
                className="workflow-card-radio"
                type="radio"
                name={radioGroupName}
                checked={workflow.kind === 'continueBranch'}
                onChange={() => chooseWorkflow({ kind: 'continueBranch', branch: workflow.kind === 'continueBranch' ? workflow.branch : null })}
              />
              <span className="workflow-card-content">
                <strong>Chat in a specific branch</strong>
                <span>Check out an existing branch in its own worktree.</span>
              </span>
            </label>

            <label
              className={`workflow-card workflow-card--discouraged ${workflow.kind === 'direct' ? 'workflow-card--active' : ''}`}
              onClick={() => chooseWorkflow({ kind: 'direct' })}
            >
              <input
                className="workflow-card-radio"
                type="radio"
                name={radioGroupName}
                checked={workflow.kind === 'direct'}
                onChange={() => chooseWorkflow({ kind: 'direct' })}
              />
              <span className="workflow-card-content">
                <strong>Work in this folder</strong>
                <span>Edits the current checkout directly. Use when isolation is not needed.</span>
              </span>
            </label>
          </div>

          {workflow.kind !== 'direct' && workflow.kind !== 'planFromBranch' && (
            <div className="new-conv-workflow-detail">
              <div className="git-workflow-panel">
                <div className="git-workflow-summary">
                  Isolated workflows use a separate git worktree so this folder stays untouched.
                  {gitMetadataLoading && <span className="branch-combobox-loading"> Loading branches...</span>}
                </div>

                {workflow.kind === 'planFromTask' && (
                  <>
                    <button
                      type="button"
                      className={`git-workflow-option ${taskPickerOpen || selectedTask ? 'git-workflow-option--active' : ''}`}
                      onClick={() => {
                        loadProjectTasks?.();
                        setTaskPickerOpen(open => !open);
                      }}
                    >
                      <span className="git-workflow-title">Task file</span>
                      <span className="git-workflow-desc">
                        {selectedTask
                          ? `${selectedTask.priority} ${selectedTask.id}: ${selectedTask.slug}`
                          : tasksLoading || !tasksLoaded
                            ? 'Loading tasks...'
                            : taskPickerOpen
                              ? 'Select a task below'
                              : `${activeTasks.length} active tasks`}
                      </span>
                    </button>
                    {(taskPickerOpen || !selectedTask) && (
                      <div className="task-start-list">
                        <input
                          type="search"
                          className="settings-input task-start-search"
                          placeholder="Search tasks by number or name..."
                          value={taskSearch}
                          onChange={(e) => setTaskSearch(e.target.value)}
                        />
                        {tasksLoading && <div className="task-start-empty">Loading tasks...</div>}
                        {!tasksLoading && tasksLoaded && activeTasks.length === 0 && <div className="task-start-empty">No active tasks found.</div>}
                        {!tasksLoading && tasksLoaded && activeTasks.length > 0 && filteredTasks.length === 0 && <div className="task-start-empty">No tasks match “{taskSearch}”.</div>}
                        {pagedTasks.map(t => (
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
                        {taskPageCount > 1 && (
                          <div className="task-start-pagination">
                            <span>
                              Showing {clampedTaskPage * taskPageSize + 1}-{Math.min((clampedTaskPage + 1) * taskPageSize, filteredTasks.length)} of {filteredTasks.length}
                            </span>
                            <span className="task-start-pagination-controls">
                              <button
                                type="button"
                                className="task-start-page-button"
                                disabled={clampedTaskPage === 0}
                                onClick={() => setTaskPage(page => Math.max(0, page - 1))}
                              >
                                Prev
                              </button>
                              <button
                                type="button"
                                className="task-start-page-button"
                                disabled={clampedTaskPage >= taskPageCount - 1}
                                onClick={() => setTaskPage(page => Math.min(taskPageCount - 1, page + 1))}
                              >
                                Next
                              </button>
                            </span>
                          </div>
                        )}
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
                  </>
                )}

                {workflow.kind === 'continueBranch' && (
                  <div className="branch-combobox">
                    <span className="settings-field-label">Branch to continue</span>
                    <input
                      ref={inputRef}
                      type="text"
                      className="settings-input branch-combobox-input"
                      placeholder={comboOpen ? 'Type to filter branches...' : undefined}
                      value={comboOpen ? branchSearch : selectedName}
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
                              key={`continue:${b.name}`}
                              className={`branch-combobox-item ${selectedName === b.name ? 'branch-combobox-item--selected' : ''}`}
                              onClick={() => selectContinueBranch(b.name)}
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
            </div>
          )}
        </div>
      )}

      {(() => {
        const selectedBranch = workflow.kind === 'continueBranch'
          ? displayBranches.find(b => b.name === selectedName)
          : undefined;
        const conflictSlug = selectedTask?.conversation_slug ?? selectedBranch?.conflict_slug;
        if (!conflictSlug || workflow.kind === 'direct') return null;
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
