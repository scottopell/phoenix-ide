/**
 * FileBrowser Component
 * 
 * Implements REQ-PF-001 through REQ-PF-004:
 * - Browse project files with icons
 * - Directory navigation with persistent expansion state
 * - File type detection (extension-based)
 * - Sorting (directories first, alphabetical)
 */

import { useState, useEffect, useCallback } from 'react';
import {
  Folder,
  FileText,
  FileCode,
  Settings,
  File,
  Image,
  Database,
  ChevronRight,
  ChevronDown,
  ArrowLeft,
  X,
  Loader2,
  AlertCircle,
  FolderOpen,
} from 'lucide-react';

// Types
export interface FileItem {
  name: string;
  path: string;
  is_directory: boolean;
  size?: number;
  modified_time?: number;
  file_type: 'folder' | 'markdown' | 'code' | 'config' | 'text' | 'image' | 'data' | 'unknown';
  is_text_file: boolean;
}

interface FileBrowserProps {
  isOpen: boolean;
  rootPath: string;
  conversationId: string;
  onClose: () => void;
  onFileSelect: (filePath: string, rootDir: string) => void;
}

// API functions
async function listFiles(path: string): Promise<FileItem[]> {
  const response = await fetch(`/api/files/list?path=${encodeURIComponent(path)}`);
  if (!response.ok) {
    const error = await response.json().catch(() => ({ error: 'Unknown error' }));
    throw new Error(error.error || 'Failed to list files');
  }
  const data = await response.json();
  return data.items;
}

// Utility functions
function formatFileSize(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let size = bytes;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex++;
  }

  return unitIndex === 0 ? `${size} ${units[unitIndex]}` : `${size.toFixed(1)} ${units[unitIndex]}`;
}

function formatRelativeTime(timestamp: number): string {
  const now = Date.now() / 1000;
  const diff = now - timestamp;

  if (diff < 60) return 'just now';
  if (diff < 3600) return `${Math.floor(diff / 60)} minutes ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} hours ago`;
  if (diff < 604800) return `${Math.floor(diff / 86400)} days ago`;
  if (diff < 2592000) return `${Math.floor(diff / 604800)} weeks ago`;
  return `${Math.floor(diff / 2592000)} months ago`;
}

function truncatePath(path: string, maxSegments: number = 3): string {
  const segments = path.split('/').filter(Boolean);
  if (segments.length <= maxSegments) return path;
  return '.../' + segments.slice(-maxSegments).join('/');
}

// Icon component
function FileIcon({ type, isExpanded }: { type: FileItem['file_type']; isExpanded?: boolean }) {
  const iconProps = { size: 18, className: `file-icon file-icon-${type}` };
  
  switch (type) {
    case 'folder':
      return isExpanded ? <FolderOpen {...iconProps} /> : <Folder {...iconProps} />;
    case 'markdown':
      return <FileText {...iconProps} />;
    case 'code':
      return <FileCode {...iconProps} />;
    case 'config':
      return <Settings {...iconProps} />;
    case 'text':
      return <FileText {...iconProps} />;
    case 'image':
      return <Image {...iconProps} />;
    case 'data':
      return <Database {...iconProps} />;
    default:
      return <File {...iconProps} />;
  }
}

// Expansion state storage per conversation
const expansionStates = new Map<string, Set<string>>();

export function FileBrowser({
  isOpen,
  rootPath,
  conversationId,
  onClose,
  onFileSelect,
}: FileBrowserProps) {
  const [currentPath, setCurrentPath] = useState(rootPath);
  const [items, setItems] = useState<FileItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(() => {
    return expansionStates.get(conversationId) || new Set();
  });
  const [loadingPaths, setLoadingPaths] = useState<Set<string>>(new Set());
  const [childItems, setChildItems] = useState<Map<string, FileItem[]>>(new Map());

  // Persist expansion state
  useEffect(() => {
    expansionStates.set(conversationId, expandedPaths);
  }, [conversationId, expandedPaths]);

  // Load directory contents
  const loadDirectory = useCallback(async (path: string) => {
    setLoading(true);
    setError(null);
    try {
      const result = await listFiles(path);
      setItems(result);
      setCurrentPath(path);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load directory');
    } finally {
      setLoading(false);
    }
  }, []);

  // Load children for expanded folder
  const loadChildren = useCallback(async (path: string) => {
    if (childItems.has(path)) return;
    
    setLoadingPaths(prev => new Set(prev).add(path));
    try {
      const result = await listFiles(path);
      setChildItems(prev => new Map(prev).set(path, result));
    } catch (err) {
      console.error('Failed to load children:', err);
    } finally {
      setLoadingPaths(prev => {
        const next = new Set(prev);
        next.delete(path);
        return next;
      });
    }
  }, [childItems]);

  // Initial load
  useEffect(() => {
    if (isOpen) {
      loadDirectory(rootPath);
    }
  }, [isOpen, rootPath, loadDirectory]);

  // Navigate to parent
  const navigateUp = useCallback(() => {
    const parentPath = currentPath.split('/').slice(0, -1).join('/') || '/';
    loadDirectory(parentPath);
  }, [currentPath, loadDirectory]);

  // Toggle folder expansion
  const toggleExpand = useCallback((path: string) => {
    setExpandedPaths(prev => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
        loadChildren(path);
      }
      return next;
    });
  }, [loadChildren]);

  // Handle item click
  const handleItemClick = useCallback((item: FileItem) => {
    if (item.is_directory) {
      toggleExpand(item.path);
    } else if (item.is_text_file) {
      onFileSelect(item.path, rootPath);
    }
  }, [toggleExpand, onFileSelect, rootPath]);

  // Check if at root
  const isAtRoot = currentPath === rootPath || currentPath === '/';

  // Render a file/folder item
  const renderItem = (item: FileItem, depth: number = 0) => {
    const isExpanded = expandedPaths.has(item.path);
    const isLoadingChildren = loadingPaths.has(item.path);
    const children = childItems.get(item.path) || [];
    const isDisabled = !item.is_directory && !item.is_text_file;

    return (
      <div key={item.path}>
        <div
          className={`file-browser-item ${isDisabled ? 'file-browser-item--disabled' : ''}`}
          style={{ paddingLeft: 16 + depth * 16 }}
          onClick={() => !isDisabled && handleItemClick(item)}
          role="button"
          tabIndex={isDisabled ? -1 : 0}
          aria-disabled={isDisabled}
          title={isDisabled ? 'Non-text file' : item.path}
        >
          {item.is_directory && (
            <span className="file-browser-expand-icon">
              {isLoadingChildren ? (
                <Loader2 size={14} className="spinning" />
              ) : isExpanded ? (
                <ChevronDown size={14} />
              ) : (
                <ChevronRight size={14} />
              )}
            </span>
          )}
          <FileIcon type={item.file_type} isExpanded={isExpanded} />
          <span className="file-browser-name">{item.name}</span>
          {!item.is_directory && (
            <span className="file-browser-meta">
              {item.size !== undefined && (
                <span className="file-browser-size">{formatFileSize(item.size)}</span>
              )}
              {item.modified_time && (
                <span className="file-browser-time">{formatRelativeTime(item.modified_time)}</span>
              )}
            </span>
          )}
          {isDisabled && (
            <span className="file-browser-disabled-label">Non-text file</span>
          )}
        </div>
        {item.is_directory && isExpanded && (
          <div className="file-browser-children">
            {isLoadingChildren ? (
              <div className="file-browser-loading" style={{ paddingLeft: 32 + depth * 16 }}>
                <Loader2 size={16} className="spinning" /> Loading...
              </div>
            ) : children.length === 0 ? (
              <div className="file-browser-empty" style={{ paddingLeft: 32 + depth * 16 }}>
                Empty directory
              </div>
            ) : (
              children.map(child => renderItem(child, depth + 1))
            )}
          </div>
        )}
      </div>
    );
  };

  if (!isOpen) return null;

  return (
    <div className="file-browser-overlay">
      <div className="file-browser-container">
        {/* Header */}
        <div className="file-browser-header">
          <button
            className="file-browser-btn"
            onClick={navigateUp}
            disabled={isAtRoot}
            aria-label="Go to parent directory"
          >
            <ArrowLeft size={20} />
          </button>
          <div className="file-browser-path" title={currentPath}>
            {truncatePath(currentPath)}
          </div>
          <button
            className="file-browser-btn"
            onClick={onClose}
            aria-label="Close file browser"
          >
            <X size={20} />
          </button>
        </div>

        {/* Content */}
        <div className="file-browser-content">
          {loading ? (
            <div className="file-browser-loading">
              <Loader2 size={24} className="spinning" />
              <span>Loading...</span>
            </div>
          ) : error ? (
            <div className="file-browser-error">
              <AlertCircle size={24} />
              <span>{error}</span>
              <button onClick={() => loadDirectory(currentPath)}>Retry</button>
            </div>
          ) : items.length === 0 ? (
            <div className="file-browser-empty">
              <Folder size={48} className="file-browser-empty-icon" />
              <span>Empty directory</span>
            </div>
          ) : (
            <div className="file-browser-list">
              {items.map(item => renderItem(item, 0))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
