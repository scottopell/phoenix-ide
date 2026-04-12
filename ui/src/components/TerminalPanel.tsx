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
  /** Click on the expanded-state close button collapses back to strip */
  onCollapse: () => void;
  /** Fallback prompt text when xterm buffer has no content yet */
  cwd?: string;
  /** Server-user's $SHELL, used to tailor the absent-state hint snippet. */
  shell?: string | undefined;
  /** Server-user's $HOME, used by the "let Phoenix set this up for me"
   *  button as the seeded conversation's working directory (REQ-TERM-020). */
  homeDir?: string | undefined;
  /**
   * Called when the user clicks "Let Phoenix set this up for me" in the
   * snippet modal. The parent owns navigation and the createConversation
   * API call because it has the conversation id, model, and router context.
   * TerminalPanel just builds the prompt and hands it off.
   */
  onAssistSetup?: (promptText: string, seedLabel: string, homeDir: string) => Promise<void> | void;
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

/**
 * REQ-TERM-020: build the initial prompt that the seeded conversation will
 * hydrate into its input area. The prompt asks Phoenix to investigate the
 * user's dotfiles setup, pick the right target file, and apply the OSC 133
 * snippet safely without touching unrelated configuration.
 *
 * The prompt is pre-filled but NOT auto-submitted. The user reviews it and
 * hits Send (REQ-SEED-001).
 */
function buildAssistPrompt(shellPath: string, snippet: ShellSnippet): string {
  return `I want to enable OSC 133 shell integration in my shell so Phoenix IDE's terminal HUD can track my commands (running, exit codes, durations). My shell is ${shellPath}.

Please:

1. INVESTIGATE my dotfiles setup. Check:
   - Whether ~/.zshrc (or equivalent for bash/fish) exists and whether it's a regular file or a symlink
   - Framework markers: oh-my-zsh, prezto, zim, powerlevel10k, starship
   - Dotfile managers: chezmoi (~/.local/share/chezmoi/), yadm (~/.yadm/ or ~/.config/yadm/), stow/dotbot/rcm (symlinked targets to a git repo), home-manager / NixOS (read-only generated configs)
   - Existing "Phoenix terminal integration" marker comments (idempotency — if already installed, tell me and exit)

2. DECIDE the right place to write the snippet:
   - oh-my-zsh → create ~/.oh-my-zsh/custom/phoenix-integration.zsh (auto-sourced)
   - fish → create ~/.config/fish/conf.d/phoenix-integration.fish
   - chezmoi → use \`chezmoi source-path\` to find the managed source, edit it there, then \`chezmoi apply\`
   - yadm → edit ~/.zshrc (or equivalent) directly; it's tracked in yadm's bare repo
   - symlinked dotfiles → follow the symlink to the target file, edit the target
   - plain → append to ~/.zshrc (or ~/.bashrc for bash)
   - NixOS / home-manager → DO NOT EDIT. Tell me where to manually add the snippet in my home.nix.

3. VERIFY BEFORE WRITE. Check if the snippet is already present (grep for "Phoenix terminal integration" or \`__phoenix_precmd\`). If so, confirm with me and exit without changes.

4. APPLY the edit. Do not touch unrelated configuration.

5. CONFIRM by reading the file back to verify the snippet landed correctly.

6. TELL ME how to activate it (source the file, or restart my shell).

7. GIT HYGIENE: if the edited file is tracked (yadm, chezmoi, a dotfiles repo), STAGE the change and SHOW git status but ASK before committing. Never auto-commit my dotfiles.

Constraints:
- Edit nothing outside my shell config
- Do not install new tools
- For exotic setups (home-manager, nushell, etc.) show me the snippet and explain the manual steps — do not attempt automation you cannot verify
- Ask before committing anything to git

The snippet to install (${snippet.shellName}):

\`\`\`
${snippet.snippet}
\`\`\`
`;
}

export function TerminalPanel({
  conversationId,
  height,
  collapsed,
  onExpand,
  onCollapse,
  cwd,
  shell,
  homeDir,
  onAssistSetup,
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
  // Ref mirror so long-lived callbacks (WS onmessage + activity timeout)
  // can check the current activity without re-registering. Specifically
  // prevents the byte-activity timeout from demoting a `disconnected`
  // state back to `idle` after ws.onclose has fired.
  const activityRef = useRef<ActivityState>('disconnected');
  activityRef.current = activity;
  const unreadRef = useRef<number>(0);
  const [unreadDisplay, setUnreadDisplay] = useState<number>(0);
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

  // Reconnect counter: incrementing forces the mount effect to tear down the
  // current xterm + WS and spawn a fresh one (backend spawns a new PTY since
  // the previous is gone). Wired to the "click to reconnect" affordance in
  // the disconnected-state UI.
  const [reconnectNonce, setReconnectNonce] = useState(0);

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

    // Reset detection + command state on (re-)mount so a reconnect gets a
    // fresh 5s detection window and clears any stale command from the
    // previous PTY. Also cleared on conversationId change.
    //
    // Intentionally NOT resetting `activity` here — let ws.onopen flip it
    // to 'idle' once the new handshake completes. Pre-clearing on mount
    // produced a dim→undim flash when the effect re-ran spontaneously.
    integrationStatusRef.current = 'unknown';
    setIntegrationStatus('unknown');
    currentCommandRef.current = null;
    setCurrentCommand(null);
    setLastCompletedCommand(null);
    setReportedCwd(null);

    // Defer allocation via setTimeout(0) so React 18 StrictMode's
    // synchronous double-invoke in dev doesn't allocate → tear down →
    // re-allocate an xterm + WebSocket in quick succession. That pattern
    // surfaced as:
    //   - "WebSocket closed before the connection is established" errors
    //     from ws1 being closed mid-handshake
    //   - xterm's internal refresh loop crashing on a disposed renderer
    //     (TypeError: this._renderer.value.dimensions)
    //   - A brief dim→undim flash in the HUD as ws1 fired onclose then
    //     ws2 fired onopen ~50ms later
    // With this deferral, effect run 1's cleanup cancels before the timer
    // fires, so only run 2 actually allocates resources.
    let cancelled = false;
    let cleanupReal: (() => void) | null = null;

    const initTimer = window.setTimeout(() => {
      if (cancelled || !containerRef.current) return;

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
      if (integrationStatusRef.current === 'absent') {
        // Detection settled to absent before this marker arrived. Lock holds.
        return;
      }

      // Parse "<kind>" or "<kind>;<payload>"
      const semi = data.indexOf(';');
      const kind = semi === -1 ? data : data.slice(0, semi);
      const payload = semi === -1 ? '' : data.slice(semi + 1);

      // REQ-TERM-015 (revised): detection promotes unknown → detected only
      // on C. A and B alone are insufficient — p10k emits A/B from its
      // prompt hooks but never C/D, and a HUD that says "detected" without
      // being able to track commands would mislead.
      if (kind === 'C' && integrationStatusRef.current === 'unknown') {
        integrationStatusRef.current = 'detected';
        setIntegrationStatus('detected');
        if (detectionTimeoutRef.current !== null) {
          window.clearTimeout(detectionTimeoutRef.current);
          detectionTimeoutRef.current = null;
        }
      }

      // A and B markers while unknown do nothing — we wait for a C.
      if (integrationStatusRef.current === 'unknown') return;

      switch (kind) {
        case 'A':
        case 'B':
          // No-op in the revised model. A marked the start of a new prompt
          // (used to clear last_completed_command) but clearing now happens
          // on the next C, which gives the ✓/✗ indicator a useful lifetime.
          break;
        case 'C': {
          // FTCS_COMMAND_EXECUTED — start a new command lifecycle.
          // Clears last_completed_command (was previously done on A but
          // that fired ~50ms after D, making the success indicator invisible).
          setLastCompletedCommand(null);
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
          // Unknown 133 sub-marker (;P, ;E, etc.). Ignore.
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
        //
        // Don't promote `disconnected` → `running`: once the session is
        // dead, the byte-activity path stays dead. This can happen if a
        // server-side shutdown message arrives as data just before the
        // close handshake.
        if (activityRef.current === 'disconnected') return;
        setActivity('running');
        if (activityTimeoutRef.current !== null) {
          window.clearTimeout(activityTimeoutRef.current);
        }
        activityTimeoutRef.current = window.setTimeout(() => {
          activityTimeoutRef.current = null;
          // Don't demote disconnected → idle. ws.onclose may have fired
          // between this timer being scheduled and it firing, and the
          // disconnected state needs to stick until explicit reconnect.
          if (activityRef.current === 'disconnected') return;
          setActivity('idle');
        }, 500);
      }
    };

    const clearPendingActivityDecay = () => {
      if (activityTimeoutRef.current !== null) {
        window.clearTimeout(activityTimeoutRef.current);
        activityTimeoutRef.current = null;
      }
    };

    ws.onerror = () => {
      clearPendingActivityDecay();
      setStatus('Connection error');
      setActivity('disconnected');
    };
    ws.onclose = () => {
      clearPendingActivityDecay();
      setStatus('Shell exited');
      setActivity('disconnected');
      term.write('\r\n\x1b[90m[Shell exited]\x1b[0m\r\n');
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

      // Stash real cleanup on the outer var so the effect's return can
      // call it once allocation has completed.
      cleanupReal = () => {
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
        // Unbind handlers BEFORE close() so the onclose for a WS that's
        // still mid-handshake (or the onclose from a clean teardown) can't
        // race with a fresh effect run and clobber its freshly-set state.
        ws.onopen = null;
        ws.onclose = null;
        ws.onerror = null;
        ws.onmessage = null;
        try {
          ws.close();
        } catch {
          // ignore — close on an already-closed ws is a no-op
        }
        try {
          term.dispose();
        } catch {
          // xterm.js has a race where internal rAF / refresh callbacks
          // can fire on a partially-disposed renderer and throw
          // "undefined is not an object (evaluating
          // 'this._renderer.value.dimensions')". Swallow — we're tearing
          // down anyway.
        }
        termRef.current = null;
        fitAddonRef.current = null;
        wsRef.current = null;
      };
    }, 0);

    return () => {
      cancelled = true;
      window.clearTimeout(initTimer);
      if (cleanupReal) cleanupReal();
    };
  }, [conversationId, setStatus, reconnectNonce]);

  const reconnect = useCallback(() => {
    setReconnectNonce((n) => n + 1);
  }, []);

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

  // Dot color / semantic state. Disconnected is handled by a panel-level
  // "dead" treatment (see below); here we only compute the five normal dot
  // variants. Order of priority: disconnected wins over everything.
  const isDisconnected = activity === 'disconnected';
  type DotVariant = 'unknown' | 'absent' | 'idle-ok' | 'running' | 'failed';
  const dotVariant: DotVariant = (() => {
    if (integrationStatus === 'unknown') return 'unknown';
    if (integrationStatus === 'absent') return 'absent';
    // detected
    if (currentCommand !== null) return 'running';
    if (lastCompletedCommand && (lastCompletedCommand.exitCode ?? 0) !== 0) {
      return 'failed';
    }
    return 'idle-ok';
  })();

  const dotClass = `terminal-live-dot terminal-live-dot--${dotVariant}`;

  // Header click semantics:
  //   disconnected: click anywhere → reconnect (revive the PTY)
  //   collapsed:    click anywhere → expand
  //   expanded:     click on the header body does nothing (close button handles it)
  const handleHeaderClick = isDisconnected
    ? reconnect
    : collapsed
      ? onExpand
      : undefined;
  const headerClickable = isDisconnected || collapsed;

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

  // REQ-TERM-020: build a detailed prompt for a seeded conversation that
  // asks Phoenix to investigate the user's dotfiles setup and apply the
  // integration snippet safely. The parent handles the API call + navigation.
  const [assistInFlight, setAssistInFlight] = useState(false);
  const canAssist =
    snippet !== null &&
    !!shell &&
    !!homeDir &&
    typeof onAssistSetup === 'function' &&
    !assistInFlight;

  const handleAssistSetup = async () => {
    if (!snippet || !shell || !homeDir || !onAssistSetup) return;
    const promptText = buildAssistPrompt(shell, snippet);
    const seedLabel = `Shell integration setup (${snippet.shellName})`;
    setAssistInFlight(true);
    try {
      await onAssistSetup(promptText, seedLabel, homeDir);
      // Parent navigates; this component unmounts. No need to close the
      // modal — it's disposed with the page.
    } catch (err) {
      // Surface nothing fancy — the button re-enables so the user can retry.
      // Console is the only place the error goes; REQ-TERM-020 is best-effort.
      // eslint-disable-next-line no-console
      console.error('Assist setup failed:', err);
      setAssistInFlight(false);
    }
  };

  // Render the collapsed-mode prompt area. Five variants driven by
  // integrationStatus + command lifecycle. No buffer sampler — it produced
  // ugly fragments for two-line powerline prompts; cleaner to show the
  // static cwd and rely on OSC 133 for live data when available.
  const renderCollapsedHud = () => {
    if (isDisconnected) {
      return (
        <span className="terminal-panel-prompt terminal-panel-prompt--dead">
          Shell exited — click to start a new one
        </span>
      );
    }
    if (integrationStatus === 'unknown') {
      // Within the 5s detection window — show a calm placeholder, no sampler.
      return (
        <span className="terminal-panel-prompt terminal-panel-prompt--dim">
          ❯_ Terminal
        </span>
      );
    }
    if (integrationStatus === 'absent') {
      // Shell integration not detected. Show the static conversation cwd
      // so the user has a useful anchor. Hover the dot for the hint.
      return (
        <span className="terminal-panel-prompt">
          <span className="terminal-hud-cwd terminal-hud-cwd--dim">
            {formatCwdPlain(cwd ?? '') || '❯_ Terminal'}
          </span>
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

  const panelClass = `terminal-panel${isDisconnected ? ' terminal-panel--dead' : ''}`;

  return (
    <div
      className={panelClass}
      style={{ height: `${height}px` }}
      onClick={isDisconnected ? reconnect : undefined}
    >
      <div
        className={`terminal-panel-header${collapsed ? ' terminal-panel-header--collapsed' : ''}`}
        onClick={handleHeaderClick}
        style={headerClickable ? { cursor: 'pointer' } : undefined}
      >
        {!isDisconnected && (
          <button
            type="button"
            className={`terminal-panel-chevron${collapsed ? '' : ' terminal-panel-chevron--expanded'}`}
            aria-label={collapsed ? 'Expand terminal' : 'Collapse terminal'}
            title={collapsed ? 'Expand terminal' : 'Collapse terminal'}
            onClick={(e) => {
              e.stopPropagation();
              if (collapsed) onExpand();
              else onCollapse();
            }}
          >
            {collapsed ? '⌃' : '⌄'}
          </button>
        )}
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
        {collapsed || isDisconnected ? (
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
                    <button
                      type="button"
                      className="terminal-snippet-modal-assist"
                      onClick={handleAssistSetup}
                      disabled={!canAssist}
                      title={
                        canAssist
                          ? 'Spin off a focused conversation that installs this snippet safely'
                          : 'Shell or home directory unknown'
                      }
                    >
                      {assistInFlight ? 'Starting…' : 'Let Phoenix set this up for me'}
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
