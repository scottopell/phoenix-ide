# Terminal Panel

## Scope

This spec governs the frontend `TerminalPanel.tsx` component. The backend
PTY session, WebSocket protocol, and OSC marker semantics are owned by
`specs/terminal/`; that spec explicitly defers UI panel placement to the
implementation, and this document fills that gap. See
`specs/terminal-panel/executive.md` for the full boundary.

## User Story

As a Phoenix user inside a conversation, I want a terminal pane I can
open without leaving the page, with my real shell, dotfiles, and
working directory — and I want to be able to tell at a glance whether
the shell is alive, whether a command is running, and whether the last
command succeeded or failed, even when the terminal pane is collapsed
out of the way.

## Transparency Contract

The panel must let the user confidently answer:

1. Is the terminal connected right now? (live / disconnected)
2. If disconnected, can I get it back without losing context?
3. Is a command currently running? Which one? For how long?
4. What was the last command, and did it succeed?
5. Is shell integration enabled? If not, how do I enable it?
6. Did the panel preserve my output across collapse/expand?

Each numbered question maps to one or more requirements below.

---

## Requirements

### REQ-TPANEL-001: xterm.js Lifecycle and Container Management

WHEN the panel mounts for a conversation
THE SYSTEM SHALL allocate a single xterm.js `Terminal` instance with a
`FitAddon` attached, sized to fit the panel container

WHEN the panel container resizes (window resize, parent layout change,
collapse/expand)
THE SYSTEM SHALL invoke `FitAddon.fit()` to recompute terminal dimensions
AND emit a resize frame on the WebSocket so the backend PTY agrees on
geometry (REQ-TERM-006)

WHEN the panel unmounts OR a fresh reconnect is requested
THE SYSTEM SHALL dispose the xterm instance and the FitAddon, detach all
event handlers, and clear all timers and refs

WHEN the panel collapses (display:none) without unmounting
THE SYSTEM SHALL keep the xterm instance, scrollback, WebSocket, and PTY
state intact (see REQ-TPANEL-011)

**Rationale:** xterm.js is expensive to construct and has global side
effects (renderer rAF loop). One instance per panel mount, disposed
deterministically. Collapse-without-unmount keeps long-running output
preserved.

---

### REQ-TPANEL-002: WebSocket Connection Lifecycle

WHEN the panel mounts and the conversation has a valid id
THE SYSTEM SHALL open a binary WebSocket to
`/api/conversations/:id/terminal` with the same session credentials the
rest of the app uses (REQ-TERM-013)

WHEN the WebSocket transitions to `open`
THE SYSTEM SHALL send the initial resize frame (REQ-TERM-005), clear any
status banner text, and transition the activity sampler to `idle`

WHEN the WebSocket fires `error` or `close`
THE SYSTEM SHALL transition the activity sampler to `disconnected`,
display "Shell exited" or "Connection error" in the HUD, and lock the
sampler in `disconnected` until an explicit user-initiated reconnect

WHEN the user clicks the disconnected HUD header
THE SYSTEM SHALL increment a reconnect nonce that re-runs the mount
effect: dispose the dead WS + xterm, allocate fresh instances, and
re-open the WebSocket from a clean slate

**Rationale:** The panel does not auto-retry. Auto-retry hides the fact
that something went wrong and makes intermittent failures invisible;
manual reconnect keeps the user in the loop and avoids retry storms when
the backend is genuinely down.

---

### REQ-TPANEL-003: Binary Frame Protocol (Input/Output)

WHEN the user types into the terminal
THE SYSTEM SHALL encode the keystroke bytes as `0x00 + utf8(bytes)` and
send via WebSocket

WHEN the panel computes a new terminal size
THE SYSTEM SHALL encode it as `0x01 + u16be(cols) + u16be(rows)` and
send via WebSocket

WHEN a binary frame arrives with first byte `0x00`
THE SYSTEM SHALL feed the remaining bytes verbatim into xterm.js's
`write()` method, preserving order

WHEN a binary frame arrives with any other first byte
THE SYSTEM SHALL discard it silently (forward-compatible: a future
backend variant may introduce new frame kinds)

**Rationale:** Mirrors the backend frame contract (REQ-TERM-004). The
discard-on-unknown-kind behaviour is the conservative choice for
forward compat — the alternative (crash on unknown kind) makes the
backend hard to evolve.

---

### REQ-TPANEL-004: OSC 133 Shell Integration Detection

WHEN the panel mounts
THE SYSTEM SHALL register an OSC 133 handler on the xterm instance and
start a 5-second detection window timer

WHEN an OSC 133;C marker arrives within the detection window
THE SYSTEM SHALL transition `integrationStatus` from `unknown` to
`detected` and cancel the detection timer

WHEN the 5-second timer fires without any OSC 133 marker
THE SYSTEM SHALL transition `integrationStatus` from `unknown` to
`absent`

WHEN `integrationStatus` is `detected` or `absent`
THE SYSTEM SHALL NOT transition it back to `unknown` and SHALL NOT
re-arm the detection timer for the lifetime of this connection

**Rationale:** Monotonic. A flapping detection state would produce a
flapping HUD. The detection window applies to the connection lifetime,
not the panel lifetime — a fresh reconnect re-runs detection because it
allocates a new xterm + new handlers (REQ-TPANEL-001).

---

### REQ-TPANEL-005: Command Lifecycle Tracking

WHEN an OSC 133;C marker arrives with a payload
THE SYSTEM SHALL record a new `currentCommand` with `commandText` from
the payload, `startedAt = Date.now()`, and `lastCompletedCommand = null`

WHEN an OSC 133;D marker arrives while `currentCommand` is non-null
THE SYSTEM SHALL parse the optional exit code from the payload, set
`currentCommand.exitCode` and `finishedAt = Date.now()`, then move
`currentCommand` to `lastCompletedCommand` and clear `currentCommand`

WHEN an OSC 133;D marker arrives while `currentCommand` is null
THE SYSTEM SHALL discard it silently (the user pressed Enter without
typing a command, or the shell emits stray markers)

WHEN an OSC 133;A or OSC 133;B marker arrives
THE SYSTEM SHALL ignore it (prompt-boundary markers are not used by the
HUD; they exist only for command-text capture which the C payload
already provides)

**Rationale:** The shell is the source of truth; the panel just
mirrors. Mismatched D-without-C is silent because shells legitimately
emit the pattern in some edge cases (e.g., empty Enter at the prompt).

---

### REQ-TPANEL-006: HUD Overlay — Five Variants

WHEN the panel is collapsed OR `activitySampler` is `disconnected`
THE SYSTEM SHALL render the HUD overlay in the panel header

The HUD has five render paths, each driven by the combination of
`activitySampler` + `integrationStatus` + `currentCommand` +
`lastCompletedCommand`:

| Variant | Trigger | Content |
|---|---|---|
| Disconnected | `activitySampler = disconnected` | "Shell exited — click to reconnect" |
| Unknown | `integrationStatus = unknown` and not disconnected | Placeholder "❯_ Terminal" |
| Absent | `integrationStatus = absent` and `currentCommand = null` and `lastCompletedCommand = null` | Static cwd (truncated to 40 chars) |
| Running | `currentCommand != null` | cwd + `$ commandText` + live elapsed duration |
| Idle | `lastCompletedCommand != null` and `currentCommand = null` | cwd + ✓/✗/• glyph + commandText + (exit code or duration) |

Glyph mapping:
- `exitCode === 0` → ✓ (success)
- `exitCode > 0` → ✗ (failure)
- `exitCode === null` → • (unknown — shell omitted the D payload)

**Rationale:** Five variants because the four state-machine combinations
that matter (live state, integration state, currently running,
last-completed) collapse to five rendering decisions. The Disconnected
override comes first because it's the only state where reconnection is
the next user action; everything else is "you're connected, here's
what's happening."

---

### REQ-TPANEL-007: Fallback Byte-Activity Sampler

WHEN `integrationStatus` is `detected`
THE SYSTEM SHALL use `currentCommand` and `lastCompletedCommand` as the
source of truth for the HUD's activity indicator

WHEN `integrationStatus` is NOT `detected`
THE SYSTEM SHALL drive the HUD's activity indicator from the
byte-activity sampler:
- WebSocket message arrival transitions `activitySampler` from `idle`
  to `running`
- A 500ms quiet timer with no further bytes transitions `running` back
  to `idle`

WHEN `activitySampler` is `disconnected`
THE SYSTEM SHALL NOT promote it back to `idle` or `running` from byte
arrival; only an explicit reconnect (REQ-TPANEL-002) clears the lock

**Rationale:** Without OSC 133, the panel cannot tell what the shell is
doing — only that bytes are flowing. The 500ms decay is a heuristic
that "the shell is producing output continuously" maps to "running."
This is best-effort UX; users with detected shell integration get the
authoritative path.

---

### REQ-TPANEL-008: Shell Integration Absent — Setup CTA

WHEN `integrationStatus = absent` AND the user hovers the activity dot
in the HUD
THE SYSTEM SHALL show a hint tooltip: "⚠️ Shell integration not
detected (`<shell-name>`)" with subtext "Click for `<shell-name>`
snippet"

WHEN the user clicks the activity dot in the absent state
THE SYSTEM SHALL open a modal showing the shell-specific snippet, the
suggested rc-file path, and two action buttons: "Copy to clipboard"
and "Let Phoenix set this up for me"

WHEN the user closes the modal (button, ESC, backdrop click)
THE SYSTEM SHALL hide it and restore focus to the panel

WHEN the user's shell is not recognised by the snippet catalog
THE SYSTEM SHALL show a fallback message in the modal explaining that
the user's shell may still work if it emits OSC 133 / OSC 7 markers,
without offering a paste-in snippet

**Rationale:** The absent state is the hand-off point between Phoenix
and the user. The snippet modal makes the path explicit; the assist
button (REQ-TPANEL-009) automates it for users who don't want to edit
their dotfiles by hand.

---

### REQ-TPANEL-009: Seeded Conversation for Assist Setup

WHEN the user clicks "Let Phoenix set this up for me" in the snippet
modal
THE SYSTEM SHALL build a detailed prompt instructing the agent to
inspect the user's dotfiles, identify the right rc file, and safely
apply the snippet
AND invoke the parent's `onAssistSetup(promptText, seedLabel, homeDir)`
callback to spawn a seeded conversation in `$HOME` (see
`specs/seeded-conversations/`)

WHEN the assist-setup callback fails
THE SYSTEM SHALL surface the failure visibly to the user (toast,
modal-inline error, or equivalent) — not only via `console.error`

WHEN the assist-setup callback is absent OR the home directory is
unavailable
THE SYSTEM SHALL disable the "Let Phoenix set this up" button with a
tooltip explaining what's missing

**Rationale:** Seeding the conversation puts the user in control: they
review the prompt, see what Phoenix proposes to change, and can cancel
before any file is written. The `console.error`-only failure path is
spec'd here as a target; today's implementation does not yet meet the
visible-failure clause.

---

### REQ-TPANEL-010: Theme Integration via CSS Variables

WHEN the panel mounts
THE SYSTEM SHALL read terminal colors from the CSS variables
`--terminal-bg`, `--terminal-fg`, `--terminal-cursor` on
`document.documentElement`, with hardcoded fallbacks for each, and
pass them as the xterm.js `theme` option

WHEN the app theme toggles (dark ↔ light)
THE SYSTEM SHALL re-read the CSS variables and assign the resulting
theme object to `term.options.theme` on the live xterm instance — no
PTY/WebSocket teardown

**Rationale:** Single source of truth (the CSS theme) — the panel does
not own colors, only delegation. xterm.js's live `options.theme` setter
makes the toggle instantaneous (no flash of the old theme).

---

### REQ-TPANEL-011: Collapse/Expand Preservation and Unread Tracking

WHEN the user collapses the panel
THE SYSTEM SHALL hide the xterm container with `display: none` (NOT
unmount it), and start counting incoming output lines as "unread"

WHEN the user expands the panel
THE SYSTEM SHALL un-hide the xterm container, reset the unread counter
to zero, and call `FitAddon.fit()` to recompute geometry

WHEN unread count is non-zero AND panel is collapsed
THE SYSTEM SHALL render an unread badge on the panel header showing the
count

**Rationale:** Long-running commands need to keep producing output even
when the user is looking elsewhere; unmounting and re-mounting xterm
would lose the scrollback. The unread counter is the cheap user-facing
hint that "something happened while you were away."

---

### REQ-TPANEL-012: OSC 7 Working-Directory Reporting

WHEN an OSC 7 marker arrives (`file://hostname/path`)
THE SYSTEM SHALL parse out the path and update `reportedCwd`

WHEN the HUD renders the cwd field (REQ-TPANEL-006 absent / running /
idle paths)
THE SYSTEM SHALL display `reportedCwd` if non-null, otherwise fall back
to the conversation's `cwd` prop

**Rationale:** Shells with OSC 7 hooks (zsh + powerlevel10k, fish 3.6+,
bash with custom precmd) report the live cwd as the user `cd`s around.
Without OSC 7, the HUD shows the conversation's starting directory —
correct on first prompt, stale after a `cd`.

---

### REQ-TPANEL-013: Single-Attach Conflict Resolution (409)

WHEN the WebSocket close fires with a code/reason indicating the
backend rejected the connection due to an existing active terminal
(REQ-TERM-001 / REQ-TERM-003 enforce this with HTTP 409 on the upgrade)
THE SYSTEM SHALL distinguish this case from a generic connection
failure
AND surface a "Reclaim this terminal" affordance that, when clicked,
disconnects the other session and re-attaches this client

WHEN the user has not yet engaged the reclaim affordance
THE SYSTEM SHALL NOT auto-reclaim (silently kicking out another user's
session without consent is not acceptable)

**Rationale:** Currently NOT implemented — the close path treats every
failure as a generic "Connection error" with no distinction, which
leaves the user in a confusing dead state when they have a stale
connection from a previous tab. This requirement is the spec target;
the gap is acknowledged in the executive's status table.
