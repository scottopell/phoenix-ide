/**
 * FileExplorerPanel — Desktop file explorer panel (middle column)
 * REQ-FE-001, REQ-FE-004, REQ-FE-005
 */

import { useState, useCallback } from 'react';
import { FileTree } from './FileTree';
import { RecentFilesStrip } from './RecentFilesStrip';
import { McpStatusPanel } from '../McpStatusPanel';
import { SkillsPanel } from '../SkillsPanel';
import { SkillViewer } from '../SkillViewer';
import { TasksPanel } from '../TasksPanel';
import { TaskViewer } from '../TaskViewer';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import { useRecentFiles } from '../../hooks/useRecentFiles';
import type { SkillEntry, TaskEntry } from '../../api';

interface Props {
  collapsed: boolean;
  onToggle: () => void;
  rootPath: string;
  conversationId: string | undefined;
  showToast: (message: string, duration?: number) => void;
  /** Branch name of the current conversation (for extracting task ID in Work mode) */
  branchName?: string | null | undefined;
}

/** Extract task ID from a Work branch name like "task-08617-some-slug" */
function extractTaskId(branchName: string | null | undefined): string | undefined {
  if (!branchName) return undefined;
  const match = branchName.match(/^task-([A-Za-z0-9]+)-/);
  return match ? match[1] : undefined;
}

export function FileExplorerPanel({ collapsed, onToggle, rootPath, conversationId, showToast, branchName }: Props) {
  const { openFile, activeFile } = useFileExplorer();
  const { recentFiles, addRecentFile } = useRecentFiles(conversationId);
  const [refreshKey, setRefreshKey] = useState(0);
  const handleRefresh = useCallback(() => setRefreshKey(k => k + 1), []);
  const [selectedSkill, setSelectedSkill] = useState<SkillEntry | null>(null);
  const [selectedTask, setSelectedTask] = useState<TaskEntry | null>(null);
  const [skillsPanelExpanded, setSkillsPanelExpanded] = useState(false);

  const currentTaskId = extractTaskId(branchName);

  const handleFileSelect = (filePath: string, rootDir: string) => {
    addRecentFile(filePath);
    openFile(filePath, rootDir);
  };

  const handleRecentClick = (path: string) => {
    addRecentFile(path);
    openFile(path, rootPath);
  };

  if (collapsed) {
    return (
      <aside className="fe-panel fe-panel--collapsed">
        <button className="fe-toggle" onClick={onToggle} title="Expand file explorer">
          &#9654;
        </button>
        <RecentFilesStrip files={recentFiles} onFileClick={handleRecentClick} />
        <div className="fe-collapsed-badges">
          <button className="fe-collapsed-badge" onClick={onToggle} title="MCP Servers">
            MCP
          </button>
          <button className="fe-collapsed-badge" onClick={onToggle} title="Skills">
            /
          </button>
          <button className="fe-collapsed-badge" onClick={onToggle} title="Tasks">
            T
          </button>
        </div>
      </aside>
    );
  }

  // Detail viewer replaces the tree+panels when a skill or task is selected
  const detailViewer = selectedSkill
    ? <SkillViewer skill={selectedSkill} onBack={() => setSelectedSkill(null)} />
    : selectedTask
      ? <TaskViewer task={selectedTask} tasksDir={`${rootPath}/tasks`} onBack={() => setSelectedTask(null)} />
      : null;

  return (
    <aside className="fe-panel fe-panel--expanded">
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
          <McpStatusPanel showToast={showToast} />
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
        </>
      )}
    </aside>
  );
}
