/**
 * FileExplorerPanel — Desktop file explorer panel (middle column)
 * REQ-FE-001, REQ-FE-004, REQ-FE-005
 */

import { useState, useCallback } from 'react';
import { FileTree } from './FileTree';
import { RecentFilesStrip } from './RecentFilesStrip';
import { McpStatusPanel } from '../McpStatusPanel';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import { useRecentFiles } from '../../hooks/useRecentFiles';

interface Props {
  collapsed: boolean;
  onToggle: () => void;
  rootPath: string;
  conversationId: string | undefined;
  showToast: (message: string, duration?: number) => void;
}

export function FileExplorerPanel({ collapsed, onToggle, rootPath, conversationId, showToast }: Props) {
  const { openFile, activeFile } = useFileExplorer();
  const { recentFiles, addRecentFile } = useRecentFiles(conversationId);
  const [refreshKey, setRefreshKey] = useState(0);
  const handleRefresh = useCallback(() => setRefreshKey(k => k + 1), []);

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
      </aside>
    );
  }

  return (
    <aside className="fe-panel fe-panel--expanded">
      <div className="fe-header">
        <button className="fe-toggle" onClick={onToggle} title="Collapse">&#9666;</button>
        <span className="fe-title">Files</span>
        <button className="fe-refresh" onClick={handleRefresh} title="Refresh file tree">&#8635;</button>
      </div>
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
    </aside>
  );
}
