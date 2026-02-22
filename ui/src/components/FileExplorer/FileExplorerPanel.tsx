/**
 * FileExplorerPanel — Desktop file explorer panel (middle column)
 * REQ-FE-001, REQ-FE-004, REQ-FE-005
 */

import { FileTree } from './FileTree';
import { RecentFilesStrip } from './RecentFilesStrip';
import { useFileExplorer } from '../../hooks/useFileExplorer';
import { useRecentFiles } from '../../hooks/useRecentFiles';

interface Props {
  collapsed: boolean;
  onToggle: () => void;
  rootPath: string;
  conversationId: string | undefined;
}

export function FileExplorerPanel({ collapsed, onToggle, rootPath, conversationId }: Props) {
  const { openFile, activeFile } = useFileExplorer();
  const { recentFiles, addRecentFile } = useRecentFiles(conversationId);

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
        <RecentFilesStrip files={recentFiles} onFileClick={handleRecentClick} />
        <button className="fe-toggle" onClick={onToggle} title="Expand file explorer">
          ▶
        </button>
      </aside>
    );
  }

  return (
    <aside className="fe-panel fe-panel--expanded">
      <div className="fe-header">
        <span className="fe-title">Files</span>
        <button className="fe-toggle" onClick={onToggle} title="Collapse file explorer">
          ◀
        </button>
      </div>
      <div className="fe-tree-scroll">
        <FileTree
          rootPath={rootPath}
          onFileSelect={handleFileSelect}
          activeFile={activeFile}
          conversationId={conversationId}
        />
      </div>
    </aside>
  );
}
