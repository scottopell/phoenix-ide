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
THE SYSTEM SHALL invoke the tmux binary with `-S <absolute-socket-path>`
where `<absolute-socket-path>` is the conversation's dedicated socket
AND `<absolute-socket-path>` SHALL be
`<phoenix-data-dir>/tmux-sockets/conv-<conversation_id>.sock`
(default: `~/.phoenix-ide/tmux-sockets/conv-<conversation_id>.sock`)

THE SYSTEM SHALL ensure that the socket directory exists with 0700
permissions before any tmux invocation for any conversation, creating it
lazily on first use

THE SYSTEM SHALL pass `-S <absolute-socket-path>` as the first arguments
to tmux, ahead of any agent-supplied arguments. Tmux's CLI rejects two
server-selection flags on one command, so an agent that passes its own
`-L` or `-S` will receive a usage error from tmux rather than escape the
conversation's socket.

THE SYSTEM SHALL NOT use `-L <label>` (relative-to-`TMUX_TMPDIR`) for
this purpose. The original draft of this spec proposed `-L` plus
per-invocation `TMUX_TMPDIR` overrides; the panel review surfaced that
the env-var dance has subtle ordering issues with multiple concurrent
invocations and that an agent's stray `-L` would override Phoenix's
under that scheme. `-S` makes the socket path explicit and immutable
from outside Phoenix.

**Rationale:** `-S <absolute-path>` makes the kernel-level Unix-socket
boundary the isolation mechanism. Each conversation talks to its own
tmux server; the agent cannot reach another conversation's server because
they use different socket files, and the tmux servers themselves are
unaware of each other. No CLI argument parsing or whitelisting is
required to enforce isolation.

---

### REQ-TMUX-002: Lazy Server Spawn

WHEN the first tmux operation for a conversation occurs (agent's `tmux`
tool call OR in-app terminal open)
AND the conversation's tmux server is not yet running
THE SYSTEM SHALL spawn the server by issuing
`tmux -S <absolute-sock> new-session -d -s main`
(creating a single detached session named `main`)
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
- `wait_seconds` (optional integer, default 30): max seconds to block on
  the subprocess

WHEN the agent calls `tmux(args=[...])`
THE SYSTEM SHALL execute `tmux -S <conv-sock> <args...>` as a child process
of Phoenix
AND return the combined stdout/stderr (subject to the same output capture
contract; 128KB middle-truncation default — see REQ-TMUX-012)
AND return the child's exit code

WHEN the binary `tmux` is not available on the system
THE SYSTEM SHALL not register the `tmux` tool (or register it with a
description that explains it is unavailable)
AND any agent invocation SHALL return `error: "tmux_binary_unavailable"`
with a clear message

THE SYSTEM SHALL NOT parse, rewrite, or whitelist subcommands inside `args`
beyond prepending `-S <conv-sock>`. The only Phoenix-injected flag is the
socket selector.

**Rationale:** The LLM already knows tmux from training data. A
pass-through tool surface costs nothing to maintain; a verb wrapper would
drift from tmux's evolving CLI and constrain the agent to whatever subset
Phoenix chose to expose. The kernel-socket isolation makes the
pass-through safe.

---

### REQ-TMUX-004: In-App Terminal Auto-Attaches When Tmux Available

WHEN a user opens the in-app terminal for a conversation
AND the tmux binary is available on the system
THE SYSTEM SHALL connect the terminal to the conversation's tmux server by
spawning `tmux -S <conv-sock> attach -t main` inside the PTY
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

THE existing constraint from `specs/terminal/` REQ-TERM-003 — exactly one
in-app terminal per conversation — SHALL apply on both the tmux-attach
and direct-PTY paths.

**Rationale:** "Always attach when possible" gives users free scrollback,
free persistence across Phoenix restart, and a shared view between agent
and user. The fallback preserves the v1 terminal experience on systems
without tmux.

The single-attach constraint is preserved on both paths even though
tmux's protocol natively supports multi-client attaches. Multi-attach
adds UI design questions (resize coordination across connections, how to
identify which connection sent which input) that the panel review
flagged as not v1-essential. The constraint can be relaxed in a future
revision; relaxing it does not change anything in the
specs/tmux-integration/ surfaces.

---

### REQ-TMUX-005: Server Survives Phoenix Process Restart

WHEN Phoenix shuts down (graceful, crash, or SIGKILL)
THE SYSTEM SHALL leave the conversation's tmux server running
AND any windows / panes / scrollback inside that server SHALL persist

WHEN Phoenix starts up and a conversation's tmux operation occurs
THE SYSTEM SHALL probe whether the server is alive by issuing
`tmux -S <conv-sock> ls`
WHEN the probe succeeds
THE SYSTEM SHALL re-use the existing server (no spawn)

**Rationale:** This is the core value-prop of going through tmux: long-
running work survives `./dev.py restart`, crashes, and graceful shutdowns.
Phoenix needs no per-restart bookkeeping; the OS keeps the tmux server
running independently.

---

### REQ-TMUX-006: Stale Socket Detection (System Reboot Recovery)

WHEN a conversation's tmux operation occurs
AND the socket file exists on disk but the tmux server process is gone
(typical post-system-reboot state)
THE SYSTEM SHALL detect this by issuing `tmux -S <conv-sock> ls` and
observing the failure
AND unlink the stale socket file
AND lazily spawn a fresh server (REQ-TMUX-002)

THE SYSTEM SHALL NOT attempt to recover the prior session's content.
Recovery requires explicit user/agent action (re-running the long-runner,
etc.).

THE SYSTEM SHALL NOT inject any breadcrumb, notice, or message into the
recovered tmux session. The original draft proposed a `tmux send-keys -l`
breadcrumb in the next attached pane; the panel review found that
`send-keys -l` writes to the slave PTY's stdin — it does not render to a
display layer — so the breadcrumb would be interpreted as input by
whatever process happened to be running in the pane (a vim session, a
REPL, a custom shell prompt). Silent recreate is the safe v1 behavior.

**Rationale:** System reboots kill the tmux server like any other user
process. The recovery semantics are: socket gone, server gone, fresh
session. No silent failure (we detect and recreate); no
input-stream-corrupting breadcrumb.

---

### REQ-TMUX-007: Server Termination on Conversation Hard-Delete

WHEN a conversation is hard-deleted
THE SYSTEM SHALL run `tmux -S <conv-sock> kill-server` (idempotent — no-op
if server is already gone)
AND unlink the socket file
AND remove the conversation's entry from any in-memory tmux registry
THE SYSTEM SHALL run this within the bedrock hard-delete cascade alongside
the bash handle cleanup (`specs/bash/` REQ-BASH-006) and any project
cleanup (`specs/projects/`); strict transactional atomicity is not claimed
because tmux kill-server is a subprocess invocation, not a SQL statement,
but the cascade SHALL ensure each step is invoked.

**Rationale:** Conversations are the unit of long-lived state in Phoenix;
when one is deleted, its associated tmux server and all its scrollback
must go too. The tmux server kill cannot share a SQL transaction with
the conversation row deletion (it's a subprocess), but the cascade
orchestrator runs both within the same delete handler; partial failure
is logged.

> **Bedrock dependency:** the hard-delete cascade requires bedrock to
> emit a `ConversationHardDeleted` event (or expose a cascade-orchestrator
> hook) that this spec — and `specs/bash/`, `specs/projects/` — can
> subscribe to. At the time of this revision, bedrock has neither
> directly. The cascade integration is gated on adding that hook.

---

### REQ-TMUX-008: Conversation Soft-State (Archive, Close) Does Not Affect Server

WHEN a conversation transitions to a non-active soft state (archived,
closed-but-not-deleted, conversation tab closed in the UI)
THE SYSTEM SHALL NOT terminate the conversation's tmux server
AND the server's windows / panes / scrollback SHALL remain available
upon the conversation's next active touch

**Rationale:** "Comes back tomorrow, dev server still running" is the
explicit pitch of using tmux. Archive is a UI/organisational signal, not
a resource-management signal.

---

### REQ-TMUX-009: Tool Description Communicates Two-Tier Persistence Model

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

### REQ-TMUX-010: Tool Cancellation and Output Limits

WHEN the agent's `tmux` tool call exceeds `wait_seconds` (default 30,
upper bound `TMUX_TOOL_MAX_WAIT_SECONDS` = 900)
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

### REQ-TMUX-011: Tool Surface Hardening — Phoenix-Injected Flag Authority

THE SYSTEM SHALL inject `-S <conv-sock>` as the first arguments to tmux,
ahead of the agent's `args`

WHEN the agent's `args` contains a leading `-L`, `-S`, or any other tmux
server-selection flag
THE SYSTEM SHALL still place Phoenix's `-S <conv-sock>` first
AND tmux's CLI parser will reject the duplicate server-selection flag
with a usage error (tmux does not accept both `-L` and `-S`, and does
not accept two `-S` flags); the agent receives a clear error from tmux
itself. Phoenix does not need to parse or strip the agent's flag.

THE SYSTEM SHALL NOT remove or rewrite any of the agent's `args`. The
agent's flags following Phoenix's injection are passed through as-is.

THE SYSTEM SHALL document in the tool description that `-L` and `-S` flags
in `args` are ineffective: the conversation's socket is fixed at the
Phoenix layer and cannot be overridden.

**Rationale:** The position of `-S <conv-sock>` is what makes the
boundary structural. Tmux's parser handles the rest. We do not need to
reject the agent's `-L` / `-S` arguments — at worst they produce a tmux
usage error, never a server-selection escape.

---

### REQ-TMUX-012: Output Capture Format

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

### REQ-TMUX-013: Stateless Tool with Per-Conversation Server Registry

WHEN the `tmux` tool is invoked
THE SYSTEM SHALL receive all execution context via a `ToolContext`
parameter
AND derive the conversation id from `ToolContext.conversation_id`
AND access the tmux server registry via `ctx.tmux()`, returning
`Result<Arc<RwLock<TmuxServer>>, TmuxError>` to match the shape of the
existing `ctx.browser()` accessor and the `ctx.bash_handles()` accessor
defined in `specs/bash/`

WHEN the tmux tool is constructed
THE SYSTEM SHALL NOT store per-conversation state on the tool itself
AND the tool instance SHALL be reusable across conversations

**Rationale:** Same statelessness contract as bash and browser, with the
same accessor shape (`async + Result + Arc<RwLock<...>>`). The registry
handles socket-path resolution, server-state probing, and lifecycle on
conversation-delete.

---

## Configuration Constants

| Name | Default | Description |
|---|---|---|
| `TMUX_TOOL_DEFAULT_WAIT_SECONDS` | 30 | Default `wait_seconds` for the `tmux` tool call |
| `TMUX_TOOL_MAX_WAIT_SECONDS` | 900 | Upper bound on `wait_seconds` for one tmux tool call |
| `TMUX_OUTPUT_MAX_BYTES` | 128 * 1024 | Max combined stdout+stderr before middle-truncation |
| `TMUX_TRUNCATION_KEEP_BYTES` | 4096 | Bytes preserved from each end on truncation |
| `TMUX_SOCKET_DIR` | `~/.phoenix-ide/tmux-sockets/` | Socket directory; permissions 0700 |
| `TMUX_DEFAULT_SESSION` | `main` | Session name created on lazy spawn |
