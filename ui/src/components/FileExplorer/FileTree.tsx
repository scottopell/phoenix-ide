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

import { memo, useState, useEffect, useRef, useCallback, useMemo } from 'react';
import {
  ChevronRight,
  ChevronDown,
  Loader2,
  AlertCircle,
} from 'lucide-react';
import { computeAncestors, isUnderRoot } from './computeAncestors';
import { useFocusScopeCommands } from '../../hooks/useFocusScope';
import type { FileViewerKind } from '../../generated/FileViewerKind';

/** Custom drag type for file-tree → composer drag-and-drop. The InputArea
 *  drop handler checks this before the OS `Files` type so the two drop
 *  modes don't conflict. */
export const FILE_TREE_DRAG_TYPE = 'application/x-phoenix-file-path';

// Types
export interface FileItem {
  name: string;
  path: string;
  is_directory: boolean;
  size?: number;
  modified_time?: number;
  /** Server's verdict on how the viewer treats this entry — the single source
   *  of openability, shared with quick-open and @-mention. */
  viewer: FileViewerKind;
  is_gitignored: boolean;
}

const EMPTY_FILE_ITEMS: FileItem[] = [];

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
// FileTreeItem — memoized per-node so only nodes with changed props re-render
// ============================================================================

interface FileTreeItemProps {
  item: FileItem;
  rootPath: string;
  depth: number;
  isExpanded: boolean;
  isLoadingChildren: boolean;
  isActive: boolean;
  visibleChildren: FileItem[];
  childrenByPath: Map<string, FileItem[]>;
  expandedPaths: Set<string>;
  loadingPaths: Set<string>;
  activeFile: string | null | undefined;
  onItemClick: (item: FileItem) => void;
}

const FileTreeItem = memo(function FileTreeItem({
  item,
  rootPath,
  depth,
  isExpanded,
  isLoadingChildren,
  isActive,
  visibleChildren,
  childrenByPath,
  expandedPaths,
  loadingPaths,
  activeFile,
  onItemClick,
}: FileTreeItemProps) {
  const isDisabled = !item.is_directory && item.viewer.kind === 'opaque';
  const className = [
    'ft-item',
    isDisabled && 'ft-item--disabled',
    isActive && 'ft-item--active',
    item.is_gitignored && 'ft-item--dimmed',
  ].filter(Boolean).join(' ');

  const handleDragStart = useCallback((e: React.DragEvent) => {
    // All items are draggable — opaque/binary files drag as ./path refs
    // (isText: false in the payload), even though they can't be opened in
    // the viewer. Only the click-to-open action is disabled for them.
    const root = rootPath.endsWith('/') ? rootPath.slice(0, -1) : rootPath;
    const prefix = root + '/';
    const relativePath = item.path.startsWith(prefix) ? item.path.slice(prefix.length) : item.path;
    e.dataTransfer.setData(FILE_TREE_DRAG_TYPE, JSON.stringify({
      path: item.path,
      relativePath,
      isDirectory: item.is_directory,
      isText: item.viewer.kind === 'text',
    }));
    e.dataTransfer.effectAllowed = 'copy';
  }, [item.path, item.is_directory, item.viewer.kind, rootPath]);

  return (
    <div>
      <div
        className={className}
        style={{ paddingLeft: 12 + depth * 16 }}
        onClick={() => !isDisabled && onItemClick(item)}
        role="button"
        tabIndex={isDisabled ? -1 : 0}
        title={isDisabled ? 'Non-viewable file' : item.path}
        data-path={item.path}
        data-is-directory={item.is_directory ? 'true' : 'false'}
        data-is-text={(!item.is_directory && item.viewer.kind === 'text') ? 'true' : 'false'}
        aria-expanded={item.is_directory ? isExpanded : undefined}
        draggable
        onDragStart={handleDragStart}
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
              const childChildren = childrenByPath.get(child.path) ?? EMPTY_FILE_ITEMS;
              const childActive = activeFile === child.path;
              return (
                <FileTreeItem
                  key={child.path}
                  item={child}
                  rootPath={rootPath}
                  depth={depth + 1}
                  isExpanded={childExpanded}
                  isLoadingChildren={childLoading}
                  isActive={childActive}
                  visibleChildren={childChildren}
                  childrenByPath={childrenByPath}
                  expandedPaths={expandedPaths}
                  loadingPaths={loadingPaths}
                  activeFile={activeFile}
                  onItemClick={onItemClick}
                />
              );
            })
          )}
        </div>
      )}
    </div>
  );
}, areFileTreeItemPropsEqual);

function areFileTreeItemPropsEqual(prev: FileTreeItemProps, next: FileTreeItemProps): boolean {
  if (
    prev.item !== next.item ||
    prev.rootPath !== next.rootPath ||
    prev.depth !== next.depth ||
    prev.isExpanded !== next.isExpanded ||
    prev.isLoadingChildren !== next.isLoadingChildren ||
    prev.isActive !== next.isActive ||
    prev.visibleChildren !== next.visibleChildren ||
    prev.onItemClick !== next.onItemClick
  ) {
    return false;
  }

  if (!next.item.is_directory || !next.isExpanded) return true;

  return !hasVisibleSubtreeStateChange(prev, next, next.visibleChildren);
}

function hasVisibleSubtreeStateChange(
  prev: FileTreeItemProps,
  next: FileTreeItemProps,
  visibleChildren: FileItem[],
): boolean {
  for (const child of visibleChildren) {
    const wasExpanded = prev.expandedPaths.has(child.path);
    const isExpanded = next.expandedPaths.has(child.path);
    if (wasExpanded !== isExpanded) return true;
    if (prev.loadingPaths.has(child.path) !== next.loadingPaths.has(child.path)) return true;
    if ((prev.activeFile === child.path) !== (next.activeFile === child.path)) return true;

    const prevChildren = prev.childrenByPath.get(child.path) ?? EMPTY_FILE_ITEMS;
    const nextChildren = next.childrenByPath.get(child.path) ?? EMPTY_FILE_ITEMS;
    if (prevChildren !== nextChildren) return true;

    if ((wasExpanded || isExpanded) && hasVisibleSubtreeStateChange(prev, next, nextChildren)) {
      return true;
    }
  }

  return false;
}

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

  // Focus scope: while a tree item has keyboard focus, push a scope so the
  // sidebar's useKeyboardNav defers (REQ-KB-001 / REQ-KB-008). Unlike modal
  // panels that register on mount, the tree is persistent — so we push/pop
  // based on whether any item is focused.
  const { pushScope, popScope } = useFocusScopeCommands();
  const [treeFocused, setTreeFocused] = useState(false);
  useEffect(() => {
    if (!treeFocused) return;
    pushScope('file-tree');
    return () => popScope('file-tree');
  }, [treeFocused, pushScope, popScope]);

  const handleTreeFocus = useCallback((e: React.FocusEvent<HTMLDivElement>) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
      setTreeFocused(true);
    }
  }, []);
  const handleTreeBlur = useCallback((e: React.FocusEvent<HTMLDivElement>) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
      setTreeFocused(false);
    }
  }, []);

  // Keyboard navigation: move focus between visible tree items (REQ-KB-003).
  // All .ft-item elements in the DOM are visible — collapsed directories
  // don't render their children.
  const handleTreeKeyDown = useCallback((e: React.KeyboardEvent<HTMLDivElement>) => {
    const root = treeRootRef.current;
    if (!root) return;
    // Filter out disabled rows (opaque/non-viewable files) — they have
    // tabIndex={-1} and click disabled, so they shouldn't receive keyboard
    // focus via arrow navigation either.
    const allItems = Array.from(root.querySelectorAll<HTMLElement>('.ft-item'))
      .filter(el => !el.classList.contains('ft-item--disabled'));
    if (allItems.length === 0) return;
    const currentIndex = allItems.findIndex(el => el === document.activeElement);

    switch (e.key) {
      case 'ArrowDown': {
        e.preventDefault();
        e.stopPropagation();
        const next = currentIndex >= 0 && currentIndex < allItems.length - 1
          ? allItems[currentIndex + 1]!
          : allItems[0]!;
        next.focus();
        break;
      }
      case 'ArrowUp': {
        e.preventDefault();
        e.stopPropagation();
        const prev = currentIndex > 0
          ? allItems[currentIndex - 1]!
          : allItems[allItems.length - 1]!;
        prev.focus();
        break;
      }
      case 'Home': {
        e.preventDefault();
        e.stopPropagation();
        allItems[0]!.focus();
        break;
      }
      case 'End': {
        e.preventDefault();
        e.stopPropagation();
        allItems[allItems.length - 1]!.focus();
        break;
      }
      case 'Enter':
      case ' ': {
        if (currentIndex >= 0) {
          e.preventDefault();
          e.stopPropagation();
          allItems[currentIndex]!.click();
        }
        break;
      }
      case 'ArrowRight': {
        if (currentIndex < 0) return;
        const el = allItems[currentIndex]!;
        if (el.dataset['isDirectory'] !== 'true') return;
        const isExpanded = el.getAttribute('aria-expanded') === 'true';
        if (!isExpanded) {
          e.preventDefault();
          e.stopPropagation();
          el.click();
        } else {
          // Move to first child — but only if the next row is actually
          // deeper (a child). If the directory is expanded but has no
          // rendered children (empty, dotfiles only, still loading), the
          // next row is a sibling, not a child, so stay put.
          e.preventDefault();
          e.stopPropagation();
          if (currentIndex < allItems.length - 1) {
            const currentDepth = parseInt(el.style.paddingLeft || '0', 10);
            const nextDepth = parseInt(allItems[currentIndex + 1]!.style.paddingLeft || '0', 10);
            if (nextDepth > currentDepth) {
              allItems[currentIndex + 1]!.focus();
            }
          }
        }
        break;
      }
      case 'ArrowLeft': {
        if (currentIndex < 0) return;
        const el = allItems[currentIndex]!;
        if (el.dataset['isDirectory'] === 'true') {
          const isExpanded = el.getAttribute('aria-expanded') === 'true';
          if (isExpanded) {
            e.preventDefault();
            e.stopPropagation();
            el.click();
            break;
          }
        }
        // Move to parent directory (the nearest preceding item at a lower depth)
        e.preventDefault();
        e.stopPropagation();
        const currentDepth = parseInt(el.style.paddingLeft || '0', 10);
        for (let i = currentIndex - 1; i >= 0; i--) {
          const prevDepth = parseInt(allItems[i]!.style.paddingLeft || '0', 10);
          if (prevDepth < currentDepth) {
            allItems[i]!.focus();
            break;
          }
        }
        break;
      }
      case 'Escape': {
        // If the file-tree context menu is open, let the event propagate so
        // the menu's document-level Escape listener can close it.
        if (document.querySelector('.file-tree-context-menu')) return;
        e.preventDefault();
        e.stopPropagation();
        (document.activeElement as HTMLElement | null)?.blur();
        break;
      }
    }
  }, []);

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
  //
  // The `cancelled` flag prevents a timer leak: without it, an async callback
  // that is mid-execution when the cleanup runs would call `scheduleRefresh()`
  // and create a new timer that the cleanup can't cancel — an infinite chain
  // of leaked timers that survive unmount and eventually exhaust the heap.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    function scheduleRefresh() {
      const jitter = Math.random() * 4000 - 2000; // +/- 2s
      timer = setTimeout(async () => {
        if (cancelled) return;
        if (document.visibilityState === 'visible') {
          try {
            const result = await listFiles(rootPath);
            if (cancelled) return;
            setItems(prev => {
              if (fingerprintFiles(prev) === fingerprintFiles(result)) {
                return prev; // unchanged — skip re-render
              }
              return result;
            });
          } catch { /* silent -- next tick will retry */ }
        }
        if (cancelled) return;
        scheduleRefresh();
      }, 10000 + jitter);
    }
    scheduleRefresh();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
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
    } else if (item.viewer.kind !== 'opaque') {
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

  const childrenByPath = useMemo(() => {
    const next = new Map<string, FileItem[]>();
    for (const [path, children] of childItems) {
      next.set(path, children.filter(child => !child.name.startsWith('.')));
    }
    return next;
  }, [childItems]);

  // Compact display: last two path segments or ~/dir
  const dirLabel = useMemo(() => computeDirLabel(rootPath), [rootPath]);


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
    <div
      className="ft-root"
      ref={treeRootRef}
      data-root-path={rootPath}
      tabIndex={-1}
      onFocus={handleTreeFocus}
      onBlur={handleTreeBlur}
      onKeyDown={handleTreeKeyDown}
    >
      <div className="ft-dir-label" title={rootPath}>{dirLabel}</div>
      {visibleItems.map(item => {
        const isExpanded = expandedPaths.has(item.path);
        const isLoadingChildren = loadingPaths.has(item.path);
        const visibleChildren = childrenByPath.get(item.path) ?? EMPTY_FILE_ITEMS;
        const isActive = activeFile === item.path;
        return (
          <FileTreeItem
            key={item.path}
            item={item}
            rootPath={rootPath}
            depth={0}
            isExpanded={isExpanded}
            isLoadingChildren={isLoadingChildren}
            isActive={isActive}
            visibleChildren={visibleChildren}
            childrenByPath={childrenByPath}
            expandedPaths={expandedPaths}
            loadingPaths={loadingPaths}
            activeFile={activeFile}
            onItemClick={handleItemClick}
          />
        );
      })}
    </div>
  );
}
