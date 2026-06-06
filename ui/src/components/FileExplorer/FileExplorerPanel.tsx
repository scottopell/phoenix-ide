/**
 * FileExplorerPanel — Desktop file explorer panel (middle column)
 * REQ-FE-001, REQ-FE-004, REQ-FE-005
 */

import { useState, useCallback } from 'react';
import { FileTree } from './FileTree';
import { McpStatusPanel } from '../McpStatusPanel';
import { SkillsPanel } from '../SkillsPanel';
import { SkillViewer } from '../SkillViewer';
import { TasksPanel } from '../TasksPanel';
import { TaskViewer } from '../TaskViewer';
import { WorkScopeSection } from '../WorkScopePanel';
import { workScopeLiveCount } from '../workScopeHelpers';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import type { SkillEntry, TaskEntry, WorkScopeInventory } from '../../api';

interface Props {
  collapsed: boolean;
  onToggle: () => void;
  rootPath: string;
  conversationId: string | undefined;
  showToast: (message: string, duration?: number) => void;
  /** Error-styled toast (red). Used by `McpStatusPanel` for failure
   *  paths so they don't render with the same green styling as
   *  success messages (REQ-NOTIF-002). */
  showError: (message: string, duration?: number) => void;
  /** Branch name of the current conversation (for extracting task ID in Work mode) */
  branchName?: string | null | undefined;
  /** Slug of the active conversation. Passed to TaskViewer, which reads the
   *  live conversation row from the store to seed a "start working on this
   *  task" sub-conversation (REQ-SEED-001 through -004). */
  activeSlug: string;
  /** Width in px when expanded — driven by useResizablePane */
  width?: number | undefined;
  /** The conversation's work-scope key. When present, the Work scope section
   *  (and its collapsed-rail badge) render; resources are WorkScope-keyed so
   *  this single key addresses every backgrounded bash handle, the tmux
   *  server, and the browser session (REQ-WSUI-010). */
  workScopeKey?: string | null | undefined;
  /** Live work-scope inventory from the conversation atom (SSE-fed). Drives
   *  the collapsed-rail live count and overrides the section's initial fetch. */
  liveWorkScope?: WorkScopeInventory | null | undefined;
}

/** Extract task ID from a Work branch name like "task-08617-some-slug" */
function extractTaskId(branchName: string | null | undefined): string | undefined {
  if (!branchName) return undefined;
  const match = branchName.match(/^task-([A-Za-z0-9]+)-/);
  return match ? match[1] : undefined;
}

export function FileExplorerPanel({ collapsed, onToggle, rootPath, conversationId, showToast, showError, branchName, activeSlug, width, workScopeKey, liveWorkScope }: Props) {
  const { openFile, activeFile } = useFileExplorer();
  const [refreshKey, setRefreshKey] = useState(0);
  const handleRefresh = useCallback(() => setRefreshKey(k => k + 1), []);
  const [selectedSkill, setSelectedSkill] = useState<SkillEntry | null>(null);
  const [selectedTask, setSelectedTask] = useState<TaskEntry | null>(null);
  const [skillsPanelExpanded, setSkillsPanelExpanded] = useState(false);
  // Default-expanded: this is an at-a-glance resource view, so the section is
  // open on first paint rather than requiring a click like Skills/Tasks.
  const [workScopeExpanded, setWorkScopeExpanded] = useState(true);

  const currentTaskId = extractTaskId(branchName);

  const handleFileSelect = (filePath: string, rootDir: string) => {
    openFile(filePath, rootDir);
  };

  if (collapsed) {
    return (
      <aside className="fe-panel fe-panel--collapsed">
        <button className="fe-toggle" onClick={onToggle} title="Expand file explorer">
          &#9654;
        </button>
        <div className="fe-collapsed-badges">
          <button className="fe-collapsed-badge" onClick={onToggle} title="Files">
            Files
          </button>
          <button className="fe-collapsed-badge" onClick={onToggle} title="MCP Servers">
            MCP
          </button>
          <button className="fe-collapsed-badge" onClick={onToggle} title="Skills">
            /
          </button>
          <button className="fe-collapsed-badge" onClick={onToggle} title="Tasks">
            T
          </button>
          {workScopeKey && (
            <button
              className={`fe-collapsed-badge${workScopeLiveCount(liveWorkScope ?? null) > 0 ? ' fe-collapsed-badge--active' : ''}`}
              onClick={onToggle}
              title={`Work scope · ${workScopeLiveCount(liveWorkScope ?? null)} running`}
            >
              {workScopeLiveCount(liveWorkScope ?? null)}
            </button>
          )}
        </div>
      </aside>
    );
  }

  // Detail viewer replaces the tree+panels when a skill or task is selected
  const detailViewer = selectedSkill
    ? <SkillViewer skill={selectedSkill} onBack={() => setSelectedSkill(null)} />
    : selectedTask
      ? <TaskViewer
          task={selectedTask}
          tasksDir={`${rootPath}/tasks`}
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
        <button className="fe-toggle" onClick={onToggle} title="Collapse">&#9666;</button>
        <span className="fe-title">Files</span>
        <button className="fe-refresh" onClick={handleRefresh} title="Refresh file tree">&#8635;</button>
      </div>
      {detailViewer || (
        <>
          <div className="fe-tree-scroll">
            <FileTree
              rootPath={rootPath}
              onFileSelect={handleFileSelect}
              activeFile={activeFile}
              conversationId={conversationId}
              refreshKey={refreshKey}
            />
          </div>
          <McpStatusPanel showToast={showToast} showError={showError} />
          <SkillsPanel
            conversationId={conversationId}
            onSkillClick={setSelectedSkill}
            expanded={skillsPanelExpanded}
            onToggleExpanded={setSkillsPanelExpanded}
          />
          <TasksPanel
            conversationId={conversationId}
            currentTaskId={currentTaskId}
            onTaskClick={setSelectedTask}
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
