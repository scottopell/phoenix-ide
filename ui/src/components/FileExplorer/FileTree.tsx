/**
 * FileTree Component
 *
 * Core tree component extracted from FileBrowser.
 * Used in both FileExplorerPanel (desktop) and FileBrowserOverlay (mobile).
 *
 * REQ-FE-002: File tree display with expansion persistence
 * REQ-FE-003: File selection
 * REQ-FE-009: Active file highlight, loading indicators
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

interface FileTreeProps {
  rootPath: string;
  onFileSelect: (filePath: string, rootDir: string) => void;
  activeFile?: string | null | undefined;
  conversationId?: string | undefined;
}

// API
async function listFiles(path: string): Promise<FileItem[]> {
  const response = await fetch(`/api/files/list?path=${encodeURIComponent(path)}`);
  if (!response.ok) {
    const error = await response.json().catch(() => ({ error: 'Unknown error' }));
    throw new Error(error.error || 'Failed to list files');
  }
  const data = await response.json();
  return data.items;
}

// Expansion state persistence
function expansionKey(convId: string): string {
  return `phoenix:file-tree-expansion:${convId}`;
}

function loadExpansion(convId: string | undefined): Set<string> {
  if (!convId) return new Set();
  try {
    const raw = localStorage.getItem(expansionKey(convId));
    return raw ? new Set(JSON.parse(raw)) : new Set();
  } catch {
    return new Set();
  }
}

function saveExpansion(convId: string, expanded: Set<string>) {
  localStorage.setItem(expansionKey(convId), JSON.stringify([...expanded]));
}

// Icon component
export function FileIcon({ type, isExpanded }: { type: FileItem['file_type']; isExpanded?: boolean }) {
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

export function FileTree({ rootPath, onFileSelect, activeFile, conversationId }: FileTreeProps) {
  const [items, setItems] = useState<FileItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Bundle conversationId + expandedPaths into a single atom so they can't desync.
  // The save effect always sees a consistent (convId, paths) pair.
  const [expansion, setExpansion] = useState(() => ({
    convId: conversationId,
    paths: loadExpansion(conversationId),
  }));
  const [loadingPaths, setLoadingPaths] = useState<Set<string>>(new Set());
  const [childItems, setChildItems] = useState<Map<string, FileItem[]>>(new Map());

  // When conversation changes, atomically load new expansion state
  useEffect(() => {
    setExpansion({ convId: conversationId, paths: loadExpansion(conversationId) });
    setChildItems(new Map());
  }, [conversationId]);

  // Persist — always correct because convId is part of the atom
  useEffect(() => {
    if (expansion.convId) {
      saveExpansion(expansion.convId, expansion.paths);
    }
  }, [expansion]);

  // Convenience alias
  const expandedPaths = expansion.paths;

  // Load root directory contents
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    listFiles(rootPath)
      .then(result => { if (!cancelled) setItems(result); })
      .catch(err => { if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to load'); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [rootPath]);

  // Load children for expanded folder
  const loadChildren = useCallback(async (path: string) => {
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
  }, []);

  // Auto-load children for already-expanded paths when switching conversations
  useEffect(() => {
    for (const path of expandedPaths) {
      if (!childItems.has(path)) {
        loadChildren(path);
      }
    }
  }, [expandedPaths, childItems, loadChildren]);

  // Toggle folder expansion
  const toggleExpand = useCallback((path: string) => {
    setExpansion(prev => {
      const next = new Set(prev.paths);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
        if (!childItems.has(path)) {
          loadChildren(path);
        }
      }
      return { ...prev, paths: next };
    });
  }, [childItems, loadChildren]);

  // Handle item click
  const handleItemClick = useCallback((item: FileItem) => {
    if (item.is_directory) {
      toggleExpand(item.path);
    } else if (item.is_text_file) {
      onFileSelect(item.path, rootPath);
    }
  }, [toggleExpand, onFileSelect, rootPath]);

  // Render a file/folder item
  const renderItem = (item: FileItem, depth: number = 0) => {
    const isExpanded = expandedPaths.has(item.path);
    const isLoadingChildren = loadingPaths.has(item.path);
    const children = childItems.get(item.path) || [];
    const isDisabled = !item.is_directory && !item.is_text_file;
    const isActive = activeFile === item.path;

    return (
      <div key={item.path}>
        <div
          className={[
            'ft-item',
            isDisabled && 'ft-item--disabled',
            isActive && 'ft-item--active',
          ].filter(Boolean).join(' ')}
          style={{ paddingLeft: 12 + depth * 16 }}
          onClick={() => !isDisabled && handleItemClick(item)}
          role="button"
          tabIndex={isDisabled ? -1 : 0}
          title={isDisabled ? 'Non-text file' : item.path}
        >
          {item.is_directory && (
            <span className="ft-expand-icon">
              {isLoadingChildren ? (
                <Loader2 size={14} className="spinning" />
              ) : isExpanded ? (
                <ChevronDown size={14} />
              ) : (
                <ChevronRight size={14} />
              )}
            </span>
          )}
          {!item.is_directory && <span className="ft-indent-spacer" />}
          <FileIcon type={item.file_type} isExpanded={isExpanded} />
          <span className="ft-name">{item.name}</span>
        </div>
        {item.is_directory && isExpanded && (
          <div className="ft-children">
            {isLoadingChildren && children.length === 0 ? (
              <div className="ft-loading" style={{ paddingLeft: 28 + depth * 16 }}>
                <Loader2 size={14} className="spinning" /> Loading...
              </div>
            ) : children.length === 0 ? (
              <div className="ft-empty" style={{ paddingLeft: 28 + depth * 16 }}>
                Empty
              </div>
            ) : (
              children.map(child => renderItem(child, depth + 1))
            )}
          </div>
        )}
      </div>
    );
  };

  if (loading) {
    return (
      <div className="ft-status">
        <Loader2 size={20} className="spinning" />
        <span>Loading...</span>
      </div>
    );
  }

  if (error) {
    return (
      <div className="ft-status ft-status--error">
        <AlertCircle size={20} />
        <span>{error}</span>
      </div>
    );
  }

  if (items.length === 0) {
    return (
      <div className="ft-status">
        <Folder size={20} />
        <span>Empty directory</span>
      </div>
    );
  }

  return (
    <div className="ft-root">
      {items.map(item => renderItem(item, 0))}
    </div>
  );
}
