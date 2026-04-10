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

### REQ-TERM-006: Resize Propagated to PTY and Parser

WHEN the client sends a resize frame (type `0x01`)
THE SYSTEM SHALL apply the new dimensions to the PTY via `ioctl(TIOCSWINSZ)`
AND update the server-side vt100 parser to the same dimensions
AND the kernel SHALL deliver `SIGWINCH` to the shell's foreground process group automatically

**Rationale:** The vt100 parser must stay in sync with the actual terminal dimensions.
A parser out of sync produces corrupted screen reads for the agent tool.

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

### REQ-TERM-010: vt100 Parser Fed Every Byte In Order

WHEN bytes are read from the PTY master fd
THE SYSTEM SHALL send those bytes to the WebSocket client
AND feed the same bytes to the server-side vt100 parser
AND these two operations SHALL occur in the same handler with no gaps or reordering

WHEN the terminal is resized (REQ-TERM-006)
THE SYSTEM SHALL resize the vt100 parser to the new dimensions

**Rationale:** The vt100 parser is a state machine. Dropped or reordered bytes
corrupt its internal state permanently for that session. The parser's screen
contents are the source of truth for the agent tool (REQ-TERM-011).

---

### REQ-TERM-011: `read_terminal` Agent Tool

WHEN an LLM agent calls the `read_terminal` tool for a conversation
AND a terminal is active for that conversation
THE SYSTEM SHALL return the current vt100 parser screen contents as plain text

WHEN no terminal is active for that conversation
THE SYSTEM SHALL return an error indicating no terminal is open

**Rationale:** Enables agent workflows that inspect command output, check build
results, or verify the state of a running program without a human round-trip.

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

**Rationale:** The vt100 parser is a state machine — dropped bytes corrupt
its state permanently for that session. Backpressure propagates to the kernel
PTY buffer, which correctly slows the producing process rather than losing data.
