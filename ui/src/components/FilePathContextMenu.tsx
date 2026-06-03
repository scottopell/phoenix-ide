import { useState, useEffect, useCallback, useRef } from 'react';
import { copyToClipboard } from '../utils/clipboard';
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
    const filePathEl = target?.closest('.file-path-link') as HTMLElement | null;
    if (!filePathEl) return;

    const absolutePath = filePathEl.dataset['fileAbsolutePath'];
    const relativePath = filePathEl.dataset['fileRelativePath'];
    if (!absolutePath || !relativePath) return;

    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, absolutePath, relativePath });
  }, []);

  useEffect(() => {
    const container = document.getElementById('messages');
    if (!container) return;
    container.addEventListener('contextmenu', handleContextMenu, { capture: true });
    return () => container.removeEventListener('contextmenu', handleContextMenu, { capture: true });
  }, [handleContextMenu]);

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
