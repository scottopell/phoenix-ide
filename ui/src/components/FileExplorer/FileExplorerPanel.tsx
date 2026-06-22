/**
 * FileExplorerPanel — Desktop file explorer panel (middle column)
 * REQ-FE-001, REQ-FE-004, REQ-FE-005
 */

import { useState, useCallback, useEffect, useMemo } from 'react';
import { FileTree } from './FileTree';
import { McpStatusPanel } from '../McpStatusPanel';
import { SkillsPanel } from '../SkillsPanel';
import { SkillViewer } from '../SkillViewer';
import { TasksPanel } from '../TasksPanel';
import { TaskViewer } from '../TaskViewer';
import { WorkScopeSection } from '../WorkScopePanel';
import { useSeededLiveCount } from '../useWorkScopeSeed';
import { workScopeLiveCount } from '../workScopeHelpers';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import type { SkillEntry, TaskEntry, WorkScopeInventory } from '../../api';

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

export function FileExplorerPanel({ collapsed, onToggle, rootPath, conversationId, showToast, showError, branchName, activeSlug, width, workScopeKey, liveWorkScope }: Props) {
  const { openFile, activeFile } = useFileExplorer();
  const [refreshKey, setRefreshKey] = useState(0);
  const handleRefresh = useCallback(() => setRefreshKey(k => k + 1), []);
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

  const currentTaskId = extractTaskId(branchName);
  const workScopeCount = useSeededLiveCount(workScopeKey, liveWorkScope);
  const liveAttentionCount = liveWorkScope ? workScopeLiveCount(liveWorkScope) : workScopeCount;
  const hasFileRoot = !!rootPath;
  const projectName = rootPath ? (rootPath.split('/').filter(Boolean).slice(-1)[0] || rootPath) : 'Read-only';

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
          onBack={() => setSelectedTask(null)}
        />
      : null;

  return (
    <aside
      className="fe-panel fe-panel--expanded"
      style={width !== undefined ? { width: `${width}px` } : undefined}
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
                rootPath={rootPath}
                onFileSelect={handleFileSelect}
                activeFile={activeFile}
                conversationId={conversationId}
                refreshKey={refreshKey}
              />
            </div>
          )}
          <McpStatusPanel showToast={showToast} showError={showError} />
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
