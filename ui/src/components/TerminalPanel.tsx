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

import { useEffect, useRef, useCallback } from 'react';
import { Terminal } from 'xterm';
import { FitAddon } from 'xterm-addon-fit';
import 'xterm/css/xterm.css';

interface TerminalPanelProps {
  conversationId: string;
}

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

export function TerminalPanel({ conversationId }: TerminalPanelProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const statusRef = useRef<HTMLDivElement>(null);

  const setStatus = useCallback((msg: string) => {
    if (statusRef.current) statusRef.current.textContent = msg;
  }, []);

  useEffect(() => {
    if (!containerRef.current) return;

    // --- xterm.js setup ---
    const term = new Terminal({
      cursorBlink: true,
      theme: { background: '#1a1a1a', foreground: '#d4d4d4', cursor: '#d4d4d4' },
      fontFamily: '"Cascadia Code", "JetBrains Mono", "Fira Code", monospace',
      fontSize: 13,
      scrollback: 1000,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef.current);
    fitAddon.fit();
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
    };

    ws.onmessage = (event: MessageEvent<ArrayBuffer>) => {
      const data = new Uint8Array(event.data);
      if (data.length === 0) return;
      if (data[0] === 0x00) {
        // PTY output → write to xterm.js
        term.write(data.slice(1));
      }
      // 0x01 (resize) is only sent server→client as a future extension; ignore for now.
    };

    ws.onerror = () => setStatus('Connection error');
    ws.onclose = () => {
      setStatus('Terminal closed');
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
    const handleResize = () => {
      fitAddon.fit();
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(resizeFrame(term.cols, term.rows));
      }
    };
    window.addEventListener('resize', handleResize);

    return () => {
      disposeOnData.dispose();
      window.removeEventListener('resize', handleResize);
      ws.close();
      term.dispose();
      termRef.current = null;
      fitAddonRef.current = null;
      wsRef.current = null;
    };
  }, [conversationId, setStatus]);

  return (
    <div className="terminal-panel">
      <div className="terminal-panel-header">
        <span className="terminal-panel-title">Terminal</span>
        <div ref={statusRef} className="terminal-panel-status" />
      </div>
      <div ref={containerRef} className="terminal-panel-xterm" />
    </div>
  );
}
