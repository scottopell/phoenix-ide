/**
 * FileExplorerPanel — Desktop file explorer panel (middle column)
 * REQ-FE-001, REQ-FE-004, REQ-FE-005
 */

import { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { ExternalLink } from 'lucide-react';
import { FileTree } from './FileTree';
import { FileTreeContextMenu } from './FileTreeContextMenu';
import { McpStatusPanel } from '../McpStatusPanel';
import { SkillsPanel } from '../SkillsPanel';
import { SkillViewer } from '../SkillViewer';
import { TasksPanel } from '../TasksPanel';
import { TaskViewer } from '../TaskViewer';
import { WorkScopeSection } from '../WorkScopePanel';
import { useSeededLiveCount } from '../useWorkScopeSeed';
import { workScopeLiveCount } from '../workScopeHelpers';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import { GroundingSection, GroundingState } from '../GroundingPanel';
import { useViewerSlotCommands } from '../../contexts/ViewerSlotContext';
import { api, type ConversationGitStatusResponse, type SkillEntry, type TaskEntry, type WorkScopeInventory } from '../../api';
import './FileExplorerPanel.css';
import { checkoutLabel } from './gitStatusPresentation';

interface Props {
  collapsed: boolean;
  onToggle: () => void;
  rootPath?: string | null | undefined;
  conversationId: string | undefined;
  showToast: (message: string, duration?: number) => void;
  showError: (message: string, duration?: number) => void;
  branchName?: string | null | undefined;
  activeSlug: string;
  width?: number | undefined;
  workScopeKey?: string | null | undefined;
  liveWorkScope?: WorkScopeInventory | null | undefined;
  canOpenWorkspaceDiff?: boolean | undefined;
}

function extractTaskId(branchName: string | null | undefined): string | undefined {
  if (!branchName) return undefined;
  const match = branchName.match(/^task-([A-Za-z0-9]+)-/);
  return match ? match[1] : undefined;
}

const DEFAULT_TASK_GROUP_EXPANDED: Record<string, boolean> = {
  'in-progress': true,
  ready: true,
  blocked: true,
  brainstorming: false,
  done: false,
  'wont-do': false,
};

function describeGitSummary(status: ConversationGitStatusResponse | null | undefined): { summary: string; count?: number; attention: boolean } {
  if (!status) return { summary: 'Checking git…', attention: false };
  switch (status.kind) {
    case 'snapshot': {
      const counts = status.counts;
      const checkout = checkoutLabel(status.checkout_status);
      const upstreamUnavailable = status.checkout_status.kind === 'named_branch'
        && status.checkout_status.remote_status.kind === 'unavailable';
      if (counts.changed_paths === 0) return { summary: `${checkout} · clean`, attention: upstreamUnavailable };
      const parts = [
        `${counts.changed_paths} changed`,
        counts.staged_paths > 0 ? `${counts.staged_paths} staged` : null,
        counts.unstaged_paths > 0 ? `${counts.unstaged_paths} unstaged` : null,
        counts.untracked_paths > 0 ? `${counts.untracked_paths} untracked` : null,
      ].filter(Boolean);
      return { summary: `${checkout} · ${parts.join(' · ')}`, count: counts.changed_paths, attention: upstreamUnavailable || counts.conflicted_paths > 0 };
    }
    case 'non_git':
      return { summary: 'Not a git workspace', attention: false };
    case 'unavailable':
      return { summary: status.reason, attention: true };
    default:
      return { summary: 'Git status unavailable', attention: true };
  }
}

function GitStatusDetails({ status }: { status: Extract<ConversationGitStatusResponse, { kind: 'snapshot' }> }) {
  const { counts } = status;

  return (
    <div className="git-status-details" aria-label="Git grounding details">
      <div className="git-status-checkout" title={checkoutLabel(status.checkout_status)}>
        {checkoutLabel(status.checkout_status)}
      </div>
      {counts.changed_paths === 0 ? (
        <div className="git-status-clean">nothing to commit, working tree clean</div>
      ) : (
        <div className="git-status-groups">
          {counts.staged_paths > 0 && (
            <div className="git-status-group git-status-group--staged">
              <span>Changes to be committed</span><strong>{counts.staged_paths}</strong>
            </div>
          )}
          {counts.unstaged_paths > 0 && (
            <div className="git-status-group git-status-group--unstaged">
              <span>Changes not staged for commit</span><strong>{counts.unstaged_paths}</strong>
            </div>
          )}
          {counts.untracked_paths > 0 && (
            <div className="git-status-group git-status-group--untracked">
              <span>Untracked files</span><strong>{counts.untracked_paths}</strong>
            </div>
          )}
          {counts.conflicted_paths > 0 && (
            <div className="git-status-group git-status-group--conflicted">
              <span>Unmerged paths</span><strong>{counts.conflicted_paths}</strong>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function FileExplorerPanel({ collapsed, onToggle, rootPath, conversationId, showToast, showError, branchName, activeSlug, width, workScopeKey, liveWorkScope, canOpenWorkspaceDiff = false }: Props) {
  const { openFile, activeFile } = useFileExplorer();
  const { openDiffFullscreen } = useViewerSlotCommands();
  const [refreshKey, setRefreshKey] = useState(0);
  const gitRequestRef = useRef<AbortController | null>(null);
  const [gitStatus, setGitStatus] = useState<ConversationGitStatusResponse | null>(null);
  const [gitExpanded, setGitExpanded] = useState(true);
  const [selectedSkill, setSelectedSkill] = useState<SkillEntry | null>(null);
  const [selectedTask, setSelectedTask] = useState<TaskEntry | null>(null);
  const [skillsPanelExpanded, setSkillsPanelExpanded] = useState(false);
  const [skillsGroupExpanded, setSkillsGroupExpanded] = useState<Set<string> | null>(null);
  const [skillsScrollTop, setSkillsScrollTop] = useState(0);
  const [tasksPanelExpanded, setTasksPanelExpanded] = useState(false);
  const defaultTaskGroupExpanded = useMemo(() => ({ ...DEFAULT_TASK_GROUP_EXPANDED }), []);
  const [taskGroupExpanded, setTaskGroupExpanded] = useState(defaultTaskGroupExpanded);
  const [tasksScrollTop, setTasksScrollTop] = useState(0);
  const [workScopeExpanded, setWorkScopeExpanded] = useState(true);

  useEffect(() => {
    setSelectedSkill(null);
    setSelectedTask(null);
    setSkillsPanelExpanded(false);
    setSkillsGroupExpanded(null);
    setSkillsScrollTop(0);
    setTasksPanelExpanded(false);
    setTaskGroupExpanded(defaultTaskGroupExpanded);
    setTasksScrollTop(0);
  }, [conversationId, rootPath, defaultTaskGroupExpanded]);

  useEffect(() => {
    setGitStatus(null);
    setGitExpanded(true);
  }, [conversationId, rootPath]);

  const loadGitStatus = useCallback(async () => {
    if (!conversationId || !rootPath) {
      setGitStatus(null);
      return;
    }
    gitRequestRef.current?.abort();
    const controller = new AbortController();
    gitRequestRef.current = controller;
    try {
      const next = await api.getConversationGitStatus(conversationId, controller.signal);
      if (!controller.signal.aborted) setGitStatus(next ?? { kind: 'unavailable', reason: 'Git status is unavailable.' });
    } catch (error) {
      if (!controller.signal.aborted) {
        setGitStatus({
          kind: 'unavailable',
          reason: error instanceof Error ? error.message : 'Git status is unavailable.',
        });
      }
    }
  }, [conversationId, rootPath]);

  useEffect(() => {
    void loadGitStatus();
    return () => gitRequestRef.current?.abort();
  }, [loadGitStatus]);

  const wasCollapsedRef = useRef(collapsed);
  useEffect(() => {
    if (wasCollapsedRef.current && !collapsed) void loadGitStatus();
    wasCollapsedRef.current = collapsed;
  }, [collapsed, loadGitStatus]);

  const handleRefresh = useCallback(() => {
    setRefreshKey(k => k + 1);
    void loadGitStatus();
  }, [loadGitStatus]);

  const currentTaskId = extractTaskId(branchName);
  const workScopeCount = useSeededLiveCount(workScopeKey, liveWorkScope);
  const liveAttentionCount = liveWorkScope ? workScopeLiveCount(liveWorkScope) : workScopeCount;
  const hasFileRoot = !!rootPath;
  const projectName = rootPath ? (rootPath.split('/').filter(Boolean).slice(-1)[0] || rootPath) : 'Read-only';
  const gitSummary = describeGitSummary(gitStatus) ?? { summary: 'Git status unavailable', attention: true };

  const handleFileSelect = useCallback((filePath: string, rootDir: string) => {
    openFile(filePath, rootDir);
  }, [openFile]);

  if (collapsed) {
    return (
      <aside className="fe-panel fe-panel--collapsed" aria-label="Conversation grounding panel collapsed">
        <button className="fe-toggle" onClick={onToggle} title="Expand grounding panel" aria-label="Expand grounding panel">
          &#9654;
        </button>
        <div className="fe-collapsed-title" title="Grounding">G</div>
        <div className="fe-collapsed-badges" aria-label="Grounding sections">
          {hasFileRoot && <button className="fe-collapsed-badge" onClick={onToggle} title="Project files">Files</button>}
          <button className="fe-collapsed-badge" onClick={onToggle} title="MCP capabilities">MCP</button>
          <button className="fe-collapsed-badge" onClick={onToggle} title="Skills">Skills</button>
          <button className="fe-collapsed-badge" onClick={onToggle} title="Tasks">Tasks</button>
          {workScopeKey && (
            <button
              className={`fe-collapsed-badge${liveAttentionCount > 0 ? ' fe-collapsed-badge--active' : ''}`}
              onClick={onToggle}
              title={`Work scope · ${liveAttentionCount} live`}
            >
              Work {liveAttentionCount}
            </button>
          )}
        </div>
      </aside>
    );
  }

  const detailViewer = selectedSkill
    ? <SkillViewer skill={selectedSkill} onBack={() => setSelectedSkill(null)} />
    : selectedTask
      ? <TaskViewer
          task={selectedTask}
          tasksDir={rootPath ? `${rootPath}/tasks` : null}
          activeSlug={activeSlug}
          readOnly={!rootPath}
          onBack={() => setSelectedTask(null)}
        />
      : null;

  return (
    <aside
      className="fe-panel fe-panel--expanded"
      // `--file-explorer-pane-width` (set on `.desktop-layout` by the divider's
      // live-drag channel) wins over the committed `width` prop during a drag,
      // so resizing does not re-render the file tree per frame; the prop is the
      // fallback for hosts that don't drive the variable.
      style={width !== undefined ? { width: `var(--file-explorer-pane-width, ${width}px)` } : undefined}
    >
      <div className="fe-header">
        <button className="fe-toggle" onClick={onToggle} title="Collapse grounding panel" aria-label="Collapse grounding panel">&#9666;</button>
        <div className="fe-title-stack">
          <span className="fe-title">Grounding</span>
          <span className="fe-subtitle" title={rootPath ?? undefined}>
            {rootPath ? `${projectName} · ${branchName ?? 'no branch'}` : 'Read-only history'}
          </span>
        </div>
        {hasFileRoot && (
          <button className="fe-refresh" onClick={handleRefresh} title="Refresh file tree" aria-label="Refresh file tree">&#8635;</button>
        )}
      </div>
      {detailViewer || (
        <>
          {rootPath && (
            <div className="fe-tree-scroll">
              <FileTree
                key={`${conversationId ?? ''}\0${rootPath}`}
                rootPath={rootPath}
                onFileSelect={handleFileSelect}
                activeFile={activeFile}
                conversationId={conversationId}
                refreshKey={refreshKey}
                gitStatus={gitStatus}
                onRefreshTick={loadGitStatus}
              />
            </div>
          )}
          <FileTreeContextMenu />
          {rootPath && conversationId && (
            <GroundingSection
              icon="Δ"
              title="Git"
              {...(!gitExpanded ? { summary: gitSummary.summary } : {})}
              {...(gitSummary.count !== undefined ? { count: gitSummary.count } : {})}
              attention={gitSummary.attention}
              expanded={gitExpanded}
              onToggle={() => setGitExpanded((value) => !value)}
              action={canOpenWorkspaceDiff ? (
                <button
                  type="button"
                  className="git-grounding-open-diff"
                  onClick={(event) => {
                    event.stopPropagation();
                    openDiffFullscreen('workspace');
                  }}
                  aria-label="Open Git diff"
                  title="Open Workspace Diff"
                >
                  <ExternalLink size={13} aria-hidden="true" />
                </button>
              ) : undefined}
            >
              {gitStatus?.kind === 'snapshot' ? (
                <GitStatusDetails status={gitStatus} />
              ) : gitStatus?.kind === 'non_git' ? (
                <GroundingState>Git status is unavailable because this root is not in a repository.</GroundingState>
              ) : gitStatus?.kind === 'unavailable' ? (
                <GroundingState tone="error">{gitStatus.reason}</GroundingState>
              ) : (
                <GroundingState tone="loading">Checking workspace status…</GroundingState>
              )}
            </GroundingSection>
          )}
          <McpStatusPanel showToast={showToast} showError={showError} readOnly={!rootPath} />
          <SkillsPanel
            conversationId={conversationId}
            onSkillClick={setSelectedSkill}
            expanded={skillsPanelExpanded}
            onToggleExpanded={setSkillsPanelExpanded}
            expandedGroups={skillsGroupExpanded}
            onExpandedGroupsChange={setSkillsGroupExpanded}
            scrollTop={skillsScrollTop}
            onScrollTopChange={setSkillsScrollTop}
          />
          <TasksPanel
            conversationId={conversationId}
            currentTaskId={currentTaskId}
            onTaskClick={setSelectedTask}
            expanded={tasksPanelExpanded}
            onToggleExpanded={setTasksPanelExpanded}
            groupExpanded={taskGroupExpanded}
            onGroupExpandedChange={setTaskGroupExpanded}
            scrollTop={tasksScrollTop}
            onScrollTopChange={setTasksScrollTop}
          />
          {workScopeKey && (
            <WorkScopeSection
              scopeKey={workScopeKey}
              liveInventory={liveWorkScope}
              expanded={workScopeExpanded}
              onToggleExpanded={setWorkScopeExpanded}
            />
          )}
        </>
      )}
    </aside>
  );
}
