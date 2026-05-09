# Terminal Panel Design

This document describes the technical architecture for the
`TerminalPanel.tsx` frontend component, implementing
`specs/terminal-panel/requirements.md`.

## Component Boundary

```
┌─ Conversation page (specs/conversation-ui/) ──────────────────────┐
│                                                                  │
│   ┌─ TerminalPanel.tsx (this spec) ─────────────────────────┐   │
│   │                                                         │   │
│   │   ┌─ HUD overlay (header) ──────────────────────────┐  │   │
│   │   │  activity dot · cwd $ command · duration        │  │   │
│   │   └─────────────────────────────────────────────────┘  │   │
│   │                                                         │   │
│   │   ┌─ xterm.js container ───────────────────────────┐  │   │
│   │   │  (display:none when collapsed)                  │  │   │
│   │   └─────────────────────────────────────────────────┘  │   │
│   │                                                         │   │
│   │   ┌─ Snippet modal (when integrationStatus=absent) ┐  │   │
│   │   │  shell snippet · copy · "set this up for me"   │  │   │
│   │   └─────────────────────────────────────────────────┘  │   │
│   │                                                         │   │
│   │   WebSocket: /api/conversations/:id/terminal           │   │
│   │              binary: 0x00 data ↕  0x01 resize →         │   │
│   └─────────────────────────────────────────────────────────┘   │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
                              ↕ binary frames
┌─ Backend (specs/terminal/) ───────────────────────────────────────┐
│   PTY · vt100 parser · OSC 133 server-side · agent tools         │
└──────────────────────────────────────────────────────────────────┘
```

The frontend OWNS xterm.js and the WebSocket client. The backend OWNS
the PTY, the vt100 parser (server-side, for `read_terminal`), and the
session lifecycle. They share only the binary frame protocol
(REQ-TERM-004) and the OSC 133/OSC 7 marker format (REQ-TERM-015..018).

## State Machines

The panel runs four interacting state machines plus a handful of
purely-local UI flags. The state machines are formalised in
`terminal-panel.allium`; this section explains them in prose.

### WebSocketLifecycle

`not_connected → connecting → connected → disconnected`

- `connected` is the steady state during which binary frames flow.
- `disconnected` is a sticky state: `error` and `close` both land
  there, and the activity sampler is locked to it until the user
  explicitly invokes `reconnect()`.
- `reconnect()` increments a nonce; the panel's mount effect depends
  on the nonce, so a bump tears down xterm + WS and re-runs from
  `not_connected`.

The single-attach 409 from the backend (REQ-TERM-001) currently lands
in `disconnected` indistinguishably from any other close reason. Spec
target: a fifth state `conflict` with a "Reclaim" transition (see
REQ-TPANEL-008).

### IntegrationDetection

`unknown → detected | absent`

- `unknown` is the initial state, in effect for the 5-second detection
  window.
- `detected` fires on the first OSC 133;C marker arrival within the
  window.
- `absent` fires on window expiry.
- Both `detected` and `absent` are absorbing — once entered, the
  machine cannot leave them. The next reconnect (which allocates a
  fresh xterm + handlers) re-runs detection.

The 5-second window mirrors the backend's
`shell_integration_detection_window` config (REQ-TERM-015 in the
backend spec). The two are not synced at runtime; if the backend
changes its value, the frontend constant must change too. Tracked as a
known mirroring (see Open Questions).

### CommandTracker

`idle → running → completed → idle (next C marker)`

- `running` records `commandText`, `startedAt`.
- `completed` records `exitCode` (nullable), `finishedAt`.
- A new `running` transition on the next C marker moves the previous
  completed command into `lastCompletedCommand` (the HUD's "what just
  happened" slot).

This machine drives the HUD's running and idle variants
(REQ-TPANEL-003). It only runs when integration is detected; without
OSC 133, the panel cannot construct command boundaries.

### ActivitySampler

`idle ↔ running`, plus the sticky `disconnected`

- Driven by raw byte arrival on the WebSocket.
- `idle → running` on any data frame.
- `running → idle` on a 500ms quiet timer.
- Locked to `disconnected` once `WebSocketLifecycle = disconnected`,
  reset only by the next reconnect's nonce bump.

This machine is the fallback HUD source when integration is `absent`.
When integration is `detected`, the HUD ignores it and uses
`CommandTracker` instead.

### Local UI flags (not state machines)

These are useState booleans/values; no transition graph needed.

- `snippetModalOpen` — boolean, gated by `integrationStatus = absent`
  + user click.
- `hintTooltipVisible` — boolean, hover state on the activity dot.
- `copyAck` — boolean, 1.5s flash after copy.
- `assistInFlight` — boolean, async guard while the parent's
  `onAssistSetup` callback is in flight.
- `unreadDisplay` / `unreadRef` — counter, batched line count while
  collapsed.

## React Effect Topology

```
mount (deps: conversationId, reconnectNonce)
├── Allocate xterm.js + FitAddon (deferred via setTimeout to dodge
│   StrictMode double-invoke + dispose race)
├── Register OSC 133, OSC 7 handlers
├── Open WebSocket
├── Wire ws.onopen/onmessage/onerror/onclose
├── Start 5s detection timer
└── return () => {
      Dispose xterm + addon + handlers
      Clear timers + refs
      Null ws handlers + close ws
    }

mount (deps: theme)
└── Re-read CSS vars, assign term.options.theme

mount (deps: collapsed)
└── If expanded: reset unread, call fit()

interval (deps: currentCommand)
└── Every 100ms while running, force re-render via setRunningTick to
    update the live duration display
```

The deferred allocation in the main mount effect handles React 18's
StrictMode: a synchronous mount → cleanup → mount sequence on the
first render would otherwise allocate xterm twice and fight over the
WebSocket. The `setTimeout(0)` + cancel-on-cleanup pattern is a
specific workaround for this case.

## Theme Integration

Three CSS variables on `:root[data-theme=...]`:

```css
--terminal-bg     /* xterm background */
--terminal-fg     /* xterm foreground (default text) */
--terminal-cursor /* cursor colour */
```

Hardcoded fallbacks in `readXtermTheme()` (`TerminalPanel.tsx:70-81`)
guard against missing variables. The fallbacks deliberately match the
dark theme — a missing CSS variable is more likely to indicate a
temporary unstyled-content flash than a deliberate light-theme
intention.

## OSC Parsing

xterm.js exposes OSC handlers via `term.parser.registerOscHandler(N,
fn)`. The panel registers two:

- **OSC 133** for FinalTerm shell integration (commands, prompts, exit
  codes). The handler dispatches on the sub-marker letter (A/B/C/D).
- **OSC 7** for current-working-directory reporting. The handler
  parses the `file://hostname/path` URL and updates `reportedCwd`.

Both handlers are registered on mount and disposed on unmount/reconnect
via the dispose handles xterm returns. They run inside xterm.js's
parser pipeline, so the byte stream itself is never observed
out-of-order.

## HUD Rendering Decision Tree

```
isDisconnected ? Disconnected
  : integrationStatus === 'unknown' ? Unknown
  : integrationStatus === 'absent' ?
      currentCommand ? Running
      : lastCompletedCommand ? Idle
      : Absent (static cwd, fallback)
  : (integrationStatus === 'detected')
      currentCommand ? Running
      : lastCompletedCommand ? Idle
      : Absent (static cwd; integration is detected but no command yet)
```

The activity dot colour is computed independently from the same inputs
plus the byte-activity sampler — but only when integration is absent,
per the rationale in REQ-TPANEL-003 (the system never makes up
command outcomes the shell didn't report).

## Shell Integration Setup CTA

Snippet catalog lives in `ui/src/shellIntegrationSnippets.ts` (one
entry per supported shell: bash, zsh, fish). The catalog provides:

- `snippet`: the actual shell code to paste into the rc file
- `rcFile`: the suggested rc file path (e.g. `~/.zshrc`)
- `shellDisplayName`: a user-facing name

The "Let Phoenix set this up for me" path delegates to the parent via
`onAssistSetup(promptText, seedLabel, homeDir)`. The parent (a
conversation-page or app-level handler) is expected to:

1. Create a new conversation seeded with `homeDir` as cwd.
2. Pre-fill the input area with `promptText`.
3. Tag the conversation with `seedLabel` for breadcrumb context.

The mechanism is `specs/seeded-conversations/` (REQ-SEED-001 through
-004); the panel just builds the prompt and hands off.

## Cleanup Race with xterm.js Renderer

xterm.js's renderer schedules updates via `requestAnimationFrame`. If
a rAF callback fires after `term.dispose()` has nulled internal
references but before the rAF queue is drained, it throws "renderer
dimensions is not an object". The cleanup path swallows this
specifically because the alternative — racing the dispose against the
rAF cancellation — has no clean answer in xterm.js's public API. See
TerminalPanel.tsx:599-603 for the swallowed try/catch.

## Open Questions

- **REQ-TPANEL-008 (409 conflict UX)**: target spec'd, implementation
  is a known gap. The reclaim flow needs a backend endpoint
  (`DELETE /api/conversations/:id/terminal` or similar) that revokes
  the existing session before this client reconnects. Coordinate with
  `specs/terminal/` before implementation.

- **Detection-window mirroring**: The 5-second detection window is
  hardcoded as `DETECTION_WINDOW_MS = 5000` on the frontend
  (`TerminalPanel.tsx:123`) and as
  `shell_integration_detection_window: Duration = 5.s` on the backend
  (`specs/terminal/terminal.allium`). A backend change would silently
  desync. Options: (a) expose the window via the WebSocket init
  handshake; (b) accept the manual coordination cost and add a
  comment on each side pointing at the other. v1 takes option (b);
  re-evaluate when there's actual demand to change the window.

- **Assist-setup error visibility (REQ-TPANEL-006)**: the `console.error`
  fallback is the current implementation; the spec requires user-visible
  failure surfacing. Pick a mechanism — toast (specs/notifications/),
  modal-inline error, or banner on the panel header — and implement.

- **Activity sampler explicit guard**: the byte-activity sampler runs
  unconditionally inside `ws.onmessage`; its effects are gated by
  reading `integrationStatusRef`. Restructuring so the sampler skips
  the work entirely when integration is detected would be marginally
  cleaner but the runtime cost is negligible (a single ref read per
  message). Not blocking.
