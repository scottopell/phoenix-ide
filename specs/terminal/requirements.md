# Terminal: PTY-Backed Browser Terminal

## User Story

As a developer using PhoenixIDE, I need a real, fully-featured terminal in the browser
so that I can run shell commands, inspect output, and interact with my environment
without leaving the IDE or SSH-ing into the server separately.

## Transparency Contract

The user must be able to confidently answer:

1. Is there a terminal open for this conversation?
2. What directory did the terminal start in?
3. Can I run arbitrary commands with my real shell and dotfiles?
4. Can the agent see what is on my terminal screen?

## Requirements

### REQ-TERM-001: PTY-Backed Terminal per Conversation

WHEN a user opens the terminal panel for a conversation
THE SYSTEM SHALL spawn a real PTY-backed shell process (`$SHELL -i`) using the
conversation's working directory as the starting directory
AND establish a WebSocket connection to relay I/O between the browser and the PTY

WHEN the terminal is already active for that conversation
THE SYSTEM SHALL reject the new WebSocket connection with HTTP 409
AND NOT spawn a second shell

**Rationale:** Exactly one terminal per conversation keeps the lifecycle simple and
state unambiguous. The PTY ensures full terminal emulation: readline, vim, htop,
colour, and interactive programs all work without special handling.

---

### REQ-TERM-002: Explicit Shell Environment Construction

WHEN the shell process is spawned
THE SYSTEM SHALL construct the child environment explicitly
AND NOT inherit the API server's process environment

The constructed environment SHALL include at minimum:

| Variable   | Value                  |
|------------|------------------------|
| `TERM`     | `xterm-256color`       |
| `COLORTERM`| `truecolor`            |
| `HOME`     | user home directory    |
| `USER`     | current username       |
| `SHELL`    | path to user's shell   |
| `PATH`     | user's PATH            |
| `LANG`     | `en_US.UTF-8`          |

**Rationale:** The API server environment may contain secrets (API keys, tokens).
Inheriting it blindly risks leaking secrets into the shell. `TERM=xterm-256color`
is the most safety-critical variable: wrong or missing causes readline degradation,
broken arrow keys, and misbehaviour in vim, htop, and similar programs.

---

### REQ-TERM-003: Exactly One Terminal per Conversation

WHEN a terminal WebSocket connection is requested
AND a terminal is already active for that conversation
THE SYSTEM SHALL return HTTP 409 Conflict
AND NOT spawn a second PTY

WHEN no terminal is active for the conversation
THE SYSTEM SHALL accept the WebSocket upgrade and proceed with spawn

**Rationale:** Correct-by-construction. The UI must not offer to open a terminal
when one is already active, eliminating the 409 path at runtime.

---

### REQ-TERM-004: Binary WebSocket Framing

WHEN transmitting PTY data or control frames over the WebSocket connection
THE SYSTEM SHALL use binary frames exclusively
AND prefix every frame with a single type byte:

```
0x00 | <raw bytes>              PTY data  (bidirectional)
0x01 | <u16be cols> <u16be rows>  Resize event (client → server only)
```

WHEN the client sends a text WebSocket frame
THE SYSTEM SHALL close the connection with an appropriate error
AND NOT write the text frame bytes to the PTY master fd

**Rationale:** PTY output is arbitrary bytes, not valid UTF-8. Text frames silently
corrupt multi-byte sequences and binary control codes.

---

### REQ-TERM-005: Initial Resize Before First Prompt

WHEN the WebSocket connection is established and before any user input is processed
THE SYSTEM SHALL send a resize event using the client's current xterm.js dimensions
AND apply that resize to the PTY before the shell produces its first prompt

**Rationale:** Without this, the shell starts at the PTY default (80×24), causing
the first prompt to render incorrectly and wrap at the wrong column.

---

### REQ-TERM-006: Resize Propagated to PTY

WHEN the client sends a resize frame (type `0x01`)
THE SYSTEM SHALL apply the new dimensions to the PTY via `ioctl(TIOCSWINSZ)`
AND the kernel SHALL deliver `SIGWINCH` to the shell's foreground process group automatically

**Rationale:** The PTY must reflect the actual terminal dimensions so that the shell
and any running programs (vim, htop, etc.) can wrap and render correctly. Resize is
PTY-only; the CommandTracker (REQ-TERM-010) has no concept of screen dimensions.

---

### REQ-TERM-007: EIO Treated as Clean Termination

WHEN reading from the PTY master fd returns `EIO`
THE SYSTEM SHALL treat this as clean shell exit
AND close the WebSocket connection
AND reap the child process via `waitpid`
AND NOT log the condition as an error

**Rationale:** `EIO` is the kernel's PTY EOF signal when the slave fd is closed
(i.e., the shell has exited). It is not an error condition. Logging it as an error
produces noise on every normal terminal close.

---

### REQ-TERM-008: Master fd Closed on WebSocket Disconnect

WHEN the WebSocket connection closes for any reason (user-initiated or network drop)
THE SYSTEM SHALL close the PTY master fd
AND the kernel SHALL deliver `SIGHUP` to the shell's process group automatically

WHEN the Rust process holding a terminal session panics or exits abnormally
THE SYSTEM SHALL close the master fd via Rust's `Drop` semantics on `TerminalHandle`
AND NOT leave the master fd open in any exit path

**Rationale:** An unclosed master fd leaves orphan shells consuming resources.
`Drop`-on-close ensures the SIGHUP chain fires even on abnormal exit paths,
making orphan prevention correct by construction rather than by discipline.

---

### REQ-TERM-009: Child Process Reaped After Shell Exit

WHEN the shell process exits
THE SYSTEM SHALL call `waitpid` to reap the child
AND NOT leave zombie processes

**Rationale:** Unreaned children remain as zombies in the process table until the
API server exits. Long-lived servers with many terminal sessions accumulate zombies.

---

### REQ-TERM-010: Command Tracker Fed Every Byte In Order

WHEN bytes are read from the PTY master fd
THE SYSTEM SHALL send those bytes to the WebSocket client
AND feed the same bytes to the server-side CommandTracker
AND these two operations SHALL occur in the same handler with no gaps or reordering

**Rationale:** The CommandTracker is a state machine over OSC 133 C/D boundaries.
Dropped or reordered bytes can corrupt C/D pairing, producing garbled output records
or missed commands for that session.

---

### REQ-TERM-021: Command Record Store

WHEN a terminal session is active AND `shell_integration_status = detected`
THE SYSTEM SHALL maintain a ring buffer of at most 5 `CommandRecord` entries for
that session

Each `CommandRecord` SHALL contain:
- `command_text`: the text payload from OSC 133;C (may be empty string)
- `output`: text captured between the C and D markers; only printable characters and
  newlines, with ANSI escape sequences discarded
- `exit_code`: integer from the OSC 133;D payload; `None` if D omits it — do NOT
  substitute 0
- `started_at`: timestamp when the C marker was processed
- `duration_ms`: milliseconds elapsed from C to D

WHEN `shell_integration_status != detected`
THE SYSTEM SHALL keep the ring buffer always empty

WHEN a 6th command completes
THE SYSTEM SHALL evict the oldest record from the ring buffer

WHEN the captured output for a command exceeds 128KB
THE SYSTEM SHALL write the full output bytes to disk at
`~/.phoenix-ide/terminal-output/<session-id>/<seq>.txt`
AND store a truncated preview with the disk path appended in the record's `output` field

**Rationale:** Structured command records give agent tools precise access to recent
command output without requiring a screen-scrape of the full terminal buffer. The
128KB threshold matches the bash tool's `MAX_OUTPUT_LENGTH` constant. Writing large
output to disk avoids unbounded memory growth while still making the full output
accessible.

---

### REQ-TERM-022: `terminal_last_command` Agent Tool

WHEN an LLM agent calls `terminal_last_command` for a conversation
AND a terminal is active for that conversation
AND `shell_integration_status = detected`
AND the ring buffer is non-empty
THE SYSTEM SHALL return the most recent `CommandRecord` as structured data

WHEN `shell_integration_status != detected`
THE SYSTEM SHALL return an error:
`"shell integration is not active for this terminal — install the shell integration snippet to enable command tracking"`

WHEN `shell_integration_status = detected` AND the ring buffer is empty (no command
has completed this session)
THE SYSTEM SHALL return an error:
`"no commands have completed in this terminal session yet"`

**Rationale:** Gives agents a direct, zero-ambiguity view of the most recently
completed command and its output without requiring screen coordinate arithmetic or
quiescence heuristics.

---

### REQ-TERM-023: `terminal_command_history` Agent Tool

WHEN an LLM agent calls `terminal_command_history` for a conversation
AND a terminal is active for that conversation
AND `shell_integration_status = detected`
AND the ring buffer is non-empty
THE SYSTEM SHALL return the last `count` `CommandRecord` entries newest-first,
where `count` is an integer parameter (default 3, max 5)

WHEN `shell_integration_status != detected`
THE SYSTEM SHALL return the same error as REQ-TERM-022

WHEN `shell_integration_status = detected` AND the ring buffer is empty
THE SYSTEM SHALL return the same error as REQ-TERM-022

**Rationale:** Lets agents inspect a short history of commands when the most recent
result alone is insufficient. Cap of 5 matches the ring buffer capacity.

---

### REQ-TERM-012: Terminal Torn Down with Conversation

WHEN a conversation reaches a terminal state (completed, failed, context_exhausted)
AND a terminal session is active for that conversation
THE SYSTEM SHALL close the master fd
AND the kernel SHALL deliver SIGHUP to the shell
AND the terminal session SHALL be torn down

---

### REQ-TERM-013: WebSocket Endpoint Authentication

WHEN a WebSocket upgrade request arrives at the terminal endpoint
THE SYSTEM SHALL validate the request carries a valid API session
AND SHALL use the same session mechanism as all other API endpoints

WHEN the upgrade request has no valid session
THE SYSTEM SHALL reject the upgrade with HTTP 401
AND NOT create a PTY or spawn a shell process

**Rationale:** The terminal provides direct shell access to the server. An
unauthenticated terminal endpoint would allow any network-accessible client
to run arbitrary commands. Session validation must occur before any PTY
is created, not after the WebSocket is established.

---

### REQ-TERM-014: Output Channel Backpressure

WHEN the server's internal channel buffering PTY output for the WebSocket
exceeds its configured bound
THE SYSTEM SHALL pause reads from the PTY master fd
AND NOT drop bytes to relieve backpressure

**Rationale:** The CommandTracker is a state machine — dropped bytes corrupt
C/D boundary pairing permanently for that session. Backpressure propagates to
the kernel PTY buffer, which correctly slows the producing process rather than
losing data.

---

### REQ-TERM-015: OSC 133 Shell Integration Detection

WHEN a new terminal session is spawned
THE SYSTEM SHALL begin a detection window of configurable duration (default 5 seconds)
during which incoming PTY bytes are inspected for OSC 133 escape sequences

WHEN an OSC 133 `C` marker is observed within the detection window
THE SYSTEM SHALL mark the terminal session as `shell_integration_status = detected`
AND SHALL activate command lifecycle tracking for the remainder of the session

WHEN only OSC 133 `A` and/or `B` markers are observed within the detection window (as with powerlevel10k's default integration)
THE SYSTEM SHALL NOT promote to `detected` on those markers alone

WHEN the detection window elapses without observing a `C` marker
THE SYSTEM SHALL mark the terminal session as `shell_integration_status = absent`
AND SHALL leave the detection state locked for the remainder of the session
AND SHALL NOT re-attempt detection until a new terminal session is spawned

**Rationale:** OSC 133 (FinalTerm shell integration) is the de-facto standard
for structured prompt/command semantics in terminal emulators. Detection
requires a `C` marker specifically because the purpose of the rich HUD is
command lifecycle tracking — prompt boundaries (A, B) alone give us nothing
actionable. powerlevel10k, a common prompt framework, emits only A/B via its
built-in shell integration hook; users with p10k see `absent` until they
supplement their configuration with preexec/precmd hooks that emit C/D.

The 5-second window accommodates slow shell startup (instant prompts, SSH
login banners, `nvm use` chains) without excessively delaying the absent-state
UI affordances. Monotonic detection keeps runtime state simple and predictable:
users don't see the HUD flip between modes mid-session.

To maximise the chance that detection fires for users running prompt
frameworks that gate shell integration on terminal identity, the PTY spawn
env (REQ-TERM-002) includes `ITERM_SHELL_INTEGRATION_INSTALLED=Yes` and
`TERM_PROGRAM=phoenix-ide`. The former specifically unlocks p10k's built-in
OSC 133;A/B emission.

---

### REQ-TERM-016: Shell Integration Command Lifecycle Tracking

WHEN `shell_integration_status = detected` AND an OSC 133 `C` marker is received
THE SYSTEM SHALL record the command text payload (may be empty)
AND SHALL store an executing command as the terminal's `current_command`
with `started_at` set to the current time
AND SHALL clear any `last_completed_command` from the previous command

WHEN `shell_integration_status = detected` AND an OSC 133 `D` marker is received
AND `current_command` is present
THE SYSTEM SHALL record the exit code from the `D` payload if present (may be absent)
AND SHALL store the finished command as `last_completed_command`
with `finished_at` set to the current time
AND SHALL clear `current_command`

WHEN `shell_integration_status = detected` AND an OSC 133 `D` marker is received
AND no `current_command` is present
THE SYSTEM SHALL treat the event as a no-op and log at debug level

WHEN `shell_integration_status = detected` AND an OSC 133 `A` or `B` marker is received
THE SYSTEM SHALL accept the marker without state change (reserved for forward compatibility)

**Rationale:** The C/D subset of OSC 133 captures the essential command
lifecycle: execution begins, execution ends. The A marker (prompt start)
and B marker (end-of-prompt / start-of-input) are accepted for forward
compatibility but do not trigger state changes.

Clearing `last_completed_command` on the next `C` (rather than on the next
`A`) gives the ✓/✗ indicator a useful visible lifetime. In shells that emit
`A` on every prompt render, `A` fires approximately 50 ms after `D`, which
makes clearing on `A` effectively invisible to the user. Clearing on `C`
instead gives the user the entire "reading result → thinking → typing the
next command" window to see the outcome of the previous command. Missing
`current_command` on `D` is tolerated because shells can emit `D` on empty
prompts or after signals.

---

### REQ-TERM-017: Shell Integration Absent Hint

WHEN `shell_integration_status = absent` for the current terminal session
THE SYSTEM SHALL display a passive hint indicator associated with the terminal's status dot
AND SHALL offer the user a shell-specific enablement snippet on demand

WHEN the user hovers the status dot in the absent state
THE SYSTEM SHALL show a tooltip explaining that shell integration is not detected
AND naming the detected shell (e.g. "zsh")

WHEN the user clicks the status dot in the absent state
THE SYSTEM SHALL display a modal containing a snippet tailored to the user's shell
AND SHALL include a copy-to-clipboard affordance
AND SHALL NOT show instructions for other shells

WHEN the user's shell is `zsh`, `bash`, or `fish` (determined from `$SHELL` captured at spawn time per REQ-TERM-002)
THE SYSTEM SHALL display the shell-specific snippet for that shell

WHEN the user's shell is something else
THE SYSTEM SHALL display a generic message indicating that shell integration is not supported out of the box for that shell

**Rationale:** A passive tooltip on the terminal's status indicator surfaces
the hint at the moment of engagement (hovering the dot) without intruding on
the chat workflow or requiring a dismissable banner. Tailoring to the detected
shell avoids overwhelming the user with options for shells they don't use and
makes the copy-paste one step instead of two. The snippet bundles both OSC 133
markers and OSC 7 cwd reporting into a single paste so users get both features
at once.

---

### REQ-TERM-018: OSC 7 Working Directory Reporting

WHEN an OSC 7 escape sequence is received from the PTY containing a `file://host/path` URL
THE SYSTEM SHALL parse the path component (percent-decoded)
AND SHALL update the terminal session's `reported_cwd` for HUD display

WHEN no OSC 7 sequence has been observed for a terminal session
THE SYSTEM SHALL fall back to the conversation's static working directory (REQ-BED-010) for HUD display

WHEN an OSC 7 sequence is received with an unparseable payload
THE SYSTEM SHALL log the parse failure at debug level
AND SHALL leave `reported_cwd` unchanged

**Rationale:** OSC 7 provides a reliable, shell-native way to track the current
directory without parsing prompt text. It pairs naturally with OSC 133 (both
are typically enabled by the same shell integration snippet) and ensures the
HUD stays accurate when the user `cd`s inside the shell session, without the
staleness of a static conversation-scoped cwd.

---

### REQ-TERM-019: Disconnected-State Visual Treatment and Reconnect

WHEN the WebSocket connection to a terminal session closes for any reason
(server restart, shell exit, network error, explicit WS close)
THE SYSTEM SHALL apply a visually distinct "dead" treatment to the terminal
panel (e.g. reduced opacity, muted colour treatment)
AND SHALL replace the prompt strip text with an explicit "disconnected"
message that cues the user that the terminal is no longer running

WHEN the user clicks anywhere on a disconnected terminal panel
THE SYSTEM SHALL close any lingering WebSocket handle
AND SHALL open a new WebSocket connection to the conversation's terminal
endpoint, which triggers a fresh PTY spawn on the backend
AND SHALL reset the shell_integration_status to `unknown`, restarting the
5-second detection window for the new session

**Rationale:** The disconnected state needs to be unambiguous. Using a dot
colour alone (e.g. red) conflates with "last command failed" and fails to
communicate that the terminal has no PTY at all. A panel-level visual
treatment plus an explicit reconnect affordance matches the user's mental
model that the terminal is a thing that can be "off" or "on," and restores
agency without requiring navigation away from the conversation.

---

### REQ-TERM-020: Shell Integration Setup Assist

WHEN a terminal session has `shell_integration_status = absent`
AND the user opens the shell integration modal (per REQ-TERM-017)
THE SYSTEM SHALL offer the user an action that creates a seeded sub-
conversation rooted in the user's home directory, pre-loaded with a
prompt that directs Phoenix to investigate the user's dotfiles setup
and apply the integration snippet on the user's behalf

WHEN the user invokes the setup-assist action
THE SYSTEM SHALL create a new conversation via the seeded-conversations
primitive (per REQ-SEED-001 through REQ-SEED-004) with:
- `cwd` = the server user's `$HOME`
- `conv_mode` = `direct`
- `parent_conversation_id` = the current conversation
- `seed_label` = "Shell integration setup ({shell})"
- a draft prompt tailored to the detected shell that instructs Phoenix
  to detect the dotfiles management style (plain, oh-my-zsh, chezmoi,
  yadm, symlinked git repo, home-manager, etc.), verify idempotency,
  apply the snippet in the correct location, and confirm to the user
AND SHALL NOT submit the draft automatically — the user reviews the
prompt and decides whether to proceed

WHEN the setup-assist action is offered
THE SYSTEM SHALL also retain the manual "Copy to clipboard" action as
the alternative path for users who prefer to handle the change
themselves

**Rationale:** Phoenix IS the agent that can do this work. Telling the
user to manually copy-paste into their rc file is a concession to the
era when IDEs were not LLM-powered. The seeded-conversations primitive
makes it cheap to offer the automated path without special-casing
terminal setup in the conversation engine. Keeping the manual option
side-by-side preserves choice for users who don't want Phoenix
touching their dotfiles.

The agent-facing prompt needs to cover a wide variety of dotfile
management styles because user setups vary enormously. The spawned
conversation should investigate, not assume, and should punt gracefully
on exotic setups (home-manager, nushell, etc.) rather than edit things
it doesn't understand.
