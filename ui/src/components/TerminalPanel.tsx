/**
 * TerminalPanel — PTY-backed terminal rendered via xterm.js.
 *
 * Connects to `GET /api/conversations/:id/terminal` over binary WebSocket.
 * Binary frame protocol:
 *   byte 0 = 0x00 → PTY data (bidirectional)
 *   byte 0 = 0x01 → resize: u16be cols, u16be rows (client → server)
 *
 * REQ-TERM-004, REQ-TERM-005, REQ-TERM-006
 *
 * OSC 133 (FinalTerm shell integration) and OSC 7 (cwd reporting) are
 * detected and consumed in the browser via xterm.js OSC handlers
 * (REQ-TERM-015 through REQ-TERM-018). When an OSC 133 marker arrives
 * within the 5s detection window the HUD switches to the rich
 * "detected" path. Otherwise it falls back to the byte-activity sampler.
 */

import { useEffect, useRef, useCallback, useState } from 'react';
import { Terminal } from 'xterm';
import { FitAddon } from 'xterm-addon-fit';
import 'xterm/css/xterm.css';
import {
  getSnippetForShell,
  shellDisplayName,
  type ShellSnippet,
} from '../shellIntegrationSnippets';

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
  /** Server-user's $SHELL, used to tailor the absent-state hint snippet. */
  shell?: string | undefined;
}

type ActivityState = 'idle' | 'running' | 'disconnected';

/** REQ-TERM-015 detection state. Monotonic: unknown → detected | absent. */
type ShellIntegrationStatus = 'unknown' | 'detected' | 'absent';

/** REQ-TERM-016 command lifecycle slot. */
interface CommandExecution {
  commandText: string;
  startedAt: number;
  exitCode: number | null;
  finishedAt: number | null;
}

/** REQ-TERM-015. Frontend mirrors `config.shell_integration_detection_window`. */
const DETECTION_WINDOW_MS = 5000;

/** Build the WebSocket URL for a conversation's terminal endpoint. */
function terminalWsUrl(conversationId: string): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${window.location.host}/api/conversations/${conversationId}/terminal`;
}

/** Encode a resize frame: 0x01 + u16be cols + u16be rows */
function resizeFrame(cols: number, rows: number): Uint8Array {
  const buf = new Uint8Array(5);
  buf[0] = 0x01;
  new DataView(buf.buffer).setUint16(1, cols, false);
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
  const trimmed = cwd.replace(/\/+$/, '');
  return truncateLeft(trimmed, 40) + ' ❯';
}

/** Format a cwd path for the rich HUD — no glyph, just the path. */
function formatCwdPlain(cwd: string): string {
  const trimmed = cwd.replace(/\/+$/, '');
  return truncateLeft(trimmed, 40);
}

/** Truncate command text to first line, capped to 50 chars. */
function formatCommandText(text: string): string {
  const firstLine = text.split('\n', 1)[0] ?? '';
  const trimmed = firstLine.trim();
  if (trimmed.length === 0) return '(no command text)';
  if (trimmed.length > 50) return trimmed.slice(0, 49) + '…';
  return trimmed;
}

/** Format running duration in seconds with 1 decimal: 2.3s, 12.1s, 145.0s. */
function formatDuration(ms: number): string {
  const seconds = ms / 1000;
  return `${seconds.toFixed(1)}s`;
}

export function TerminalPanel({
  conversationId,
  height,
  collapsed,
  onExpand,
  cwd,
  shell,
}: TerminalPanelProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const statusRef = useRef<HTMLDivElement>(null);
  const collapsedRef = useRef(collapsed);
  collapsedRef.current = collapsed;

  // Fallback (sampler) HUD state
  const [activity, setActivity] = useState<ActivityState>('disconnected');
  const unreadRef = useRef<number>(0);
  const [unreadDisplay, setUnreadDisplay] = useState<number>(0);
  const [promptLine, setPromptLine] = useState<string>('');
  const activityTimeoutRef = useRef<number | null>(null);

  // REQ-TERM-015/016/018: shell integration state
  const [integrationStatus, setIntegrationStatus] =
    useState<ShellIntegrationStatus>('unknown');
  // Mirror in a ref so the OSC handlers (closure-captured at mount) can read
  // the *current* status without re-registering. Without this the handlers
  // would only ever see `unknown` and the monotonic invariant would be moot.
  const integrationStatusRef = useRef<ShellIntegrationStatus>('unknown');
  integrationStatusRef.current = integrationStatus;

  const [currentCommand, setCurrentCommand] = useState<CommandExecution | null>(null);
  const currentCommandRef = useRef<CommandExecution | null>(null);
  currentCommandRef.current = currentCommand;

  const [lastCompletedCommand, setLastCompletedCommand] =
    useState<CommandExecution | null>(null);
  const [reportedCwd, setReportedCwd] = useState<string | null>(null);

  const detectionTimeoutRef = useRef<number | null>(null);

  // 100ms ticker bumped only while currentCommand is non-null. Drives the
  // live duration display in the HUD without re-rendering on every tick when
  // nothing is running.
  const [, setRunningTick] = useState(0);

  // Hint UI (absent state) state — tooltip + snippet modal
  const [hintTooltipVisible, setHintTooltipVisible] = useState(false);
  const [snippetModalOpen, setSnippetModalOpen] = useState(false);
  const [copyAck, setCopyAck] = useState(false);

  const setStatus = useCallback((msg: string) => {
    if (statusRef.current) statusRef.current.textContent = msg;
  }, []);

  // Mount xterm once for the lifetime of the conversation. The xterm container
  // is always rendered but hidden via `display: none` when the panel is
  // collapsed, preserving the WebSocket, PTY, scrollback, and any running
  // shell state across collapse/expand cycles.
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
    if (!collapsedRef.current) {
      try {
        fitAddon.fit();
      } catch {
        // ignore — deferred fit will retry
      }
    }
    termRef.current = term;
    fitAddonRef.current = fitAddon;

    // --- OSC 133 / OSC 7 handlers (REQ-TERM-015/016/018) ---
    // Register before WS opens so even bytes arriving in the very first
    // message are inspected. The callbacks read state via refs, so they stay
    // correct across re-renders without needing to re-register.
    const handleOsc133 = (data: string): void => {
      // Detection: any marker promotes unknown → detected. Locked thereafter
      // (ShellIntegrationStatusMonotonic).
      if (integrationStatusRef.current === 'unknown') {
        integrationStatusRef.current = 'detected';
        setIntegrationStatus('detected');
        if (detectionTimeoutRef.current !== null) {
          window.clearTimeout(detectionTimeoutRef.current);
          detectionTimeoutRef.current = null;
        }
      }
      if (integrationStatusRef.current === 'absent') {
        // Detection settled to absent before this marker arrived. Lock holds.
        return;
      }

      // Parse "<kind>" or "<kind>;<payload>"
      const semi = data.indexOf(';');
      const kind = semi === -1 ? data : data.slice(0, semi);
      const payload = semi === -1 ? '' : data.slice(semi + 1);

      switch (kind) {
        case 'A':
          // FTCS_PROMPT_START — clear last_completed_command
          setLastCompletedCommand(null);
          break;
        case 'B':
          // FTCS_COMMAND_START — accepted, no state change (forward compat)
          break;
        case 'C': {
          // FTCS_COMMAND_EXECUTED — start a new command lifecycle.
          // Overwrites any prior current_command (nested subshell case).
          const cmd: CommandExecution = {
            commandText: payload,
            startedAt: Date.now(),
            exitCode: null,
            finishedAt: null,
          };
          currentCommandRef.current = cmd;
          setCurrentCommand(cmd);
          break;
        }
        case 'D': {
          // FTCS_COMMAND_FINISHED — finalise the current command, if any.
          const cur = currentCommandRef.current;
          if (!cur) {
            // Spec: log at debug, no state change.
            // eslint-disable-next-line no-console
            console.debug(
              'OSC 133;D received with no current_command; ignoring',
            );
            break;
          }
          let exitCode: number | null;
          if (payload === '') {
            exitCode = null;
          } else {
            const parsed = parseInt(payload, 10);
            exitCode = Number.isNaN(parsed) ? null : parsed;
          }
          const finished: CommandExecution = {
            commandText: cur.commandText,
            startedAt: cur.startedAt,
            exitCode,
            finishedAt: Date.now(),
          };
          currentCommandRef.current = null;
          setCurrentCommand(null);
          setLastCompletedCommand(finished);
          break;
        }
        default:
          // Unknown 133 sub-marker. Detection has already fired above; we
          // just ignore the body. (Implementations sometimes use ;P, ;E, etc.)
          break;
      }
    };

    const handleOsc7 = (data: string): void => {
      // Payload format: file://hostname/absolute/path (percent-encoded)
      const m = data.match(/^file:\/\/[^/]*(\/.*)$/);
      if (!m) {
        // eslint-disable-next-line no-console
        console.debug('OSC 7 parse failed:', data);
        return;
      }
      try {
        const decoded = decodeURIComponent(m[1]!);
        setReportedCwd(decoded);
      } catch {
        // eslint-disable-next-line no-console
        console.debug('OSC 7 percent-decode failed:', data);
      }
    };

    const osc133Dispose = term.parser.registerOscHandler(133, (data: string) => {
      handleOsc133(data);
      return true;
    });
    const osc7Dispose = term.parser.registerOscHandler(7, (data: string) => {
      handleOsc7(data);
      return true;
    });

    // --- Detection timeout (REQ-TERM-015) ---
    detectionTimeoutRef.current = window.setTimeout(() => {
      detectionTimeoutRef.current = null;
      if (integrationStatusRef.current === 'unknown') {
        integrationStatusRef.current = 'absent';
        setIntegrationStatus('absent');
      }
    }, DETECTION_WINDOW_MS);

    // --- WebSocket connection ---
    const ws = new WebSocket(terminalWsUrl(conversationId));
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;
    setStatus('Connecting…');

    ws.onopen = () => {
      const { cols, rows } = term;
      ws.send(resizeFrame(cols, rows));
      setStatus('');
      setActivity('idle');
    };

    ws.onmessage = (event: MessageEvent<ArrayBuffer>) => {
      const data = new Uint8Array(event.data);
      if (data.length === 0) return;
      if (data[0] === 0x00) {
        const payload = data.slice(1);
        term.write(payload);
        if (collapsedRef.current) {
          let n = 0;
          for (let i = 0; i < payload.length; i++) {
            if (payload[i] === 0x0a) n++;
          }
          if (n > 0) unreadRef.current += n;
        }
        // Byte-activity heuristic — only used while integrationStatus is not
        // `detected`. When detected, the dot color is driven by the OSC 133
        // command lifecycle instead.
        setActivity('running');
        if (activityTimeoutRef.current !== null) {
          window.clearTimeout(activityTimeoutRef.current);
        }
        activityTimeoutRef.current = window.setTimeout(() => {
          setActivity('idle');
          activityTimeoutRef.current = null;
        }, 500);
      }
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

    const disposeOnData = term.onData((text) => {
      if (ws.readyState === WebSocket.OPEN) {
        const encoded = new TextEncoder().encode(text);
        ws.send(dataFrame(encoded));
      }
    });

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
      osc133Dispose.dispose();
      osc7Dispose.dispose();
      window.removeEventListener('resize', handleResize);
      if (activityTimeoutRef.current !== null) {
        window.clearTimeout(activityTimeoutRef.current);
        activityTimeoutRef.current = null;
      }
      if (detectionTimeoutRef.current !== null) {
        window.clearTimeout(detectionTimeoutRef.current);
        detectionTimeoutRef.current = null;
      }
      ws.close();
      term.dispose();
      termRef.current = null;
      fitAddonRef.current = null;
      wsRef.current = null;
    };
  }, [conversationId, setStatus]);

  // Refit when the parent height changes (drag-resize).
  useEffect(() => {
    if (collapsed) return;
    const fit = fitAddonRef.current;
    const term = termRef.current;
    const ws = wsRef.current;
    if (!fit || !term) return;
    const id = requestAnimationFrame(() => {
      try {
        fit.fit();
        if (ws && ws.readyState === WebSocket.OPEN) {
          ws.send(resizeFrame(term.cols, term.rows));
        }
      } catch {
        // FitAddon throws if the container is 0×0; ignore.
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

  // Sample the xterm buffer for the last non-blank line (~300ms).
  // FALLBACK PATH ONLY — when integrationStatus === 'detected', the rich HUD
  // draws from currentCommand / lastCompletedCommand instead and the prompt
  // line text is unused. We still run the sampler in `unknown` so the HUD
  // has something to show before detection settles.
  useEffect(() => {
    if (integrationStatus === 'detected') return;
    const sample = () => {
      const term = termRef.current;
      if (!term) return;
      const buf = term.buffer.active;
      const startY = buf.cursorY + buf.baseY;
      const lineText = (y: number): string => {
        if (y < 0) return '';
        const line = buf.getLine(y);
        if (!line) return '';
        return line.translateToString(true).trimEnd();
      };
      let found = '';
      let foundY = -1;
      for (let dy = 0; dy <= 5; dy++) {
        const text = lineText(startY - dy);
        if (text && text.trim().length > 0) {
          found = text;
          foundY = startY - dy;
          break;
        }
      }
      if (found && found.trim().length <= 10 && foundY > 0) {
        for (let dy = 1; dy <= 3; dy++) {
          const above = lineText(foundY - dy);
          if (above && above.trim().length > found.trim().length) {
            found = `${above} ${found.trim()}`;
            break;
          }
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
  }, [cwd, integrationStatus]);

  // Live duration ticker — runs only while a command is executing.
  useEffect(() => {
    if (!currentCommand) return;
    const id = window.setInterval(() => {
      setRunningTick((t) => t + 1);
    }, 100);
    return () => window.clearInterval(id);
  }, [currentCommand]);

  // ESC closes the snippet modal.
  useEffect(() => {
    if (!snippetModalOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setSnippetModalOpen(false);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [snippetModalOpen]);

  // Auto-clear copy ack after 1.5s
  useEffect(() => {
    if (!copyAck) return;
    const id = window.setTimeout(() => setCopyAck(false), 1500);
    return () => window.clearTimeout(id);
  }, [copyAck]);

  // --- Rendering helpers ---

  // Dot color: in detected mode, drive from current_command; in fallback,
  // use the byte-activity heuristic. Disconnected always wins (WS dead).
  const dotState: ActivityState =
    activity === 'disconnected'
      ? 'disconnected'
      : integrationStatus === 'detected'
        ? currentCommand !== null
          ? 'running'
          : 'idle'
        : activity;

  const dotClass =
    dotState === 'running'
      ? 'terminal-live-dot terminal-live-dot--running'
      : dotState === 'disconnected'
        ? 'terminal-live-dot terminal-live-dot--disconnected'
        : 'terminal-live-dot terminal-live-dot--idle';

  const headerClickable = collapsed;
  const handleHeaderClick = headerClickable ? onExpand : undefined;

  // For the rich HUD: prefer reported_cwd, fall back to conversation cwd.
  const effectiveCwd = reportedCwd ?? cwd ?? '';

  const snippet: ShellSnippet | null = getSnippetForShell(shell);
  const shellLabel = shellDisplayName(shell);

  const handleDotMouseEnter = () => {
    if (integrationStatus === 'absent') setHintTooltipVisible(true);
  };
  const handleDotMouseLeave = () => {
    setHintTooltipVisible(false);
  };
  const handleDotClick = (e: React.MouseEvent) => {
    if (integrationStatus !== 'absent') return;
    e.stopPropagation();
    setSnippetModalOpen(true);
    setHintTooltipVisible(false);
  };

  const handleCopySnippet = async () => {
    if (!snippet) return;
    try {
      await navigator.clipboard.writeText(snippet.snippet);
      setCopyAck(true);
    } catch {
      // ignore — UI gives no special error path; user can select+copy manually
    }
  };

  // Render the collapsed-mode prompt area: rich HUD if detected, else
  // sampler-driven text. Both branches are functionally identical re: layout
  // (one flex line) so the existing CSS keeps working.
  const renderCollapsedHud = () => {
    if (integrationStatus !== 'detected') {
      return (
        <span className="terminal-panel-prompt">
          {promptLine || '❯_ Terminal'}
        </span>
      );
    }
    // Detected: rich HUD
    if (currentCommand !== null) {
      const elapsedMs = Date.now() - currentCommand.startedAt;
      return (
        <span className="terminal-panel-prompt">
          <span className="terminal-hud-cwd">{formatCwdPlain(effectiveCwd)}</span>
          <span className="terminal-hud-sep"> $ </span>
          <span className="terminal-hud-cmd">
            {formatCommandText(currentCommand.commandText)}
          </span>
          <span className="terminal-hud-dur"> {formatDuration(elapsedMs)}</span>
        </span>
      );
    }
    if (lastCompletedCommand !== null) {
      const dur =
        lastCompletedCommand.finishedAt !== null
          ? formatDuration(
              lastCompletedCommand.finishedAt - lastCompletedCommand.startedAt,
            )
          : '';
      const code = lastCompletedCommand.exitCode;
      let glyph: string;
      let glyphClass: string;
      let suffix: string;
      if (code === 0) {
        glyph = '✓';
        glyphClass = 'terminal-hud-glyph terminal-hud-glyph--ok';
        suffix = `(${dur})`;
      } else if (code === null) {
        glyph = '•';
        glyphClass = 'terminal-hud-glyph terminal-hud-glyph--unknown';
        suffix = `(${dur})`;
      } else {
        glyph = '✗';
        glyphClass = 'terminal-hud-glyph terminal-hud-glyph--err';
        suffix = `(exit ${code})`;
      }
      return (
        <span className="terminal-panel-prompt">
          <span className="terminal-hud-cwd">{formatCwdPlain(effectiveCwd)}</span>
          <span className={glyphClass}> {glyph} </span>
          <span className="terminal-hud-cmd">
            {formatCommandText(lastCompletedCommand.commandText)}
          </span>
          <span className="terminal-hud-dur"> {suffix}</span>
        </span>
      );
    }
    // Idle
    return (
      <span className="terminal-panel-prompt">
        <span className="terminal-hud-cwd">{formatCwdPlain(effectiveCwd)}</span>
      </span>
    );
  };

  return (
    <div className="terminal-panel" style={{ height: `${height}px` }}>
      <div
        className={`terminal-panel-header${collapsed ? ' terminal-panel-header--collapsed' : ''}`}
        onClick={handleHeaderClick}
        style={headerClickable ? { cursor: 'pointer' } : undefined}
      >
        <span
          className={`terminal-live-dot-wrap${integrationStatus === 'absent' ? ' terminal-live-dot-wrap--hint' : ''}`}
          onMouseEnter={handleDotMouseEnter}
          onMouseLeave={handleDotMouseLeave}
          onClick={handleDotClick}
        >
          <span className={dotClass} aria-hidden="true" />
          {hintTooltipVisible && integrationStatus === 'absent' && (
            <span className="terminal-hint-tooltip" role="tooltip">
              <strong>⚠ Shell integration not detected ({shellLabel})</strong>
              <span className="terminal-hint-tooltip-sub">
                {snippet
                  ? `Click for ${snippet.shellName} snippet`
                  : 'Click for details'}
              </span>
            </span>
          )}
        </span>
        {collapsed ? (
          renderCollapsedHud()
        ) : (
          <span className="terminal-panel-prompt">❯_ Terminal</span>
        )}
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

      {snippetModalOpen && (
        <div
          className="terminal-snippet-modal-backdrop"
          onClick={() => setSnippetModalOpen(false)}
        >
          <div
            className="terminal-snippet-modal"
            onClick={(e) => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
          >
            <div className="terminal-snippet-modal-header">
              <span className="terminal-snippet-modal-title">
                Enable shell integration ({shellLabel})
              </span>
              <button
                type="button"
                className="terminal-snippet-modal-close"
                onClick={() => setSnippetModalOpen(false)}
                aria-label="Close"
              >
                ×
              </button>
            </div>
            <div className="terminal-snippet-modal-body">
              {snippet ? (
                <>
                  <p className="terminal-snippet-modal-help">
                    Paste this into <code>{snippet.rcFile}</code>, then re-source
                    it (or restart your shell). Phoenix will detect the markers
                    on the next terminal session.
                  </p>
                  <pre className="terminal-snippet-modal-pre">{snippet.snippet}</pre>
                  <div className="terminal-snippet-modal-actions">
                    <button
                      type="button"
                      className="terminal-snippet-modal-copy"
                      onClick={handleCopySnippet}
                    >
                      {copyAck ? 'Copied!' : 'Copy to clipboard'}
                    </button>
                  </div>
                </>
              ) : (
                <p className="terminal-snippet-modal-help">
                  Phoenix doesn't ship a built-in shell integration snippet for{' '}
                  <code>{shellLabel}</code>. Phoenix consumes OSC 133 (FinalTerm
                  shell integration) and OSC 7 (cwd reporting); if your shell can
                  emit those sequences, the rich HUD will activate automatically.
                </p>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
