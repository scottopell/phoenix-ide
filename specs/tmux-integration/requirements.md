# Tmux Integration

## User Story

As an LLM agent, I sometimes need to run a process that survives me — a dev
server, a long-running build, an interactive REPL — and inspect it later
without forcing the conversation to keep blocking. As a user, I want to
reattach to that work after closing my browser tab or after Phoenix
restarts. The bash tool deliberately offers neither: it has no TTY, no
persistence across Phoenix restart, and a fixed memory budget per handle.
Tmux integration provides the second tier of process management — a tool
named `tmux`, plus an in-app terminal that automatically attaches to the
conversation's tmux session whenever a tmux binary is available.

## Background: why tmux specifically

The persistent-and-attachable problem is exactly what tmux was built to
solve: a tmux server holds shells, scrollback, and pane state across client
disconnects, and a fresh client can attach to the existing server at any
time. Reimplementing that is reinventing the wheel; integrating it is
mostly plumbing. The integration's job is to (a) give each conversation
its own isolated tmux server, (b) expose tmux to the agent in a way it
already understands, and (c) make the in-app terminal pick up the same
session by default when one exists.

## Requirements

### REQ-TMUX-001: Per-Conversation Tmux Server (Socket Isolation)

WHEN any tmux operation occurs for a conversation (agent calls the `tmux`
tool, or the in-app terminal is opened)
THE SYSTEM SHALL invoke the tmux binary with `-L <socket-path>` where
`<socket-path>` is the conversation's dedicated socket
AND `<socket-path>` SHALL be `~/.phoenix-ide/tmux-sockets/conv-<conversation_id>.sock`
(or the equivalent path under a non-default `PHOENIX_DATA_DIR`)

THE SYSTEM SHALL ensure that the socket directory
(`~/.phoenix-ide/tmux-sockets/`) exists with 0700 permissions before any
tmux invocation for any conversation, creating it lazily on first use

THE SYSTEM SHALL NOT use `-S` (absolute socket path) or any other tmux
isolation flag in place of `-L`; the relative form keeps the socket file
co-located with other Phoenix state at a predictable path

**Rationale:** `-L` makes the kernel-level Unix-socket boundary the
isolation mechanism. Each conversation talks to its own tmux server; the
agent cannot reach another conversation's server because they use
different socket files, and the tmux servers themselves are unaware of each
other. No CLI argument parsing or whitelisting is required to enforce
isolation.

---

### REQ-TMUX-002: Lazy Server Spawn

WHEN the first tmux operation for a conversation occurs (agent's `tmux`
tool call OR in-app terminal open)
AND the conversation's tmux server is not yet running
THE SYSTEM SHALL spawn the server by issuing `tmux -L <sock> new-session
-d -s main` (creating a single detached session named `main`)
AND THEN execute the requested operation (the agent's command, or the
terminal's attach)

WHEN the agent's tmux operation creates additional sessions or operates on
sessions other than `main`
THE SYSTEM SHALL NOT block — additional sessions within the conversation's
server are permitted (the socket is the boundary, not the session set)

**Rationale:** Eager spawn would create idle tmux servers for every
conversation regardless of whether the user or agent ever uses them.
Lazy spawn ensures resource cost is paid only when needed. The default
`main` session matters for tmux's default-target rules: `tmux capture-pane
-p` with no `-t` argument resolves to the most recent session, which is
`main` for a fresh server.

---

### REQ-TMUX-003: `tmux` Agent Tool — Pure Pass-Through

THE SYSTEM SHALL register an agent tool named `tmux` whose schema accepts:
- `args` (required array of strings): the tmux subcommand and its arguments

WHEN the agent calls `tmux(args=[...])`
THE SYSTEM SHALL execute `tmux -L <conv-sock> <args...>` as a child process
of Phoenix
AND return the combined stdout/stderr (subject to the same output capture
contract as bash, for consistency: 128KB middle-truncation default — see
REQ-TMUX-013)
AND return the child's exit code

WHEN the binary `tmux` is not available on the system
THE SYSTEM SHALL not register the `tmux` tool (or register it with a
description that explains it is unavailable)
AND any agent invocation SHALL return `error: "tmux_binary_unavailable"`
with a clear message

THE SYSTEM SHALL NOT parse, rewrite, or whitelist subcommands inside `args`
beyond prepending `-L <conv-sock>`. The only Phoenix-injected flag is the
socket selector.

**Rationale:** The LLM already knows tmux from training data. A pass-through
tool surface costs nothing to maintain; a verb wrapper would drift from
tmux's evolving CLI and constrain the agent to whatever subset Phoenix
chose to expose. The kernel-socket isolation makes the pass-through safe.

---

### REQ-TMUX-004: In-App Terminal Auto-Attaches When Tmux Available

WHEN a user opens the in-app terminal for a conversation
AND the tmux binary is available on the system
THE SYSTEM SHALL connect the terminal to the conversation's tmux server by
spawning `tmux -L <conv-sock> attach -t main` inside the PTY
AND apply the same WebSocket I/O relay, resize handling, and lifecycle
machinery already specified in `specs/terminal/`

WHEN the conversation's tmux server is not yet running at terminal-open time
THE SYSTEM SHALL lazily spawn it (REQ-TMUX-002) before issuing `attach`
AND the user SHALL see the freshly created `main` session, not an error
about "no sessions"

WHEN the tmux binary is not available
THE SYSTEM SHALL fall back to the direct-PTY behavior already specified
by `specs/terminal/` (spawning the user's `$SHELL -i` directly without
tmux)

**Rationale:** "Always attach when possible" gives users free scrollback,
free persistence across Phoenix restart, and a shared view between agent
and user. The fallback preserves the v1 terminal experience on systems
without tmux.

---

### REQ-TMUX-005: Multiple Terminal Clients Allowed Per Conversation

WHEN the conversation's tmux server is running
AND multiple in-app terminal connections request to attach (e.g., user
opens a second terminal pane in the same conversation, or two browser
tabs are open to the same conversation)
THE SYSTEM SHALL permit each connection to issue its own
`tmux attach -t main`
AND each tmux client process is independent (tmux's native multi-client
support — terminals share state because they attach to the same session)

THE SYSTEM SHALL retire the prior `specs/terminal/` constraint
"exactly one terminal per conversation" when tmux is available; the
constraint stands only on the direct-PTY fallback path.

**Rationale:** Tmux multi-attach is free at the protocol level and is the
correct model when multiple views into the same conversation make sense
(user + agent's tool calls; user comparing two windows). The
single-terminal constraint exists in the current direct-PTY path because
two clients reading the same master fd would race.

---

### REQ-TMUX-006: Server Survives Phoenix Process Restart

WHEN Phoenix shuts down (graceful, crash, or SIGKILL)
THE SYSTEM SHALL leave the conversation's tmux server running
AND any windows / panes / scrollback inside that server SHALL persist

WHEN Phoenix starts up and a conversation's tmux operation occurs
THE SYSTEM SHALL probe whether the server is alive by issuing
`tmux -L <conv-sock> ls`
WHEN the probe succeeds
THE SYSTEM SHALL re-use the existing server (no spawn)

**Rationale:** This is the core value-prop of going through tmux: long-
running work survives `./dev.py restart`, crashes, and graceful shutdowns.
Phoenix needs no per-restart bookkeeping; the OS keeps the tmux server
running independently.

---

### REQ-TMUX-007: Stale Socket Detection (System Reboot Recovery)

WHEN a conversation's tmux operation occurs
AND the socket file exists on disk but the tmux server process is gone
(typical post-system-reboot state — socket file may persist if
`~/.phoenix-ide/tmux-sockets/` is on a non-tmpfs)
THE SYSTEM SHALL detect this by issuing `tmux -L <conv-sock> ls` and
observing the failure
AND unlink the stale socket file
AND lazily spawn a fresh server (REQ-TMUX-002)
AND on the next attach, render a one-line breadcrumb in the new pane:
`[phoenix] previous tmux session lost at <best-known-shutdown-or-startup-ts>`

THE SYSTEM SHALL NOT attempt to recover the prior session's content. The
breadcrumb informs the user; recovery requires explicit user/agent action
(re-running the long-runner, etc.).

**Rationale:** System reboots kill the tmux server like any other user
process. Pit of success: the user comes back, sees a fresh prompt with a
clear note explaining what happened, instead of "session not found"
errors or, worse, silent failure of agent tool calls operating against
an empty server.

---

### REQ-TMUX-008: Server Termination on Conversation Hard-Delete

WHEN a conversation is hard-deleted
THE SYSTEM SHALL run `tmux -L <conv-sock> kill-server` (idempotent — no-op
if server is already gone)
AND unlink the socket file
AND remove the conversation's entry from any in-memory tmux registry
THE SYSTEM SHALL run this in the same transactional block as the bedrock
hard-delete cascade so partial failures do not leave orphaned tmux
servers

**Rationale:** Conversations are the unit of long-lived state in Phoenix;
when one is deleted, its associated tmux server and all its scrollback
must go too. This matches the `specs/bash/` REQ-BASH-006 cascade and the
bedrock hard-delete contract.

---

### REQ-TMUX-009: Conversation Soft-State (Archive, Close) Does Not Affect Server

WHEN a conversation transitions to a non-active soft state (archived,
closed-but-not-deleted, conversation tab closed in the UI)
THE SYSTEM SHALL NOT terminate the conversation's tmux server
AND the server's windows / panes / scrollback SHALL remain available
upon the conversation's next active touch

**Rationale:** "Comes back tomorrow, dev server still running" is the
explicit pitch of using tmux. Archive is a UI/organisational signal, not
a resource-management signal.

---

### REQ-TMUX-010: Tool Description Communicates Two-Tier Persistence Model

THE `tmux` tool's description SHALL state explicitly:
- The tool runs against a per-conversation tmux server with kernel-level
  isolation from other conversations.
- Processes started inside this tmux server survive Phoenix process
  restarts (but not system reboots).
- The full tmux CLI is available; common subcommands include
  `new-window`, `capture-pane`, `send-keys`, `list-windows`,
  `kill-window`.
- Use `tmux` for processes that need a TTY, that should survive Phoenix
  restart, or that you want to interact with via stdin. Use `bash` for
  one-shot non-interactive commands.

**Rationale:** The agent must know when to reach for which tool. A pit-of-
success description leads agents to the right choice without trial-and-
error.

---

### REQ-TMUX-011: Tool Cancellation and Output Limits

WHEN the agent's `tmux` tool call exceeds `TMUX_TOOL_TIMEOUT_SECONDS`
(default 30; tunable per-call via the optional `wait_seconds` arg
mirroring bash)
THE SYSTEM SHALL kill the `tmux <args>` child process
AND return a structured `{ status: "timed_out", waited_ms, ... output ... }`
response

WHEN the tmux subprocess produces more than `TMUX_OUTPUT_MAX_BYTES`
(default 128 KB)
THE SYSTEM SHALL truncate in the middle, preserving the first and last
4 KB, with a `[output truncated in middle: got X, max is Y]` marker
inserted between them
AND indicate the truncation in the response

WHEN the agent supplies `cancel` via cancellation token (e.g., user
cancels the conversation turn mid-call)
THE SYSTEM SHALL terminate the `tmux <args>` child and return a
`{ status: "cancelled" }` response

**Rationale:** Most `tmux` subcommands return quickly (capture-pane,
list-windows, send-keys). The few that block (`tmux attach`, `tmux
wait-for`) should be guarded by a timeout because the tool is
non-interactive — no human is at the other end to type the detach key.

---

### REQ-TMUX-012: Tool Surface Hardening — Phoenix-Injected Flag Authority

THE SYSTEM SHALL inject `-L <conv-sock>` as the first arguments to tmux,
ahead of the agent's `args`
WHEN the agent's `args` contains a leading `-L`, `-S`, or any other tmux
server-selection flag at the front of the argument list
THE SYSTEM SHALL still place Phoenix's `-L <conv-sock>` first
AND tmux's CLI parser interprets the first `-L` it sees and rejects or
ignores subsequent server-selection flags as appropriate (specifically:
tmux requires `-L`/`-S` before the subcommand and does not accept two
of them)

THE SYSTEM SHALL NOT remove or rewrite any of the agent's `args`. The
agent's flags following Phoenix's injection are passed through as-is.

THE SYSTEM SHALL document in the tool description that `-L` and `-S` flags
in `args` are ineffective: the conversation's socket is fixed at the
Phoenix layer and cannot be overridden.

**Rationale:** The position of `-L <conv-sock>` is what makes the
boundary structural. Tmux's parser handles the rest. We do not need to
reject the agent's `-L` arguments — at worst they produce a tmux usage
error, never a server-selection escape.

---

### REQ-TMUX-013: Output Capture Format

THE `tmux` tool SHALL return responses with this shape:

```json
{
  "status": "ok" | "timed_out" | "cancelled",
  "exit_code": <int | null>,
  "duration_ms": <int>,
  "stdout": "<string>",
  "stderr": "<string>",
  "truncated": <bool>
}
```

`stdout` and `stderr` are returned separately (unlike bash, which
combines them). Tmux tooling commands tend to print structured data to
stdout and warnings to stderr; keeping them separate gives the agent a
reliable parse signal. Truncation is tracked as a single bool spanning
both streams (the most common failure mode is one stream blowing the
budget).

**Rationale:** Different shape from bash because the tools serve different
purposes: bash produces scrollback (combined makes sense); tmux
subcommands produce structured CLI output (separation matters).

---

### REQ-TMUX-014: Stateless Tool with Per-Conversation Server Registry

WHEN the `tmux` tool is invoked
THE SYSTEM SHALL receive all execution context via a `ToolContext`
parameter
AND derive the conversation id from `ToolContext.conversation_id`
AND access the tmux server registry via `ctx.tmux()` (analogous to
`ctx.browser()` and the new `ctx.bash_handles()`)

WHEN the tmux tool is constructed
THE SYSTEM SHALL NOT store per-conversation state on the tool itself
AND the tool instance SHALL be reusable across conversations

**Rationale:** Same statelessness contract as bash and browser. The
registry handles socket-path resolution, server-state probing, and
lifecycle on conversation-delete.

---

## Configuration Constants

| Name | Default | Description |
|---|---|---|
| `TMUX_TOOL_TIMEOUT_SECONDS` | 30 | Default wait for the tmux tool call |
| `TMUX_OUTPUT_MAX_BYTES` | 128 * 1024 | Max combined stdout+stderr before middle-truncation |
| `TMUX_SOCKET_DIR` | `~/.phoenix-ide/tmux-sockets/` | Socket directory; permissions 0700 |
| `TMUX_DEFAULT_SESSION` | `main` | Session name created on lazy spawn |
