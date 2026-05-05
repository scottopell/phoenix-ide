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

import { memo, useState, useEffect, useRef, useCallback, useMemo, createContext, useContext } from 'react';
import {
  ChevronRight,
  ChevronDown,
  Loader2,
  AlertCircle,
} from 'lucide-react';
import { computeAncestors, isUnderRoot } from './computeAncestors';

// Types
export interface FileItem {
  name: string;
  path: string;
  is_directory: boolean;
  size?: number;
  modified_time?: number;
  file_type: 'folder' | 'markdown' | 'code' | 'config' | 'text' | 'image' | 'data' | 'unknown';
  is_text_file: boolean;
  is_gitignored: boolean;
}

interface FileTreeProps {
  rootPath: string;
  onFileSelect: (filePath: string, rootDir: string) => void;
  activeFile?: string | null | undefined;
  conversationId?: string | undefined;
  refreshKey?: number;
}

function extensionColor(name: string): string | undefined {
  const ext = name.split('.').pop()?.toLowerCase();
  switch (ext) {
    case 'rs': return 'var(--accent-orange, #e8863a)';
    case 'ts': case 'tsx': return 'var(--accent-blue, #5c9fd6)';
    case 'js': case 'jsx': return 'var(--accent-yellow, #d4b84b)';
    case 'py': return 'var(--accent-green, #6ab04c)';
    case 'md': case 'txt': return 'var(--text-muted)';
    case 'json': case 'toml': case 'yaml': case 'yml': return 'var(--accent-yellow, #d4b84b)';
    case 'css': return 'var(--accent-purple, #c678dd)';
    case 'html': return 'var(--accent-red, #e06c75)';
    case 'sh': case 'bash': return 'var(--accent-green, #6ab04c)';
    case 'sql': return 'var(--accent-blue, #61afef)';
    case 'lock': return 'var(--text-muted)';
    default: return undefined;
  }
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

/**
 * Cheap fingerprint for a FileItem[]: concatenates name + modified_time per
 * item. Two arrays with the same fingerprint are treated as equal for the
 * purpose of the 10s auto-refresh loop — in that case we skip `setItems` so
 * the whole tree doesn't re-render.
 *
 * This is a hash only in spirit; collisions are harmless because the worst
 * outcome is one skipped re-render until the next tick.
 */
function fingerprintFiles(items: FileItem[]): string {
  const parts: string[] = [];
  for (const it of items) {
    parts.push(it.name);
    parts.push(String(it.modified_time ?? 0));
    parts.push(it.is_directory ? 'd' : 'f');
  }
  return parts.join('|');
}

function computeDirLabel(rootPath: string): string {
  const home = '/Users/';
  if (rootPath.startsWith(home)) {
    const rest = rootPath.slice(home.length);
    const parts = rest.split('/').filter(Boolean);
    if (parts.length <= 2) return '~/' + parts.join('/');
    return '.../' + parts.slice(-2).join('/');
  }
  const parts = rootPath.split('/').filter(Boolean);
  if (parts.length <= 2) return '/' + parts.join('/');
  return '.../' + parts.slice(-2).join('/');
}

// ============================================================================
// Shared context for identity-unstable collections (not passed as props so
// they don't defeat React.memo).
// ============================================================================

interface TreeCollections {
  childItems: Map<string, FileItem[]>;
  expandedPaths: Set<string>;
  loadingPaths: Set<string>;
  activeFile: string | null | undefined;
}

const TreeCollectionsCtx = createContext<TreeCollections>({
  childItems: new Map(),
  expandedPaths: new Set(),
  loadingPaths: new Set(),
  activeFile: null,
});

// ============================================================================
// FileTreeItem — memoized per-node so only nodes with changed props re-render
// ============================================================================

interface FileTreeItemProps {
  item: FileItem;
  depth: number;
  isExpanded: boolean;
  isLoadingChildren: boolean;
  isActive: boolean;
  visibleChildren: FileItem[];
  onItemClick: (item: FileItem) => void;
}

const FileTreeItem = memo(function FileTreeItem({
  item,
  depth,
  isExpanded,
  isLoadingChildren,
  isActive,
  visibleChildren,
  onItemClick,
}: FileTreeItemProps) {
  const { childItems, expandedPaths, loadingPaths, activeFile } = useContext(TreeCollectionsCtx);
  const isDisabled = !item.is_directory && !item.is_text_file;
  const className = [
    'ft-item',
    isDisabled && 'ft-item--disabled',
    isActive && 'ft-item--active',
    item.is_gitignored && 'ft-item--dimmed',
  ].filter(Boolean).join(' ');

  return (
    <div>
      <div
        className={className}
        style={{ paddingLeft: 12 + depth * 16 }}
        onClick={() => !isDisabled && onItemClick(item)}
        role="button"
        tabIndex={isDisabled ? -1 : 0}
        title={isDisabled ? 'Non-text file' : item.path}
        data-path={item.path}
      >
        {item.is_directory && (
          <span className="ft-expand-icon">
            {isLoadingChildren ? (
              <Loader2 size={12} className="spinning" />
            ) : isExpanded ? (
              <ChevronDown size={12} />
            ) : (
              <ChevronRight size={12} />
            )}
          </span>
        )}
        {!item.is_directory && <span className="ft-indent-spacer" />}
        {!item.is_directory && (
          <span className="ft-dot" style={{ color: extensionColor(item.name) || 'var(--text-muted)' }}>
            &#8226;
          </span>
        )}
        <span className={`ft-name ${item.is_directory ? 'ft-name--folder' : ''}`}>{item.name}</span>
      </div>
      {item.is_directory && isExpanded && (
        <div className="ft-children">
          {isLoadingChildren && visibleChildren.length === 0 ? (
            <div className="ft-loading" style={{ paddingLeft: 28 + depth * 16 }}>
              <Loader2 size={14} className="spinning" /> Loading...
            </div>
          ) : visibleChildren.length === 0 ? (
            <div className="ft-empty" style={{ paddingLeft: 28 + depth * 16 }}>
              Empty
            </div>
          ) : (
            visibleChildren.map((child) => {
              const childExpanded = expandedPaths.has(child.path);
              const childLoading = loadingPaths.has(child.path);
              const childChildren = (childItems.get(child.path) || []).filter(c => !c.name.startsWith('.'));
              const childActive = activeFile === child.path;
              return (
                <FileTreeItem
                  key={child.path}
                  item={child}
                  depth={depth + 1}
                  isExpanded={childExpanded}
                  isLoadingChildren={childLoading}
                  isActive={childActive}
                  visibleChildren={childChildren}
                  onItemClick={onItemClick}
                />
              );
            })
          )}
        </div>
      )}
    </div>
  );
});

// ============================================================================
// FileTree
// ============================================================================

export function FileTree({ rootPath, onFileSelect, activeFile, conversationId, refreshKey }: FileTreeProps) {
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
  const treeRootRef = useRef<HTMLDivElement | null>(null);
  // Tracks the activeFile we last successfully scrolled to, so an updated
  // childItems map (from any later directory load) doesn't yank scroll
  // position back to the same row over and over.
  const lastRevealedRef = useRef<string | null>(null);

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

  // Reveal-on-active-file: when activeFile changes, expand every ancestor
  // directory between rootPath and the file. The existing
  // expanded-but-not-loaded effect below picks up the merged paths and
  // fetches their children, so the row eventually materializes in the DOM
  // and the scroll effect (further down) brings it into view.
  //
  // Keyed on [activeFile, rootPath] only — NOT on expandedPaths — so a user
  // who manually collapses an ancestor of the current activeFile doesn't get
  // their collapse undone on the next re-render.
  useEffect(() => {
    if (!activeFile) return;
    // Out-of-root activeFile (cwd mismatch / cross-tree open): there's nothing
    // to scroll to in this tree. Mark it as already-revealed so the scroll
    // effect's guard short-circuits and we don't burn a querySelector on
    // every subsequent childItems update.
    if (!isUnderRoot(rootPath, activeFile)) {
      lastRevealedRef.current = activeFile;
      return;
    }
    // In-root: a fresh activeFile means the scroll effect should re-attempt
    // until the row appears in the DOM.
    lastRevealedRef.current = null;
    const ancestors = computeAncestors(rootPath, activeFile);
    if (ancestors.length === 0) return; // file directly at root — no ancestors
    setExpansion(prev => {
      let changed = false;
      const next = new Set(prev.paths);
      for (const a of ancestors) {
        if (!next.has(a)) {
          next.add(a);
          changed = true;
        }
      }
      return changed ? { ...prev, paths: next } : prev;
    });
  }, [activeFile, rootPath]);

  // Scroll-into-view: try to bring the active row on-screen. Re-runs whenever
  // childItems changes because a freshly-loaded directory may finally include
  // the active row in the DOM. The lastRevealedRef guard makes this a one-shot
  // per activeFile, so subsequent unrelated childItems updates don't re-scroll
  // (and out-of-root activeFiles are short-circuited by the reveal effect
  // above setting lastRevealedRef directly).
  useEffect(() => {
    if (!activeFile) return;
    if (lastRevealedRef.current === activeFile) return;
    const root = treeRootRef.current;
    if (!root) return;
    // CSS.escape handles paths with quotes, backslashes, or any other char
    // that would break a raw attribute-selector string. Wrap in try/catch as
    // a belt-and-suspenders guard against any environment without CSS.escape.
    let el: HTMLElement | null = null;
    try {
      const selector = `[data-path="${CSS.escape(activeFile)}"]`;
      el = root.querySelector<HTMLElement>(selector);
    } catch {
      return;
    }
    if (!el) return;
    el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    lastRevealedRef.current = activeFile;
  }, [activeFile, childItems]);

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
  }, [rootPath, refreshKey]);

  // Auto-refresh every ~10s while page is visible. Only `setItems` if the
  // fingerprint changes — otherwise a tree of unchanged files would re-render
  // the entire subtree every 10 seconds for no reason.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    function scheduleRefresh() {
      const jitter = Math.random() * 4000 - 2000; // +/- 2s
      timer = setTimeout(async () => {
        if (document.visibilityState === 'visible') {
          try {
            const result = await listFiles(rootPath);
            setItems(prev => {
              if (fingerprintFiles(prev) === fingerprintFiles(result)) {
                return prev; // unchanged — skip re-render
              }
              return result;
            });
          } catch { /* silent -- next tick will retry */ }
        }
        scheduleRefresh();
      }, 10000 + jitter);
    }
    scheduleRefresh();
    return () => clearTimeout(timer);
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

  // Filter out dotfiles/directories at root level by default — memoized so the
  // reference is stable as long as `items` is (which, with the fingerprint
  // check above, now really means "stable unless the directory content
  // actually changed").
  const visibleItems = useMemo(
    () => items.filter(item => !item.name.startsWith('.')),
    [items]
  );

  // Compact display: last two path segments or ~/dir
  const dirLabel = useMemo(() => computeDirLabel(rootPath), [rootPath]);

  const treeCollections = useMemo<TreeCollections>(
    () => ({ childItems, expandedPaths, loadingPaths, activeFile }),
    [childItems, expandedPaths, loadingPaths, activeFile],
  );

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
        <span>Empty directory</span>
      </div>
    );
  }

  return (
    <TreeCollectionsCtx.Provider value={treeCollections}>
      <div className="ft-root" ref={treeRootRef}>
        <div className="ft-dir-label" title={rootPath}>{dirLabel}</div>
        {visibleItems.map(item => {
          const isExpanded = expandedPaths.has(item.path);
          const isLoadingChildren = loadingPaths.has(item.path);
          const visibleChildren = (childItems.get(item.path) || []).filter(c => !c.name.startsWith('.'));
          const isActive = activeFile === item.path;
          return (
            <FileTreeItem
              key={item.path}
              item={item}
              depth={0}
              isExpanded={isExpanded}
              isLoadingChildren={isLoadingChildren}
              isActive={isActive}
              visibleChildren={visibleChildren}
              onItemClick={handleItemClick}
            />
          );
        })}
      </div>
    </TreeCollectionsCtx.Provider>
  );
}
