# Inline Terminal — Design

## Overview

The inline terminal reuses the entire PTY stack from `specs/terminal` — kernel
PTY, binary-WebSocket transport, xterm.js emulator, the vt100 parser, and the
OSC 133 `CommandTracker` — and relocates it into the conversation composer. The
novel work is not the terminal; it is the bridge from a live terminal's
OSC-133-delimited commands to durable, user-attributed `bash` tool rounds in
conversation history, and the conversation-lifecycle gating that keeps the user
and the agent from driving the conversation at the same time.

The `CommandTracker` already decomposes a session into discrete `CommandRecord`s
(`command_text`, ANSI-stripped `output`, `exit_code: Option`, `duration_ms`).
That decomposition is the keystone: an inline command round is a `CommandRecord`
projected into a `bash`-`run` tool-use/tool-result pair.

## The `!` sigil and the composer

`!` as the first character of the composer is the open trigger (REQ-IT-001). It
is not parsed by the `@`/`/` message expander (`specs/inline-references`,
`specs/skills`): those sigils rewrite a prose message that is still submitted as
a `UserMessage`. `!` instead switches the composer into terminal mode and opens a
PTY session — no prose message is submitted, and no agent turn is requested. The
distinction is structural: a prose message flows through bedrock's
`UserSendsMessage` (which requests the LLM); opening the inline terminal does
not.

Leaving terminal mode — ending the shell (`exit`, `Ctrl-D`) or an explicit
return-to-composer affordance — closes the session. There is no half-open state:
returning to the prose composer is a guaranteed terminal flush point for any
open command bracket (REQ-IT-005).

## Session lifecycle and conversation gating

An inline terminal session is keyed to one conversation and has two states,
`closed` and `live`. Opening requires the conversation to be idle; while a
session is `live` the conversation is busy and the agent cannot run (REQ-IT-002,
REQ-IT-006).

The conversation-busy representation is a new `core_status` the conversation
enters while an inline session is live — call it `inline_terminal` — analogous
to `executing_tools` but user-driven and never followed by an LLM request. It
must be added to bedrock's busy predicate (`Conversation.is_busy`) so the
existing `RejectMessageWhileBusy` path covers it, and it exits to `idle` on
session close. This is the bedrock coordination point for the feature; the
inline-terminal Allium models the observable contract (open requires idle; a
live session implies the conversation is not idle; close returns to idle) rather
than redefining bedrock's enum.

Modeling the session as a conversation-busy state — rather than as an orthogonal
side panel like the `specs/terminal` panel terminal — is deliberate. Inline
rounds and agent tool rounds both write to the same history; running them
concurrently would interleave two writers into the tool-round sequence. Mutual
exclusion via `core_status` makes that interleaving structurally impossible.

The inline session is a **separate** terminal from the per-WorkScope panel
terminal (`specs/terminal` REQ-TERM-003: exactly one panel terminal per
WorkScope). Keeping them distinct preserves that invariant and keeps each `!`
session self-contained. The inline session is short-lived: spawned on open, torn
down on close.

## Command lifecycle → inline rounds

The inline session observes the underlying terminal's OSC 133 command boundaries
(owned by `specs/terminal`: `CommandExecutionStarted` on `C`,
`CommandExecutionFinished` on `D`) and maintains one piece of its own state: the
**open bracket**, the `CommandRecord` of the command awaiting commit.

- **Start (`C`).** A new command starts. If a bracket is already open (the
  previous command never produced a `D`), it is first committed as an interrupted
  round (supersession), then the new bracket opens. This is the "`C` then another
  `C`" flush.
- **Complete (`D`).** The open bracket is finalized — output captured, exit code
  taken from the `D` payload (absent when `D` omits it; never fabricated as `0`,
  matching `CommandRecord.exit_code`'s `Option`) — and committed as a completed
  round. The bracket closes.
- **Close.** When the session closes with a bracket still open, that bracket is
  committed as an interrupted round (the guaranteed terminal flush), then the
  session goes `closed`.

The result is a total contract: **every command that starts resolves to exactly
one committed round**, `completed` or `interrupted` (REQ-IT-005). There is no
separate garbage-collection of dangling brackets; each bracket is closed exactly
once, by its own `D` or by supersession.

A clean `Ctrl-C` typically emits `D` with exit code 130 and commits as a normal
completed round; the interrupted path is reached only when no `D` arrives —
session-teardown mid-command, or a session whose shell never had OSC 133
integration. The interrupted outcome is therefore the uncommon case, which is
the correct shape for it.

## Track B: shaping a round into shared history

An inline command round materializes as the same two-message shape the agent's
own tool rounds use, so the LLM history builder needs no special case:

- a `MessageContent::Agent` message carrying a `bash` tool-use
  `ContentBlock` (`op: run`, `cmd:` the command text), and
- a `MessageContent::Tool` message carrying the matching tool-result
  (the captured output; the exit code surfaced in the result text as the
  `bash` tool already formats it).

The tool-use input is a `BashToolInput` with `op: BashOp::Run` and `cmd` set —
the same typed input an agent `bash` call produces — so the round is
indistinguishable in *shape* from an agent round and reads back cleanly on the
next turn (REQ-IT-003, REQ-IT-004).

### User origin as the single source of truth

What distinguishes an inline round from an agent round is **origin**, and origin
is stored once (REQ-IT-007). The conversation UI renders the round as
user-initiated, and any provenance audit (REQ-IT-008) reads the same marker; the
LLM-history role (agent tool activity) is *derived*, not separately stored. This
follows the precedent of `MessageContent::Skill`, which is delivered to the LLM
as a user-role message yet attributed in history as system-generated — one
stored fact, two derived presentations.

The origin marker is modeled as a discriminator on the persisted tool round, not
as a parallel "is_user" boolean alongside an agent/user enum elsewhere. A single
`origin` field whose value is `user` for inline rounds (and, by construction,
`agent` for the existing agent path) keeps the two presentations from drifting.

### Why output is not a bare string

`CommandRecord.output` is ANSI-stripped text today, which is correct for ordinary
commands but degrades to a stream of control sequences for full-screen TUI
programs (out of scope, REQ-IT-005 / Out of Scope). The tool-result content is
therefore modeled as the existing typed `ToolResult` content, not a bare
`String`, so a future screen-snapshot variant (a vt100 final-screen capture, as
`read_terminal` already produces) can be added without migrating persisted
rounds.

## Persistence

Inline rounds persist through the same message path as agent tool rounds — an
`Agent` message and a `Tool` message — with the `origin` discriminator on the
round. No new top-level `MessageContent` variant is required: the rounds are
ordinary agent/tool messages distinguished only by origin, which keeps the LLM
history builder and crash recovery on their existing paths. The inline session
itself (the live PTY) is ephemeral and not persisted, matching the panel
terminal; only the committed rounds are durable.

## Deny-gate and provenance

The agent command deny-gate (`specs/permissions`) does not apply to inline
commands: they run in the user's own interactive shell, which the deny-gate has
never governed (the panel terminal is equally ungated). The honesty guarantee is
provenance, not gating — because inline rounds enter history shaped like agent
`bash` rounds, the `origin: user` marker (REQ-IT-007) is what stops an audit from
attributing an un-gated user command to the agent (REQ-IT-008).

## Requirement-to-surface map

| Requirement | Realized by |
|---|---|
| REQ-IT-001 | `!` composer trigger; PTY session reusing `specs/terminal` transport |
| REQ-IT-002 | `UserOpensInlineTerminal` gate on `core_status = idle`; `inline_terminal` busy state; `AtMostOneLiveInlineSessionPerConversation` |
| REQ-IT-003 | `InlineCommandCompleted` → `Agent`+`Tool` round (Track B) |
| REQ-IT-004 | `BashToolInput { op: run }`; no `wait`/`kill`/handle ops emitted |
| REQ-IT-005 | `InlineCommandStarting` (supersession) + close-with-open-bracket; `ClosedSessionHasNoOpenBracket`; `EveryStartedCommandResolvesOnce` guarantee |
| REQ-IT-006 | `UserClosesInlineTerminal` → idle; `NoAgentTurn` guarantee |
| REQ-IT-007 | single `origin` discriminator; `UserOriginated` guarantee |
| REQ-IT-008 | provenance from REQ-IT-007; deny-gate non-application |
