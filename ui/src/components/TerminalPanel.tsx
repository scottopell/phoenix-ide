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
import { Terminal, type ITheme } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import {
  getSnippetForShell,
  shellDisplayName,
  type ShellSnippet,
} from '../shellIntegrationSnippets';
import { useTheme } from '../hooks/useTheme';
import { copyToClipboard } from '../utils/clipboard';

const CheckIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="20 6 9 17 4 12" />
  </svg>
);
const XIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <line x1="18" y1="6" x2="6" y2="18" />
    <line x1="6" y1="6" x2="18" y2="18" />
  </svg>
);
const CircleDot = () => (
  <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none" aria-hidden="true">
    <circle cx="12" cy="12" r="4" />
  </svg>
);
const ChevronUpHeader = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="18 15 12 9 6 15" />
  </svg>
);
const ChevronDownHeader = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="6 9 12 15 18 9" />
  </svg>
);
const AlertTriangleInline = () => (
  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" style={{ verticalAlign: '-2px', marginRight: '4px' }}>
    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
    <line x1="12" y1="9" x2="12" y2="13" />
    <line x1="12" y1="17" x2="12.01" y2="17" />
  </svg>
);

/**
 * Read the current xterm-relevant CSS variables from `:root[data-theme=...]`.
 * Pulls the values the chrome already uses (--terminal-bg, --terminal-fg,
 * --terminal-cursor) so xterm.js's drawing surface matches the theme rather
 * than diverging via a hardcoded colour pair (task 24681).
 */
function readXtermTheme(): ITheme {
  const styles = getComputedStyle(document.documentElement);
  const get = (name: string, fallback: string): string => {
    const v = styles.getPropertyValue(name).trim();
    return v.length > 0 ? v : fallback;
  };
  return {
    background: get('--terminal-bg', '#1a1a1a'),
    foreground: get('--terminal-fg', '#d4d4d4'),
    cursor: get('--terminal-cursor', '#d4d4d4'),
    // Without these, xterm.js falls back to a translucent white that's
    // invisible against the light-mode terminal background (#f6f8fa).
    selectionBackground: get('--terminal-selection-bg', '#3a3d41'),
    selectionInactiveBackground: get(
      '--terminal-selection-inactive-bg',
      '#2a2d31',
    ),
  };
}

/**
 * Identity of the terminal session this panel attaches to. The backend
 * keys terminals by `WorkScope` (REQ-TERM-WS-001); the frontend mirrors
 * that with a discriminated union so each variant carries exactly the
 * fields it needs.
 *
 * - `conversation`: a per-conversation terminal (the default UX on the
 *   conversation page). Continuation conversations that resolve to the
 *   same worktree share the backend terminal automatically — the panel
 *   doesn't know about that, it just connects to its conversation's
 *   endpoint and gets reclaimed onto the shared session.
 * - `global`: the singleton terminal surfaced on `/new`. Not bound to
 *   any conversation; survives every individual conversation; lives for
 *   the lifetime of the Phoenix process.
 */
export type TerminalScope =
  | { kind: 'conversation'; conversationId: string }
  | { kind: 'global' };

interface TerminalPanelProps {
  scope: TerminalScope;
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
  /** Failure-styled toast (red). Surfaces user-facing errors from the
   *  assist-setup path so the user knows the click failed instead of
   *  silently re-enabling the button. REQ-TPANEL-006 / REQ-NOTIF-002. */
  showError?: (message: string, duration?: number) => void;
  /**
   * Fired on Cmd/Ctrl+Shift+L when the terminal has focus AND a non-empty
   * selection exists. The raw selected text is passed up — the caller is
   * responsible for fencing it and inserting it into the message composer
   * (task 02672). Empty-selection presses are silently no-op'd here.
   */
  onSendSelectionToDraft?: (selection: string) => void;
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

/**
 * How long a mouseup-armed clipboard write waits for tmux's OSC 52 to arrive
 * before giving up. The PTY round-trip is sub-frame; this only guards against a
 * mouseup that produced no tmux copy (plain click, empty selection).
 */
const OSC52_ARM_TIMEOUT_MS = 500;

/** Build the WebSocket URL for a terminal scope. */
function terminalWsUrl(scope: TerminalScope): string {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  if (scope.kind === 'global') {
    return `${proto}//${window.location.host}/api/terminal/global`;
  }
  return `${proto}//${window.location.host}/api/conversations/${scope.conversationId}/terminal`;
}

/** Stable React-key / dependency string for a scope. Prefixes mirror the
 *  backend's `WorkScope::stable_key()` so namespaces stay disjoint —
 *  `conversation:global` and `global:` are distinct keys, so a future
 *  conversation id that happens to equal a reserved variant name can't
 *  collide and suppress a reconnect on scope change. */
function scopeKey(scope: TerminalScope): string {
  return scope.kind === 'global' ? 'global:' : `conversation:${scope.conversationId}`;
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

/** URI scheme for click-to-run command suggestions (emitted by `phx --suggest`
 *  as OSC 8 hyperlinks). The decoded command is dropped onto the shell prompt
 *  for the user to review and run — never auto-executed. */
const PHXRUN_SCHEME = 'phxrun:';

/** Decode a base64 string into UTF-8 text (atob yields a binary string). */
function decodeBase64Utf8(b64: string): string {
  const bin = atob(b64);
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
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
  scope,
  height,
  collapsed,
  onExpand,
  onCollapse,
  cwd,
  shell,
  homeDir,
  onAssistSetup,
  showError,
  onSendSelectionToDraft,
}: TerminalPanelProps) {
  const scopeId = scopeKey(scope);
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const statusRef = useRef<HTMLDivElement>(null);
  const collapsedRef = useRef(collapsed);
  collapsedRef.current = collapsed;

  // Mirror the selection-callback prop so the mount-time key handler always
  // sees the latest closure without re-registering on every parent re-render.
  const onSendSelectionToDraftRef = useRef(onSendSelectionToDraft);
  onSendSelectionToDraftRef.current = onSendSelectionToDraft;

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
    // Theme is sourced from the same CSS variables the rest of the UI uses
    // (task 24681). On theme change the `useTheme()` effect below reapplies
    // them live without tearing down the PTY.
    const term = new Terminal({
      cursorBlink: true,
      theme: readXtermTheme(),
      fontFamily: '"SauceCodePro NF Mono", "Cascadia Code", "JetBrains Mono", "Fira Code", monospace',
      fontSize: 13,
      scrollback: 1000,
      // When the PTY child is `tmux attach` with `mouse on`, tmux requests
      // SGR mouse tracking, so xterm forwards drags to tmux instead of making a
      // local DOM selection — which is what term.getSelection() (send-to-LLM)
      // relies on. xterm's force-local-selection modifier is platform-split
      // (SelectionService.shouldForceSelection): Shift+drag on non-mac, and on
      // macOS *only* Option+drag, gated behind this opt. Shift does nothing on
      // macOS. Plain-drag copy still works via tmux's OSC 52 (see Drag-to-copy
      // below); this is only for the modifier-drag path that yields an
      // xterm-side selection.
      macOptionClickForcesSelection: true,
      // Intercept OSC 8 hyperlinks. `phxrun:` links are command suggestions
      // (from `phx`): clicking drops the decoded command onto the shell prompt
      // WITHOUT a trailing newline, so the user reviews it and presses Enter —
      // suggestion, never auto-execution. Ordinary http(s) links keep the
      // default open-in-new-tab behavior. allowNonHttpProtocols is required for
      // xterm to surface the custom scheme to this handler.
      linkHandler: {
        allowNonHttpProtocols: true,
        activate: (_event, uri) => {
          if (uri.startsWith(PHXRUN_SCHEME)) {
            let decoded: string;
            try {
              decoded = decodeBase64Utf8(uri.slice(PHXRUN_SCHEME.length));
            } catch {
              return;
            }
            // A phxrun link must only place printable command text on the
            // prompt — never submit it or drive the terminal. The "review
            // before run" guarantee can't depend on a well-formed payload: any
            // process that writes to the terminal can emit a phxrun: link. So
            // cut at the first CR/LF (a bare CR submits too, via the PTY's
            // icrnl) and strip every other C0 control byte / DEL — ESC
            // sequences, Ctrl-C/Ctrl-D, tab-completion triggers. Multibyte
            // UTF-8 (>= U+0080) is preserved.
            const cr = decoded.indexOf('\r');
            const lf = decoded.indexOf('\n');
            let end = decoded.length;
            if (cr >= 0) end = Math.min(end, cr);
            if (lf >= 0) end = Math.min(end, lf);
            const command = Array.from(decoded.slice(0, end))
              .filter((ch) => {
                const c = ch.codePointAt(0) ?? 0;
                return c >= 0x20 && c !== 0x7f;
              })
              .join('');
            const ws = wsRef.current;
            if (command && ws && ws.readyState === WebSocket.OPEN) {
              ws.send(dataFrame(new TextEncoder().encode(command)));
            }
            return;
          }
          if (uri.startsWith('http://') || uri.startsWith('https://')) {
            window.open(uri, '_blank', 'noopener,noreferrer');
          }
        },
      },
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

    // Cmd/Ctrl+Shift+L: send the current terminal selection up to the
    // message composer (task 02672). Empty selection → silent no-op.
    // Returning `false` from the custom handler tells xterm.js to skip
    // its own processing of the event; preventDefault stops the browser
    // default (some browsers focus the URL bar on Cmd+L). The shortcut
    // works regardless of OSC 133 status or whether a command is running.
    // It reads xterm's own selection: under a direct shell any drag selects;
    // under `tmux attach` with mouse tracking on, the drag must escape tmux's
    // mouse grab to land an xterm-side selection — Shift+drag off macOS,
    // Option+drag on macOS (see macOptionClickForcesSelection above).
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== 'keydown') return true;
      const isSendSelection =
        e.shiftKey &&
        (e.metaKey || e.ctrlKey) &&
        !e.altKey &&
        (e.key === 'L' || e.key === 'l');
      if (!isSendSelection) return true;
      e.preventDefault();
      const cb = onSendSelectionToDraftRef.current;
      if (cb && term.hasSelection()) {
        const sel = term.getSelection();
        if (sel.length > 0) cb(sel);
      }
      return false;
    });

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
        console.debug('OSC 7 parse failed:', data);
        return;
      }
      try {
        const decoded = decodeURIComponent(m[1]!);
        setReportedCwd(decoded);
      } catch {
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

    // --- Drag-to-copy ---
    // Two mutually-exclusive paths per drag, both finishing on mouseup:
    //
    //   xterm owns the selection (Shift/Alt+drag escapes tmux's mouse grab, or a
    //   direct-shell child with no mouse tracking) — copied synchronously inside
    //   the mouseup gesture.
    //
    //   tmux owns the selection (plain drag under `mouse on`) — xterm has no DOM
    //   selection; tmux is configured `set-clipboard on` and emits OSC 52 with
    //   the copied text over the PTY *after* mouseup. WebKit only permits a
    //   clipboard write that *begins* inside a user gesture, so the async OSC 52
    //   text cannot call writeText directly (Chromium's standing clipboard-write
    //   permission lets it, WebKit's does not). Instead mouseup arms a
    //   ClipboardItem whose payload Promise the OSC 52 handler resolves.
    let pendingTmuxClipboard: { resolve: (blob: Blob) => void; reject: () => void } | null = null;

    const handleOsc52 = (data: string): void => {
      const semi = data.indexOf(';');
      const payload = semi === -1 ? data : data.slice(semi + 1);
      // A `?` payload is a clipboard *read* query; we never answer it (answering
      // would leak clipboard contents to whatever is running in the PTY).
      if (payload === '?' || payload === '') return;
      // The payload is attacker-influenceable PTY output. Bound it before
      // decoding so a pathological sequence can't force a multi-hundred-MB
      // atob/decode (and clipboard overwrite). 1 MiB of base64 (~768 KB of
      // text) is far past any realistic terminal selection.
      if (payload.length > 1024 * 1024) {
        console.debug('OSC 52 payload exceeds size cap; ignoring', payload.length);
        return;
      }
      let text: string;
      try {
        const bytes = Uint8Array.from(atob(payload), (c) => c.charCodeAt(0));
        text = new TextDecoder().decode(bytes);
      } catch {
        console.debug('OSC 52 base64 decode failed');
        return;
      }
      if (pendingTmuxClipboard) {
        // Fulfill the gesture-armed write from the preceding mouseup.
        pendingTmuxClipboard.resolve(new Blob([text], { type: 'text/plain' }));
        pendingTmuxClipboard = null;
        return;
      }
      // No armed write (browser lacks async ClipboardItem, or OSC 52 arrived
      // outside a drag) — direct write. Succeeds where the page holds standing
      // clipboard-write permission (Chromium); a silent no-op on WebKit.
      void copyToClipboard(text);
    };
    const osc52Dispose = term.parser.registerOscHandler(52, (data: string) => {
      handleOsc52(data);
      return true;
    });

    const canArmAsyncClipboard =
      typeof ClipboardItem !== 'undefined' && typeof navigator.clipboard?.write === 'function';

    const handleMouseUp = (): void => {
      if (term.hasSelection()) {
        const selection = term.getSelection();
        if (selection.length > 0) void copyToClipboard(selection);
        return;
      }
      // No xterm selection: a plain tmux drag whose OSC 52 is in flight. Arm a
      // clipboard write now, inside the gesture; handleOsc52 resolves it. If no
      // OSC 52 lands (plain click, or the drag selected nothing) the payload
      // Promise is rejected on timeout, leaving the clipboard untouched.
      if (!canArmAsyncClipboard) return;
      if (pendingTmuxClipboard) pendingTmuxClipboard.reject();
      let settle!: { resolve: (blob: Blob) => void; reject: () => void };
      const blobPromise = new Promise<Blob>((resolve, reject) => {
        settle = { resolve, reject };
      });
      pendingTmuxClipboard = settle;
      window.setTimeout(() => {
        if (pendingTmuxClipboard === settle) {
          pendingTmuxClipboard = null;
          settle.reject();
        }
      }, OSC52_ARM_TIMEOUT_MS);
      void navigator.clipboard
        .write([new ClipboardItem({ 'text/plain': blobPromise })])
        .catch(() => {
          // Rejected payload (timeout) or denied permission — clipboard intact.
        });
    };
    const termEl = containerRef.current;
    termEl.addEventListener('mouseup', handleMouseUp);

    // --- Detection timeout (REQ-TERM-015) ---
    detectionTimeoutRef.current = window.setTimeout(() => {
      detectionTimeoutRef.current = null;
      if (integrationStatusRef.current === 'unknown') {
        integrationStatusRef.current = 'absent';
        setIntegrationStatus('absent');
      }
    }, DETECTION_WINDOW_MS);

    // --- WebSocket connection ---
    const ws = new WebSocket(terminalWsUrl(scope));
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;
    setStatus('Connecting…');

    // Outbound input buffer. xterm hands us a keystroke the instant it's
    // typed, which can be before the WebSocket finishes its handshake
    // (readyState CONNECTING) — e.g. typing into a freshly-opened terminal
    // while the connection is still being established. Sending only when
    // OPEN silently dropped those bytes; instead we queue them here and
    // flush in order on open. A keystroke xterm accepted is therefore
    // always either in flight or queued — never discarded.
    let pendingInput: Uint8Array[] = [];
    const flushPendingInput = () => {
      if (pendingInput.length === 0) return;
      const queued = pendingInput;
      pendingInput = [];
      for (const payload of queued) {
        ws.send(dataFrame(payload));
      }
    };

    ws.onopen = () => {
      const { cols, rows } = term;
      // Resize frame first: the server's initial handshake reads frames
      // until the first resize and the PTY is sized from it. Flushing
      // buffered input only after the resize keeps that ordering intact.
      ws.send(resizeFrame(cols, rows));
      flushPendingInput();
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
      const encoded = new TextEncoder().encode(text);
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(dataFrame(encoded));
      } else if (ws.readyState === WebSocket.CONNECTING) {
        // Pre-handshake: queue rather than drop; onopen flushes in order, so
        // input typed into a freshly-opened terminal reaches the shell once
        // the handshake completes. The debug log makes a would-be-dropped
        // keystroke visible (capability gaps are logged, not silenced) so a
        // recurrence is diagnosable.
        pendingInput.push(encoded);
        console.debug(
          `[terminal] buffered ${encoded.length}B of input during WS handshake (readyState=CONNECTING)`,
        );
      }
      // CLOSING/CLOSED: the session is dead (e.g. "Shell exited"). This
      // socket can never fire onopen, so buffering would leak memory + logs
      // and never deliver. Drop — clicking reconnect spawns a fresh WS with
      // its own buffer.
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
        osc52Dispose.dispose();
        if (pendingTmuxClipboard) {
          pendingTmuxClipboard.reject();
          pendingTmuxClipboard = null;
        }
        termEl.removeEventListener('mouseup', handleMouseUp);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scopeId, setStatus, reconnectNonce]);

  const reconnect = useCallback(() => {
    setReconnectNonce((n) => n + 1);
  }, []);

  // Reapply the xterm theme when the app theme changes (task 24681).
  // xterm.js's `options.theme` is a getter/setter pair; assigning a fresh
  // ITheme triggers an internal redraw without needing to tear down the
  // PTY or remount the terminal. The colours come from the same CSS
  // variables the chrome reads in index.css, so the whole panel switches
  // atomically.
  const { theme } = useTheme();
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    term.options.theme = readXtermTheme();
  }, [theme]);

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
    if (await copyToClipboard(snippet.snippet)) {
      setCopyAck(true);
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
      console.error('Assist setup failed:', err);
      showError?.('Could not start shell-integration assistant — try again', 4000);
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
          Shell exited —{' '}
          <strong className="terminal-panel-prompt-cta">click</strong>
          {' '}to start a new one
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
      let glyphNode: JSX.Element;
      let glyphClass: string;
      let suffix: string;
      if (code === 0) {
        glyphNode = <CheckIcon />;
        glyphClass = 'terminal-hud-glyph terminal-hud-glyph--ok';
        suffix = `(${dur})`;
      } else if (code === null) {
        glyphNode = <CircleDot />;
        glyphClass = 'terminal-hud-glyph terminal-hud-glyph--unknown';
        suffix = `(${dur})`;
      } else {
        glyphNode = <XIcon />;
        glyphClass = 'terminal-hud-glyph terminal-hud-glyph--err';
        suffix = `(exit ${code})`;
      }
      return (
        <span className="terminal-panel-prompt">
          <span className="terminal-hud-cwd">{formatCwdPlain(effectiveCwd)}</span>
          <span className={glyphClass}> {glyphNode} </span>
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
    <div className={panelClass} style={{ height: `${height}px` }}>
      <div
        className={`terminal-panel-header${collapsed ? ' terminal-panel-header--collapsed' : ''}`}
        onClick={handleHeaderClick}
        style={headerClickable ? { cursor: 'pointer' } : undefined}
      >
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
          {collapsed ? <ChevronUpHeader /> : <ChevronDownHeader />}
        </button>
        <span
          className={`terminal-live-dot-wrap${integrationStatus === 'absent' ? ' terminal-live-dot-wrap--hint' : ''}`}
          onMouseEnter={handleDotMouseEnter}
          onMouseLeave={handleDotMouseLeave}
          onClick={handleDotClick}
        >
          <span className={dotClass} aria-hidden="true" />
          {hintTooltipVisible && integrationStatus === 'absent' && (
            <span className="terminal-hint-tooltip" role="tooltip">
              <strong><AlertTriangleInline />Shell integration not detected ({shellLabel})</strong>
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
