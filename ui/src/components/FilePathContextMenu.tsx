import { useState, useEffect, useCallback, useRef } from 'react';
import { copyToClipboard } from '../utils/clipboard';
import {
  FILE_PATH_CONTEXT_MENU_OPEN_EVENT,
  MESSAGE_CONTEXT_MENU_OPEN_EVENT,
} from './contextMenuEvents';
import './MessageContextMenu.css';

interface MenuState {
  x: number;
  y: number;
  absolutePath: string;
  relativePath: string;
}

export function FilePathContextMenu() {
  const [menu, setMenu] = useState<MenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const handleContextMenu = useCallback((e: MouseEvent) => {
    if (e.shiftKey) return;

    const target = e.target as HTMLElement | null;
    const messagesContainer = document.getElementById('messages');
    if (!target || !messagesContainer?.contains(target)) return;

    const filePathEl = target.closest('.file-path-link') as HTMLElement | null;
    if (!filePathEl) return;

    const absolutePath = filePathEl.dataset['fileAbsolutePath'];
    const relativePath = filePathEl.dataset['fileRelativePath'];
    if (!absolutePath || !relativePath) return;

    e.preventDefault();
    e.stopPropagation();
    window.dispatchEvent(new Event(FILE_PATH_CONTEXT_MENU_OPEN_EVENT));
    setMenu({ x: e.clientX, y: e.clientY, absolutePath, relativePath });
  }, []);

  useEffect(() => {
    document.addEventListener('contextmenu', handleContextMenu, { capture: true });
    return () => document.removeEventListener('contextmenu', handleContextMenu, { capture: true });
  }, [handleContextMenu]);

  useEffect(() => {
    const closeMenu = () => setMenu(null);
    window.addEventListener(MESSAGE_CONTEXT_MENU_OPEN_EVENT, closeMenu);
    return () => window.removeEventListener(MESSAGE_CONTEXT_MENU_OPEN_EVENT, closeMenu);
  }, []);

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

  const copyAbsolutePath = () => {
    void copyToClipboard(menu.absolutePath);
    setMenu(null);
  };

  const copyRelativePath = () => {
    void copyToClipboard(menu.relativePath);
    setMenu(null);
  };

  return (
    <div
      ref={menuRef}
      className="msg-context-menu file-path-context-menu"
      style={{ left: menu.x, top: menu.y }}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <button className="msg-context-item" onClick={copyAbsolutePath}>
        Copy absolute path
      </button>
      <button className="msg-context-item" onClick={copyRelativePath}>
        Copy relative path
      </button>
    </div>
  );
}
