# Terminal Panel — Executive Summary

## Scope and Boundary

This spec governs the **frontend `TerminalPanel.tsx` component** — the React surface that mounts xterm.js, manages the WebSocket connection to the backend PTY, parses OSC escape sequences in the browser, renders the HUD overlay, and offers the shell-integration setup affordance.

The companion backend spec — `specs/terminal/` — owns the PTY spawn, the WebSocket protocol, the server-side vt100 parser, the `read_terminal` agent tool, and the conversation-teardown cascade. That spec explicitly defers "UI panel placement" to the implementation; this document fills that gap.

**In scope here:**
- xterm.js mount/dispose lifecycle and theme integration
- WebSocket connection state machine as the panel sees it
- Browser-side OSC 133 parsing and the 5-second shell-integration detection window
- Command lifecycle tracking (currentCommand, lastCompletedCommand)
- HUD overlay variants (disconnected, unknown, absent, running, idle)
- Fallback byte-activity sampler when shell integration is absent
- Shell-integration-absent CTA: hint tooltip + snippet modal + "Let Phoenix set this up" seeded conversation
- Collapse/expand state preservation and unread-line tracking
- Reconnect affordance (manual, via header click)
- OSC 7 cwd reporting and HUD fallback

**Owned by other specs:**
- `specs/terminal/` — PTY lifecycle, WebSocket protocol, OSC 133 marker semantics on the server side, agent tools, single-attach 409 enforcement
- `specs/seeded-conversations/` — the route + draft-prefill mechanism the assist-setup CTA uses
- `specs/conversation-ui/` — the parent layout that hosts the panel
- `specs/keyboard-interaction/` — global keyboard shortcuts (panel doesn't define its own)

## Why It Exists

Phoenix's backend gives the agent and the user a real PTY-backed terminal per conversation. The frontend panel turns that contract into something the user can actually interact with: an xterm.js viewport, a status HUD that surfaces the current command and its outcome, and an opinionated path to install shell integration when it's missing. The panel also bridges the gap between the backend's "session is active or not" model and the user's "is this terminal actually working right now?" question — through a handful of fallback states (unknown / absent / disconnected) that the backend doesn't model directly.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-TPANEL-001:** xterm.js Lifecycle & Container Management | ✅ Complete | `ui/src/components/TerminalPanel.tsx:341-359` (mount with FitAddon), `:596-607` (dispose) |
| **REQ-TPANEL-002:** WebSocket Connection Lifecycle | ✅ Complete | `:481-547` (state transitions); single-attach conflict handling is incomplete — see REQ-TPANEL-013 |
| **REQ-TPANEL-003:** Binary Frame Protocol | ✅ Complete | `:131-146` (encode/decode 0x00 data + 0x01 resize) |
| **REQ-TPANEL-004:** OSC 133 Shell Integration Detection | ✅ Complete | `:123` (5s window const), `:365-387` (detected path), `:472-478` (timeout → absent), monotonic lock at `:366-369,390` |
| **REQ-TPANEL-005:** Command Lifecycle Tracking | ✅ Complete | `:399-444` (OSC 133 A/B/C/D parse), `:268-273` (currentCommand + lastCompletedCommand state) |
| **REQ-TPANEL-006:** HUD Overlay — Five Variants | ✅ Complete | `:789-873` (renderCollapsedHud); five paths: disconnected / unknown / absent / running / idle |
| **REQ-TPANEL-007:** Fallback Byte-Activity Sampler | ✅ Complete | `:519-526` (500ms decay), gated on `integrationStatus !== 'detected'` per `:506-508` |
| **REQ-TPANEL-008:** Shell Integration Absent-State CTA | ✅ Complete | Hint tooltip `:904-913`; snippet modal `:933-999`; copy + assist actions `:746,750-755,768-783` |
| **REQ-TPANEL-009:** Seeded Conversation for Assist Setup | 🚧 Partial | `:184-225` builds the prompt, `:768-783` invokes the parent callback. Errors currently surface only to `console.error` — a user-visible toast is missing |
| **REQ-TPANEL-010:** Theme Integration via CSS Variables | ✅ Complete | `:70-81` (read), `:343` (apply on mount), `:628-633` (re-apply on theme toggle) |
| **REQ-TPANEL-011:** Collapse/Expand Preservation & Unread Tracking | ✅ Complete | `:299-301,930` (display:none preserves PTY/WS/scrollback); unread counter `:500-505,664-670,921-925` |
| **REQ-TPANEL-012:** OSC 7 CWD Reporting | ✅ Complete | `:456` (parse), `:732` (HUD fallback to conversation cwd) |
| **REQ-TPANEL-013:** Single-Attach Conflict Resolution (409) | ❌ Not Started | Backend rejects duplicate connections with 409 (REQ-TERM-001 / -003); frontend currently treats this as a generic connection error. Spec target: detect the 409 close code and offer a "Reclaim this terminal" action (or equivalent) instead of a silent retry loop |

**Progress:** 11 of 13 complete, 1 partial, 1 not started. Implementation surface is mature; the gaps are in error visibility (REQ-TPANEL-009) and the unmodelled 409-conflict UX (REQ-TPANEL-013).

## Behavioural Specification

`specs/terminal-panel/terminal-panel.allium` models the four state machines that interact inside the panel:

- `WebSocketLifecycle`: `not_connected → connecting → connected → {disconnected}`. Disconnected is a terminal state until `reconnect()` is invoked, which increments a nonce and forces a fresh `not_connected` instance.
- `IntegrationDetection`: `unknown → detected | absent` (monotonic; 5s window).
- `CommandTracker`: `idle → running → completed`, fed by OSC 133 A/B/C/D markers.
- `ActivitySampler`: `idle ↔ running`, fed by raw byte arrival; gated off when integration is `detected` (HUD uses CommandTracker instead) and locked to `disconnected` once the WS closes until the next nonce-bumped reconnect.

Open questions: see REQ-TPANEL-013 (409 conflict resolution) and the rationale block on `IntegrationDetectionWindowMirrored` (the 5-second window is hard-coded on both sides; a backend change would silently desynchronise).

## Cross-Spec Cross-References

- `specs/terminal/`: REQ-TERM-001 through REQ-TERM-023 own the backend session, the WebSocket protocol, and the server-side parser. The frontend respects that protocol and adds its own lifecycle on top.
- `specs/seeded-conversations/`: the assist-setup CTA hands off to a seeded conversation via the parent's `onAssistSetup(promptText, seedLabel, homeDir)` callback. The seed-prefill mechanism is owned there.
- `specs/conversation-ui/`: the parent layout decides where the panel sits (mobile bottom strip vs desktop side pane); panel itself is layout-agnostic.
