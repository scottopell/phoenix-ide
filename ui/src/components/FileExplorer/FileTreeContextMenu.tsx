import { useState, useEffect, useCallback, useRef } from 'react';
import { copyToClipboard } from '../../utils/clipboard';
import {
  FILE_PATH_CONTEXT_MENU_OPEN_EVENT,
  MESSAGE_CONTEXT_MENU_OPEN_EVENT,
  FILE_TREE_CONTEXT_MENU_OPEN_EVENT,
} from '../contextMenuEvents';
import '../MessageContextMenu.css';

interface MenuState {
  x: number;
  y: number;
  absolutePath: string;
  relativePath: string;
  isDirectory: boolean;
  isText: boolean;
}

function computeRelativePath(rootPath: string, absolutePath: string): string {
  const root = rootPath.endsWith('/') ? rootPath.slice(0, -1) : rootPath;
  const prefix = root + '/';
  if (absolutePath.startsWith(prefix)) return absolutePath.slice(prefix.length);
  if (absolutePath === root) return '.';
  return absolutePath;
}

export function FileTreeContextMenu() {
  const [menu, setMenu] = useState<MenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const handleContextMenu = useCallback((e: MouseEvent) => {
    if (e.shiftKey) return;

    const target = e.target as HTMLElement | null;
    if (!target) return;

    // Only handle right-clicks within the file tree container
    const treeContainer = target.closest('.ft-root') as HTMLElement | null;
    if (!treeContainer) return;

    const itemEl = target.closest('.ft-item') as HTMLElement | null;
    if (!itemEl) return;

    const absolutePath = itemEl.dataset['path'];
    if (!absolutePath) return;

    const rootPath = treeContainer.dataset['rootPath'] || '';
    const isDirectory = itemEl.dataset['isDirectory'] === 'true';
    const isText = itemEl.dataset['isText'] === 'true';

    e.preventDefault();
    e.stopPropagation();
    window.dispatchEvent(new Event(FILE_TREE_CONTEXT_MENU_OPEN_EVENT));
    setMenu({
      x: e.clientX,
      y: e.clientY,
      absolutePath,
      relativePath: computeRelativePath(rootPath, absolutePath),
      isDirectory,
      isText,
    });
  }, []);

  useEffect(() => {
    document.addEventListener('contextmenu', handleContextMenu, { capture: true });
    return () => document.removeEventListener('contextmenu', handleContextMenu, { capture: true });
  }, [handleContextMenu]);

  // Close when other context menus open
  useEffect(() => {
    const closeMenu = () => setMenu(null);
    window.addEventListener(MESSAGE_CONTEXT_MENU_OPEN_EVENT, closeMenu);
    window.addEventListener(FILE_PATH_CONTEXT_MENU_OPEN_EVENT, closeMenu);
    return () => {
      window.removeEventListener(MESSAGE_CONTEXT_MENU_OPEN_EVENT, closeMenu);
      window.removeEventListener(FILE_PATH_CONTEXT_MENU_OPEN_EVENT, closeMenu);
    };
  }, []);

  // Close on click outside or Escape
  useEffect(() => {
    if (!menu) return;

    const handleClick = () => setMenu(null);
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setMenu(null);
    };
    const timer = setTimeout(() => {
      document.addEventListener('mousedown', handleClick);
      document.addEventListener('keydown', handleKey);
    }, 0);

    return () => {
      clearTimeout(timer);
      document.removeEventListener('mousedown', handleClick);
      document.removeEventListener('keydown', handleKey);
    };
  }, [menu]);

  // Clamp menu position to viewport
  useEffect(() => {
    if (!menu || !menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    let { x, y } = menu;
    let clamped = false;
    if (rect.right > window.innerWidth) {
      x = window.innerWidth - rect.width - 8;
      clamped = true;
    }
    if (rect.bottom > window.innerHeight) {
      y = window.innerHeight - rect.height - 8;
      clamped = true;
    }
    if (clamped) setMenu({ ...menu, x, y });
  }, [menu]);

  if (!menu) return null;

  const insertDraft = (text: string) => {
    window.dispatchEvent(new CustomEvent('phoenix:insert-draft', { detail: { text } }));
    setMenu(null);
  };

  const copyAbsolute = () => {
    void copyToClipboard(menu.absolutePath);
    setMenu(null);
  };

  const copyRelative = () => {
    void copyToClipboard(menu.relativePath);
    setMenu(null);
  };

  return (
    <div
      ref={menuRef}
      className="msg-context-menu file-tree-context-menu"
      style={{ left: menu.x, top: menu.y }}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <button className="msg-context-item" onClick={copyRelative}>
        Copy relative path
      </button>
      <button className="msg-context-item" onClick={copyAbsolute}>
        Copy absolute path
      </button>
      <div className="msg-context-divider" />
      {menu.isText && (
        <button className="msg-context-item" onClick={() => insertDraft(`@${menu.relativePath} `)}>
          Insert @file reference
        </button>
      )}
      <button className="msg-context-item" onClick={() => insertDraft(`./${menu.relativePath} `)}>
        Insert ./path reference
      </button>
    </div>
  );
}
