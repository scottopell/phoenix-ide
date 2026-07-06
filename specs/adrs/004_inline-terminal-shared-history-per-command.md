# ADR-004: Inline terminal records user commands as per-command bash rounds in shared history

- **Status:** Accepted
- **Date:** 2026-07-06
- **Affects:** REQ-IT-003, REQ-IT-004, REQ-IT-005, REQ-IT-007 (Allium: `InlineTerminalSession`, `InlineCommandRound`, `InlineCommandBridge`)

## Context

The inline terminal (`specs/inline-terminal/`) lets a user run shell commands
themselves — via a `!`-triggered PTY in the composer — without provoking an agent
turn. The open question is what relationship those user-run commands have to the
conversation the agent shares: are they a private side channel, or part of the
record the agent reads?

Two forces pull against each other. Keeping user commands out of the model is
simpler — no attribution, no schema change, no history shaping. But the point of
running commands *inside* a Phoenix conversation (rather than a separate
terminal) is that the agent works there too; if the human reproduces a failure or
edits a file and the agent can't see it, a later handoff is blind exactly when it
matters. The terminal stack already decomposes a session into discrete
`CommandRecord`s at OSC 133 boundaries, so per-command capture is available
without new machinery.

## Options considered

1. **Manual-only track (LLM-invisible).** User commands run and render in the
   transcript but never enter LLM history. Simplest: no attribution model, no
   persisted `origin`, no interrupted-round bookkeeping. Cost: the agent has no
   memory of what the human did — surprising and wrong the moment the user hands
   back to the agent after disruptive manual work.
2. **Shared history, per command.** Each OSC-133-delimited command commits as a
   user-originated `bash`-`run` tool round (an `Agent` message carrying the
   `tool_use` plus a `Tool` message carrying the result), shaped exactly like an
   agent `bash` round so the LLM history builder reads it with no special case.
   Cost: an `origin` discriminator on the persisted message, a rule that a
   started-but-uncompleted command still commits (interrupted), and a
   conversation busy-state coupling so the agent can't run concurrently.
3. **Shared history, whole-session snapshot.** Commit a single vt100 screen
   snapshot per session on close. Fewer rounds and no per-command bookkeeping,
   but it collapses N commands into one blurry result, loses per-command exit
   codes, and has no natural handling for a long or interactive session.

## Decision

Adopt option 2: **per-command commit into shared history.** Each command the user
runs becomes one user-originated `bash`-`run` round the agent later reads like its
own tool call. Three coupled choices make this coherent:

- **Per-command granularity** (not whole-session) because the terminal's OSC 133
  `CommandTracker` already produces one `CommandRecord` per command, and
  per-command rounds carry exit codes and read back cleanly.
- **A dedicated `InlinePty`**, distinct from the per-WorkScope panel terminal
  (`terminal/Terminal`), so the inline session is a separate, short-lived
  terminal and does not fall under `specs/terminal`'s `OneTerminalPerWorkScope`
  invariant.
- **User origin as a single stored fact.** One `origin` marker on the `Agent`
  message (the `tool_use` carrier); the paired `Tool` message derives origin by
  `tool_use_id`. The LLM-history role and the UI attribution are both derived
  from that one marker, never stored twice — the same role-vs-attribution split
  `MessageContent::Skill` already uses.

The interrupted-round rule (a command that starts but never emits OSC 133 `D`
still commits when superseded by the next command or by session close) exists so
that shared history has no holes: every started command resolves to exactly one
round. That totality is what makes the shared-history guarantee trustworthy.

## Consequences

- **Positive:** the agent inherits an accurate, gap-free record of what the human
  did, and reads it on the same path as its own tool rounds — no history-builder
  special case.
- **Positive:** per-command exit codes and results survive, so the agent can
  reason about individual commands rather than a screen blob.
- **Negative:** a schema change is owed — the `Agent` message gains an `origin`
  column (tracked as `OriginDiscriminatorSchema`).
- **Negative:** a bedrock coupling is owed — a non-idle `inline_terminal`
  `core_status` so the agent cannot run while a session is live (tracked as
  `BedrockInlineTerminalBusyState`).
- **Negative:** per-command commit fires on OSC 133 `D`; a shell without shell
  integration emits no markers, so it yields a working terminal but commits no
  rounds. This degradation is surfaced by the implementation, not modeled.
- **Neutral:** the separate `InlinePty` means the inline session does not share
  scrollback or state with the panel terminal.
- **Neutral:** output fidelity for cursor-addressing programs (`vim`, progress
  bars) is a separate concern (`TuiCursorAddressedOutput`), inherited from the
  shared `CommandTracker`.

## References

- Related ADRs: ADR-000 (adopt spEARS v2); ADR-005 (user tool invocation scope).
- Feature spec: `specs/inline-terminal/requirements.md`
- Behavioural spec: `specs/inline-terminal/inline-terminal.allium`
- Executive summary: `specs/inline-terminal/executive.md`
- Reused machinery: `specs/terminal/` (PTY, OSC 133 `CommandTracker`,
  `CommandRecord`).
