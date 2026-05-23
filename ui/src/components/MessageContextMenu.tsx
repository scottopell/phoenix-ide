import { useState, useEffect, useCallback, useRef } from 'react';
import type { Message, ContentBlock } from '../api';
import { copyToClipboard } from '../utils/clipboard';
import './MessageContextMenu.css';

interface ToolContext {
  command?: string;
  output?: string;
}

interface MenuState {
  x: number;
  y: number;
  message: Message;
  toolContext?: ToolContext;
}

interface MessageContextMenuProps {
  messages: Message[];
}

/** Extract raw markdown text from a message's content */
function getRawMarkdown(message: Message): string {
  const type = message.message_type || (message as unknown as Record<string, unknown>)['type'];

  if (type === 'user') {
    const content = message.content as { text?: string };
    return content.text || (typeof message.content === 'string' ? message.content as string : '');
  }

  if (type === 'agent') {
    const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
    return blocks
      .filter((b) => b.type === 'text' && b.text)
      .map((b) => b.text!)
      .join('\n\n');
  }

  return '';
}

/** Extract plain text (strip markdown) by reading innerText from the DOM */
function getPlainText(element: HTMLElement): string {
  const content = element.querySelector('.message-content');
  return content?.textContent || '';
}

export function MessageContextMenu({ messages }: MessageContextMenuProps) {
  const [menu, setMenu] = useState<MenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const handleContextMenu = useCallback(
    (e: MouseEvent) => {
      // Walk up from target to find .message element
      let el = e.target as HTMLElement | null;

      // Detect tool context while walking up
      let toolContext: ToolContext | undefined;
      let walkEl = e.target as HTMLElement | null;
      while (walkEl && !walkEl.classList.contains('message')) {
        // Check for tool block input (command)
        if (walkEl.classList.contains('tool-block-input')) {
          toolContext = { ...toolContext, command: walkEl.textContent?.trim() ?? '' };
        }
        // Check for tool block output
        if (walkEl.classList.contains('tool-block-output-content') || walkEl.classList.contains('tool-block-output')) {
          const outputContent = walkEl.querySelector('.tool-block-output-content');
          toolContext = { ...toolContext, output: (outputContent ?? walkEl).textContent?.trim() ?? '' };
        }
        // Check for the tool block itself (grab both if not yet found)
        if (walkEl.classList.contains('tool-block') && !toolContext) {
          const input = walkEl.querySelector('.tool-block-input');
          const output = walkEl.querySelector('.tool-block-output-content');
          if (input || output) {
            const ctx: ToolContext = {};
            if (input?.textContent) ctx.command = input.textContent.trim();
            if (output?.textContent) ctx.output = output.textContent.trim();
            toolContext = ctx;
          }
        }
        walkEl = walkEl.parentElement;
      }

      while (el && !el.classList.contains('message')) {
        el = el.parentElement;
      }
      if (!el) return; // Not on a message

      const seqId = el.dataset['sequenceId'];
      if (!seqId) return;

      const msg = messages.find((m) => String(m.sequence_id) === seqId);
      if (!msg) return;

      e.preventDefault();
      const newMenu: MenuState = { x: e.clientX, y: e.clientY, message: msg };
      if (toolContext) newMenu.toolContext = toolContext;
      setMenu(newMenu);
    },
    [messages]
  );

  // Close on click outside or Escape
  useEffect(() => {
    if (!menu) return;

    const handleClick = () => setMenu(null);
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setMenu(null);
    };
    // Use setTimeout so the current right-click doesn't immediately close
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

  // Attach context menu listener to #messages container
  useEffect(() => {
    const container = document.getElementById('messages');
    if (!container) return;
    container.addEventListener('contextmenu', handleContextMenu);
    return () => container.removeEventListener('contextmenu', handleContextMenu);
  }, [handleContextMenu]);

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

  const hasSelection = (window.getSelection()?.toString().length ?? 0) > 0;

  const copyMarkdown = () => {
    const md = getRawMarkdown(menu.message);
    void copyToClipboard(md);
    setMenu(null);
  };

  const copyPlainText = () => {
    const el = document.querySelector(
      `.message[data-sequence-id="${menu.message.sequence_id}"]`
    ) as HTMLElement | null;
    const text = el ? getPlainText(el) : getRawMarkdown(menu.message);
    void copyToClipboard(text);
    setMenu(null);
  };

  const copySelection = () => {
    const selection = window.getSelection()?.toString() || '';
    void copyToClipboard(selection);
    setMenu(null);
  };

  const selectAll = () => {
    const el = document.querySelector(
      `.message[data-sequence-id="${menu.message.sequence_id}"] .message-content`
    );
    if (el) {
      const range = document.createRange();
      range.selectNodeContents(el);
      const sel = window.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(range);
    }
    setMenu(null);
  };

  const copyCommand = () => {
    if (menu.toolContext?.command) {
      void copyToClipboard(menu.toolContext.command);
    }
    setMenu(null);
  };

  const copyOutput = () => {
    if (menu.toolContext?.output) {
      void copyToClipboard(menu.toolContext.output);
    }
    setMenu(null);
  };

  return (
    <div
      ref={menuRef}
      className="msg-context-menu"
      style={{ left: menu.x, top: menu.y }}
      onMouseDown={(e) => e.stopPropagation()}
    >
      {menu.toolContext?.command && (
        <button className="msg-context-item" onClick={copyCommand}>
          Copy command
        </button>
      )}
      {menu.toolContext?.output && (
        <button className="msg-context-item" onClick={copyOutput}>
          Copy output
        </button>
      )}
      {menu.toolContext && <div className="msg-context-divider" />}
      <button className="msg-context-item" onClick={copyMarkdown}>
        Copy as Markdown
      </button>
      <button className="msg-context-item" onClick={copyPlainText}>
        Copy as Plain Text
      </button>
      <div className="msg-context-divider" />
      {hasSelection && (
        <button className="msg-context-item" onClick={copySelection}>
          Copy Selection
        </button>
      )}
      <button className="msg-context-item" onClick={selectAll}>
        Select All
      </button>
    </div>
  );
}
