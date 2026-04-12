/**
 * TerminalPanel — PTY-backed terminal rendered via xterm.js.
 *
 * Connects to `GET /api/conversations/:id/terminal` over binary WebSocket.
 * Binary frame protocol:
 *   byte 0 = 0x00 → PTY data (bidirectional)
 *   byte 0 = 0x01 → resize: u16be cols, u16be rows (client → server)
 *
 * REQ-TERM-004, REQ-TERM-005, REQ-TERM-006
 */

import { useEffect, useRef, useCallback, useState } from 'react';
import { Terminal } from 'xterm';
import { FitAddon } from 'xterm-addon-fit';
import 'xterm/css/xterm.css';

interface TerminalPanelProps {
  conversationId: string;
  /** Total height in px (including header strip) */
  height: number;
  /** When true, only the header strip renders — no xterm */
  collapsed: boolean;
  /** Click on the header strip restores from collapsed */
  onExpand: () => void;
  /** Fallback prompt text when xterm buffer has no content yet */
  cwd?: string;
}

type ActivityState = 'idle' | 'running' | 'disconnected';

/** Build the WebSocket URL for a conversation's terminal endpoint. */
function terminalWsUrl(conversationId: string): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${window.location.host}/api/conversations/${conversationId}/terminal`;
}

/** Encode a resize frame: 0x01 + u16be cols + u16be rows */
function resizeFrame(cols: number, rows: number): Uint8Array {
  const buf = new Uint8Array(5);
  buf[0] = 0x01;
  new DataView(buf.buffer).setUint16(1, cols, false);  // big-endian
  new DataView(buf.buffer).setUint16(3, rows, false);
  return buf;
}

/** Encode a data frame: 0x00 + payload bytes */
function dataFrame(payload: Uint8Array): Uint8Array {
  const buf = new Uint8Array(1 + payload.length);
  buf[0] = 0x00;
  buf.set(payload, 1);
  return buf;
}

/** Truncate from the LEFT, preserving the tail (cwd + prompt glyph). */
function truncateLeft(s: string, max: number): string {
  if (s.length <= max) return s;
  return '…' + s.slice(s.length - (max - 1));
}

/** Format a cwd for fallback display — last 40 chars, replace $HOME with ~. */
function formatCwd(cwd: string): string {
  // No reliable $HOME in the browser; just truncate from the left.
  const trimmed = cwd.replace(/\/+$/, '');
  return truncateLeft(trimmed, 40) + ' ❯';
}

export function TerminalPanel({ conversationId, height, collapsed, onExpand, cwd }: TerminalPanelProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const statusRef = useRef<HTMLDivElement>(null);
  // Stable ref so long-lived listeners (window resize, height-change effect)
  // can check the current collapsed state without re-subscribing.
  const collapsedRef = useRef(collapsed);
  collapsedRef.current = collapsed;

  // HUD state: activity / unread counter / sampled prompt line
  const [activity, setActivity] = useState<ActivityState>('disconnected');
  const unreadRef = useRef<number>(0);
  const [unreadDisplay, setUnreadDisplay] = useState<number>(0);
  const [promptLine, setPromptLine] = useState<string>('');
  const activityTimeoutRef = useRef<number | null>(null);

  const setStatus = useCallback((msg: string) => {
    if (statusRef.current) statusRef.current.textContent = msg;
  }, []);

  // Mount xterm once for the lifetime of the conversation. The xterm container
  // is always rendered but hidden via `display: none` when the panel is
  // collapsed, preserving the WebSocket, PTY, scrollback, and any running
  // shell state across collapse/expand cycles. FitAddon is only invoked while
  // the panel is expanded (it throws on 0-height parents).
  useEffect(() => {
    if (!containerRef.current) return;

    // --- xterm.js setup ---
    const term = new Terminal({
      cursorBlink: true,
      theme: { background: '#1a1a1a', foreground: '#d4d4d4', cursor: '#d4d4d4' },
      fontFamily: '"SauceCodePro NF Mono", "Cascadia Code", "JetBrains Mono", "Fira Code", monospace',
      fontSize: 13,
      scrollback: 1000,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef.current);
    // Only fit immediately if the container is visible; otherwise the
    // height-change effect will run fit() when the panel expands.
    if (!collapsedRef.current) {
      try {
        fitAddon.fit();
      } catch {
        // ignore — deferred fit will retry
      }
    }
    termRef.current = term;
    fitAddonRef.current = fitAddon;

    // --- WebSocket connection ---
    const ws = new WebSocket(terminalWsUrl(conversationId));
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;
    setStatus('Connecting…');

    ws.onopen = () => {
      // REQ-TERM-005: send initial resize as first message so the server
      // knows dimensions before spawning the shell.
      const { cols, rows } = term;
      ws.send(resizeFrame(cols, rows));
      setStatus('');
      setActivity('idle');
    };

    ws.onmessage = (event: MessageEvent<ArrayBuffer>) => {
      const data = new Uint8Array(event.data);
      if (data.length === 0) return;
      if (data[0] === 0x00) {
        // PTY output → write to xterm.js
        const payload = data.slice(1);
        term.write(payload);
        // Count newlines while collapsed for the unread counter
        if (collapsedRef.current) {
          let n = 0;
          for (let i = 0; i < payload.length; i++) {
            if (payload[i] === 0x0a) n++;
          }
          if (n > 0) unreadRef.current += n;
        }
        // Flip to running and schedule decay back to idle after 500ms
        setActivity('running');
        if (activityTimeoutRef.current !== null) {
          window.clearTimeout(activityTimeoutRef.current);
        }
        activityTimeoutRef.current = window.setTimeout(() => {
          setActivity('idle');
          activityTimeoutRef.current = null;
        }, 500);
      }
      // 0x01 (resize) is only sent server→client as a future extension; ignore for now.
    };

    ws.onerror = () => {
      setStatus('Connection error');
      setActivity('disconnected');
    };
    ws.onclose = () => {
      setStatus('Terminal closed');
      setActivity('disconnected');
      term.write('\r\n\x1b[90m[Terminal disconnected]\x1b[0m\r\n');
    };

    // --- xterm.js → server (user keystrokes) ---
    const disposeOnData = term.onData((text) => {
      if (ws.readyState === WebSocket.OPEN) {
        const encoded = new TextEncoder().encode(text);
        ws.send(dataFrame(encoded));
      }
    });

    // --- Resize handling (REQ-TERM-006) ---
    // Skip fit() while collapsed — FitAddon throws on a `display: none` parent.
    // The next expand triggers a fit via the height-change effect below.
    const handleResize = () => {
      if (collapsedRef.current) return;
      try {
        fitAddon.fit();
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(resizeFrame(term.cols, term.rows));
        }
      } catch {
        // ignore — next resize will retry
      }
    };
    window.addEventListener('resize', handleResize);

    return () => {
      disposeOnData.dispose();
      window.removeEventListener('resize', handleResize);
      if (activityTimeoutRef.current !== null) {
        window.clearTimeout(activityTimeoutRef.current);
        activityTimeoutRef.current = null;
      }
      ws.close();
      term.dispose();
      termRef.current = null;
      fitAddonRef.current = null;
      wsRef.current = null;
    };
  }, [conversationId, setStatus]);

  // Refit when the parent height changes (drag-resize) — same effect path as
  // a window resize, but driven by the prop changing instead of a DOM event.
  useEffect(() => {
    if (collapsed) return;
    const fit = fitAddonRef.current;
    const term = termRef.current;
    const ws = wsRef.current;
    if (!fit || !term) return;
    // Defer one frame so the parent <div> has its new height applied.
    const id = requestAnimationFrame(() => {
      try {
        fit.fit();
        if (ws && ws.readyState === WebSocket.OPEN) {
          ws.send(resizeFrame(term.cols, term.rows));
        }
      } catch {
        // FitAddon throws if the container is 0×0; ignore — next height change retries.
      }
    });
    return () => cancelAnimationFrame(id);
  }, [height, collapsed]);

  // Reset unread counter when collapse flips true → false
  useEffect(() => {
    if (!collapsed) {
      unreadRef.current = 0;
      setUnreadDisplay(0);
    }
  }, [collapsed]);

  // Throttled flush of unread counter from ref to state (~200ms)
  useEffect(() => {
    const id = window.setInterval(() => {
      const cur = unreadRef.current;
      setUnreadDisplay((prev) => (prev === cur ? prev : cur));
    }, 200);
    return () => window.clearInterval(id);
  }, []);

  // Sample the xterm buffer for the last non-blank line (~300ms)
  useEffect(() => {
    const sample = () => {
      const term = termRef.current;
      if (!term) return;
      const buf = term.buffer.active;
      const startY = buf.cursorY + buf.baseY;
      let found = '';
      for (let dy = 0; dy <= 5; dy++) {
        const y = startY - dy;
        if (y < 0) break;
        const line = buf.getLine(y);
        if (!line) continue;
        const text = line.translateToString(true);
        if (text && text.trim().length > 0) {
          found = text.trimEnd();
          break;
        }
      }
      if (!found) {
        if (cwd && cwd.length > 0) {
          found = formatCwd(cwd);
        } else {
          found = '';
        }
      }
      const truncated = truncateLeft(found, 60);
      setPromptLine((prev) => (prev === truncated ? prev : truncated));
    };
    sample();
    const id = window.setInterval(sample, 300);
    return () => window.clearInterval(id);
  }, [cwd]);

  const dotClass =
    activity === 'running'
      ? 'terminal-live-dot terminal-live-dot--running'
      : activity === 'disconnected'
        ? 'terminal-live-dot terminal-live-dot--disconnected'
        : 'terminal-live-dot terminal-live-dot--idle';

  const headerClickable = collapsed;
  const handleHeaderClick = headerClickable ? onExpand : undefined;

  return (
    <div className="terminal-panel" style={{ height: `${height}px` }}>
      <div
        className={`terminal-panel-header${collapsed ? ' terminal-panel-header--collapsed' : ''}`}
        onClick={handleHeaderClick}
        style={headerClickable ? { cursor: 'pointer' } : undefined}
      >
        <span className={dotClass} aria-hidden="true" />
        <span className="terminal-panel-prompt">
          {collapsed ? (promptLine || '❯_ Terminal') : '❯_ Terminal'}
        </span>
        <div ref={statusRef} className="terminal-panel-status" />
        {collapsed && unreadDisplay > 0 && (
          <span className="terminal-panel-unread">
            +{unreadDisplay} {unreadDisplay === 1 ? 'line' : 'lines'}
          </span>
        )}
        {collapsed && (
          <span
            className={`terminal-panel-chevron${collapsed ? '' : ' terminal-panel-chevron--open'}`}
            aria-hidden="true"
            onClick={(e) => {
              e.stopPropagation();
              onExpand();
            }}
          >
            ⌃
          </span>
        )}
      </div>
      <div
        ref={containerRef}
        className="terminal-panel-xterm"
        style={collapsed ? { display: 'none' } : undefined}
      />
    </div>
  );
}
