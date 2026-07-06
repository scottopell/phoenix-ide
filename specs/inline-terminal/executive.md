# Inline Terminal — Executive Summary

## What It Is

A real, PTY-backed terminal hosted inside the conversation composer, opened by
typing `!` as the first character of the message input. The user runs shell
commands themselves — full line editing, completion, colour, interactive
programs — and each command is recorded into the conversation as a `bash` tool
round the agent can later read. Issuing those commands never asks the model for
a response: the user drives the conversation forward without an agent turn.

It reuses the entire terminal stack from `specs/terminal` (PTY, binary
WebSocket, xterm.js, vt100 parser, OSC 133 command tracking) and adds the bridge
from a live terminal's commands to durable, user-attributed history.

## Why It Exists

A user sometimes wants to do the work themselves — run a build, poke at the
filesystem, reproduce a failure — without provoking the agent, while keeping the
agent's eventual view of the conversation accurate. The inline terminal makes
"the human took the wheel for a few commands" a first-class, recorded part of the
conversation rather than an out-of-band side channel the agent never sees.

## Scope

**Included:**
- The `!` composer trigger and inline PTY session lifecycle (open/close)
- Conversation gating: open only when idle; the conversation is busy while live;
  closing returns to idle with no agent turn
- Per-command commit: each OSC-133-delimited command becomes one user-originated
  `bash`-`run` tool round in shared history (visible to a later agent turn)
- The interrupted-round rule: a started-but-uncompleted command is committed
  (never dropped) when its bracket is superseded by the next command or by
  session close
- User origin as the single source of truth for round attribution

**Excluded / deferred:**
- Cursor-addressing program output (`vim`, `htop`, progress bars) — alt-screen
  programs want an empty result, in-place rewriting wants vt100-resolved text; a
  shared `specs/terminal` `CommandTracker` concern, and the result type leaves
  room for either
- Backgrounded command operations (the `bash` wait/kill/handle model)
- Sharing one session with the per-WorkScope panel terminal
- The general `$tool` / `T.tool` user-invocation syntax for other eligible tools (`specs/user-tool-invocation`)

## Design decisions

The rationale — the alternatives weighed and their tradeoffs — lives in the
project ADR chain:

- **ADR-004** — Track B shared history, per-command commit, the dedicated
  `InlinePty`, and user-origin attribution: why user commands become
  agent-visible `bash` rounds rather than a private track.
- **ADR-005** — user tool invocation scope (self-service; director tools
  deferred), of which the inline terminal is the `bash` specialization.

At a glance: `!` opens the terminal (distinct from `@`/`/` prose expansion); each
OSC-133 command commits as a `bash`-`run` round; a started-but-uncompleted
command commits interrupted (every `C` resolves to exactly one round); the
conversation is busy while live and returns to idle on close with no agent turn;
the deny-gate does not apply (server-hosted shell), with honesty from
`origin: user` provenance.

## The Central Invariant

Every command that starts in an inline session resolves to **exactly one**
committed round — `completed` (its OSC 133 `D` arrived) or `interrupted` (its
bracket was superseded by the next command or by session close). There is no
dangling state and no silent loss. This totality is what makes the shared-history
guarantee trustworthy: the agent's later view of "what the human did" has no
holes, even when the human killed a command or closed the session mid-run.

## Status Summary

The feature is specified but not yet implemented.

| Requirement | Status |
|---|---|
| REQ-IT-001: `!` opens an inline terminal in the composer | Planned |
| REQ-IT-002: Opening gated to an idle conversation; busy while live | Planned |
| REQ-IT-003: Each command committed as a user-originated tool round | Planned |
| REQ-IT-004: Inline rounds use the `bash` run operation only | Planned |
| REQ-IT-005: A started command is never dropped (interrupted round) | Planned |
| REQ-IT-006: Closing the session never triggers an agent turn | Planned |
| REQ-IT-007: User origin is the single source of truth for attribution | Planned |
| REQ-IT-008: Inline commands run un-gated, with honest provenance | Planned |
