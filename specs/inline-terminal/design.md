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

Leaving terminal mode closes the session, from either of two sources: the shell
process terminating (`exit`, `Ctrl-D`), or the return-to-composer gesture
(below). There is no half-open state — returning to the prose composer is a
guaranteed terminal flush point for any open command bracket (REQ-IT-005).

### Return-to-composer gesture

The deliberate way back to prose without ending the shell is **backspace on an
empty terminal input line**. A naive "backspace on empty means exit" collides
with the common "hold backspace to clear the whole line" motion: the keystrokes
that empty the line — and the ones still arriving as the user releases the key —
would fire the exit by accident. The gesture is therefore debounced. A
backspace-on-empty returns to the composer only when the input line has been
continuously empty and idle for `return_to_composer_idle_debounce`, making the
exit an isolated, intentional press rather than the tail of a delete burst.
Backspaces that land within the debounce window after the line empties are
consumed by the terminal, not read as the gesture.

Detecting "the input line is empty" depends on what the terminal layer can
observe — the cursor position relative to the shell's command-start column, or
the OSC 133 prompt/command-start markers (`specs/terminal`) — and the debounce
window is a tuning parameter calibrated against real typing. The stable contract
is the disambiguation intent: a deliberate, isolated backspace-on-empty returns
to the composer; clearing input never does.

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
**open bracket** — the command *text* of the command awaiting commit
(`open_command: String?` in the Allium), enough to commit an interrupted round
without retaining the terminal's full `CommandRecord`.

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

Those boundaries arrive as the `InlineCommandBridge` surface's
`Osc133CommandStart` / `Osc133CommandFinish` events, derived from the terminal's
OSC 133 tracking. Modeling them as surface inputs (`expects:`) rather than events
some rule emits is what makes the commit rules reachable. Because the open
bracket is the session's **own** `open_command`, the supersession flush and the
close flush resolve it without reading the terminal's transient
`current_command`: flush-then-reopen happens inside one rule, so there is no
cross-spec ordering race against the terminal's own `C` handling — the data-loss
window REQ-IT-005 guards against is structurally closed, and the bracket check
stays defined even after the terminal tears down.

The result is a total contract: **every command that starts resolves to exactly
one committed round**, `completed` or `interrupted` (REQ-IT-005). There is no
separate garbage-collection of dangling brackets; each bracket is closed exactly
once, by its own `D` or by supersession.

A clean `Ctrl-C` typically emits `D` with exit code 130 and commits as a normal
completed round; the interrupted path is reached only when no `D` arrives —
session-teardown mid-command, or a session whose shell never had OSC 133
integration. The interrupted outcome is therefore the uncommon case, which is
the correct shape for it.

### Shell integration prerequisite

Per-command commit depends on OSC 133 command markers. In a shell with no
integration (`shell_integration_status = absent` in `specs/terminal`), no `C` /
`D` ever fires: the user gets a working interactive terminal, but no rounds are
committed, and the empty-line detection the return-to-composer gesture relies on
(cursor position / prompt markers) is unavailable, leaving shell `exit` / `Ctrl-D`
as the only way out. Both degradations share the same population; the
implementation surfaces them to the user with an actionable "enable shell
integration" recommendation. The spec assumes integration is present rather than
modeling the degraded mode.

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

A tool round is two message rows — the `Agent` message carrying the `tool_use`
and the `Tool` message carrying the `tool_result` — so "stored once" means origin
lives on exactly one of them: the `Agent` message (the authored `tool_use`). The
paired `Tool` message carries no copy; its attribution is derived by pairing on
`tool_use_id`, so origin cannot diverge across the pair. The marker is a single
`origin` field (value `user` for inline rounds, `agent` by construction for the
existing agent path), not a parallel "is_user" boolean alongside an enum
elsewhere — one stored fact, all presentations derived.

### Why output is not a bare string

`CommandRecord.output` is ANSI-stripped text, which is correct for line-oriented
commands but degrades to a smear of paint traffic for cursor-addressing programs
(out of scope; see `TuiCursorAddressedOutput`). The resolution differs by kind:
an alternate-screen program (`vim`, `htop`) restores the main screen and
discards its scratch buffer on exit, so its truthful output is *empty* — an edit
is a filesystem side effect, observed as for an agent `bash` edit, not captured
text; an in-place-rewriting program (progress bars) wants the vt100-parser-
resolved screen text so `\r`-overwrites collapse to the final line. Both are a
`specs/terminal` `CommandTracker` concern the inline terminal inherits, not an
inline-terminal-specific fix. The tool-result content is therefore modeled as
the existing typed `ToolResult`, not a bare `String`, so either resolution fits
without migrating persisted rounds.

## Persistence

Inline rounds persist through the same message path as agent tool rounds — an
`Agent` message and a `Tool` message — with the `origin` field on the `Agent`
message (the `tool_use` carrier) and the `Tool` message deriving it by pairing on
`tool_use_id`. No new top-level `MessageContent` *variant* is required: the rounds
are ordinary agent/tool messages distinguished only by origin, which keeps the
LLM history builder and crash recovery on their existing paths. There is still a
schema change owed — the `Agent` message gains the `origin` field, which
`MessageContent` does not carry today — tracked as the `OriginDiscriminatorSchema`
deferred entry (default `agent` for pre-feature rows, a true-absence default
since existing rounds were all agent-authored). The inline session itself (the
live PTY) is ephemeral and not persisted, matching the panel
terminal; only the committed rounds are durable.

## Deny-gate and provenance

The agent command deny-gate (`specs/permissions`) does not apply to inline
commands: they run in the server-hosted interactive shell — the server's Unix
user, as the panel terminal — which the deny-gate has never governed (the panel
terminal is equally ungated). The honesty guarantee is
provenance, not gating — because inline rounds enter history shaped like agent
`bash` rounds, the `origin: user` marker (REQ-IT-007) is what stops an audit from
attributing an un-gated user command to the agent (REQ-IT-008).

## Requirement-to-surface map

| Requirement | Realized by |
|---|---|
| REQ-IT-001 | `!` composer trigger; PTY session reusing `specs/terminal` transport |
| REQ-IT-002 | `UserOpensInlineTerminal` gate on `core_status = idle`; `inline_terminal` busy state; `AtMostOneLiveInlineSessionPerConversation` |
| REQ-IT-003 | `InlineCommandBridge` `expects: Osc133CommandFinish` → `InlineCommandCompleted` → `Agent`+`Tool` round (Track B) |
| REQ-IT-004 | `BashToolInput { op: run }`; no `wait`/`kill`/handle ops emitted |
| REQ-IT-005 | `InlineCommandBridge` (`Osc133CommandStart`/`Finish`); `InlineCommandStarting` supersession-flush from session `open_command` + close-with-open-bracket; `ClosedSessionHasNoOpenBracket`; `CommitOrderingPinned` + `EveryStartedCommandResolvesOnce` guarantees |
| REQ-IT-006 | `UserClosesInlineTerminal` → idle; `NoAgentTurn` guarantee |
| REQ-IT-007 | single `origin` discriminator; `UserOriginated` guarantee |
| REQ-IT-008 | provenance from REQ-IT-007; deny-gate non-application |
