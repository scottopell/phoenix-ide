/**
 * FileBrowserOverlay — Mobile modal overlay hosting FileTree
 * REQ-FE-010
 */

import { X } from 'lucide-react';
import { FileTree } from './FileTree';

interface Props {
  isOpen: boolean;
  rootPath: string;
  conversationId: string;
  onClose: () => void;
  onFileSelect: (filePath: string, rootDir: string) => void;
}

export function FileBrowserOverlay({ isOpen, rootPath, conversationId, onClose, onFileSelect }: Props) {
  if (!isOpen) return null;

  const displayPath = rootPath.length > 40
    ? '.../' + rootPath.split('/').slice(-2).join('/')
    : rootPath;

  return (
    <div className="file-browser-overlay" onClick={onClose}>
      <div className="file-browser-container" onClick={e => e.stopPropagation()}>
        <div className="file-browser-header">
          <div className="file-browser-path" title={rootPath}>{displayPath}</div>
          <button className="file-browser-btn" onClick={onClose} aria-label="Close">
            <X size={20} />
          </button>
        </div>
        <div className="file-browser-content">
          <FileTree
            rootPath={rootPath}
            onFileSelect={onFileSelect}
            conversationId={conversationId}
          />
        </div>
      </div>
    </div>
  );
}
