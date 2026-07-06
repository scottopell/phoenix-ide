# Inline Terminal — Requirements

## User Need

A user wants to drive a Phoenix conversation directly, running shell commands
themselves without provoking an agent turn, while keeping the work the agent
later sees coherent. Typing `!` as the first character of the composer turns the
message input into a real interactive terminal scoped to the conversation. Each
command the user runs is recorded into the conversation as if it were a `bash`
tool call the agent could have made — so a later agent turn inherits an accurate
picture of what the human did — but issuing those commands never asks the model
for a response.

This is the first concrete consumer of **user-originated tool rounds**: tool
calls and results that appear in conversation history and LLM context attributed
to the user rather than the agent. The attribution machinery is designed to
generalize to other user-invoked tools, but only the inline terminal is in scope
here.

## Terminology

- **Inline terminal session** — a PTY-backed terminal hosted inside the composer
  for one conversation, opened by the `!` sigil. Distinct from the
  conversation's panel terminal (`specs/terminal`).
- **Inline command round** — the unit committed to history for one command run
  in an inline terminal session: a `bash` tool-use block plus its tool-result,
  attributed to user origin.
- **Open bracket** — a command that has started (OSC 133 `C`) but not yet
  produced a completion marker (OSC 133 `D`).
- **Supersession** — an open bracket is closed by something other than its own
  `D`: the next command's `C`, or the session closing.

## Requirements

### REQ-IT-001 — `!` opens an inline terminal in the composer

When the composer input begins with `!`, the input area becomes an interactive
PTY terminal for the conversation, reusing the terminal transport, emulator, and
OSC 133 command tracking defined in `specs/terminal`. The terminal runs the
user's `$SHELL -i` (the same interactive-shell spawn contract as `specs/terminal`)
with the conversation's working directory as its cwd. It is a
full terminal — line editing, completion, colour, and interactive programs work
as in any native terminal — not a single command string.

Shell integration (OSC 133 markers) is a prerequisite for the per-command
history commit (REQ-IT-003) and for the empty-line detection the
return-to-composer gesture relies on (REQ-IT-006). A shell without integration
still yields a working interactive terminal, but commits no rounds and leaves
shell exit (`Ctrl-D`) as the only close path; the implementation surfaces this
degradation to the user rather than the spec modeling it.

### REQ-IT-002 — Opening is gated to an idle conversation

An inline terminal session may be opened only when the conversation is idle.
While a session is live the conversation is busy: it does not accept an
agent-triggering user message, and the agent does not run. Attempting to open a
session while the conversation is busy is rejected. At most one live inline
terminal session exists per conversation.

### REQ-IT-003 — Each command is committed as a user-originated tool round

Each OSC-133-delimited command run in the session is committed to conversation
history as one inline command round: a `bash` tool-use block (the command text)
paired with its tool-result (the captured output and, when known, the exit
code). The round is delivered to the LLM as a tool-use/tool-result pair so a
later agent turn sees the command and its result, exactly as it would see its
own `bash` calls (the **shared-history** model).

### REQ-IT-004 — Inline rounds use the `bash` run operation only

An inline command round is shaped as a `bash` tool call with operation `run`.
The backgrounded-command operations of the `bash` tool (`wait`, `kill`, and the
handle/label/since machinery) are never produced by the inline terminal path. A
long-running foreground command occupies the terminal until it exits or the user
interrupts it, as in any native terminal.

### REQ-IT-005 — A started command is never dropped from history

A command that has started (OSC 133 `C`) but produced no completion marker
(OSC 133 `D`) is committed as an **interrupted** round when its bracket is
superseded — by the next command's `C`, or by the session closing — capturing
the output observed so far with the exit code absent. Every started command
resolves to exactly one committed round, either completed or interrupted; none
vanishes silently. The agent's view of user activity is therefore complete even
when the user kills a command or closes the session mid-run.

### REQ-IT-006 — Closing the session never triggers an agent turn

Closing the inline terminal session — by ending the shell (`exit`, `Ctrl-D`) or
the deliberate return-to-composer gesture (a debounced backspace on an empty
input line, distinguished from clearing input) — returns the conversation to
idle. No rule on the inline terminal path issues an LLM request. Driving the
conversation through the inline terminal advances it without any agent turn.

### REQ-IT-007 — User origin is the single source of truth for attribution

Every inline command round carries one origin marker: user. The role the round
takes in LLM history (a tool-use/tool-result pair the model reads as prior
agent tool activity) and the attribution shown in the conversation UI (a
user-initiated command) are both derived from that single marker. The two
representations are never stored independently. This mirrors the existing
`MessageContent::Skill` split, where one message is delivered to the LLM in a
role distinct from its history attribution.

### REQ-IT-008 — Inline commands run un-gated as the server's shell user, with honest provenance

Inline commands run in the Phoenix server's interactive shell, with the same
process identity as the panel terminal's shell (`specs/terminal`) — the server's
Unix user, not the browsing user's OS account, which may be on a different
machine. They are not subject to the agent command deny-gate
(`specs/permissions`); the user authorizes each command by typing it. Because those commands enter history as `bash` tool
rounds, their user origin (REQ-IT-007) keeps the record honest: a later audit
distinguishes user-run commands from agent-issued ones, so un-gated user
commands are never mistaken for commands the agent chose to run.

## Out of Scope

These are deliberate non-goals; the design must leave room for them but does not
deliver them.

- **Cursor-addressing program output.** For full-screen programs (`vim`,
  `htop`) and in-place-rewriting output (progress bars), the bytes OSC 133
  brackets are screen-painting traffic, not line output, so ANSI-stripping them
  yields a smear. The correct result for an alternate-screen program is empty
  (it restores the main screen on exit); in-place rewriting wants vt100-resolved
  screen text. This is a shared `specs/terminal` `CommandTracker` concern the
  inline terminal inherits; the result type leaves room for either resolution
  without a schema change. Not delivered here.
- **Backgrounded command operations.** The `bash` tool's wait-handle model
  (`wait`, `kill`, polling partial output) has no inline analog (REQ-IT-004).
- **Sharing identity with the panel terminal.** The inline session is a
  separate, short-lived terminal, not the per-WorkScope panel terminal of
  `specs/terminal`. Reconciling the two into one shared session is not in scope.
- **The general user-tool-invocation syntax.** A general `$tool` / `T.tool`
  syntax for invoking other eligible tools (chiefly MCP / project integrations
  with no shell equivalent) directly as the user shares the user-origin
  attribution model; its umbrella requirements live in
  `specs/user-tool-invocation`, of which the inline terminal is the `bash`
  specialization.
