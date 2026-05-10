# Terminal Panel — Executive Summary

## Scope and Boundary

This spec governs the **frontend `TerminalPanel.tsx` component** — the React surface that mounts xterm.js, manages the WebSocket connection to the backend PTY, parses OSC escape sequences in the browser, renders the HUD overlay, and offers the shell-integration setup affordance.

The companion backend spec — `specs/terminal/` — owns the PTY spawn, the WebSocket protocol, the server-side vt100 parser, the `terminal_last_command` / `terminal_command_history` agent tools (replacements for the original `read_terminal` tool, split at commit `99c5df1`), and the conversation-teardown cascade. That spec explicitly defers "UI panel placement" to the implementation; this document fills that gap.

Note: agent-spawned commands have a separate path via the `tmux` tool (`specs/tmux-integration/`). The `terminal_*_command` tools are user-mediated — they let the agent observe what the user just ran in the panel — and remain useful even with tmux, because tmux gives the agent its own persistent session rather than visibility into the user's interactive one.

**In scope here (user-facing experiences):**
- The terminal experience: connecting, typing into a real shell, seeing output as if I'd opened a local terminal
- Knowing whether the terminal is alive, and recovering when it isn't (without losing the rest of my conversation)
- Live command status: what's running, what just finished and how
- The current working directory in the header, tracking `cd` when shell integration supports it
- Output that survives across collapse/expand, with an unread hint when something happened while I was away
- Being told when shell integration is missing, with a concrete path to enable it (snippet, copy, or a guided conversation)
- A theme that matches the rest of the app
- A specific recovery path when the same terminal is open in another tab — not a silent take-over

**Owned by other specs:**
- `specs/terminal/` — the backend PTY session, WebSocket protocol, OSC marker semantics on the server side, the `terminal_last_command` / `terminal_command_history` agent tools (replacements for the original `read_terminal` tool, split at commit 99c5df1), single-attach 409 enforcement
- `specs/seeded-conversations/` — the route + draft-prefill mechanism the assist-setup CTA uses
- `specs/conversation-ui/` — the parent layout that hosts the panel
- `specs/keyboard-interaction/` — global keyboard shortcuts (panel doesn't define its own)

## Why It Exists

Phoenix's backend gives the agent and the user a real PTY-backed terminal per conversation. The frontend panel turns that contract into something the user can actually interact with: an xterm.js viewport, a status HUD that surfaces the current command and its outcome, and an opinionated path to install shell integration when it's missing. The panel also bridges the gap between the backend's "session is active or not" model and the user's "is this terminal actually working right now?" question — through a handful of fallback states (unknown / absent / disconnected) that the backend doesn't model directly.

## Status Summary

Status is per user-visible outcome. Code anchors point at the implementation that delivers each outcome — see `design.md` for the architecture across xterm.js, the WebSocket client, OSC parsing, and the HUD render.

| Requirement | Status | Notes |
|---|---|---|
| **REQ-TPANEL-001:** Open and Use a Terminal | ✅ Complete | `ui/src/components/TerminalPanel.tsx:341-359` (xterm + FitAddon mount), `:481-547` (WebSocket I/O), `:131-146` (binary frame protocol), `:636-653` (resize handling) |
| **REQ-TPANEL-002:** Disconnect Is Visible, Recovery Is Explicit | ✅ Complete | `:539-547` (disconnect state), `:790-797` (Disconnected HUD with click-to-reconnect), `:618-620` (reconnect bumps nonce, fresh PTY); explicit no-auto-retry policy |
| **REQ-TPANEL-003:** Live Command Status in the Header | ✅ Complete | `:789-873` (HUD render across 5 variants), `:399-444` (OSC 133 A/B/C/D parse driving running/idle), `:519-526` (500ms byte-activity decay for the absent-integration fallback) |
| **REQ-TPANEL-004:** Current Working Directory in the Header | ✅ Complete | `:456` (OSC 7 parse), `:732` (fallback to conversation cwd at render time) |
| **REQ-TPANEL-005:** Output Survives Collapse, with Unread Hint | ✅ Complete | `:299-301,930` (display:none preserves PTY/WS/scrollback), `:500-505,664-670,921-925` (unread counter) |
| **REQ-TPANEL-006:** Shell Integration Setup CTA | ✅ Complete | Detection + hint + snippet modal (`:123,365-387,472-478,904-913,933-999`); the "Let Phoenix set this up" hand-off (`:184-225,768-783`) surfaces failures via `showError` (red toast) in addition to `console.error` — see REQ-NOTIF-002 |
| **REQ-TPANEL-007:** Theme Matches the App | ✅ Complete | `:70-81` (read CSS vars), `:343` (apply on mount), `:628-633` (re-apply on theme toggle, no PTY teardown) |
| **REQ-TPANEL-008:** Conflict Resolution When Already Open Elsewhere | ❌ Not Started | Backend rejects duplicate connections with 409 (REQ-TERM-001 / -003); frontend folds this into a generic "Connection error" today (`:539-547`) with no reclaim path. Spec target: distinguish the 409 close code and offer a "Reclaim this terminal" action |

**Progress:** 7 of 8 complete, 1 not started. Remaining gap is the unmodelled conflict UX in REQ-TPANEL-008 (needs a backend reclaim endpoint coordinated with `specs/terminal/`).

## Behavioural Specification

`specs/terminal-panel/terminal-panel.allium` models the four state machines that produce the user-visible behaviour above:

- `WebSocketLifecycle` — drives REQ-TPANEL-001 (open and use) and REQ-TPANEL-002 (disconnect visibility + recovery). Disconnected is sticky until reconnect; no auto-retry, by design.
- `IntegrationDetection` — drives REQ-TPANEL-006 (shell integration missing → CTA). Monotonic: once a 5-second window has settled to detected or absent, that state is final for the connection lifetime.
- `CommandTracker` — drives REQ-TPANEL-003 (running command + last completed command). Fed by OSC 133 A/B/C/D markers from the shell.
- `ActivitySampler` — fallback HUD source when `IntegrationDetection = absent`; the byte-arrival heuristic that produces the coarser "shell is producing output" / "shell is quiet" indicator REQ-TPANEL-003 references in its rationale.

Open questions: see REQ-TPANEL-008 (409 conflict resolution) and the design.md note on the 5-second detection window being hard-coded on both sides (a backend change would silently desynchronise).

## Cross-Spec Cross-References

- `specs/terminal/`: REQ-TERM-001 through REQ-TERM-023 own the backend session, the WebSocket protocol, and the server-side parser. The frontend respects that protocol and adds its own lifecycle on top.
- `specs/seeded-conversations/`: the assist-setup CTA hands off to a seeded conversation via the parent's `onAssistSetup(promptText, seedLabel, homeDir)` callback. The seed-prefill mechanism is owned there.
- `specs/conversation-ui/`: the parent layout decides where the panel sits (mobile bottom strip vs desktop side pane); panel itself is layout-agnostic.
